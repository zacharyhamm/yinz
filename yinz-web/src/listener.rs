//! Desired-state listener generations. Listener release precedes filesystem cleanup/drain.
use crate::{
    auth::password::{read_state, PasswordState},
    listener_file::OwnedRecord,
    service::{self, State},
    WebStatus,
};
use hyper::{server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{watch, Semaphore},
    task::JoinSet,
    time::{Instant, Sleep},
};
use tokio_util::sync::CancellationToken;
use yinz_runtime::errors::prelude::*;

const DRAIN: Duration = Duration::from_secs(2);

async fn ready(state: &State, warning: &mut Option<String>) -> bool {
    let path = state.settings.data_dir.clone();
    let password = tokio::task::spawn_blocking(move || read_state(&path))
        .await
        .unwrap_or_else(|_| PasswordState::Invalid("password reader exited".into()));
    match password {
        PasswordState::Invalid(reason) => {
            if warning.as_ref() != Some(&reason) {
                log::warn!("web listener waiting for a valid private password file: {reason}");
                *warning = Some(reason);
            }
            false
        },
        state => {
            *warning = None;
            matches!(state, PasswordState::Ready(_))
        },
    }
}

pub(crate) async fn run(
    state: Arc<State>,
    cancel: CancellationToken,
    status: watch::Sender<WebStatus>,
) {
    let mut drains = JoinSet::new();
    let permits = Arc::new(Semaphore::new(state.settings.max_connections));
    let poll = state
        .settings
        .retry_interval
        .min(Duration::from_millis(500))
        .max(Duration::from_millis(1));
    let mut password_warning = None;
    let mut attempts = 0u32;
    let mut last_error = None;
    loop {
        while drains.try_join_next().is_some() {}
        let eligible = tokio::select! { biased; _ = cancel.cancelled() => break, ready = ready(&state, &mut password_warning) => ready };
        if state.backend.quiesced() || !eligible {
            status.send_replace(if state.backend.quiesced() {
                WebStatus::Quiesced
            } else {
                WebStatus::WaitingForPassword
            });
            tokio::select! { _ = cancel.cancelled() => break, _ = tokio::time::sleep(poll) => {} }
            continue;
        }
        let bound = tokio::select! { biased; _ = cancel.cancelled() => break, result = TcpListener::bind(state.settings.bind) => result };
        let listener = match bound {
            Ok(listener) => listener,
            Err(error) => {
                attempts = attempts.saturating_add(1);
                let text = error.to_string();
                if last_error.as_ref() != Some(&text) {
                    if error.kind() == io::ErrorKind::AddrInUse {
                        log::info!("web listener {} waiting for port", state.settings.bind);
                    } else {
                        log::warn!("web listener {}: {error}", state.settings.bind);
                    }
                    last_error = Some(text.clone());
                }
                status.send_replace(WebStatus::Binding {
                    attempts,
                    last_error: text,
                });
                let factor = if error.kind() == io::ErrorKind::AddrInUse {
                    1
                } else {
                    10
                };
                let mut random = [0; 2];
                let jitter = if state.settings.retry_interval >= Duration::from_secs(3)
                    && getrandom::fill(&mut random).is_ok()
                {
                    Duration::from_millis(u64::from(u16::from_ne_bytes(random)) % 1001)
                } else {
                    Duration::ZERO
                };
                tokio::select! { _ = cancel.cancelled() => break, _ = tokio::time::sleep(state.settings.retry_interval * factor + jitter) => {} }
                continue;
            },
        };
        let addr = match listener.local_addr() {
            Ok(addr) => addr,
            Err(error) => {
                log::warn!("inspect web listener: {error}");
                continue;
            },
        };
        let dir = state.settings.data_dir.clone();
        let name = state.backend.session_name();
        let mut current_name = name.clone();
        let startup_config = state.settings.startup_config.clone();
        let mut publication = tokio::task::spawn_blocking(move || {
            OwnedRecord::create(&dir, addr, name, startup_config)
        });
        let record = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                drop(listener);
                drains.spawn(async move {
                    if let Ok(Ok(record)) = publication.await {
                        let _ = tokio::task::spawn_blocking(move || drop(record)).await;
                    }
                });
                break;
            },
            result = &mut publication => match result {
                Ok(Ok(record)) => Arc::new(record),
                result => {
                    drop(listener);
                    log::warn!("cannot publish web listener record: {}", match result { Ok(Err(e)) => e.to_string(), Err(e) => e.to_string(), _ => String::new() });
                    status.send_replace(WebStatus::Binding { attempts: attempts.saturating_add(1), last_error: "cannot publish private listener record".into() });
                    tokio::select! { _ = cancel.cancelled() => break, _ = tokio::time::sleep(state.settings.retry_interval * 10) => {} }
                    continue;
                }
            }
        };
        // Recheck after publication: cancellation/quiesce must not publish a stale Serving.
        let still_ready = tokio::select! {
            biased;
            _ = cancel.cancelled() => false,
            value = ready(&state, &mut password_warning) => value,
        };
        if !still_ready || cancel.is_cancelled() || state.backend.quiesced() {
            drop(listener);
            drains.spawn(async move {
                let _ = tokio::task::spawn_blocking(move || drop(record)).await;
            });
            continue;
        }
        attempts = 0;
        last_error = None;
        let generation = CancellationToken::new();
        status.send_replace(WebStatus::Serving { addr });
        state.backend.activate();
        let mut connections = JoinSet::new();
        let mut interval = tokio::time::interval(poll);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {
                    let is_ready = tokio::select! { biased; _ = cancel.cancelled() => break, value = ready(&state, &mut password_warning) => value };
                    if state.backend.quiesced() || !is_ready { break; }
                    let name = state.backend.session_name();
                    if current_name != name {
                        current_name = name.clone();
                        let record = record.clone();
                        let mut refresh = tokio::task::spawn_blocking(move || record.refresh(name));
                        let result = tokio::select! {
                            biased;
                            _ = cancel.cancelled() => {
                                drains.spawn(async move { let _ = refresh.await; });
                                break;
                            },
                            result = &mut refresh => result
                        };
                        if !matches!(result, Ok(Ok(()))) { log::warn!("cannot refresh web listener record"); break; }
                    }
                },
                Some(_) = connections.join_next(), if !connections.is_empty() => {},
                accepted = listener.accept() => match accepted {
                    Ok((stream, peer)) => {
                        if let Ok(permit) = permits.clone().try_acquire_owned() {
                            let state = state.clone();
                            let generation = generation.clone();
                            let instance = record.instance().to_owned();
                            connections.spawn(async move {
                                let _permit = permit;
                                let io = TokioIo::new(IdleIo::new(stream, state.settings.header_timeout));
                                let service_state = state.clone();
                                let admission = generation.clone();
                                let service = service_fn(move |req| service::serve(req, service_state.clone(), peer, admission.clone(), instance.clone()));
                                let mut builder = http1::Builder::new();
                                builder.timer(TokioTimer::new()).keep_alive(true).half_close(false).max_buf_size(16 * 1024).max_headers(64).header_read_timeout(state.settings.header_timeout);
                                let connection = builder.serve_connection(io, service);
                                tokio::pin!(connection);
                                tokio::select! {
                                    _ = &mut connection => return,
                                    _ = generation.cancelled() => {},
                                    _ = tokio::time::sleep(Duration::from_secs(300)) => {},
                                }
                                connection.as_mut().graceful_shutdown();
                                let _ = tokio::time::timeout(DRAIN, connection).await;
                            });
                        }
                    },
                    Err(error) => {
                        log::warn!("accept web connection: {error}");
                        if matches!(error.raw_os_error(), Some(libc::EMFILE | libc::ENFILE)) {
                            tokio::select! { _ = cancel.cancelled() => break, _ = tokio::time::sleep(Duration::from_millis(100)) => {} }
                        }
                    }
                }
            }
        }
        generation.cancel();
        drop(listener);
        // Cleanup and draining never hold the listening socket. A new generation can bind
        // immediately; each old service closure retains its own cancelled admission token.
        drains.spawn(async move {
            let cleanup = tokio::task::spawn_blocking(move || drop(record));
            if tokio::time::timeout(DRAIN, async {
                while connections.join_next().await.is_some() {}
            })
            .await
            .is_err()
            {
                connections.abort_all();
                while connections.join_next().await.is_some() {}
            }
            cleanup
                .await
                .context("join web listener cleanup")
                .non_fatal();
        });
    }
    while drains.join_next().await.is_some() {}
    status.send_replace(WebStatus::Stopped);
}

/// An idle connection must expire even before Hyper begins parsing its next request.
struct IdleIo {
    stream: TcpStream,
    timer: Pin<Box<Sleep>>,
    timeout: Duration,
}
impl IdleIo {
    fn new(stream: TcpStream, timeout: Duration) -> Self {
        Self {
            stream,
            timer: Box::pin(tokio::time::sleep(timeout)),
            timeout,
        }
    }
}
impl AsyncRead for IdleIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        match Pin::new(&mut this.stream).poll_read(cx, buf) {
            Poll::Ready(result) => {
                if buf.filled().len() > before {
                    this.timer.as_mut().reset(Instant::now() + this.timeout);
                }
                Poll::Ready(result)
            },
            Poll::Pending => match this.timer.as_mut().poll(cx) {
                Poll::Ready(_) => Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "idle web connection",
                ))),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}
impl AsyncWrite for IdleIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
#[path = "unit/listener.rs"]
mod tests;

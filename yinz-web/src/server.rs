//! The public API of the web server: settings, status, the plugin [`Backend`] and the
//! [`WebServer`] handle that owns the listener task.
//!
//! All state lives in one `Arc<State>` per `WebServer`; nothing is `static`.

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use yinz_runtime::errors::prelude::*;
use yinz_types::input::web::WebConfig;
use yinz_types::web::{Site, WebReply, WebRequest};

pub const DEFAULT_GET_TIMEOUT: Duration = Duration::from_secs(15);
/// Above the 32 s deadline of a prompt to a peer session.
pub const DEFAULT_POST_TIMEOUT: Duration = Duration::from_secs(45);
pub const DEFAULT_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_RETRY_INTERVAL: Duration = Duration::from_secs(3);
pub const DEFAULT_MAX_CONNECTIONS: usize = 64;

/// What [`Backend::dispatch`] returns.
pub type DispatchFuture = Pin<Box<dyn Future<Output = Option<WebReply>> + Send>>;

/// The session behind the server: the plugin thread, the frozen site and the update state.
pub trait Backend: Send + Sync + 'static {
    /// `None`: the reply slot was dropped (plugin thread exited or instance disposed), 503.
    fn dispatch(&self, request: WebRequest) -> DispatchFuture;
    fn site(&self) -> Option<Arc<Site>>;
    /// Whether a live update has quiesced the session (`senders.session_update_quiesced`).
    fn quiesced(&self) -> bool;
    /// Sent once per successful bind: load the web plugin.
    fn activate(&self);
    /// Read per call; sessions can be renamed.
    fn session_name(&self) -> String;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSettings {
    /// `127.0.0.1:7419` by default.
    pub bind: SocketAddr,
    /// Extra exact hosts, added to loopback and `*.ts.net`.
    pub allowed_hosts: Vec<String>,
    /// Extra exact origins, `scheme://host:port`.
    pub allowed_origins: Vec<String>,
    /// Empty turns the Tailscale identity check off.
    pub tailscale_users: Vec<String>,
    pub session_days: u32,
    pub data_dir: PathBuf,
    pub get_timeout: Duration,
    pub post_timeout: Duration,
    pub header_timeout: Duration,
    /// Plus 0-1 s of jitter; tests use 50 ms.
    pub retry_interval: Duration,
    pub max_connections: usize,
}

impl WebSettings {
    /// The effective settings of a `web { }` block, or the reason it cannot start.
    pub fn from_config(c: &WebConfig, data_dir: &Path) -> Result<Self> {
        c.validate()
            .map_err(|error| anyhow!(error))
            .context("invalid web config")?;
        let bind = c
            .bind_addr()
            .map_err(|error| anyhow!(error))
            .context("invalid web bind address")?;
        Ok(WebSettings {
            bind,
            allowed_hosts: c.allowed_hosts.clone(),
            allowed_origins: c.allowed_origins.clone(),
            tailscale_users: c.tailscale_users.clone(),
            session_days: c.session_days_or_default(),
            data_dir: data_dir.to_path_buf(),
            get_timeout: DEFAULT_GET_TIMEOUT,
            post_timeout: DEFAULT_POST_TIMEOUT,
            header_timeout: DEFAULT_HEADER_TIMEOUT,
            retry_interval: DEFAULT_RETRY_INTERVAL,
            max_connections: DEFAULT_MAX_CONNECTIONS,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebStatus {
    Disabled,
    WaitingForPassword,
    Quiesced,
    Binding { attempts: u32, last_error: String },
    Serving { addr: SocketAddr },
    Stopped,
}

/// The running server. Dropping it stops the listener without blocking.
pub struct WebServer {
    cancel: CancellationToken,
    status: watch::Receiver<WebStatus>,
}

impl WebServer {
    pub fn start(
        settings: WebSettings,
        backend: Arc<dyn Backend>,
        rt: &tokio::runtime::Handle,
    ) -> Self {
        let cancel = CancellationToken::new();
        let (status, receiver) = watch::channel(WebStatus::WaitingForPassword);
        let state = Arc::new(crate::service::State {
            settings,
            backend,
            funnel_warning: std::sync::Mutex::new(None),
        });
        rt.spawn(crate::listener::run(state, cancel.clone(), status));
        WebServer {
            cancel,
            status: receiver,
        }
    }

    #[cfg(test)]
    pub(crate) fn status_receiver(&self) -> watch::Receiver<WebStatus> {
        self.status.clone()
    }

    pub fn status(&self) -> WebStatus {
        self.status.borrow().clone()
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        match self.status() {
            WebStatus::Serving { addr } => Some(addr),
            _ => None,
        }
    }

    /// Cancel only; never blocks. The listener frees the port at once and drains in the
    /// background.
    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

impl Drop for WebServer {
    fn drop(&mut self) {
        self.stop()
    }
}

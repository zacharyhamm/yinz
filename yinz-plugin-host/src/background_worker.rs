//! Named guest workers: one persistent worker process and OS thread per worker.
use crate::{plugin_map::PluginEnv, steel::next_generation, ClientId};
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{self, Sender};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::Event;

pub(super) const POST_BUDGET: usize = 64 * 1024 * 1024;
// Include the inline message and room for the channel's per-slot/block bookkeeping.
pub(super) const POST_OVERHEAD: usize = size_of::<Message>() + 4 * size_of::<usize>();

pub(crate) struct PostReservation {
    queued_bytes: Arc<AtomicUsize>,
    bytes: usize,
}

impl Drop for PostReservation {
    fn drop(&mut self) {
        self.queued_bytes.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

pub(super) fn post_job(
    sender: &Sender<Message>,
    queued_bytes: &Arc<AtomicUsize>,
    client: ClientId,
    name: String,
    payload: String,
) -> Result<()> {
    let bytes = name
        .capacity()
        .checked_add(payload.capacity())
        .and_then(|bytes| bytes.checked_add(POST_OVERHEAD))
        .context("Plugin worker post exceeds the 64 MiB queue budget")?;
    queued_bytes
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
            used.checked_add(bytes)
                .filter(|total| *total <= POST_BUDGET)
        })
        .map_err(|_| anyhow!("Plugin worker post exceeds the 64 MiB queue budget"))?;
    let reservation = PostReservation {
        queued_bytes: queued_bytes.clone(),
        bytes,
    };
    sender
        .send(Message::Job(client, name, payload, reservation))
        .map_err(|_| anyhow!("Plugin worker has stopped"))
}

pub(crate) enum Message {
    Job(ClientId, String, String, PostReservation),
    Event(ClientId, Event, Option<yinz_bus::thread_bus::PluginReplyId>),
    Permission(super::permissions::Completion),
    Tick,
    Exit,
}

struct Worker {
    generation: u64,
    sender: Sender<Message>,
    queued_bytes: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}

pub(crate) struct Workers {
    pub parent_generation: u64,
    workers: Mutex<HashMap<String, Worker>>,
    pub(super) permissions: Mutex<super::permissions::Permissions>,
}

impl Workers {
    pub fn new(parent_generation: u64) -> Self {
        Self {
            parent_generation,
            workers: Mutex::default(),
            permissions: Mutex::default(),
        }
    }

    pub fn spawn(&self, name: String, source: &str, path: &Path, env: PluginEnv) -> Result<()> {
        let generation = next_generation()?;
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let source = source.to_owned();
        let path = path.to_owned();
        let worker_name = name.clone();
        let identity = env.log_identity();
        // Guest execution and synchronous host ops stay outside Tokio, just like the UI runtime.
        let thread = thread::Builder::new()
            .name(format!("plugin-worker-{generation}"))
            .spawn(move || {
                let run = || -> Result<()> {
                    crate::steel::workers::run(
                        &worker_name,
                        &source,
                        &path,
                        env,
                        worker_cancel,
                        generation,
                        receiver,
                    )
                };
                if let Err(error) =
                    run().with_context(|| format!("Plugin worker {worker_name} failed"))
                {
                    for line in format!("{error:#}").lines() {
                        log::error!("{identity} {line}");
                    }
                }
            })
            .context("starting Plugin worker thread")?;
        self.workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                name,
                Worker {
                    generation,
                    sender,
                    queued_bytes: Arc::default(),
                    cancel,
                    thread,
                },
            );
        Ok(())
    }

    pub fn post(
        &self,
        worker_name: &str,
        client: ClientId,
        name: String,
        payload: String,
    ) -> Result<()> {
        let workers = self.workers.lock().to_anyhow()?;
        let worker = workers
            .get(worker_name)
            .with_context(|| format!("Plugin worker {worker_name:?} is not registered"))?;
        post_job(&worker.sender, &worker.queued_bytes, client, name, payload)
            .with_context(|| format!("posting to Plugin worker {worker_name:?}"))
    }

    pub fn contains(&self, generation: u64) -> bool {
        self.workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .any(|worker| worker.generation == generation)
    }

    pub fn send(&self, generation: u64, message: Message) {
        if let Some(worker) = self
            .workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .find(|worker| worker.generation == generation)
        {
            // A failed or cancelled worker may already have closed its inbox.
            let _ = worker.sender.send(message);
        }
    }

    pub fn permissions(&self, client: ClientId, event: &Event) {
        for worker in self
            .workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            let _ = worker
                .sender
                .send(Message::Event(client, event.clone(), None));
        }
    }

    pub fn stop(&self) {
        self.permissions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stop();
        let workers = std::mem::take(&mut *self.workers.lock().unwrap_or_else(|e| e.into_inner()));
        for worker in workers.values() {
            worker.cancel.store(true, Ordering::Release);
            let _ = worker.sender.send(Message::Exit);
        }
        // Release the registry lock before joining: a running callback can post to a sibling.
        for worker in workers.into_values() {
            worker
                .thread
                .join()
                .map_err(|_| anyhow!("Plugin worker thread panicked"))
                .non_fatal();
        }
    }
}

#[cfg(test)]
#[path = "unit/background_worker.rs"]
mod tests;

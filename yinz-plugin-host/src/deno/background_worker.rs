//! Named guest workers: one persistent isolate and OS thread per worker.
use super::*;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{self, Sender};

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
        .context("Deno worker post exceeds the 64 MiB queue budget")?;
    queued_bytes
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
            used.checked_add(bytes)
                .filter(|total| *total <= POST_BUDGET)
        })
        .map_err(|_| anyhow!("Deno worker post exceeds the 64 MiB queue budget"))?;
    let reservation = PostReservation {
        queued_bytes: queued_bytes.clone(),
        bytes,
    };
    sender
        .send(Message::Job(client, name, payload, reservation))
        .map_err(|_| anyhow!("Deno worker has stopped"))
}

pub(crate) enum Message {
    Job(ClientId, String, String, PostReservation),
    Event(ClientId, Event),
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
        // Guest execution and synchronous host ops stay outside Tokio, just like the UI isolate.
        let thread = thread::Builder::new()
            .name(format!("deno-worker-{generation}"))
            .spawn(move || {
                let run = || -> Result<()> {
                    let mut plugin = DenoPlugin::with_generation(
                        &source,
                        &path,
                        env,
                        worker_cancel,
                        generation,
                    )?;
                    plugin.with_runtime(|runtime, _| {
                        runtime.call("workerBind", json!(worker_name), None)?;
                        Ok(())
                    })?;
                    while let Ok(message) = receiver.recv() {
                        if plugin.is_disposed() || plugin.env().cancel.load(Ordering::Acquire) {
                            break;
                        }
                        match message {
                            Message::Job(client, name, payload, reservation) => {
                                plugin.env_mut().client_id = client;
                                plugin.with_runtime(|runtime, client| {
                                    runtime
                                        .runtime
                                        .op_state()
                                        .borrow_mut()
                                        .borrow_mut::<VecDeque<PostReservation>>()
                                        .push_back(reservation);
                                    runtime.call(
                                        "workerMessage",
                                        json!([name, payload]),
                                        Some(client),
                                    )?;
                                    runtime.poll()?;
                                    runtime.needs_render()
                                })?;
                            },
                            Message::Event(client, event) => {
                                if crate::wasm_bridge::check_event_permission(plugin.env(), &event)
                                    .0
                                    == PermissionStatus::Granted
                                {
                                    plugin.env_mut().client_id = client;
                                    if let Event::HostFolderChanged(path) = &event {
                                        plugin.env_mut().set_host_folder(path.clone());
                                    }
                                    plugin.update(&event)?;
                                }
                            },
                            Message::Tick => {
                                plugin.tick()?;
                            },
                            Message::Permission(completion) => {
                                plugin.permission_completion(completion)?;
                            },
                            Message::Exit => break,
                        }
                    }
                    Ok(())
                };
                if let Err(error) =
                    run().with_context(|| format!("Deno worker {worker_name} failed"))
                {
                    for line in format!("{error:#}").lines() {
                        log::error!("{identity} {line}");
                    }
                }
            })
            .context("starting Deno worker thread")?;
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
            .with_context(|| format!("Deno worker {worker_name:?} is not registered"))?;
        post_job(&worker.sender, &worker.queued_bytes, client, name, payload)
            .with_context(|| format!("posting to Deno worker {worker_name:?}"))
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
            let _ = worker.sender.send(Message::Event(client, event.clone()));
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
                .map_err(|_| anyhow!("Deno worker thread panicked"))
                .non_fatal();
        }
    }
}

impl DenoPlugin {
    pub(crate) fn worker_names(&mut self) -> Result<Vec<String>> {
        let names = self.with_runtime(|runtime, _| {
            serde_json::from_value::<Vec<String>>(runtime.call("workers", Value::Null, None)?)
                .context("reading Deno worker names")
        })?;
        if names.len() > 16 || names.iter().any(|name| name.is_empty() || name.len() > 128) {
            bail!("a Deno plugin supports up to 16 workers with names of 1–128 bytes");
        }
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: ClientId = ClientId::new(1);

    #[test]
    fn oversized_posts_count_retained_capacity_of_both_strings() {
        let (sender, receiver) = mpsc::channel();
        let used = Arc::default();
        for (name, payload) in [
            (String::with_capacity(POST_BUDGET), String::new()),
            (String::new(), String::with_capacity(POST_BUDGET)),
        ] {
            let error = post_job(&sender, &used, CLIENT, name, payload).unwrap_err();
            assert!(error.to_string().contains("64 MiB queue budget"));
            assert_eq!(used.load(Ordering::Relaxed), 0);
            assert!(receiver.try_recv().is_err());
        }
    }

    #[test]
    fn cumulative_posts_recover_capacity_on_dequeue_and_preserve_fifo() {
        let (sender, receiver) = mpsc::channel();
        let used = Arc::default();
        let post = |name: &str| {
            post_job(
                &sender,
                &used,
                CLIENT,
                name.into(),
                String::with_capacity(POST_BUDGET / 2 - POST_OVERHEAD - name.len()),
            )
        };
        post("first").unwrap();
        post("second").unwrap();
        assert_eq!(used.load(Ordering::Relaxed), POST_BUDGET);
        assert!(post("refused").is_err());
        let Message::Job(client, name, payload, reservation) = receiver.recv().unwrap() else {
            panic!("expected post");
        };
        assert_eq!((client, name.as_str()), (CLIENT, "first"));
        drop(reservation);
        // The executing job may still own its payload; only queued jobs count.
        post("third").unwrap();
        drop(payload);
        for expected in ["second", "third"] {
            let Message::Job(_, name, _, _) = receiver.recv().unwrap() else {
                panic!("expected post");
            };
            assert_eq!(name, expected);
        }
        assert_eq!(used.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn empty_posts_are_bounded_and_shutdown_releases_the_queue() {
        let (sender, receiver) = mpsc::channel();
        let used = Arc::default();
        for _ in 0..POST_BUDGET / POST_OVERHEAD {
            post_job(&sender, &used, CLIENT, String::new(), String::new()).unwrap();
        }
        assert!(post_job(&sender, &used, CLIENT, String::new(), String::new()).is_err());
        assert_eq!(
            used.load(Ordering::Relaxed),
            POST_BUDGET / POST_OVERHEAD * POST_OVERHEAD
        );
        drop(receiver);
        assert_eq!(used.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn send_failure_releases_the_reservation() {
        let (sender, receiver) = mpsc::channel();
        let used = Arc::default();
        drop(receiver);
        for _ in 0..2 {
            let error = post_job(
                &sender,
                &used,
                CLIENT,
                String::new(),
                String::with_capacity(POST_BUDGET - POST_OVERHEAD),
            )
            .unwrap_err();
            assert!(error.to_string().contains("stopped"));
            assert_eq!(used.load(Ordering::Relaxed), 0);
        }
    }

    #[test]
    fn concurrent_senders_cannot_exceed_the_budget() {
        let (sender, receiver) = mpsc::channel();
        let used = Arc::default();
        let accepted = AtomicUsize::new(0);
        let start = std::sync::Barrier::new(8);
        std::thread::scope(|threads| {
            for _ in 0..8 {
                threads.spawn(|| {
                    start.wait();
                    for _ in 0..16 {
                        if post_job(
                            &sender,
                            &used,
                            CLIENT,
                            String::new(),
                            String::with_capacity(POST_BUDGET / 64 - POST_OVERHEAD),
                        )
                        .is_ok()
                        {
                            accepted.fetch_add(1, Ordering::Relaxed);
                        }
                        assert!(used.load(Ordering::Relaxed) <= POST_BUDGET);
                    }
                });
            }
        });
        assert_eq!(accepted.load(Ordering::Relaxed), 64);
        assert_eq!(used.load(Ordering::Relaxed), POST_BUDGET);
        assert_eq!(receiver.try_iter().count(), 64);
        assert_eq!(used.load(Ordering::Relaxed), 0);
    }
}

#![allow(clippy::type_complexity)]
use crate::plugins::plugin_map::PluginMap;
use crate::plugins::wasm_bridge::PluginCache;
use crate::ClientId;
use crate::ThreadSenders;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use wasmi::Engine;

/// A dynamic thread pool that pins jobs to specific threads based on plugin_id
/// Starts with 1 thread and expands when threads are busy, shrinks when plugins unload
pub struct PinnedExecutor {
    // Sparse vector - Some(thread) for active threads, None for removed threads
    execution_threads: Arc<Mutex<Vec<Option<ExecutionThread>>>>,

    // Maps plugin_id -> thread_index (permanent assignment)
    plugin_assignments: Arc<Mutex<HashMap<u32, usize>>>,

    // Maps thread_index -> set of plugin_ids assigned to it
    thread_plugins: Arc<Mutex<HashMap<usize, HashSet<u32>>>>,

    // Maximum threads allowed
    max_threads: usize,

    // state to send to plugins (to be kept on execution threads)
    senders: ThreadSenders,
    plugin_map: Arc<Mutex<PluginMap>>,
    connected_clients: Arc<Mutex<Vec<ClientId>>>,
    plugin_cache: PluginCache,
    engine: Engine,
}

struct ExecutionThread {
    sender: Sender<Job>,
    jobs_in_flight: Arc<AtomicUsize>, // Busy state tracking
}

type JobWork = Box<
    dyn FnOnce(ThreadSenders, Arc<Mutex<PluginMap>>, Arc<Mutex<Vec<ClientId>>>, PluginCache, Engine)
        + Send
        + 'static,
>;

type PendingJob = (JobTag, Instant, JobWork);

/// What the executor knows about a queued job beyond its closure: whose it is,
/// a short label for the stats log ("Key", "PaneUpdate×2", "pipe:agent_hook"),
/// and whether it carries user input — the one kind of job whose queue wait is
/// somebody's keystroke latency.
struct JobTag {
    plugin_id: u32,
    label: String,
    is_input: bool,
}

enum Job {
    Work {
        tag: JobTag,
        enqueued_at: Instant,
        work: JobWork,
    },
    Shutdown, // Signal to exit the worker loop
}

/// How long a worker accumulates job stats before logging one summary line.
const STATS_LOG_EVERY: Duration = Duration::from_secs(10);

/// An input-bearing job that waited this long in the queue gets its own log
/// line the moment it starts: that wait is user-visible latency, and the
/// summary's max would smear it across ten seconds of other traffic.
const INPUT_WAIT_WARNING: Duration = Duration::from_millis(100);

/// One plugin's share of a worker's stats window.
#[derive(Default)]
struct PluginJobStats {
    jobs: u64,
    wait_total: Duration,
    wait_max: Duration,
    run_total: Duration,
    run_max: Duration,
    run_max_label: String,
    /// Job count and run time per label, so the summary can say *what* the
    /// jobs were, not only how many. Capped; the excess pools under "other".
    by_label: Vec<(String, u64, Duration)>,
}

/// Labels tracked individually per plugin per window before pooling.
const MAX_TRACKED_LABELS: usize = 12;

fn record_job(
    stats: &mut HashMap<u32, PluginJobStats>,
    tag: &JobTag,
    waited: Duration,
    ran: Duration,
) {
    let entry = stats.entry(tag.plugin_id).or_default();
    entry.jobs += 1;
    entry.wait_total += waited;
    entry.wait_max = entry.wait_max.max(waited);
    entry.run_total += ran;
    if ran >= entry.run_max {
        entry.run_max = ran;
        entry.run_max_label = tag.label.clone();
    }
    let slot = match entry
        .by_label
        .iter()
        .position(|(label, _, _)| *label == tag.label)
    {
        Some(index) => Some(index),
        None if entry.by_label.len() < MAX_TRACKED_LABELS => {
            entry.by_label.push((tag.label.clone(), 0, Duration::ZERO));
            Some(entry.by_label.len() - 1)
        },
        None => entry
            .by_label
            .iter()
            .position(|(label, _, _)| label == "other")
            .or_else(|| {
                entry.by_label.push(("other".to_owned(), 0, Duration::ZERO));
                Some(entry.by_label.len() - 1)
            }),
    };
    if let Some(index) = slot {
        let (_, count, run) = &mut entry.by_label[index];
        *count += 1;
        *run += ran;
    }
}

/// Queue a job and update the input-priority metadata with it.
fn push_pending(
    pending: &mut VecDeque<PendingJob>,
    pending_input_jobs: &mut HashMap<u32, usize>,
    job: PendingJob,
) {
    if job.0.is_input {
        *pending_input_jobs.entry(job.0.plugin_id).or_default() += 1;
    }
    pending.push_back(job);
}

/// Which pending job runs next.
///
/// The invariant is per-plugin FIFO: the only job of a plugin that may run is
/// its *oldest* queued one, so a plugin sees its events in the order they were
/// sent — `plugin_tests/batching.rs` pins that ordering end to end. Between
/// plugins the pick prefers one with an input job waiting anywhere in its
/// queue: an input job's wait is a person's keystroke latency, and without
/// this it queues behind every other plugin's already-submitted work.
fn pick_next_job(
    pending: &VecDeque<PendingJob>,
    pending_input_jobs: &HashMap<u32, usize>,
) -> Option<usize> {
    pending.front()?;
    if pending_input_jobs.is_empty() {
        return Some(0);
    }
    let index = pending
        .iter()
        .position(|(tag, _, _)| pending_input_jobs.contains_key(&tag.plugin_id));
    debug_assert!(index.is_some(), "pending input metadata is out of sync");
    index.or(Some(0))
}

/// "location[id]", or "plugin[id]" when the map is busy or the plugin is gone.
fn plugin_name(plugin_map: &Arc<Mutex<PluginMap>>, plugin_id: u32) -> String {
    let location = plugin_map.try_lock().ok().and_then(|plugin_map| {
        plugin_map
            .run_plugin_of_plugin_id(plugin_id)
            .map(|run_plugin| run_plugin.location.to_string())
    });
    match location {
        Some(location) => format!("{location}[{plugin_id}]"),
        None => format!("plugin[{plugin_id}]"),
    }
}

/// One line per window per worker, heaviest plugin first, then reset. This is
/// the measurement `docs/PLUGIN_RENDER_PIPELINE.md` points at: what each plugin
/// cost this thread, and how long jobs sat in its queue.
fn log_job_stats(
    thread_idx: usize,
    window: Duration,
    stats: &mut HashMap<u32, PluginJobStats>,
    plugin_map: &Arc<Mutex<PluginMap>>,
) {
    let mut entries: Vec<(u32, PluginJobStats)> = stats.drain().collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.1.run_total));
    let summary: Vec<String> = entries
        .iter()
        .map(|(plugin_id, s)| {
            let mut by_label = s.by_label.clone();
            by_label.sort_by_key(|entry| std::cmp::Reverse(entry.2));
            let labels: Vec<String> = by_label
                .iter()
                .map(|(label, count, run)| format!("{label}×{count} {}ms", run.as_millis()))
                .collect();
            format!(
                "{}: {} jobs, run {}ms (max {}ms {}), wait {}ms (max {}ms) [{}]",
                plugin_name(plugin_map, *plugin_id),
                s.jobs,
                s.run_total.as_millis(),
                s.run_max.as_millis(),
                s.run_max_label,
                s.wait_total.as_millis(),
                s.wait_max.as_millis(),
                labels.join(", "),
            )
        })
        .collect();
    log::info!(
        "plugin-exec-{} job stats (last {:.1}s): {}",
        thread_idx,
        window.as_secs_f64(),
        summary.join("; ")
    );
}

impl PinnedExecutor {
    /// Creates a new pinned executor with the specified maximum number of threads
    /// Starts with exactly 1 thread
    pub fn new(
        max_threads: usize,
        senders: &ThreadSenders,
        plugin_map: &Arc<Mutex<PluginMap>>,
        connected_clients: &Arc<Mutex<Vec<ClientId>>>,
        plugin_cache: &PluginCache,
        engine: &Engine,
    ) -> Self {
        let max_threads = max_threads.max(1); // At least 1

        let thread_0 = Self::spawn_thread(
            0,
            senders.clone(),
            plugin_map.clone(),
            connected_clients.clone(),
            plugin_cache.clone(),
            engine.clone(),
        );

        PinnedExecutor {
            execution_threads: Arc::new(Mutex::new(vec![Some(thread_0)])),
            plugin_assignments: Arc::new(Mutex::new(HashMap::new())),
            thread_plugins: Arc::new(Mutex::new(HashMap::new())),
            max_threads,
            senders: senders.clone(),
            plugin_map: plugin_map.clone(),
            connected_clients: connected_clients.clone(),
            plugin_cache: plugin_cache.clone(),
            engine: engine.clone(),
        }
    }

    fn spawn_thread(
        thread_idx: usize,
        senders: ThreadSenders,
        plugin_map: Arc<Mutex<PluginMap>>,
        connected_clients: Arc<Mutex<Vec<ClientId>>>,
        plugin_cache: PluginCache,
        engine: Engine,
    ) -> ExecutionThread {
        let (sender, receiver) = channel::<Job>();
        let jobs_in_flight = Arc::new(AtomicUsize::new(0));
        let jobs_in_flight_clone = jobs_in_flight.clone();

        let thread_handle = thread::Builder::new()
            .name(format!("plugin-exec-{}", thread_idx))
            .spawn({
                move || {
                    let senders = senders;
                    let plugin_map = plugin_map;
                    let connected_clients = connected_clients;
                    let plugin_cache = plugin_cache;
                    let engine = engine;
                    let mut stats: HashMap<u32, PluginJobStats> = HashMap::new();
                    let mut stats_since = Instant::now();
                    // Jobs taken off the channel but not yet run. Held locally so
                    // the pick below can see what is waiting: strict channel FIFO
                    // would make a keystroke wait behind every other plugin's
                    // queued work. Per-plugin order is still absolute — only the
                    // interleaving *between* plugins is rearranged.
                    let mut pending: VecDeque<PendingJob> = VecDeque::new();
                    let mut pending_input_jobs: HashMap<u32, usize> = HashMap::new();
                    let mut shutting_down = false;
                    loop {
                        if pending.is_empty() && !shutting_down {
                            match receiver.recv() {
                                Ok(Job::Work {
                                    tag,
                                    enqueued_at,
                                    work,
                                }) => push_pending(
                                    &mut pending,
                                    &mut pending_input_jobs,
                                    (tag, enqueued_at, work),
                                ),
                                Ok(Job::Shutdown) => shutting_down = true,
                                Err(_) => break,
                            }
                        }
                        // Everything already queued, so an input job that just
                        // arrived is visible to this pick.
                        while let Ok(job) = receiver.try_recv() {
                            match job {
                                Job::Work {
                                    tag,
                                    enqueued_at,
                                    work,
                                } => push_pending(
                                    &mut pending,
                                    &mut pending_input_jobs,
                                    (tag, enqueued_at, work),
                                ),
                                Job::Shutdown => shutting_down = true,
                            }
                        }
                        let Some(index) = pick_next_job(&pending, &pending_input_jobs) else {
                            if shutting_down {
                                break;
                            }
                            continue;
                        };
                        let Some((tag, enqueued_at, work)) = pending.remove(index) else {
                            continue;
                        };
                        if tag.is_input {
                            if let Some(count) = pending_input_jobs.get_mut(&tag.plugin_id) {
                                *count -= 1;
                                if *count == 0 {
                                    pending_input_jobs.remove(&tag.plugin_id);
                                }
                            }
                        }
                        let waited = enqueued_at.elapsed();
                        if tag.is_input && waited >= INPUT_WAIT_WARNING {
                            log::info!(
                                "plugin-exec-{}: input job ({}) for {} waited {}ms in the executor queue",
                                thread_idx,
                                tag.label,
                                plugin_name(&plugin_map, tag.plugin_id),
                                waited.as_millis()
                            );
                        }
                        let started = Instant::now();
                        work(
                            senders.clone(),
                            plugin_map.clone(),
                            connected_clients.clone(),
                            plugin_cache.clone(),
                            engine.clone(),
                        );
                        jobs_in_flight_clone.fetch_sub(1, Ordering::SeqCst);
                        record_job(&mut stats, &tag, waited, started.elapsed());
                        if stats_since.elapsed() >= STATS_LOG_EVERY {
                            log_job_stats(thread_idx, stats_since.elapsed(), &mut stats, &plugin_map);
                            stats_since = Instant::now();
                        }
                    }
                }
            });
        if let Err(e) = thread_handle {
            log::error!("Failed to spawn plugin execution thread: {}", e);
        }

        ExecutionThread {
            sender,
            jobs_in_flight,
        }
    }

    /// Register a plugin and assign it to a thread
    /// Called from wasm_bridge when loading a plugin
    pub fn register_plugin(&self, plugin_id: u32) -> usize {
        let mut assignments = self.plugin_assignments.lock().unwrap();

        // If already assigned (shouldn't happen, but defensive)
        if let Some(&thread_idx) = assignments.get(&plugin_id) {
            return thread_idx;
        }

        let mut thread_plugins = self.thread_plugins.lock().unwrap();
        let threads = self.execution_threads.lock().unwrap();

        // A thread of one's own while threads are to be had: a plugin's jobs
        // queue strictly behind its thread-mates', so co-tenancy is what a busy
        // plugin's latency is made of. The old policy preferred any *currently
        // idle* thread, which at load time — when every thread is idle — piled
        // the whole session onto thread 0.
        let empty_thread = threads
            .iter()
            .enumerate()
            .filter(|(_, t)| t.is_some())
            .map(|(idx, _)| idx)
            .find(|idx| {
                thread_plugins
                    .get(idx)
                    .map(|s| s.is_empty())
                    .unwrap_or(true)
            });

        let thread_idx = if let Some(idx) = empty_thread {
            idx
        } else if let Some(idx) = threads.iter().position(Option::is_none) {
            // Recreate a worker removed after its last plugin unloaded.
            drop(threads);
            self.add_thread(idx);
            idx
        } else if threads.len() < self.max_threads {
            // Every existing thread has a tenant - spawn a new one
            let new_idx = threads.len();
            drop(threads); // Release lock before spawning
            self.add_thread(new_idx);
            new_idx
        } else {
            // At max capacity, assign to least-loaded thread
            threads
                .iter()
                .enumerate()
                .filter_map(|(idx, t)| t.as_ref().map(|_| idx))
                .min_by_key(|&idx| thread_plugins.get(&idx).map(|s| s.len()).unwrap_or(0))
                .unwrap_or_else(|| {
                    log::error!("Failed to find free thread to run the plugin!");
                    0 // this is a misconfiguration, but we don't want to crash the app
                      // if it happens
                })
        };

        // Update mappings
        assignments.insert(plugin_id, thread_idx);
        thread_plugins
            .entry(thread_idx)
            .or_default()
            .insert(plugin_id);

        thread_idx
    }

    fn add_thread(&self, thread_idx: usize) {
        let mut threads = self.execution_threads.lock().unwrap();
        let new_thread = Self::spawn_thread(
            thread_idx,
            self.senders.clone(),
            self.plugin_map.clone(),
            self.connected_clients.clone(),
            self.plugin_cache.clone(),
            self.engine.clone(),
        );

        // Extend vector if needed
        while threads.len() <= thread_idx {
            threads.push(None);
        }
        threads[thread_idx] = Some(new_thread);
    }

    /// Execute job pinned to plugin's assigned thread
    pub fn execute_for_plugin<F>(&self, plugin_id: u32, f: F)
    where
        F: FnOnce(
                ThreadSenders,
                Arc<Mutex<PluginMap>>,
                Arc<Mutex<Vec<ClientId>>>,
                PluginCache,
                Engine,
            ) + Send
            + 'static,
    {
        self.execute_for_plugin_tagged(plugin_id, "job", false, f);
    }

    /// `execute_for_plugin` with a label for the worker's stats log, and
    /// whether the job carries user input (whose queue wait is keystroke
    /// latency and is logged on its own past `INPUT_WAIT_WARNING`).
    pub fn execute_for_plugin_tagged<F>(
        &self,
        plugin_id: u32,
        label: impl Into<String>,
        is_input: bool,
        f: F,
    ) where
        F: FnOnce(
                ThreadSenders,
                Arc<Mutex<PluginMap>>,
                Arc<Mutex<Vec<ClientId>>>,
                PluginCache,
                Engine,
            ) + Send
            + 'static,
    {
        // Look up assigned thread
        let thread_idx = {
            let assignments = self.plugin_assignments.lock().unwrap();
            assignments.get(&plugin_id).copied()
        };
        let Some(thread_idx) = thread_idx else {
            log::error!("Failed to find thread for plugin with id: {}", plugin_id);
            return;
        };

        // Get thread and mark as busy
        let threads = self.execution_threads.lock().unwrap();
        let thread = threads[thread_idx].as_ref();
        let Some(thread) = thread else {
            log::error!("Failed to find thread for plugin with id: {}", plugin_id);
            return;
        };

        // Increment busy counter BEFORE sending work
        thread.jobs_in_flight.fetch_add(1, Ordering::SeqCst);

        // Send work
        let job = Job::Work {
            tag: JobTag {
                plugin_id,
                label: label.into(),
                is_input,
            },
            enqueued_at: Instant::now(),
            work: Box::new(f),
        };
        if thread.sender.send(job).is_err() {
            // Thread died unexpectedly - this is a critical error
            thread.jobs_in_flight.fetch_sub(1, Ordering::SeqCst);
            log::error!("Plugin executor thread {} has died", thread_idx);
        }
    }

    /// Load a plugin: register it and execute the load work on its assigned thread
    /// This combines registration + execution for plugin loading
    pub fn execute_plugin_load<F>(&self, plugin_id: u32, f: F)
    where
        F: FnOnce(
                ThreadSenders,
                Arc<Mutex<PluginMap>>,
                Arc<Mutex<Vec<ClientId>>>,
                PluginCache,
                Engine,
            ) + Send
            + 'static,
    {
        // Register plugin and assign to a thread
        self.register_plugin(plugin_id);

        // Execute the load work on the assigned thread
        self.execute_for_plugin(plugin_id, f);
    }

    /// Unload a plugin: execute cleanup work, then unregister and potentially shrink pool
    /// This combines cleanup execution + unregistration for plugin unloading
    /// Requires Arc<Self> so we can clone it into the closure for unregistration
    pub fn execute_plugin_unload(
        self: &Arc<Self>,
        plugin_id: u32,
        f: impl FnOnce(
                ThreadSenders,
                Arc<Mutex<PluginMap>>,
                Arc<Mutex<Vec<ClientId>>>,
                PluginCache,
                Engine,
            ) + Send
            + 'static,
    ) {
        let executor = self.clone();
        self.execute_for_plugin(
            plugin_id,
            move |senders, plugin_map, connected_clients, plugin_cache, engine| {
                // Execute the cleanup work
                f(senders, plugin_map, connected_clients, plugin_cache, engine);

                // Unregister plugin and potentially shrink the pool
                executor.unregister_plugin(plugin_id);
            },
        );
    }

    /// Unregister a plugin and potentially shrink the pool
    /// Called from wasm_bridge after plugin cleanup is complete
    pub fn unregister_plugin(&self, plugin_id: u32) {
        let mut assignments = self.plugin_assignments.lock().unwrap();
        let mut thread_plugins = self.thread_plugins.lock().unwrap();

        if let Some(thread_idx) = assignments.remove(&plugin_id) {
            if let Some(plugins) = thread_plugins.get_mut(&thread_idx) {
                plugins.remove(&plugin_id);
            }
        }

        drop(assignments);
        drop(thread_plugins);

        // Try to shrink the pool
        self.try_shrink_pool();
    }

    fn try_shrink_pool(&self) {
        let mut threads = self.execution_threads.lock().unwrap();
        let thread_plugins = self.thread_plugins.lock().unwrap();

        // Find threads with no assigned plugins (except thread 0, always keep it)
        let threads_to_remove: Vec<usize> = threads
            .iter()
            .enumerate()
            .skip(1) // Never remove thread 0
            .filter_map(|(idx, thread_opt)| {
                if thread_opt.is_some() {
                    let has_plugins = thread_plugins
                        .get(&idx)
                        .map(|s| !s.is_empty())
                        .unwrap_or(false);
                    if !has_plugins {
                        Some(idx)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        // Shutdown and remove idle threads
        for idx in threads_to_remove {
            if let Some(thread) = threads[idx].take() {
                let _ = thread.sender.send(Job::Shutdown);
            }
        }
    }

    #[cfg(test)]
    pub fn thread_count(&self) -> usize {
        self.execution_threads
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t.is_some())
            .count()
    }
}

impl Drop for PinnedExecutor {
    fn drop(&mut self) {
        let mut threads = self.execution_threads.lock().unwrap();

        // Send shutdown to all threads
        for thread in threads.iter_mut().flatten() {
            let _ = thread.sender.send(Job::Shutdown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::Duration;

    // Test fixtures
    fn create_test_dependencies() -> (
        ThreadSenders,
        Arc<Mutex<PluginMap>>,
        Arc<Mutex<Vec<ClientId>>>,
        PluginCache,
        Engine,
    ) {
        use zellij_utils::channels::{self, SenderWithContext};

        let (send_to_pty, _receive_pty) = channels::bounded(1);
        let (send_to_screen, _receive_screen) = channels::bounded(1);
        let (send_to_plugin, _receive_plugin) = channels::bounded(1);
        let (send_to_server, _receive_server) = channels::bounded(1);
        let (send_to_pty_writer, _receive_pty_writer) = channels::bounded(1);
        let (send_to_background_jobs, _receive_background_jobs) = channels::bounded(1);

        let to_pty = SenderWithContext::new(send_to_pty);
        let to_screen = SenderWithContext::new(send_to_screen);
        let to_plugin = SenderWithContext::new(send_to_plugin);
        let to_server = SenderWithContext::new(send_to_server);
        let to_pty_writer = SenderWithContext::new(send_to_pty_writer);
        let to_background_jobs = SenderWithContext::new(send_to_background_jobs);

        let senders = ThreadSenders {
            to_pty: Some(to_pty),
            to_screen: Some(to_screen),
            to_plugin: Some(to_plugin),
            to_server: Some(to_server),
            to_pty_writer: Some(to_pty_writer),
            to_background_jobs: Some(to_background_jobs),
            session_recorder: None,
            should_silently_fail: false,
        };

        let plugin_map = Arc::new(Mutex::new(PluginMap::default()));
        let connected_clients = Arc::new(Mutex::new(vec![]));

        let plugin_cache = Arc::new(Mutex::new(std::collections::HashMap::new()));

        let engine = Engine::default();

        (senders, plugin_map, connected_clients, plugin_cache, engine)
    }

    fn create_test_executor(max_threads: usize) -> Arc<PinnedExecutor> {
        let (senders, plugin_map, clients, cache, engine) = create_test_dependencies();
        Arc::new(PinnedExecutor::new(
            max_threads,
            &senders,
            &plugin_map,
            &clients,
            &cache,
            &engine,
        ))
    }

    // Helper to create a job that signals completion via channel
    fn make_signaling_job(
        tx: Sender<()>,
    ) -> impl FnOnce(
        ThreadSenders,
        Arc<Mutex<PluginMap>>,
        Arc<Mutex<Vec<ClientId>>>,
        PluginCache,
        Engine,
    ) + Send
           + 'static {
        move |_senders, _plugin_map, _clients, _cache, _engine| {
            tx.send(()).unwrap();
        }
    }

    // Helper to verify thread assignment by capturing thread name in job
    fn get_thread_name_for_plugin(executor: &Arc<PinnedExecutor>, plugin_id: u32) -> String {
        let (tx, rx) = channel();
        executor.execute_for_plugin(plugin_id, move |_s, _p, _c, _ca, _e| {
            let name = thread::current().name().unwrap().to_string();
            tx.send(name).unwrap();
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Thread name should be received")
    }

    #[test]
    fn test_new_creates_one_thread() {
        let executor = create_test_executor(4);
        assert_eq!(executor.thread_count(), 1);
    }

    #[test]
    fn test_new_respects_min_threads() {
        let executor = create_test_executor(0);
        assert_eq!(
            executor.thread_count(),
            1,
            "Executor should enforce minimum of 1 thread"
        );
    }

    #[test]
    fn test_first_plugin_assigned_to_thread_zero() {
        let executor = create_test_executor(4);
        let thread_idx = executor.register_plugin(1);
        assert_eq!(thread_idx, 0);
    }

    #[test]
    fn test_plugin_name_does_not_wait_for_plugin_map() {
        let (_senders, plugin_map, _clients, _cache, _engine) = create_test_dependencies();
        let plugin_map_guard = plugin_map.lock().unwrap();
        let plugin_map_clone = plugin_map.clone();
        let (tx, rx) = channel();
        let lookup = thread::spawn(move || tx.send(plugin_name(&plugin_map_clone, 42)).unwrap());

        let name = rx.recv_timeout(Duration::from_secs(1));
        drop(plugin_map_guard);
        lookup.join().unwrap();

        assert_eq!(name.unwrap(), "plugin[42]");
    }

    #[test]
    fn test_plugins_spread_across_threads_before_sharing() {
        // A plugin's jobs queue strictly behind its thread-mates', so plugins
        // only share a thread once every thread has a tenant. The old policy
        // put everything registered at an idle moment on thread 0.
        let executor = create_test_executor(3);
        assert_eq!(executor.register_plugin(1), 0);
        assert_eq!(
            executor.register_plugin(2),
            1,
            "own thread while one can be spawned"
        );
        assert_eq!(executor.register_plugin(3), 2);
        let fourth = executor.register_plugin(4);
        assert!(
            fourth <= 2,
            "at capacity, the least-loaded thread is shared"
        );
        assert_eq!(executor.thread_count(), 3, "never exceeds max_threads");
    }

    #[test]
    fn test_vacant_thread_slot_is_reused_before_sharing() {
        let executor = create_test_executor(2);
        assert_eq!(executor.register_plugin(1), 0);
        assert_eq!(executor.register_plugin(2), 1);

        executor.unregister_plugin(2);

        assert_eq!(executor.thread_count(), 1);
        assert_eq!(executor.register_plugin(3), 1);
        assert_eq!(executor.thread_count(), 2);
    }

    #[test]
    fn test_input_jobs_run_before_other_plugins_queued_work() {
        // One thread, two plugins. While the thread is held, plugin 2 queues a
        // pile of work and plugin 1 queues a normal job and then an input job.
        // The input job makes plugin 1's *whole queue* go first — but never
        // reorders plugin 1's own jobs, which is the per-plugin FIFO the
        // batching tests pin.
        let executor = create_test_executor(1);
        executor.register_plugin(1);
        executor.register_plugin(2);

        let (started_tx, started_rx) = channel();
        let barrier = Arc::new(Barrier::new(2));
        let barrier_clone = barrier.clone();
        executor.execute_for_plugin(2, move |_s, _p, _c, _ca, _e| {
            started_tx.send(()).unwrap();
            barrier_clone.wait();
        });
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the blocker job should start");

        let order = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = channel();
        let queue = |plugin_id: u32, label: &'static str, is_input: bool| {
            let order = order.clone();
            let tx = tx.clone();
            executor.execute_for_plugin_tagged(
                plugin_id,
                label,
                is_input,
                move |_s, _p, _c, _ca, _e| {
                    order.lock().unwrap().push(label);
                    tx.send(()).unwrap();
                },
            );
        };
        queue(2, "b1", false);
        queue(2, "b2", false);
        queue(1, "a1", false);
        queue(1, "a2-input", true);
        queue(2, "b3", false);
        queue(1, "a3", false);
        queue(1, "a4-input", true);

        barrier.wait();
        for _ in 0..7 {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("queued job should complete");
        }

        assert_eq!(
            *order.lock().unwrap(),
            vec!["a1", "a2-input", "a3", "a4-input", "b1", "b2", "b3"],
            "the input-bearing plugin's queue runs first, in its own order"
        );
    }

    #[test]
    fn test_without_input_jobs_the_queue_stays_fifo() {
        let executor = create_test_executor(1);
        executor.register_plugin(1);
        executor.register_plugin(2);

        let (started_tx, started_rx) = channel();
        let barrier = Arc::new(Barrier::new(2));
        let barrier_clone = barrier.clone();
        executor.execute_for_plugin(2, move |_s, _p, _c, _ca, _e| {
            started_tx.send(()).unwrap();
            barrier_clone.wait();
        });
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the blocker job should start");

        let order = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = channel();
        let queue = |plugin_id: u32, label: &'static str| {
            let order = order.clone();
            let tx = tx.clone();
            executor.execute_for_plugin_tagged(
                plugin_id,
                label,
                false,
                move |_s, _p, _c, _ca, _e| {
                    order.lock().unwrap().push(label);
                    tx.send(()).unwrap();
                },
            );
        };
        queue(2, "b1");
        queue(1, "a1");
        queue(2, "b2");

        barrier.wait();
        for _ in 0..3 {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("queued job should complete");
        }

        assert_eq!(
            *order.lock().unwrap(),
            vec!["b1", "a1", "b2"],
            "no input anywhere means plain arrival order"
        );
    }

    #[test]
    fn test_new_thread_spawns_when_all_busy() {
        let executor = create_test_executor(3);

        // Register plugin 1, gets thread 0
        let thread_idx1 = executor.register_plugin(1);
        assert_eq!(thread_idx1, 0);

        // Make thread 0 busy with a barrier
        let barrier = Arc::new(Barrier::new(2));
        let barrier_clone = barrier.clone();
        executor.execute_for_plugin(1, move |_s, _p, _c, _ca, _e| {
            barrier_clone.wait();
        });

        // Give the job a moment to start executing and block
        thread::sleep(Duration::from_millis(50));

        // Register plugin 2 while thread 0 is busy
        let thread_idx2 = executor.register_plugin(2);
        assert_eq!(
            thread_idx2, 1,
            "Plugin 2 should get new thread 1 when thread 0 is busy"
        );

        // Verify thread count
        assert_eq!(executor.thread_count(), 2);

        // Release barrier
        barrier.wait();
    }

    #[test]
    fn test_respects_max_threads_limit() {
        let executor = create_test_executor(2);

        // Register plugin 1, gets thread 0
        executor.register_plugin(1);

        // Make thread 0 busy
        let barrier1 = Arc::new(Barrier::new(2));
        let barrier1_clone = barrier1.clone();
        executor.execute_for_plugin(1, move |_s, _p, _c, _ca, _e| {
            barrier1_clone.wait();
        });
        thread::sleep(Duration::from_millis(50));

        // Register plugin 2, gets thread 1
        executor.register_plugin(2);

        // Make thread 1 busy
        let barrier2 = Arc::new(Barrier::new(2));
        let barrier2_clone = barrier2.clone();
        executor.execute_for_plugin(2, move |_s, _p, _c, _ca, _e| {
            barrier2_clone.wait();
        });
        thread::sleep(Duration::from_millis(50));

        // Register plugin 3 when all threads busy
        let thread_idx3 = executor.register_plugin(3);
        assert!(
            thread_idx3 == 0 || thread_idx3 == 1,
            "Plugin 3 should be assigned to existing thread"
        );
        assert_eq!(executor.thread_count(), 2, "Should not exceed max_threads");

        // Release barriers
        barrier1.wait();
        barrier2.wait();
    }

    #[test]
    fn test_duplicate_registration_returns_same_thread() {
        let executor = create_test_executor(4);
        let thread_idx1 = executor.register_plugin(1);
        let thread_idx2 = executor.register_plugin(1);
        assert_eq!(
            thread_idx1, thread_idx2,
            "Duplicate registration should return same thread"
        );
    }

    #[test]
    fn test_load_balancing_prefers_least_loaded() {
        let executor = create_test_executor(2);

        // The first two get threads of their own.
        assert_eq!(executor.register_plugin(1), 0);
        assert_eq!(executor.register_plugin(2), 1);

        // At capacity, each newcomer lands on the least-loaded thread, so the
        // tenancy stays balanced.
        let third = executor.register_plugin(3);
        let fourth = executor.register_plugin(4);
        assert_ne!(
            third, fourth,
            "with one tenant each, consecutive registrations balance out"
        );
        assert_eq!(executor.thread_count(), 2, "never exceeds max_threads");
    }

    #[test]
    fn test_execute_for_plugin_runs_on_correct_thread() {
        let executor = create_test_executor(3);

        // Register plugin 1 to thread 0
        executor.register_plugin(1);

        // Make thread 0 busy to force plugin 2 to thread 1
        let barrier = Arc::new(Barrier::new(2));
        let barrier_clone = barrier.clone();
        executor.execute_for_plugin(1, move |_s, _p, _c, _ca, _e| {
            barrier_clone.wait();
        });
        thread::sleep(Duration::from_millis(50));

        // Register plugin 2 to thread 1
        executor.register_plugin(2);

        // Release barrier
        barrier.wait();
        thread::sleep(Duration::from_millis(50));

        // Get thread names for both plugins
        let thread_name1 = get_thread_name_for_plugin(&executor, 1);
        let thread_name2 = get_thread_name_for_plugin(&executor, 2);

        assert_eq!(thread_name1, "plugin-exec-0");
        assert_eq!(thread_name2, "plugin-exec-1");
    }

    #[test]
    fn test_execute_for_plugin_unregistered() {
        let executor = create_test_executor(4);
        let (tx, rx) = channel();

        // Execute job for unregistered plugin
        executor.execute_for_plugin(999, make_signaling_job(tx));

        // Try to receive with timeout - should timeout
        let result = rx.recv_timeout(Duration::from_millis(100));
        assert!(
            result.is_err(),
            "Job for unregistered plugin should not execute"
        );
    }

    #[test]
    fn test_job_execution_order_per_thread() {
        let executor = create_test_executor(4);
        executor.register_plugin(1);

        let order = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = channel();

        // Execute 3 jobs for plugin 1
        for i in 1..=3 {
            let order_clone = order.clone();
            let tx_clone = tx.clone();
            executor.execute_for_plugin(1, move |_s, _p, _c, _ca, _e| {
                order_clone.lock().unwrap().push(i);
                tx_clone.send(()).unwrap();
            });
        }

        // Wait for all 3 jobs to complete
        for _ in 0..3 {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("Job should complete");
        }

        assert_eq!(
            *order.lock().unwrap(),
            vec![1, 2, 3],
            "Jobs should execute in order"
        );
    }

    #[test]
    fn test_jobs_complete_successfully() {
        let executor = create_test_executor(4);
        executor.register_plugin(1);

        let (tx, rx) = channel();
        executor.execute_for_plugin(1, make_signaling_job(tx));

        let result = rx.recv_timeout(Duration::from_secs(5));
        assert!(result.is_ok(), "Job should complete successfully");
    }

    #[test]
    fn test_concurrent_jobs_on_different_threads() {
        let executor = create_test_executor(3);

        // Register plugin 1 to thread 0
        executor.register_plugin(1);

        // Make thread 0 busy to force plugin 2 to thread 1
        let barrier = Arc::new(Barrier::new(2));
        let barrier_clone = barrier.clone();
        executor.execute_for_plugin(1, move |_s, _p, _c, _ca, _e| {
            barrier_clone.wait();
        });
        thread::sleep(Duration::from_millis(50));

        // Register plugin 2 to thread 1
        executor.register_plugin(2);

        // Release barrier
        barrier.wait();
        thread::sleep(Duration::from_millis(50));

        // Execute jobs on both threads concurrently
        let sync_barrier = Arc::new(Barrier::new(3)); // 2 jobs + test thread
        let (tx1, rx1) = channel();
        let (tx2, rx2) = channel();

        let barrier1 = sync_barrier.clone();
        executor.execute_for_plugin(1, move |_s, _p, _c, _ca, _e| {
            barrier1.wait();
            tx1.send(()).unwrap();
        });

        let barrier2 = sync_barrier.clone();
        executor.execute_for_plugin(2, move |_s, _p, _c, _ca, _e| {
            barrier2.wait();
            tx2.send(()).unwrap();
        });

        // Release both jobs simultaneously
        sync_barrier.wait();

        // Both should complete
        assert!(rx1.recv_timeout(Duration::from_secs(5)).is_ok());
        assert!(rx2.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn test_execute_plugin_load_registers_and_executes() {
        let executor = create_test_executor(4);

        let (tx, rx) = channel();
        executor.execute_plugin_load(1, make_signaling_job(tx));

        // Wait for load to complete
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");

        // Verify plugin is registered
        let thread_idx = executor.register_plugin(1);
        assert_eq!(thread_idx, 0, "Plugin should already be registered");
    }

    #[test]
    fn test_execute_plugin_unload_runs_cleanup_before_unregister() {
        let executor = create_test_executor(4);

        // Load plugin
        let (tx_load, rx_load) = channel();
        executor.execute_plugin_load(1, make_signaling_job(tx_load));
        rx_load
            .recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");

        // Unload plugin with cleanup
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let (tx_unload, rx_unload) = channel();

        executor.execute_plugin_unload(1, move |_s, _p, _c, _ca, _e| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            tx_unload.send(()).unwrap();
        });

        // Wait for unload to complete
        rx_unload
            .recv_timeout(Duration::from_secs(5))
            .expect("Unload should complete");

        // Verify cleanup ran
        assert_eq!(counter.load(Ordering::SeqCst), 1, "Cleanup should have run");

        // Give unregister a moment to complete
        thread::sleep(Duration::from_millis(100));

        // Plugin should be unregistered - registering again should assign new thread
        let thread_idx = executor.register_plugin(1);
        assert_eq!(thread_idx, 0, "Plugin should be re-registered to thread 0");
    }

    #[test]
    fn test_unload_sequence_is_correct() {
        let executor = create_test_executor(4);

        // Load plugin
        let (tx_load, rx_load) = channel();
        executor.execute_plugin_load(1, make_signaling_job(tx_load));
        rx_load
            .recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");

        // Unload with sequence tracking
        let sequence = Arc::new(Mutex::new(Vec::new()));
        let sequence_clone = sequence.clone();
        let (tx_unload, rx_unload) = channel();

        executor.execute_plugin_unload(1, move |_s, _p, _c, _ca, _e| {
            sequence_clone.lock().unwrap().push("cleanup");
            tx_unload.send(()).unwrap();
        });

        // Wait for unload to complete
        rx_unload
            .recv_timeout(Duration::from_secs(5))
            .expect("Unload should complete");
        sequence.lock().unwrap().push("after");

        assert_eq!(*sequence.lock().unwrap(), vec!["cleanup", "after"]);
    }

    #[test]
    fn test_shrink_removes_idle_threads() {
        let executor = create_test_executor(4);

        // Load plugin 1 to thread 0
        let (tx1, rx1) = channel();
        executor.execute_plugin_load(1, make_signaling_job(tx1));
        rx1.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");

        // Make thread 0 busy to force plugin 2 to thread 1
        let barrier = Arc::new(Barrier::new(2));
        let barrier_clone = barrier.clone();
        executor.execute_for_plugin(1, move |_s, _p, _c, _ca, _e| {
            barrier_clone.wait();
        });
        thread::sleep(Duration::from_millis(50));

        // Load plugin 2 to thread 1
        let (tx2, rx2) = channel();
        executor.execute_plugin_load(2, make_signaling_job(tx2));
        rx2.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");

        // Release barrier
        barrier.wait();
        thread::sleep(Duration::from_millis(50));

        let thread_count_before = executor.thread_count();
        assert!(thread_count_before >= 2, "Should have at least 2 threads");

        // Unload plugin 2
        let (tx_unload2, rx_unload2) = channel();
        executor.execute_plugin_unload(2, make_signaling_job(tx_unload2));
        rx_unload2
            .recv_timeout(Duration::from_secs(5))
            .expect("Unload should complete");

        // Give shrinking a moment to complete
        thread::sleep(Duration::from_millis(100));

        // Thread count should decrease after unloading
        let thread_count_after = executor.thread_count();
        assert!(
            thread_count_after < thread_count_before,
            "Idle threads should be removed"
        );
        assert!(thread_count_after >= 1, "Thread 0 should remain");
    }

    #[test]
    fn test_thread_zero_never_removed() {
        let executor = create_test_executor(4);

        // Load a plugin and then unload it
        let (tx_load, rx_load) = channel();
        executor.execute_plugin_load(1, make_signaling_job(tx_load));
        rx_load
            .recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");

        let (tx_unload, rx_unload) = channel();
        executor.execute_plugin_unload(1, make_signaling_job(tx_unload));
        rx_unload
            .recv_timeout(Duration::from_secs(5))
            .expect("Unload should complete");

        // Give shrinking a moment
        thread::sleep(Duration::from_millis(100));

        // Thread 0 should remain
        assert!(
            executor.thread_count() >= 1,
            "Thread 0 should never be removed"
        );
    }

    #[test]
    fn test_active_threads_not_removed() {
        let executor = create_test_executor(4);

        // Load plugin 1 to thread 0
        let (tx1, rx1) = channel();
        executor.execute_plugin_load(1, make_signaling_job(tx1));
        rx1.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");

        // Force plugin 2 to thread 1 by making thread 0 busy
        let barrier = Arc::new(Barrier::new(2));
        let barrier_clone = barrier.clone();
        executor.execute_for_plugin(1, move |_s, _p, _c, _ca, _e| {
            barrier_clone.wait();
        });
        thread::sleep(Duration::from_millis(50));

        let (tx2, rx2) = channel();
        executor.execute_plugin_load(2, make_signaling_job(tx2));
        rx2.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");

        barrier.wait();
        thread::sleep(Duration::from_millis(100));

        let thread_count_with_both = executor.thread_count();
        assert!(
            thread_count_with_both >= 2,
            "Should have at least 2 threads with 2 plugins"
        );

        // Unload plugin 2
        let (tx_unload, rx_unload) = channel();
        executor.execute_plugin_unload(2, make_signaling_job(tx_unload));
        rx_unload
            .recv_timeout(Duration::from_secs(5))
            .expect("Unload should complete");

        thread::sleep(Duration::from_millis(100));

        // Plugin 1's thread should still work (verify active threads not affected)
        let (tx_test, rx_test) = channel();
        executor.execute_for_plugin(1, make_signaling_job(tx_test));
        assert!(
            rx_test.recv_timeout(Duration::from_secs(5)).is_ok(),
            "Plugin 1's thread should still work"
        );

        // Thread count should decrease after unloading
        let thread_count_after = executor.thread_count();
        assert!(
            thread_count_after < thread_count_with_both,
            "Idle thread should be removed"
        );
        assert!(thread_count_after >= 1, "Active threads should remain");
    }

    #[test]
    fn test_shrink_does_not_affect_remaining_threads() {
        let executor = create_test_executor(4);

        // Load plugin 1 to thread 0
        let (tx1, rx1) = channel();
        executor.execute_plugin_load(1, make_signaling_job(tx1));
        rx1.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");

        // Force plugin 2 to thread 1
        let barrier = Arc::new(Barrier::new(2));
        let barrier_clone = barrier.clone();
        executor.execute_for_plugin(1, move |_s, _p, _c, _ca, _e| {
            barrier_clone.wait();
        });
        thread::sleep(Duration::from_millis(50));

        let (tx2, rx2) = channel();
        executor.execute_plugin_load(2, make_signaling_job(tx2));
        rx2.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");

        barrier.wait();
        thread::sleep(Duration::from_millis(50));

        // Unload plugin 2 (shrinks pool)
        let (tx_unload, rx_unload) = channel();
        executor.execute_plugin_unload(2, make_signaling_job(tx_unload));
        rx_unload
            .recv_timeout(Duration::from_secs(5))
            .expect("Unload should complete");

        thread::sleep(Duration::from_millis(100));

        // Execute job for plugin 1
        let (tx_test, rx_test) = channel();
        executor.execute_for_plugin(1, make_signaling_job(tx_test));

        assert!(
            rx_test.recv_timeout(Duration::from_secs(5)).is_ok(),
            "Plugin 1's thread should still work"
        );
    }

    #[test]
    fn test_drop_cleans_up_gracefully() {
        let executor = create_test_executor(4);

        // Load multiple plugins on different threads
        let (tx1, rx1) = channel();
        executor.execute_plugin_load(1, make_signaling_job(tx1));
        rx1.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");

        let barrier = Arc::new(Barrier::new(2));
        let barrier_clone = barrier.clone();
        executor.execute_for_plugin(1, move |_s, _p, _c, _ca, _e| {
            barrier_clone.wait();
        });
        thread::sleep(Duration::from_millis(50));

        let (tx2, rx2) = channel();
        executor.execute_plugin_load(2, make_signaling_job(tx2));
        rx2.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");

        barrier.wait();

        // Drop executor
        drop(executor);

        // Test completes without panic
    }

    #[test]
    fn test_drop_with_jobs_in_flight() {
        let executor = create_test_executor(4);
        executor.register_plugin(1);

        let barrier = Arc::new(Barrier::new(2));
        let barrier_clone = barrier.clone();
        executor.execute_for_plugin(1, move |_s, _p, _c, _ca, _e| {
            barrier_clone.wait();
        });

        // Drop executor while job is blocked
        drop(executor);

        // Release barrier (job may or may not complete, but shouldn't panic)
        barrier.wait();

        // Test completes without panic
    }

    #[test]
    fn test_concurrent_plugin_registrations() {
        let executor = create_test_executor(4);

        let handles: Vec<_> = (1..=10)
            .map(|i| {
                let exec = executor.clone();
                thread::spawn(move || exec.register_plugin(i))
            })
            .collect();

        for handle in handles {
            let thread_idx = handle.join().expect("Thread should not panic");
            assert!(thread_idx < 4, "Thread index should be valid");
        }
    }

    #[test]
    fn test_unregister_nonexistent_plugin() {
        let executor = create_test_executor(4);

        // Unregister non-existent plugin
        executor.unregister_plugin(999);

        // Executor should still work
        let (tx, rx) = channel();
        executor.execute_plugin_load(1, make_signaling_job(tx));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn test_max_threads_one() {
        let executor = create_test_executor(1);

        // Register multiple plugins
        let thread_idx1 = executor.register_plugin(1);
        let thread_idx2 = executor.register_plugin(2);
        let thread_idx3 = executor.register_plugin(3);

        assert_eq!(thread_idx1, 0);
        assert_eq!(thread_idx2, 0);
        assert_eq!(thread_idx3, 0);
        assert_eq!(executor.thread_count(), 1);
    }

    #[test]
    fn test_rapid_load_unload_cycles() {
        let executor = create_test_executor(4);

        // Load plugin 1
        let (tx1, rx1) = channel();
        executor.execute_plugin_load(1, make_signaling_job(tx1));
        rx1.recv_timeout(Duration::from_secs(5))
            .expect("Load should complete");

        // Unload plugin 1
        let (tx2, rx2) = channel();
        executor.execute_plugin_unload(1, make_signaling_job(tx2));
        rx2.recv_timeout(Duration::from_secs(5))
            .expect("Unload should complete");

        thread::sleep(Duration::from_millis(100));

        // Load plugin 1 again
        let (tx3, rx3) = channel();
        executor.execute_plugin_load(1, make_signaling_job(tx3));
        rx3.recv_timeout(Duration::from_secs(5))
            .expect("Second load should complete");

        // Execute job for plugin 1
        let (tx4, rx4) = channel();
        executor.execute_for_plugin(1, make_signaling_job(tx4));
        assert!(
            rx4.recv_timeout(Duration::from_secs(5)).is_ok(),
            "Executor should handle cycles correctly"
        );
    }

    #[test]
    fn test_many_plugins_limited_threads() {
        let executor = create_test_executor(4);
        let (tx, rx) = channel();

        // Load 20 plugins
        for i in 1..=20 {
            let tx_clone = tx.clone();
            executor.execute_plugin_load(i, make_signaling_job(tx_clone));
        }

        // Collect 20 completion signals
        for _ in 1..=20 {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("Load should complete");
        }

        assert!(
            executor.thread_count() <= 4,
            "Should not exceed max_threads"
        );
    }

    #[test]
    fn test_full_lifecycle() {
        let executor = create_test_executor(4);
        let (tx, rx) = channel();

        // Load 5 plugins
        for i in 1..=5 {
            let tx_clone = tx.clone();
            executor.execute_plugin_load(i, make_signaling_job(tx_clone));
        }

        // Wait for all loads
        for _ in 0..5 {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("Load should complete");
        }

        // Execute 2 jobs per plugin (10 total)
        for i in 1..=5 {
            for _ in 0..2 {
                let tx_clone = tx.clone();
                executor.execute_for_plugin(i, make_signaling_job(tx_clone));
            }
        }

        // Wait for all jobs
        for _ in 0..10 {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("Job should complete");
        }

        let thread_count_before = executor.thread_count();

        // Unload 3 plugins
        for i in 1..=3 {
            let tx_clone = tx.clone();
            executor.execute_plugin_unload(i, make_signaling_job(tx_clone));
        }

        // Wait for unloads
        for _ in 0..3 {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("Unload should complete");
        }

        thread::sleep(Duration::from_millis(100));

        // Thread count should decrease or stay the same
        let thread_count_after = executor.thread_count();
        assert!(
            thread_count_after <= thread_count_before,
            "Thread count should decrease after unloads"
        );

        // Execute jobs for remaining plugins
        for i in 4..=5 {
            let tx_clone = tx.clone();
            executor.execute_for_plugin(i, make_signaling_job(tx_clone));
        }

        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("Job should complete");
        }

        // Drop executor
        drop(executor);
    }

    #[test]
    fn test_realistic_plugin_churn() {
        let executor = create_test_executor(4);
        let (tx, rx) = channel();

        // Load plugins 1, 2, 3
        for i in 1..=3 {
            let tx_clone = tx.clone();
            executor.execute_plugin_load(i, make_signaling_job(tx_clone));
        }
        for _ in 0..3 {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("Load should complete");
        }

        // Execute jobs for each
        for i in 1..=3 {
            let tx_clone = tx.clone();
            executor.execute_for_plugin(i, make_signaling_job(tx_clone));
        }
        for _ in 0..3 {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("Job should complete");
        }

        // Unload plugin 2
        let tx_clone = tx.clone();
        executor.execute_plugin_unload(2, make_signaling_job(tx_clone));
        rx.recv_timeout(Duration::from_secs(5))
            .expect("Unload should complete");

        thread::sleep(Duration::from_millis(100));

        // Load plugins 4, 5
        for i in 4..=5 {
            let tx_clone = tx.clone();
            executor.execute_plugin_load(i, make_signaling_job(tx_clone));
        }
        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("Load should complete");
        }

        // Execute jobs for plugins 1, 3, 4, 5
        for i in &[1, 3, 4, 5] {
            let tx_clone = tx.clone();
            executor.execute_for_plugin(*i, make_signaling_job(tx_clone));
        }
        for _ in 0..4 {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("Job should complete");
        }

        // Unload plugins 1, 3
        for i in &[1, 3] {
            let tx_clone = tx.clone();
            executor.execute_plugin_unload(*i, make_signaling_job(tx_clone));
        }
        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("Unload should complete");
        }

        thread::sleep(Duration::from_millis(100));

        // Verify thread count reflects active plugins (4 and 5)
        assert!(
            executor.thread_count() >= 1,
            "Should have at least thread 0"
        );

        drop(executor);
    }
}

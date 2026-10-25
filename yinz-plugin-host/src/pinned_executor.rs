use crate::plugin_map::PluginMap;
use crate::ClientId;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use yinz_bus::thread_bus::ThreadSenders;
use yinz_types::internal_ids::PluginId;

/// A dynamic thread pool that pins jobs to specific threads based on plugin_id
/// Starts with 1 thread and expands when threads are busy, shrinks when plugins unload
pub struct PinnedExecutor {
    // All pool bookkeeping under one lock. See `ExecutorState`.
    state: Arc<Mutex<ExecutorState>>,

    // Maximum threads allowed
    max_threads: usize,

    // state to send to plugins (to be kept on execution threads)
    senders: ThreadSenders,
    plugin_map: Arc<Mutex<PluginMap>>,
    connected_clients: Arc<Mutex<Vec<ClientId>>>,
}

/// The pool's mutable bookkeeping, behind a single mutex.
///
/// This was once three separate mutexes (the two fields below plus an inverse
/// thread -> plugins index), and `register_plugin` and the pool-shrink path took
/// two of them in opposite orders — an ABBA inversion that could wedge the plugin
/// thread and an execution thread permanently, which then blocked session shutdown
/// on `plugin_thread.join()`. Holding everything under one lock makes the ordering
/// unrepresentable, and also closes the check-then-act window `execute_for_plugin`
/// had between looking up a plugin's thread and using it. Spawning a thread for a
/// growing pool happens under this lock too: the old code dropped the thread lock
/// to spawn while still holding the other two, which was the inversion's other
/// half. The inverse index is gone as well: it existed only so per-thread load
/// could be read under its own lock, and keeping two maps in lock-step was a
/// desync waiting to happen.
///
/// What the lock does not do is order jobs against unregistration: a plugin's
/// assignment goes away when its `execute_plugin_unload` job finishes, and any job
/// submitted for that plugin afterwards is dropped with a log line. A caller with
/// several cleanup jobs for one plugin (one per client, in `PluginBridge::unload_plugin`)
/// must therefore unregister from the last one only; see `execute_plugin_unload`.
struct ExecutorState {
    // Sparse: Some(thread) for a live slot, None for a reclaimed one. A vacant slot
    // is reused before the vector grows, so churning `max_threads` plugins through
    // load/unload cannot strand every later plugin on thread 0.
    threads: Vec<Option<ExecutionThread>>,
    // plugin_id -> its pinned thread index.
    assignments: HashMap<PluginId, usize>,
}

impl ExecutorState {
    /// How many plugins are pinned to `thread_idx`. A linear pass over
    /// `assignments`: it runs only on plugin load and unload, never on the
    /// per-job `execute_for_plugin` path, so deriving it beats maintaining an
    /// inverse index.
    fn load_of(&self, thread_idx: usize) -> usize {
        self.assignments
            .values()
            .filter(|&&idx| idx == thread_idx)
            .count()
    }
}

struct ExecutionThread {
    sender: Sender<Job>,
    jobs_in_flight: Arc<AtomicUsize>, // Busy state tracking
}

/// A job's closure, handed the executor's shared state when it runs on its pinned thread.
type JobWork = Box<
    dyn FnOnce(ThreadSenders, Arc<Mutex<PluginMap>>, Arc<Mutex<Vec<ClientId>>>) + Send + 'static,
>;

type PendingJob = (JobTag, Instant, JobWork);

/// What the executor knows about a queued job beyond its closure: whose it is,
/// a short label for the stats log ("Key", "PaneUpdate×2", "pipe:agent_hook"),
/// and whether it carries user input — the one kind of job whose queue wait is
/// somebody's keystroke latency.
struct JobTag {
    plugin_id: PluginId,
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
    stats: &mut HashMap<PluginId, PluginJobStats>,
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
    pending_input_jobs: &mut HashMap<PluginId, usize>,
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
    pending_input_jobs: &HashMap<PluginId, usize>,
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
fn plugin_name(plugin_map: &Arc<Mutex<PluginMap>>, plugin_id: PluginId) -> String {
    let location = plugin_map.try_lock().ok().and_then(|plugin_map| {
        plugin_map
            .run_plugin_of_plugin_id(plugin_id)
            .map(|run_plugin| {
                format!(
                    "{} {}",
                    run_plugin.location,
                    plugin_map.log_identity(plugin_id).unwrap_or_default()
                )
            })
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
    stats: &mut HashMap<PluginId, PluginJobStats>,
    plugin_map: &Arc<Mutex<PluginMap>>,
) {
    let mut entries: Vec<(PluginId, PluginJobStats)> = stats.drain().collect();
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
    ) -> Self {
        let max_threads = max_threads.max(1); // At least 1

        let thread_0 = Self::spawn_thread(
            0,
            senders.clone(),
            plugin_map.clone(),
            connected_clients.clone(),
        );

        PinnedExecutor {
            state: Arc::new(Mutex::new(ExecutorState {
                threads: vec![Some(thread_0)],
                assignments: HashMap::new(),
            })),
            max_threads,
            senders: senders.clone(),
            plugin_map: plugin_map.clone(),
            connected_clients: connected_clients.clone(),
        }
    }

    /// Build a fresh execution thread named for the slot it will occupy, using this
    /// executor's shared state. The static `spawn_thread` still does the work; this
    /// only supplies the clones so callers holding the state lock stay terse.
    fn build_thread(&self, thread_idx: usize) -> ExecutionThread {
        Self::spawn_thread(
            thread_idx,
            self.senders.clone(),
            self.plugin_map.clone(),
            self.connected_clients.clone(),
        )
    }

    fn spawn_thread(
        thread_idx: usize,
        senders: ThreadSenders,
        plugin_map: Arc<Mutex<PluginMap>>,
        connected_clients: Arc<Mutex<Vec<ClientId>>>,
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
                    let mut stats: HashMap<PluginId, PluginJobStats> = HashMap::new();
                    let mut stats_since = Instant::now();
                    // Jobs taken off the channel but not yet run. Held locally so
                    // the pick below can see what is waiting: strict channel FIFO
                    // would make a keystroke wait behind every other plugin's
                    // queued work. Per-plugin order is still absolute — only the
                    // interleaving *between* plugins is rearranged.
                    let mut pending: VecDeque<PendingJob> = VecDeque::new();
                    let mut pending_input_jobs: HashMap<PluginId, usize> = HashMap::new();
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
                        if tag.is_input
                            && let Some(count) = pending_input_jobs.get_mut(&tag.plugin_id) {
                                *count -= 1;
                                if *count == 0 {
                                    pending_input_jobs.remove(&tag.plugin_id);
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
                        );
                        jobs_in_flight_clone.fetch_sub(1, Ordering::SeqCst);
                        let elapsed = started.elapsed();
                        if tag.label == "load" {
                            log::info!(
                                "startup: plugin-exec-{thread_idx} {} phase=executor_load queue={waited:?} execution={elapsed:?}",
                                plugin_name(&plugin_map, tag.plugin_id)
                            );
                        }
                        record_job(&mut stats, &tag, waited, elapsed);
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
    /// Called from plugin_bridge when loading a plugin
    pub fn register_plugin(&self, plugin_id: PluginId) -> usize {
        let mut state = self.state.lock().unwrap();

        // If already assigned (shouldn't happen, but defensive)
        if let Some(&thread_idx) = state.assignments.get(&plugin_id) {
            return thread_idx;
        }

        // A thread of one's own while threads are to be had: a plugin's jobs
        // queue strictly behind its thread-mates', so co-tenancy is what a busy
        // plugin's latency is made of. The old policy preferred any *currently
        // idle* thread, which at load time — when every thread is idle — piled
        // the whole session onto thread 0. See `docs/PLUGIN_RENDER_PIPELINE.md`.
        let empty_thread = state
            .threads
            .iter()
            .enumerate()
            .filter_map(|(idx, t)| t.as_ref().map(|_| idx))
            .find(|&idx| state.load_of(idx) == 0);

        let thread_idx = if let Some(idx) = empty_thread {
            idx
        } else if state.threads.iter().flatten().count() < self.max_threads {
            // Every live thread has a tenant and we may grow: reuse the first vacant
            // slot left by a prior shrink, else append. Counting live threads (not the
            // vector length, which includes reclaimed `None` slots) is what keeps a
            // churned pool from being wrongly seen as full and pinning everything on
            // thread 0. The spawn happens under the lock on purpose; see
            // `ExecutorState`.
            let idx = state
                .threads
                .iter()
                .position(Option::is_none)
                .unwrap_or(state.threads.len());
            let new_thread = self.build_thread(idx);
            if idx < state.threads.len() {
                state.threads[idx] = Some(new_thread);
            } else {
                state.threads.push(Some(new_thread));
            }
            idx
        } else {
            // At capacity: assign to the least-loaded live thread.
            state
                .threads
                .iter()
                .enumerate()
                .filter_map(|(idx, t)| t.as_ref().map(|_| idx))
                .min_by_key(|&idx| state.load_of(idx))
                // Unreachable: this branch runs only when live threads >=
                // max_threads >= 1, and thread 0 is never reclaimed
                // (`unregister_plugin` shrinks from index 1 up), so there is always
                // a live thread to pick. The `0` only spares an `unwrap`.
                .unwrap_or(0)
        };

        state.assignments.insert(plugin_id, thread_idx);

        thread_idx
    }

    /// Execute job pinned to plugin's assigned thread
    #[cfg(test)]
    pub fn execute_for_plugin<F>(&self, plugin_id: PluginId, f: F)
    where
        F: FnOnce(ThreadSenders, Arc<Mutex<PluginMap>>, Arc<Mutex<Vec<ClientId>>>) + Send + 'static,
    {
        self.execute_for_plugin_tagged(plugin_id, "job", false, f);
    }

    /// `execute_for_plugin` with a label for the worker's stats log, and
    /// whether the job carries user input (whose queue wait is keystroke
    /// latency and is logged on its own past `INPUT_WAIT_WARNING`).
    pub fn execute_for_plugin_tagged<F>(
        &self,
        plugin_id: PluginId,
        label: impl Into<String>,
        is_input: bool,
        f: F,
    ) -> bool
    where
        F: FnOnce(ThreadSenders, Arc<Mutex<PluginMap>>, Arc<Mutex<Vec<ClientId>>>) + Send + 'static,
    {
        // One continuous lock over the lookup and the send: the assignment and the
        // thread it names are read together, so a concurrent unregister/shrink cannot
        // reclaim the slot between the two and strand this job. The channel is
        // unbounded, so sending under the lock never blocks.
        let state = self.state.lock().unwrap();
        let Some(&thread_idx) = state.assignments.get(&plugin_id) else {
            log::error!("Failed to find thread for plugin with id: {}", plugin_id);
            return false;
        };
        let Some(Some(thread)) = state.threads.get(thread_idx) else {
            log::error!("Failed to find thread for plugin with id: {}", plugin_id);
            return false;
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
            return false;
        }
        true
    }

    /// Load a plugin: register it and execute the load work on its assigned thread
    /// This combines registration + execution for plugin loading
    #[cfg(test)]
    pub fn execute_plugin_load<F>(&self, plugin_id: PluginId, f: F) -> bool
    where
        F: FnOnce(ThreadSenders, Arc<Mutex<PluginMap>>, Arc<Mutex<Vec<ClientId>>>) + Send + 'static,
    {
        // Register plugin and assign to a thread
        self.register_plugin(plugin_id);

        // Execute the load work on the assigned thread
        self.execute_for_plugin_tagged(plugin_id, "load", false, f)
    }

    /// Unload a plugin: execute cleanup work, then unregister and potentially shrink pool
    /// This combines cleanup execution + unregistration for plugin unloading
    /// Requires Arc<Self> so we can clone it into the closure for unregistration
    ///
    /// The plugin is unregistered when *this* job finishes, and a job submitted for
    /// an unregistered plugin is dropped. A caller that queues several cleanup jobs
    /// for one plugin must therefore submit all but the last through
    /// `execute_for_plugin` and only the last through this method: jobs for one
    /// plugin run in FIFO order on its pinned thread, so the unregister then lands
    /// after every cleanup. The worker's input-priority pick (`pick_next_job`)
    /// keeps that promise — it reorders only *between* plugins, never one plugin's
    /// own jobs — so this contract and that scheduling depend on each other.
    pub fn execute_plugin_unload(
        self: &Arc<Self>,
        plugin_id: PluginId,
        f: impl FnOnce(ThreadSenders, Arc<Mutex<PluginMap>>, Arc<Mutex<Vec<ClientId>>>) + Send + 'static,
    ) -> bool {
        let executor = self.clone();
        self.execute_for_plugin_tagged(
            plugin_id,
            "unload",
            false,
            move |senders, plugin_map, connected_clients| {
                // Execute the cleanup work
                f(senders, plugin_map, connected_clients);

                // Unregister plugin and potentially shrink the pool
                executor.unregister_plugin(plugin_id);
            },
        )
    }

    /// Unregister a plugin and shrink the pool if that left a thread idle.
    /// Called from plugin_bridge after plugin cleanup is complete.
    ///
    /// The shrink is folded in under the same lock rather than being a second
    /// method that re-locks: separating them is exactly what created the ABBA
    /// inversion this refactor removes.
    pub fn unregister_plugin(&self, plugin_id: PluginId) {
        let mut state = self.state.lock().unwrap();

        state.assignments.remove(&plugin_id);

        // Reclaim any now-idle thread other than thread 0, leaving a `None` in its
        // slot for `register_plugin` to reuse. Indices are collected first so the
        // vector is only borrowed mutably once the read pass is done.
        let idle: Vec<usize> = (1..state.threads.len())
            .filter(|&idx| {
                let live = state.threads.get(idx).map(Option::is_some).unwrap_or(false);
                live && state.load_of(idx) == 0
            })
            .collect();
        for idx in idle {
            if let Some(thread) = state.threads[idx].take() {
                let _ = thread.sender.send(Job::Shutdown);
            }
        }
    }

    #[cfg(test)]
    pub fn thread_count(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .threads
            .iter()
            .filter(|t| t.is_some())
            .count()
    }
}

impl Drop for PinnedExecutor {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();

        // Send shutdown to all threads
        for thread in state.threads.iter_mut().flatten() {
            let _ = thread.sender.send(Job::Shutdown);
        }
    }
}

#[cfg(test)]
#[path = "unit/pinned_executor.rs"]
mod tests;

//! Named work queues: plugin code that runs later, when a slot is free.
//!
//! A plugin puts a job — a plugin url plus configuration — on a queue by name. Each queue runs at
//! most `concurrency` jobs at once, every job as its own fresh headless plugin instance, and hands
//! the enqueuer [`Event::QueueJobStarted`] and [`Event::QueueJobFinished`] with its context echoed
//! back. A job ends when its plugin unloads, however that happens: it called `FinishQueueJob`, it
//! was cancelled, it closed itself, its pane was closed, or it crashed.
//!
//! This module is the bookkeeping only. It never launches or unloads anything: `PluginBridge` asks
//! it what to launch with [`Queues::pop_launchable`], tells it what happened with
//! [`Queues::on_job_launched`] / [`Queues::on_launch_failed`] / [`Queues::on_plugin_unloaded`],
//! and emits the events. Keeping it free of threads and senders is what makes it testable as a
//! plain state machine, and keeps every "when does a slot free up" rule in one place.
//!
//! Two identities are in play and must not be confused: a **job id** is a monotonic `u64` handed
//! to the enqueuer and never reused; a **plugin id** is the running job's instance, which the
//! server reuses freely once the plugin unloads. `running` is keyed by plugin id because that is
//! what an unload reports, and `plugin_to_queue` is the reverse index that makes the unload hook
//! a single lookup rather than a scan of every queue.
//!
//! [`Event::QueueJobStarted`]: yinz_types::data::Event::QueueJobStarted
//! [`Event::QueueJobFinished`]: yinz_types::data::Event::QueueJobFinished

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;

use yinz_types::input::layout::RunPluginOrAlias;
use {
    yinz_types::data::QueueJobInfo, yinz_types::data::QueueJobOutcome,
    yinz_types::data::QueueJobState, yinz_types::data::QueueSnapshot,
};

use super::PluginId;
use crate::ClientId;

pub use yinz_types::internal_ids::QueueJobId as JobId;

/// A queue that nobody configured runs one job at a time.
pub const DEFAULT_CONCURRENCY: usize = 1;

/// Everything the enqueuer handed over, resolved and ready to launch.
#[derive(Debug, Clone)]
pub struct QueuedJob {
    pub job_id: JobId,
    pub queue: String,
    pub enqueuer: (PluginId, ClientId),
    pub generation: Option<u64>,
    /// Carries the enqueuer's configuration for the job; the host adds its own keys at launch.
    pub run_plugin_or_alias: RunPluginOrAlias,
    /// Never given to the job; echoed verbatim in the enqueuer's events.
    pub context: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
}

impl QueuedJob {
    fn info(&self, plugin_id: Option<PluginId>, state: QueueJobState) -> QueueJobInfo {
        QueueJobInfo {
            job_id: self.job_id,
            queue: self.queue.clone(),
            plugin_url: self.run_plugin_or_alias.location_string(),
            plugin_id,
            enqueuer_plugin_id: self.enqueuer.0,
            context: self.context.clone(),
            state,
        }
    }
}

#[derive(Debug, Clone)]
struct RunningJob {
    job: QueuedJob,
    /// Recorded by finish / cancel / failure *before* the plugin unloads, so that the unload
    /// hook knows why. A job that unloads with nothing recorded was aborted.
    outcome: Option<(QueueJobOutcome, Option<String>)>,
}

#[derive(Debug, Clone)]
struct Queue {
    concurrency: usize,
    pending: VecDeque<QueuedJob>,
    running: BTreeMap<PluginId, RunningJob>,
}

impl Queue {
    fn new() -> Self {
        Queue {
            concurrency: DEFAULT_CONCURRENCY,
            pending: VecDeque::new(),
            running: BTreeMap::new(),
        }
    }
    fn has_free_slot(&self) -> bool {
        self.running.len() < self.concurrency
    }
    fn snapshot(&self, name: &str) -> QueueSnapshot {
        let mut running: Vec<&RunningJob> = self.running.values().collect();
        running.sort_by_key(|r| r.job.job_id);
        QueueSnapshot {
            queue: name.to_owned(),
            concurrency: self.concurrency,
            pending: self
                .pending
                .iter()
                .map(|j| j.info(None, QueueJobState::Pending))
                .collect(),
            running: self
                .running
                .iter()
                .map(|(pid, r)| (r.job.job_id, r.job.info(Some(*pid), QueueJobState::Running)))
                .collect::<BTreeMap<_, _>>()
                .into_values()
                .collect(),
        }
    }
}

/// A job that is over, and what to tell its enqueuer.
#[derive(Debug, Clone, PartialEq)]
pub struct FinishedJob {
    pub job_id: JobId,
    pub queue: String,
    pub enqueuer: (PluginId, ClientId),
    pub generation: Option<u64>,
    pub context: BTreeMap<String, String>,
    pub outcome: QueueJobOutcome,
    pub payload: Option<String>,
}

impl FinishedJob {
    fn new(job: QueuedJob, outcome: QueueJobOutcome, payload: Option<String>) -> Self {
        FinishedJob {
            job_id: job.job_id,
            queue: job.queue,
            enqueuer: job.enqueuer,
            generation: job.generation,
            context: job.context,
            outcome,
            payload,
        }
    }
}

/// What `cancel` found, and what the caller must do about it.
#[derive(Debug, Clone, PartialEq)]
pub enum CancelResult {
    /// The job had not started. It is gone; tell the enqueuer.
    DroppedPending(FinishedJob),
    /// The job is running as this plugin. Its outcome is recorded as `Cancelled`; unload it, and
    /// the unload hook reports it.
    UnloadRunning(PluginId),
    NotFound,
}

#[derive(Debug, Default)]
pub struct Queues {
    queues: BTreeMap<String, Queue>,
    next_job_id: JobId,
    plugin_to_queue: HashMap<PluginId, String>,
    /// Set once the session is going away. Nothing launches after this: the plugin thread has
    /// stopped listening, so a job started now would run with nobody to unload it.
    shut_down: bool,
}

impl Queues {
    /// Put a job on `queue`, creating the queue at [`DEFAULT_CONCURRENCY`] if it does not exist.
    /// Nothing starts until the caller pops with [`Queues::pop_launchable`].
    pub fn enqueue(
        &mut self,
        queue: &str,
        enqueuer: (PluginId, ClientId),
        generation: Option<u64>,
        run_plugin_or_alias: RunPluginOrAlias,
        context: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
    ) -> JobId {
        self.next_job_id = JobId::new(self.next_job_id.get() + 1);
        let job_id = self.next_job_id;
        self.queue_mut(queue).pending.push_back(QueuedJob {
            job_id,
            queue: queue.to_owned(),
            enqueuer,
            generation,
            run_plugin_or_alias,
            context,
            cwd,
        });
        job_id
    }

    /// Set how many jobs `queue` may run at once, creating it if needed. Zero means one: a queue
    /// that can never run anything is a leak, not a setting. Lowering the limit below the number
    /// of running jobs evicts none of them.
    pub fn configure(&mut self, queue: &str, concurrency: usize) {
        self.queue_mut(queue).concurrency = concurrency.max(1);
    }

    pub fn cancel(&mut self, job_id: JobId) -> CancelResult {
        for queue in self.queues.values_mut() {
            if let Some(index) = queue.pending.iter().position(|j| j.job_id == job_id) {
                let job = queue
                    .pending
                    .remove(index)
                    .expect("index came from position");
                return CancelResult::DroppedPending(FinishedJob::new(
                    job,
                    QueueJobOutcome::Cancelled,
                    None,
                ));
            }
            if let Some((plugin_id, running)) = queue
                .running
                .iter_mut()
                .find(|(_, r)| r.job.job_id == job_id)
            {
                running.outcome = Some((QueueJobOutcome::Cancelled, None));
                return CancelResult::UnloadRunning(*plugin_id);
            }
        }
        CancelResult::NotFound
    }

    /// Record why a running job is about to unload. `false` when `plugin_id` is not a job — the
    /// caller should then do nothing, since unloading an arbitrary plugin is not what was asked.
    pub fn record_outcome(
        &mut self,
        plugin_id: PluginId,
        outcome: QueueJobOutcome,
        payload: Option<String>,
    ) -> bool {
        match self.running_mut(plugin_id) {
            Some(running) => {
                running.outcome = Some((outcome, payload));
                true
            },
            None => false,
        }
    }

    /// The next job `queue` may start, if it has a free slot and something waiting. The job is
    /// no longer pending and not yet running: the caller must follow up with
    /// [`Queues::on_job_launched`] or [`Queues::on_launch_failed`] before popping again, which is
    /// what keeps `running.len() <= concurrency` no matter how many launches fail.
    pub fn pop_launchable(&mut self, queue: &str) -> Option<QueuedJob> {
        if self.shut_down {
            return None;
        }
        let queue = self.queues.get_mut(queue)?;
        if !queue.has_free_slot() {
            return None;
        }
        queue.pending.pop_front()
    }

    pub fn on_job_launched(&mut self, job: QueuedJob, plugin_id: PluginId) {
        let queue_name = job.queue.clone();
        self.plugin_to_queue.insert(plugin_id, queue_name.clone());
        self.queue_mut(&queue_name)
            .running
            .insert(plugin_id, RunningJob { job, outcome: None });
    }

    /// The launch itself failed, before there was a plugin. The slot was never taken.
    pub fn on_launch_failed(&mut self, job: QueuedJob, error: String) -> FinishedJob {
        FinishedJob::new(job, QueueJobOutcome::Failed, Some(error))
    }

    /// A plugin unloaded. If it was a running job, free its slot and say how it ended: the
    /// recorded outcome, or `Aborted` when nothing was recorded.
    pub fn on_plugin_unloaded(&mut self, plugin_id: PluginId) -> Option<FinishedJob> {
        let queue_name = self.plugin_to_queue.remove(&plugin_id)?;
        let running = self
            .queues
            .get_mut(&queue_name)?
            .running
            .remove(&plugin_id)?;
        let (outcome, payload) = running.outcome.unwrap_or((QueueJobOutcome::Aborted, None));
        Some(FinishedJob::new(running.job, outcome, payload))
    }

    /// The session is exiting. Drop everything that has not started and refuse to start anything
    /// more. Jobs already running are unloaded by the host's cleanup and come back through
    /// [`Queues::on_plugin_unloaded`] as usual; their events go to a channel nobody reads, which
    /// is fine. What must not happen is the unload hook pumping the queue and launching a plugin
    /// after the plugin thread has stopped listening - that plugin would never be unloaded.
    pub fn shut_down(&mut self) {
        self.shut_down = true;
        for queue in self.queues.values_mut() {
            queue.pending.clear();
        }
    }

    pub fn is_shut_down(&self) -> bool {
        self.shut_down
    }

    #[cfg(test)]
    pub fn queue_of_plugin(&self, plugin_id: PluginId) -> Option<&str> {
        self.plugin_to_queue.get(&plugin_id).map(String::as_str)
    }

    /// Every queue, or just the named one. An unknown name yields nothing rather than an error:
    /// "no such queue" and "empty queue" look the same to a status display.
    pub fn snapshot(&self, queue: Option<&str>) -> Vec<QueueSnapshot> {
        match queue {
            Some(name) => self.queue_snapshot(name).into_iter().collect(),
            None => self
                .queues
                .iter()
                .map(|(name, queue)| queue.snapshot(name))
                .collect(),
        }
    }

    pub fn queue_snapshot(&self, queue: &str) -> Option<QueueSnapshot> {
        self.queues.get(queue).map(|q| q.snapshot(queue))
    }

    fn queue_mut(&mut self, name: &str) -> &mut Queue {
        self.queues
            .entry(name.to_owned())
            .or_insert_with(Queue::new)
    }

    fn running_mut(&mut self, plugin_id: PluginId) -> Option<&mut RunningJob> {
        let queue_name = self.plugin_to_queue.get(&plugin_id)?;
        self.queues.get_mut(queue_name)?.running.get_mut(&plugin_id)
    }
}

#[path = "./unit/queues_tests.rs"]
#[cfg(test)]
mod queues_tests;

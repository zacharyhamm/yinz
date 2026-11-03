//! Pause plugin execution only after already submitted writes have finished.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, Sender},
    Arc, Condvar, Mutex,
};
use std::time::{Duration, Instant};
use yinz_runtime::errors::prelude::*;

#[derive(Clone, Default)]
pub(crate) struct PauseGate {
    state: Arc<(Mutex<bool>, Condvar)>,
    retired: Arc<AtomicBool>,
}

impl PauseGate {
    pub(crate) fn resumed(&self) -> bool {
        *self
            .state
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// The acknowledgement happens after preceding work, before waiting. Later
    /// work cannot execute until resume, including work queued during the pause.
    pub(crate) fn arrive(&self, reply: Sender<()>) {
        let mut resumed = self
            .state
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _ = reply.send(());
        while !*resumed {
            resumed = self
                .state
                .1
                .wait(resumed)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    pub(crate) fn resume(&self) {
        *self
            .state
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        self.state.1.notify_all();
    }

    pub(crate) fn retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }

    fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        self.resume();
    }
}

type Reply = Sender<Result<(), String>>;

/// Intermediate manifests must never prune persisted plugin state while panes
/// temporarily belong to another server. Keep only the newest report per target
/// and replay it once the screen has supplied its complete restored manifest.
#[derive(Default)]
pub(super) struct DeferredReports(Vec<crate::PluginUpdate>);

impl DeferredReports {
    pub(super) fn retain(&mut self, updates: &mut Vec<crate::PluginUpdate>) {
        use yinz_types::data::Event;
        updates.retain(|update| {
            if !matches!(
                &update.2,
                Event::PaneUpdate(..)
                    | Event::TabUpdate(..)
                    | Event::SessionUpdate(..)
                    | Event::ModeUpdate(..)
            ) {
                return true;
            }
            let kind = yinz_types::data::EventType::from(&update.2);
            self.0.retain(|previous| {
                // A broadcast replaces each older covered targeted report too;
                // replaying an old empty targeted manifest first would let a
                // plugin prune persisted state before seeing the full one.
                let covers_plugin = update.0.is_none() || update.0 == previous.0;
                let covers_client = update.1.is_none() || update.1 == previous.1;
                !(covers_plugin
                    && covers_client
                    && yinz_types::data::EventType::from(&previous.2) == kind)
            });
            self.0.push(update.clone());
            false
        });
    }

    pub(super) fn take(&mut self) -> Vec<crate::PluginUpdate> {
        std::mem::take(&mut self.0)
    }
}

/// Wait only for loads submitted around the handoff, without waiting for
/// recurring timers or unrelated worker jobs to become idle. The plugin loop
/// keeps processing load completions while this receipt is pending.
pub(super) struct Readiness {
    pub(super) operation: String,
    plugins: std::collections::HashSet<crate::PluginId>,
    deadline: Instant,
    replies: Vec<Reply>,
}

impl Readiness {
    pub(super) fn new(operation: String, bridge: &crate::PluginBridge, reply: Reply) -> Self {
        Self {
            operation,
            plugins: bridge.update_pending_loads(),
            deadline: Instant::now() + Duration::from_secs(60),
            replies: vec![reply],
        }
    }

    pub(super) fn add_reply(&mut self, reply: Reply) {
        self.replies.push(reply);
    }

    pub(super) fn poll(&mut self, bridge: &crate::PluginBridge) -> bool {
        self.plugins.extend(bridge.update_pending_loads());
        let result = bridge.update_load_readiness(&self.plugins).or_else(|| {
            (Instant::now() >= self.deadline)
                .then(|| Err("timed out waiting for plugin loads after session update".into()))
        });
        let Some(result) = result else { return false };
        for reply in self.replies.drain(..) {
            let _ = reply.send(result.clone());
        }
        true
    }
}

/// Unlike a consumed completion receiver, this counter survives a failed or
/// cancelled pause. A later retry still waits for the same outstanding jobs.
#[derive(Clone, Default)]
pub(super) struct NativeTasks {
    state: Arc<(Mutex<usize>, Condvar)>,
}

pub(super) struct NativeTask {
    tasks: NativeTasks,
}

impl NativeTasks {
    pub(super) fn start(&self) -> NativeTask {
        *self
            .state
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner()) += 1;
        NativeTask {
            tasks: self.clone(),
        }
    }

    fn wait_until(&self, deadline: Instant) -> Result<()> {
        let mut count = self
            .state
            .0
            .lock()
            .map_err(|_| anyhow!("native task tracker is poisoned"))?;
        while *count > 0 {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .context("timed out flushing native plugin writes")?;
            (count, _) = self
                .state
                .1
                .wait_timeout(count, remaining)
                .map_err(|_| anyhow!("native task tracker is poisoned"))?;
        }
        Ok(())
    }
}

impl Drop for NativeTask {
    fn drop(&mut self) {
        let mut count = self
            .tasks
            .state
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *count -= 1;
        self.tasks.state.1.notify_all();
    }
}

#[derive(Default)]
struct Completion {
    result: Option<Result<(), String>>,
    waiters: Vec<Reply>,
}

pub(super) struct Quiescence {
    pub(super) operation: String,
    gate: PauseGate,
    completion: Arc<Mutex<Completion>>,
    initial: bool,
}

impl Quiescence {
    /// A private replacement has no guest state to flush. Gate executor loading
    /// synchronously before startup plugins are scheduled; host pane placement
    /// still runs and can acknowledge adoption without executing guest callbacks.
    pub(super) fn initial(
        operation: String,
        bridge: &crate::plugin_bridge::PluginBridge,
        clipboard: &yinz_runtime::channels::SenderWithContext<crate::clipboard_history::Message>,
    ) -> Result<Self> {
        let gate = PauseGate::default();
        let result = (|| {
            let mut arrivals = bridge.plugin_executor.pause_for_update(&gate)?;
            let (reply, arrival) = std::sync::mpsc::channel();
            clipboard
                .send(crate::clipboard_history::Message::Pause(
                    gate.clone(),
                    reply,
                ))
                .context("pause replacement clipboard writer")?;
            arrivals.push(arrival);
            Ok::<_, anyhow::Error>(arrivals)
        })();
        let arrivals = match result {
            Ok(arrivals) => arrivals,
            Err(error) => {
                gate.resume();
                return Err(error);
            },
        };
        let completion = Arc::new(Mutex::new(Completion::default()));
        let task_completion = completion.clone();
        let task_gate = gate.clone();
        yinz_bus::global_async_runtime::get_tokio_runtime().spawn_blocking(move || {
            let result = wait(arrivals, Instant::now() + Duration::from_secs(60))
                .map_err(|error| format!("{error:#}"));
            // An initial destination must never silently start its guest jobs on
            // failure. Its coordinator will abort and retire this private server.
            if result.is_err() {
                task_gate.retire();
            }
            let mut completion = task_completion
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            completion.result = Some(result.clone());
            for reply in completion.waiters.drain(..) {
                let _ = reply.send(result.clone());
            }
        });
        Ok(Self {
            operation,
            gate,
            completion,
            initial: true,
        })
    }

    pub(super) fn begin(
        operation: String,
        bridge: &crate::plugin_bridge::PluginBridge,
        clipboard: &yinz_runtime::channels::SenderWithContext<crate::clipboard_history::Message>,
        native_tasks: &NativeTasks,
    ) -> Result<Self> {
        let gate = PauseGate::default();
        let result = (|| {
            let (reply, arrival) = std::sync::mpsc::channel();
            clipboard
                .send(crate::clipboard_history::Message::Pause(
                    gate.clone(),
                    reply,
                ))
                .context("pause clipboard persistence")?;
            Ok::<_, anyhow::Error>(vec![arrival])
        })();
        let arrivals = match result {
            Ok(arrivals) => arrivals,
            Err(error) => {
                gate.resume();
                return Err(error);
            },
        };
        let native_tasks = native_tasks.clone();
        let completion = Arc::new(Mutex::new(Completion::default()));
        let task_completion = completion.clone();
        let task_gate = gate.clone();
        let map = bridge.plugin_map.clone();
        let executor = bridge.plugin_executor.clone();
        yinz_bus::global_async_runtime::get_tokio_runtime().spawn_blocking(move || {
            let deadline = Instant::now() + Duration::from_secs(60);
            let result = (|| {
                native_tasks.wait_until(deadline)?;
                wait(arrivals, deadline)?;
                // Worker replies and timer ticks travel through the parent's
                // executor. Drain workers first while that routing still runs;
                // pausing the parent first would deadlock a deferred worker job.
                let workers = map
                    .lock()
                    .map_err(|_| anyhow!("plugin map is poisoned"))?
                    .all_workers();
                let mut arrivals = Vec::new();
                let generations = worker_generations(&workers)?;
                for worker in &workers {
                    arrivals.extend(worker.pause_for_update(&task_gate)?);
                }
                wait(arrivals, deadline)?;
                wait(executor.pause_for_update(&task_gate)?, deadline)?;
                let current = map.lock().map_err(|_| anyhow!("plugin map is poisoned"))?.all_workers();
                anyhow::ensure!(generations == worker_generations(&current)?,
                    "plugin workers changed while preparing the update; retry after plugin loading completes");
                Ok(())
            })()
            .map_err(|error| format!("{error:#}"));
            if result.is_err() {
                task_gate.resume();
            }
            let mut completion = task_completion
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            completion.result = Some(result.clone());
            for reply in completion.waiters.drain(..) {
                let _ = reply.send(result.clone());
            }
        });
        Ok(Self {
            operation,
            gate,
            completion,
            initial: false,
        })
    }

    pub(super) fn resume(mut self) {
        self.initial = false;
        self.gate.resume();
    }

    pub(super) fn reply_when_ready(&self, reply: Reply) {
        let mut completion = self
            .completion
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(result) = &completion.result {
            let _ = reply.send(result.clone());
        } else {
            completion.waiters.push(reply);
        }
    }

    pub(super) fn retire(&self, bridge: &crate::plugin_bridge::PluginBridge) -> Result<()> {
        let ready = self
            .completion
            .lock()
            .map_err(|_| anyhow!("plugin pause completion is poisoned"))?
            .result
            == Some(Ok(()));
        if ready {
            let instances = bridge
                .plugin_map
                .lock()
                .map_err(|_| anyhow!("plugin map is poisoned"))?
                .running_plugins();
            for (_, _, instance) in instances {
                let mut instance = instance
                    .try_lock()
                    .map_err(|_| anyhow!("quiesced plugin is unexpectedly still running"))?;
                instance.env().cancel_tripped.store(true, Ordering::Release);
                instance.mark_failed();
            }
        }
        self.gate.retire();
        Ok(())
    }
}

impl Drop for Quiescence {
    fn drop(&mut self) {
        if self.initial {
            self.gate.retire();
        } else {
            self.gate.resume();
        }
        let mut completion = self
            .completion
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if completion.result.is_none() {
            let result = Err("plugin update pause was cancelled".to_owned());
            completion.result = Some(result.clone());
            for reply in completion.waiters.drain(..) {
                let _ = reply.send(result.clone());
            }
        }
    }
}

fn wait(arrivals: Vec<Receiver<()>>, deadline: Instant) -> Result<()> {
    for arrival in arrivals {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("timed out flushing plugin writes for update")?;
        arrival
            .recv_timeout(remaining)
            .context("plugin persistence did not reach its update pause")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/update_quiescence.rs"]
mod tests;

fn worker_generations(
    workers: &[Arc<crate::background_worker::Workers>],
) -> Result<std::collections::BTreeSet<u64>> {
    let mut generations = std::collections::BTreeSet::new();
    for worker in workers {
        generations.extend(worker.generations()?);
    }
    Ok(generations)
}

//! The host's watch timer sleeps indefinitely when no standing queries exist.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::PluginDataStore;
use yinz_types::plugin_data::DataError;

#[cfg(test)]
#[path = "unit/poll.rs"]
mod tests;

pub const DATA_WATCH_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(super) struct WatchActivity {
    active: Mutex<bool>,
    changed: Condvar,
}

impl WatchActivity {
    pub(super) fn set_active(&self, active: bool) {
        let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if *guard != active {
            *guard = active;
            self.changed.notify_all();
        }
    }
}

/// Stops and joins the polling worker when the host exits.
pub struct DataWatchPoller {
    activity: Arc<WatchActivity>,
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PluginDataStore {
    /// Start one worker per host. The callback holds the host's recipient lock while polling and
    /// delivering, just like local refreshes. The worker does not keep the store alive.
    pub fn start_watch_polling(
        self: &Arc<Self>,
        poll: impl Fn(&Self) + Send + 'static,
    ) -> Result<DataWatchPoller, DataError> {
        let store = Arc::downgrade(self);
        let activity = self.activity.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let mut worker = DataWatchPoller {
            activity: activity.clone(),
            stopped: stopped.clone(),
            thread: None,
        };
        worker.thread = Some(
            std::thread::Builder::new()
                .name("plugin-data-watch".into())
                .spawn(move || {
                    let mut active = activity.active.lock().unwrap_or_else(|e| e.into_inner());
                    while !stopped.load(Ordering::Acquire) {
                        if !*active {
                            active = activity
                                .changed
                                .wait(active)
                                .unwrap_or_else(|e| e.into_inner());
                            continue;
                        }
                        let (guard, timeout) = activity
                            .changed
                            .wait_timeout_while(active, DATA_WATCH_POLL_INTERVAL, |active| {
                                *active && !stopped.load(Ordering::Acquire)
                            })
                            .unwrap_or_else(|e| e.into_inner());
                        active = guard;
                        if timeout.timed_out() && *active && !stopped.load(Ordering::Acquire) {
                            drop(active);
                            let Some(store) = store.upgrade() else { break };
                            poll(&store);
                            active = activity.active.lock().unwrap_or_else(|e| e.into_inner());
                        }
                    }
                })
                .map_err(|e| {
                    DataError::Storage(format!("failed to start data watch polling: {e}"))
                })?,
        );
        Ok(worker)
    }
}

impl Drop for DataWatchPoller {
    fn drop(&mut self) {
        {
            let _guard = self
                .activity
                .active
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            self.stopped.store(true, Ordering::Release);
            self.activity.changed.notify_all();
        }
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                log::error!("data watch polling worker panicked");
            }
        }
    }
}

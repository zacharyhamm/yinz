//! Re-sends the client's `Announce` frame while its Zellij host is silent.
//!
//! Every client announces itself at startup; whether it is actually nested is
//! learnt from the host answering (see [`ReannounceScheduler`]). The thread
//! here sleeps on a condvar once an unanswered client's announce budget is
//! spent, and wakes on first host contact or [`NestedReannounce::stop`], so a
//! client in a plain terminal costs nothing after its first few seconds.

use crate::os_input_output::StdioAccess;
use crate::terminal_escapes::announce_nested;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use zellij_types::internal_ids::SessionName;
use {
    zellij_ipc::nested_session::reannounce_check_interval_ms,
    zellij_ipc::nested_session::ReannounceScheduler,
};

#[derive(Clone)]
pub struct NestedReannounce {
    scheduler: Arc<Mutex<ReannounceScheduler>>,
    wakeup: Arc<Condvar>,
    stop: Arc<AtomicBool>,
}

impl NestedReannounce {
    pub fn spawn(os_input: Box<dyn StdioAccess + Send>, session_name: SessionName) -> Self {
        let handle = NestedReannounce {
            scheduler: Arc::new(Mutex::new(ReannounceScheduler::new(Instant::now()))),
            wakeup: Arc::new(Condvar::new()),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let scheduler = handle.scheduler.clone();
        let wakeup = handle.wakeup.clone();
        let stop = handle.stop.clone();
        let _ = std::thread::Builder::new()
            .name("nested_reannounce".to_string())
            .spawn(move || {
                let check_interval = Duration::from_millis(reannounce_check_interval_ms());
                loop {
                    let mut current = scheduler.lock().unwrap();
                    while !stop.load(Ordering::Relaxed) && current.budget_exhausted() {
                        current = wakeup.wait(current).unwrap();
                    }
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let (mut current, _) = wakeup.wait_timeout(current, check_interval).unwrap();
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let should_announce = current.on_tick(Instant::now());
                    drop(current);
                    if should_announce {
                        announce_nested(&*os_input, &session_name);
                    }
                }
            });
        handle
    }

    pub fn note_host_contact(&self) {
        let first_contact = self
            .scheduler
            .lock()
            .unwrap()
            .note_host_contact(Instant::now());
        if first_contact {
            self.wakeup.notify_all();
        }
    }

    /// Whether a Zellij host has ever answered this client — which is how the
    /// client learns it is running inside a Zellij pane at all.
    pub fn host_contacted(&self) -> bool {
        self.scheduler.lock().unwrap().host_contacted()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wakeup.notify_all();
    }
}

//! In-memory half of the launch coordinator. No filesystem or provider calls on Screen.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use yinz_types::{agent::launch::*, internal_ids::TerminalId};

pub fn new_incarnation() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn server_instance() -> &'static str {
    static INSTANCE: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| uuid::Uuid::new_v4().to_string());
    &INSTANCE
}

#[derive(Debug)]
pub struct LaunchGate {
    pub launch_id: String,
    pub suppressed: bool,
    state: Mutex<GateState>,
    changed: std::sync::Condvar,
}
#[derive(Clone, Debug, Default)]
pub struct GateState {
    pub destination: Option<crate::route::AgentTabPlacement>,
    pub start_requested: bool,
    pub worker_pending: bool,
    pub allocated: Option<TerminalId>,
    pub exited: bool,
    pub slot: Option<LaunchSlot>,
    pub revoked: Option<LaunchReason>,
    pub remote: bool,
    pub non_execution_proven: bool,
    pub cleanup_started: bool,
    pub cleanup_unknown: bool,
    pub child_pid: Option<yinz_types::internal_ids::ProcessId>,
    pub claimed: bool,
    pub running: bool,
    pub registered: bool,
    pub finished: bool,
    pub superseded: bool,
    pub layout_released: bool,
    pub cleanup_confirmed: bool,
}
impl LaunchGate {
    pub fn new(launch_id: String, suppressed: bool) -> Self {
        Self {
            launch_id,
            suppressed,
            state: Mutex::default(),
            changed: std::sync::Condvar::new(),
        }
    }
    pub fn remote(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).remote = true;
    }
    pub fn non_execution(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .non_execution_proven = true;
    }
    pub fn started(&self, child: Option<yinz_types::internal_ids::ProcessId>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.running = true;
        state.child_pid = child;
    }
    pub fn request_cleanup(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.cleanup_started {
            return false;
        }
        state.cleanup_started = true;
        true
    }
    pub fn cleanup_unknown(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cleanup_unknown = true;
        self.changed.notify_all();
    }
    pub fn request_start(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.start_requested || state.revoked.is_some() || state.finished {
            return false;
        }
        state.start_requested = true;
        true
    }
    pub fn destination(&self, destination: crate::route::AgentTabPlacement) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .destination = Some(destination);
    }
    pub fn worker_pending(&self, pending: bool) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .worker_pending = pending;
    }
    pub fn allocated(&self, terminal: TerminalId) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .allocated = Some(terminal);
    }
    pub fn exited(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).exited = true;
    }
    pub fn wait_cleanup(&self, timeout: std::time::Duration) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.changed
            .wait_timeout_while(state, timeout, |s| {
                !s.cleanup_confirmed && !s.cleanup_unknown
            })
            .unwrap_or_else(|e| e.into_inner())
            .0
            .cleanup_confirmed
    }
    pub fn snapshot(&self) -> GateState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn admit(&self, slot: LaunchSlot) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.slot.is_none() && state.revoked.is_none() {
            state.slot = Some(slot);
        }
    }
    /// Claim and revocation share one lock. The winner defines whether exec is possible.
    pub fn claim(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.cleanup_unknown
            || state.exited
            || state.claimed
            || state.finished
            || state.revoked.is_some()
            || state.slot.is_none()
        {
            return false;
        }
        state.claimed = true;
        true
    }
    pub fn revoke(&self, reason: LaunchReason) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.finished {
            state.revoked.get_or_insert(reason);
        }
    }
    pub fn running(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).running = true;
    }
    pub fn registered(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .registered = true;
    }
    pub fn supersede(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .superseded = true;
    }
    pub fn handoff(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.revoked.is_some()
            || state.cleanup_unknown
            || state.exited
            || !state.registered
            || !state.running
        {
            return false;
        }
        state.finished = true;
        true
    }
    #[cfg(test)]
    pub fn finish(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .finished = true;
    }
    pub fn layout_released(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .layout_released = true;
    }
    pub fn cleaned(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cleanup_confirmed = true;
        self.changed.notify_all();
    }
}

#[derive(Default)]
pub struct LaunchRuntime {
    pub host_id: yinz_types::internal_ids::HostId,
    pub operations: Mutex<HashMap<String, LaunchReceipt>>,
    pub gates: Mutex<HashMap<TerminalId, Arc<LaunchGate>>>,
    /// Serializes durable intent claiming, never held during preparation or execution.
    pub intent_lock: Mutex<()>,
}
impl LaunchRuntime {
    pub fn gate(&self, terminal: TerminalId) -> Option<Arc<LaunchGate>> {
        self.gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&terminal)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn admitted() -> LaunchGate {
        let gate = LaunchGate::new("test".into(), false);
        gate.admit(LaunchSlot {
            pane_id: yinz_types::pane_id::PaneId::Terminal(TerminalId::new(1)),
            process_generation: 1,
            tab_id: yinz_types::internal_ids::TabId::new(1),
            tab_incarnation: "tab".into(),
            controller: None,
            worker_region: None,
            placement_generation: 1,
        });
        gate
    }
    #[test]
    fn launch_transaction_revocation_wins_before_claim_and_duplicates_never_execute() {
        let gate = admitted();
        gate.revoke(LaunchReason::ControllerMoved);
        assert!(!gate.claim());
        let gate = admitted();
        assert!(gate.claim());
        assert!(!gate.claim());
        gate.revoke(LaunchReason::DestinationClosed);
        assert!(gate.snapshot().claimed);
        assert!(!gate.snapshot().cleanup_confirmed);
    }
    #[test]
    fn launch_transaction_concurrent_claim_has_exactly_one_winner() {
        let gate = Arc::new(admitted());
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let gate = gate.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    gate.claim()
                })
            })
            .collect();
        barrier.wait();
        assert_eq!(
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .filter(|won| *won)
                .count(),
            1
        );
    }
}

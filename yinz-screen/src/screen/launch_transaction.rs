use super::Screen;
use yinz_bus::screen::ScreenInstruction;
use yinz_types::{
    agent::{launch::*, SpawnedAgent},
    internal_ids::TerminalId,
    pane_id::PaneId,
    pane_size::TerminalSize,
};

impl Screen {
    pub(super) fn validate_launch_slot(
        &mut self,
        terminal: TerminalId,
    ) -> Result<TerminalSize, LaunchReason> {
        let gate = self
            .bus
            .senders
            .launches
            .gate(terminal)
            .ok_or(LaunchReason::HistoryUnavailable)?;
        let state = gate.snapshot();
        if let Some(reason) = state.revoked {
            return Err(reason);
        }
        let slot = state.slot.ok_or(LaunchReason::PlacementRevoked)?;
        if let Some(controller) = slot.controller {
            let current = self
                .tab_id_containing_pane(controller)
                .ok_or(LaunchReason::ControllerClosed)?;
            if current != slot.tab_id {
                return Err(LaunchReason::ControllerMoved);
            }
        }
        let tab = self
            .tabs
            .get_mut(&slot.tab_id)
            .ok_or(LaunchReason::DestinationClosed)?;
        if tab.agent_tab_incarnation != slot.tab_incarnation {
            return Err(LaunchReason::DestinationClosed);
        }
        if let (Some(controller), Some(region)) = (slot.controller, &slot.worker_region) {
            let key = (controller, slot.tab_id);
            if self
                .agent_worker_regions
                .get(&key)
                .is_none_or(|(_, current)| current != region)
            {
                return Err(LaunchReason::PlacementRevoked);
            }
            let members = tab.worker_region_members(slot.pane_id);
            let owned: Vec<_> = self
                .agent_worker_stacks
                .get(&key)
                .into_iter()
                .flatten()
                .copied()
                .filter(|id| tab.can_anchor_tiled_stack(*id))
                .collect();
            if members.len() != owned.len() || members.iter().any(|id| !owned.contains(id)) {
                return Err(LaunchReason::PlacementRevoked);
            }
        }
        let (cols, rows) = if gate.suppressed {
            // Intentional --suppressed is recorded by admission, never inferred from existence.
            let p = tab
                .get_pane_with_id(PaneId::Terminal(terminal))
                .ok_or(LaunchReason::PlacementRevoked)?;
            (p.get_content_columns(), p.get_content_rows())
        } else {
            tab.worker_startup_size(PaneId::Terminal(terminal))
                .ok_or(LaunchReason::PlacementRevoked)?
        };
        if cols == 0 || rows == 0 {
            return Err(LaunchReason::InsufficientRoom);
        }
        Ok(TerminalSize {
            cols: cols.min(u16::MAX as usize) as u16,
            rows: rows.min(u16::MAX as usize) as u16,
            width_in_pixels: None,
            height_in_pixels: None,
        })
    }

    /// Every Screen turn fences pending starts against layout mutations. Finished workers are
    /// ordinary panes: later controller movement/closure has no destructive effect on them.
    pub(super) fn revalidate_launches(&mut self) {
        // Retire ownership as soon as a live group dissolves, including after its launches
        // have finished. A later user-created group must not revive that old region UUID.
        let keys: Vec<_> = self.agent_worker_regions.keys().copied().collect();
        for key in keys {
            let live: Vec<_> = self
                .agent_worker_stacks
                .get(&key)
                .into_iter()
                .flatten()
                .copied()
                .filter(|pane| {
                    self.tabs
                        .get(&key.1)
                        .is_some_and(|tab| tab.can_anchor_tiled_stack(*pane))
                })
                .collect();
            let valid = self.tabs.get_mut(&key.1).is_some_and(|tab| {
                let lifetime_matches =
                    self.agent_worker_regions.get(&key).is_some_and(|(old, _)| {
                        old.ptr_eq(&std::sync::Arc::downgrade(&tab.agent_tab_lifetime))
                    });
                live.first().is_some_and(|root| {
                    let members = tab.worker_region_members(*root);
                    lifetime_matches
                        && members.len() == live.len()
                        && members.iter().all(|pane| live.contains(pane))
                })
            });
            if valid {
                self.agent_worker_stacks.insert(key, live);
            } else {
                self.agent_worker_regions.remove(&key);
                self.agent_worker_stacks.remove(&key);
            }
        }
        let terminals: Vec<_> = self.managed_reservations.keys().copied().collect();
        for terminal in terminals {
            let Some(gate) = self.bus.senders.launches.gate(terminal) else {
                continue;
            };
            if gate.snapshot().finished {
                self.managed_reservations.remove(&terminal);
                continue;
            }
            if let Err(reason) = self.validate_launch_slot(terminal) {
                gate.revoke(reason);
                self.managed_reservations.remove(&terminal);
                // Queued teardown uses the original, non-reusable terminal allocation.
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::ClosePane(
                        PaneId::Terminal(terminal),
                        None,
                        None,
                        None,
                    ))
                    .ok();
            }
        }
    }

    pub(super) fn register_launch(&mut self, spawned: SpawnedAgent) -> bool {
        let PaneId::Terminal(terminal) = spawned.pane_id else {
            return false;
        };
        let Some(gate) = self.bus.senders.launches.gate(terminal) else {
            return false;
        };
        let state = gate.snapshot();
        if state.finished {
            return self
                .agent_launches
                .get(&spawned.pane_id)
                .is_some_and(|saved| saved == &spawned);
        }
        if state.cleanup_unknown
            || state.exited
            || !state.running
            || state.revoked.is_some()
            || self.validate_launch_slot(terminal).is_err()
        {
            return false;
        }
        if self
            .tabs
            .get(
                &state
                    .slot
                    .as_ref()
                    .map(|slot| slot.tab_id)
                    .unwrap_or_else(|| yinz_types::internal_ids::TabId::new(usize::MAX)),
            )
            .and_then(|tab| tab.get_pane_with_id(spawned.pane_id))
            .is_none_or(|pane| pane.exited())
        {
            return false;
        }
        self.agent_launches.insert(spawned.pane_id, spawned);
        gate.registered();
        true
    }
}

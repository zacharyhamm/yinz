//! Mother: the session's one overlay pane running a coding agent.
//!
//! `Action::ToggleMother` shows and hides it, and offers to launch one when there is none. The
//! pane lives in one tab's overlay collection like any overlay, so "available on every tab" is
//! done by carrying it: a toggle brings it to the client's tab, and a visible mother follows a
//! client that switches tabs. Mother is the pane rather than the conversation in it - `/clear`
//! keeps it - and ends when the pane closes, leaves the session, or its process exits.
use super::Screen;
use crate::ClientId;
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;
use yinz_types::pane_id::PaneId;

impl Screen {
    /// The mother pane, while its agent process is still running.
    pub(super) fn live_mother(&self) -> Option<PaneId> {
        let pane = PaneId::Terminal(self.mother?);
        self.get_pane_info(pane)
            .is_some_and(|info| !info.exited && !info.is_held)
            .then_some(pane)
    }

    pub(super) fn toggle_mother(&mut self, client: ClientId) -> Result<()> {
        let Some(pane) = self.live_mother() else {
            // An exited mother leaves its held pane on screen; the next one replaces it.
            if let Some(id) = self.mother.take() {
                self.close_pane_explicitly(PaneId::Terminal(id), None);
            }
            self.open_mother_picker(client);
            return self.log_and_report_session_state();
        };
        let Some(tab_id) = self.active_tab_ids.get(&client).copied() else {
            return Ok(());
        };
        if self.tab_containing_overlay(pane).is_none() {
            // Embedded or floated by the user since it was launched: nothing to hide in place.
            self.focus_pane_with_id(pane, true, false, client)
                .context("failed to focus mother")?;
        } else {
            let focused_here = self.tabs.get(&tab_id).is_some_and(|tab| {
                tab.pane_is_overlay(pane)
                    && !tab.overlay_carry_for(pane).hidden
                    && tab.get_active_pane_id(client) == Some(pane)
            });
            self.carry_mother(pane, tab_id);
            let tab = self
                .tabs
                .get_mut(&tab_id)
                .context("the client's tab does not exist")?;
            if focused_here {
                tab.hide_overlay_pane(pane);
            } else {
                // Also draws a hidden overlay again.
                tab.focus_pane_with_id(pane, false, false, client)
                    .context("failed to focus mother")?;
            }
        }
        self.sync_scroll_mode_on_focus(client)
            .context("failed to sync scroll mode for mother")?;
        self.log_and_report_session_state()
    }

    /// The plugin host launched `pane` as mother. Recorded here rather than from the palette's
    /// reply, which is dropped when the palette was dismissed while the agent started.
    pub(super) fn mother_spawned(&mut self, pane: PaneId) -> Result<()> {
        let PaneId::Terminal(id) = pane else {
            return Ok(());
        };
        if self.live_mother().is_some_and(|live| live != pane) {
            // Two launches raced. The first one stays mother.
            self.close_pane_explicitly(pane, None);
        } else if let Some(previous) = self.mother.replace(id).filter(|previous| *previous != id) {
            self.close_pane_explicitly(PaneId::Terminal(previous), None);
        }
        self.log_and_report_session_state()
    }

    /// A visible mother goes with a client that leaves its tab, keeping the keyboard if it had
    /// it. A hidden one stays where it is until the next toggle fetches it.
    pub(super) fn mother_follows(&mut self, client: ClientId, from: TabId, to: TabId) {
        let Some(pane) = self.mother.map(PaneId::Terminal) else {
            return;
        };
        let Some(source) = self.tabs.get(&from) else {
            return;
        };
        if !source.pane_is_overlay(pane) || source.overlay_carry_for(pane).hidden {
            return;
        }
        // Read before the move: an overlay carried after its client left arrives unfocused.
        let focused = source.get_active_pane_id(client) == Some(pane);
        self.carry_mother(pane, to);
        if focused {
            if let Some(tab) = self.tabs.get_mut(&to) {
                tab.focus_pane_with_id(pane, false, false, client)
                    .context("failed to keep mother focused across tabs")
                    .non_fatal();
            }
        }
    }

    /// Move mother's overlay into `destination`, as the overlay it is. The destination is
    /// checked first: a container taken with nowhere to put it would be dropped, pane and all.
    fn carry_mother(&mut self, pane: PaneId, destination: TabId) {
        let Some(source) = self
            .tab_containing_overlay(pane)
            .map(|tab| tab.id)
            .filter(|source| *source != destination && self.tabs.contains_key(&destination))
        else {
            return;
        };
        let container = self
            .tabs
            .get_mut(&source)
            .and_then(|tab| tab.take_overlay_container(pane));
        if let (Some(container), Some(tab)) = (container, self.tabs.get_mut(&destination)) {
            tab.accept_overlay_container(container);
        }
    }
}

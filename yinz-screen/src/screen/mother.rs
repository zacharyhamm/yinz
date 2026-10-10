//! Mother: the session's one overlay pane running a coding agent.
//!
//! `Action::ToggleMother` shows and hides it, and offers to launch one when there is none. The
//! pane lives in one tab's overlay collection like any overlay, so "available on every tab" is
//! done by carrying it: a toggle brings it to the client's tab, and a visible mother follows a
//! client that switches tabs, unless another client is using it. A client that toggles a mother
//! it shares with another only lets go of the keyboard; the last one using it hides it.
//! In-place replacements carry mother with them and receive its toggles until they close.
//! Mother is the pane rather than the conversation in it - `/clear`
//! keeps it - and ends when the pane closes, leaves the session, or its process exits.
use super::Screen;
use crate::ClientId;
use std::collections::HashSet;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{PaneInfo, PaneManifest};
use yinz_types::internal_ids::TabId;
use yinz_types::pane_id::PaneId;

/// Whether the agent process in mother's pane is still running.
pub(super) fn mother_is_live(info: &PaneInfo) -> bool {
    !info.exited && !info.is_held
}

impl Screen {
    /// The mother pane, while its agent process is still running.
    pub(super) fn live_mother(&self) -> Option<PaneId> {
        let pane = PaneId::Terminal(self.mother?);
        self.get_pane_info(pane)
            .is_some_and(|info| mother_is_live(&info))
            .then_some(pane)
    }

    /// Mother's row in a manifest the caller already has, sparing `live_mother`'s walk over
    /// every tab.
    pub(super) fn mother_in<'a>(&self, panes: &'a PaneManifest) -> Option<&'a PaneInfo> {
        let pane = PaneId::Terminal(self.mother?);
        panes.panes.values().flatten().find(|info| info.id == pane)
    }

    /// Follow restoration links without pulling a pane out from behind its stand-in.
    pub(super) fn mother_presentation(&self, mut pane: PaneId) -> Option<PaneId> {
        let tab = self.tab_containing_pane(pane)?;
        let mut visited = HashSet::new();
        while visited.insert(pane) {
            if tab.has_non_suppressed_pane_with_pid(&pane) {
                return Some(pane);
            }
            pane = tab
                .get_suppressed_panes()
                .find(|(id, _)| **id == pane)?
                .1
                .restored_by?
                .pane_id;
        }
        None
    }

    fn mother_used_by_another_client(&self, pane: PaneId, client: ClientId) -> bool {
        self.tab_containing_overlay(pane).is_some_and(|tab| {
            self.connected_clients.borrow().iter().any(|other| {
                *other != client
                    && tab.has_connected_client(other)
                    && tab.get_active_pane_id(*other) == Some(pane)
            })
        })
    }

    pub(super) fn toggle_mother(&mut self, client: ClientId) -> Result<()> {
        let Some(tab_id) = self.active_tab_ids.get(&client).copied() else {
            return Ok(());
        };
        if !self.tabs.contains_key(&tab_id) {
            log::error!("Cannot toggle mother: the client's tab does not exist");
            return Ok(());
        }
        let Some(pane) = self.live_mother() else {
            // An exited mother leaves its held pane on screen; the next one replaces it.
            if let Some(id) = self.mother.take() {
                self.close_pane_explicitly(PaneId::Terminal(id), None);
            }
            self.open_mother_picker(client);
            return self
                .log_and_report_session_state()
                .context("failed to report session state after offering mother");
        };
        // A mother parked with nothing standing in for it is fetched like any parked pane.
        let pane = self.mother_presentation(pane).unwrap_or(pane);
        if self.tab_containing_overlay(pane).is_none() {
            // Embedded or floated by the user since it was launched: nothing to hide in place.
            self.focus_pane_with_id(pane, true, false, client)
                .context("failed to focus mother")
                .non_fatal();
        } else {
            let focused_here = self.tabs.get(&tab_id).is_some_and(|tab| {
                tab.pane_is_overlay(pane)
                    && !tab.overlay_carry_for(pane).hidden
                    && tab.get_active_pane_id(client) == Some(pane)
            });
            if focused_here {
                let shared = self.mother_used_by_another_client(pane, client);
                let tab = self
                    .tabs
                    .get_mut(&tab_id)
                    .context("the client's tab does not exist")?;
                if shared {
                    // Hiding it would take the other client's input target away.
                    tab.release_overlay_focus_to_lower_layer(client);
                } else {
                    tab.hide_overlay_pane(pane);
                }
            } else {
                return self
                    .show_and_focus_mother(pane, client)
                    .context("failed to show mother");
            }
        }
        self.sync_scroll_mode_on_focus(client)
            .context("failed to sync scroll mode for mother")?;
        self.log_and_report_session_state()
            .context("failed to report session state after toggling mother")
    }

    /// Fetch without toggling: ordinary focus requests and the show half of a toggle share this.
    pub(super) fn show_and_focus_mother(&mut self, pane: PaneId, client: ClientId) -> Result<()> {
        let Some(destination) = self.active_tab_ids.get(&client).copied() else {
            return Ok(());
        };
        if !self.carry_mother(pane, destination, client) {
            return Ok(());
        }
        if let Some(tab) = self.tabs.get_mut(&destination) {
            tab.focus_pane_with_id(pane, false, false, client)
                .context("failed to focus mother")
                .non_fatal();
        }
        self.sync_scroll_mode_on_focus(client)
            .context("failed to sync scroll mode for mother")?;
        self.log_and_report_session_state()
            .context("failed to report session state after showing mother")
    }

    /// The plugin host launched `pane` as mother. Recorded as the pane is placed rather than
    /// from the palette's reply, which is dropped when the palette was dismissed while the agent
    /// started.
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
            .context("failed to report session state after registering mother")
    }

    /// A visible mother goes with a client that leaves its tab, keeping the keyboard if it had
    /// it. A hidden one stays where it is until the next toggle fetches it.
    pub(super) fn mother_follows(
        &mut self,
        client: ClientId,
        from: TabId,
        to: TabId,
        old_focused_panes: &[(ClientId, Option<PaneId>)],
    ) {
        let Some(pane) = self
            .mother
            .and_then(|id| self.mother_presentation(PaneId::Terminal(id)))
        else {
            return;
        };
        let Some(source) = self.tabs.get(&from) else {
            return;
        };
        if !source.pane_is_overlay(pane) || source.overlay_carry_for(pane).hidden {
            return;
        }
        if !self.carry_mother(pane, to, client) {
            return;
        }
        for (client, _) in old_focused_panes
            .iter()
            .filter(|(_, focused)| *focused == Some(pane))
        {
            if let Some(tab) = self.tabs.get_mut(&to) {
                tab.focus_pane_with_id(pane, false, false, *client)
                    .context("failed to keep mother focused across tabs")
                    .non_fatal();
            }
        }
    }

    /// Bring mother's overlay to `destination`, unless another client is using it where it is.
    /// Whether it ends up there.
    fn carry_mother(&mut self, pane: PaneId, destination: TabId, client: ClientId) -> bool {
        let Some(source) = self
            .tab_containing_overlay(pane)
            .map(|tab| tab.id)
            .filter(|_| self.tabs.contains_key(&destination))
        else {
            return false;
        };
        if source == destination {
            return true;
        }
        if self.mother_used_by_another_client(pane, client) {
            return false;
        }
        self.move_overlay_container(pane, destination).is_some()
    }
}

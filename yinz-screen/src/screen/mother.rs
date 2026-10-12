//! Mother: the session's one overlay pane running a coding agent.
//!
//! `Action::ToggleMother` shows and hides it, and offers to launch one when there is none. The
//! pane lives in one tab's overlay collection like any overlay, so "available on every tab" is
//! done by carrying it: a toggle brings it to the client's tab, and a visible mother follows a
//! client that switches tabs, unless another client is still looking at it. A client that asks
//! for a mother another client is using elsewhere goes to it instead. A client that toggles a
//! mother it shares with another only lets go of the keyboard; the last one using it hides it.
//! In-place replacements carry mother with them and receive its toggles until they close.
//! Mother is the pane rather than the conversation in it - `/clear`
//! keeps it - and ends when the pane closes, leaves the session, or its process exits. What an
//! exited mother leaves on screen stays registered until a replacement is placed, so a toggle
//! can fetch it for reading or a re-run while it offers the next one.
use super::Screen;
use crate::panes::OverlayCarry;
use crate::tab::ExtractedPanePlacement;
use crate::ClientId;
use std::collections::HashSet;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{OverlayPlacement, PaneInfo, PaneManifest};
use yinz_types::internal_ids::{TabId, TerminalId};
use yinz_types::pane_id::PaneId;

/// Whether the agent process in mother's pane is still running.
fn mother_is_live(info: &PaneInfo) -> bool {
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

    /// What the session reports as its mother: the registered pane while its agent is running.
    pub(super) fn reported_mother(&self, panes: &PaneManifest) -> Option<TerminalId> {
        self.mother_in(panes)
            .filter(|info| mother_is_live(info))
            .and(self.mother)
    }

    /// Mother ends with its pane: `pane` closed or left the session.
    pub(super) fn forget_mother(&mut self, pane: PaneId) {
        if self.mother.map(PaneId::Terminal) == Some(pane) {
            self.mother = None;
        }
    }

    /// Follow restoration links without pulling a pane out from behind its stand-in.
    pub(super) fn mother_presentation(&self, mut pane: PaneId) -> Option<PaneId> {
        let tab = self.tab_containing_pane(pane)?;
        if let Some(info) = tab.pane_tabs_of(pane).and_then(|id| tab.pane_tabs_info(id)) {
            pane = info.pane_id;
        }
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

    pub(super) fn hide_mother_for_pane_focus(&mut self, requested: PaneId, client: ClientId) {
        let Some(pane) = self
            .mother
            .map(PaneId::Terminal)
            .and_then(|pane| self.mother_presentation(pane))
        else {
            return;
        };
        if pane == requested || self.mother_used_by_another_client(pane, client) {
            return;
        }
        if let Some(tab) = self.tab_containing_pane_mut(pane) {
            tab.hide_overlay_pane(pane);
        }
    }

    pub(super) fn toggle_mother(&mut self, client: ClientId) -> Result<()> {
        let Some(tab_id) = self.active_tab_ids.get(&client).copied() else {
            return Ok(());
        };
        if !self.tabs.contains_key(&tab_id) {
            log::error!("Cannot toggle mother: the client's tab does not exist");
            return Ok(());
        }
        let Some(mother) = self
            .mother
            .map(PaneId::Terminal)
            .filter(|pane| self.tab_containing_pane(*pane).is_some())
        else {
            self.open_mother_picker(client);
            return self
                .log_and_report_session_state()
                .context("failed to report session state after offering mother");
        };
        let live = self.live_mother().is_some();
        let pane = self.mother_presentation(mother).unwrap_or(mother);
        if self.tab_containing_overlay(pane).is_none() {
            // Parked, or brought back from parking as a floating or tiled pane.
            if !self.reclaim_mother_overlay(pane, tab_id, client) {
                self.focus_pane_with_id(pane, true, false, client)
                    .context("failed to focus mother")
                    .non_fatal();
            }
        } else {
            let focused_here = self.tabs.get(&tab_id).is_some_and(|tab| {
                tab.pane_is_overlay(pane)
                    && !tab.overlay_carry_for(pane).hidden
                    && tab.get_active_pane_id(client) == Some(pane)
            });
            if live && focused_here {
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
                self.fetch_mother(pane, client)
                    .context("failed to show mother")?;
            }
        }
        if !live {
            // What an exited mother left stays up to be read or re-run; the next one replaces it.
            self.open_mother_picker(client);
        }
        self.sync_scroll_mode_on_focus(client)
            .context("failed to sync scroll mode for mother")?;
        self.log_and_report_session_state()
            .context("failed to report session state after toggling mother")
    }

    /// Fetch without toggling: ordinary focus requests share this with the show half of a toggle.
    pub(super) fn show_and_focus_mother(&mut self, pane: PaneId, client: ClientId) -> Result<()> {
        self.fetch_mother(pane, client)
            .context("failed to show mother")?;
        self.sync_scroll_mode_on_focus(client)
            .context("failed to sync scroll mode for mother")?;
        self.log_and_report_session_state()
            .context("failed to report session state after showing mother")
    }

    /// Bring mother's overlay to the client's tab and give it the keyboard. When another client
    /// is using it where it is, the client goes to mother instead.
    fn fetch_mother(&mut self, pane: PaneId, client: ClientId) -> Result<()> {
        let Some(destination) = self
            .active_tab_ids
            .get(&client)
            .copied()
            .filter(|tab| self.tabs.contains_key(tab))
        else {
            return Ok(());
        };
        let tab = if self.carry_mother(pane, destination, client) {
            destination
        } else {
            let Some((source, position)) = self
                .tab_containing_overlay(pane)
                .map(|tab| (tab.id, tab.position))
            else {
                return Ok(());
            };
            self.switch_active_tab(position, None, true, client)
                .context("failed to go to mother's tab")?;
            source
        };
        if let Some(tab) = self.tabs.get_mut(&tab) {
            tab.focus_pane_with_id(pane, false, false, client)
                .context("failed to focus mother")
                .non_fatal();
        }
        Ok(())
    }

    /// Put a mother that is no longer an overlay back on the surface, in `destination`, with the
    /// keyboard. Whether it ended up there.
    fn reclaim_mother_overlay(
        &mut self,
        pane: PaneId,
        destination: TabId,
        client: ClientId,
    ) -> bool {
        let Some(source) = self.tab_containing_pane(pane) else {
            return false;
        };
        let original = source.placement_for_extraction(pane);
        let last_tiled = matches!(original, ExtractedPanePlacement::Tiled)
            && source.has_non_suppressed_pane_with_pid(&pane)
            && !source.pane_is_stack_list_member(&pane)
            && source.get_selectable_tiled_panes_count() <= 1;
        // A stand-in cannot leave the pane it replaced, and a tab keeps its last tiled pane.
        if last_tiled || source.stands_in_for_a_pane(pane).is_some() {
            return false;
        }
        let source = source.id;
        let Some(extracted) = self.extract_pane_for_transfer(source, pane) else {
            return false;
        };
        let overlay =
            ExtractedPanePlacement::Overlay(OverlayPlacement::mother(), OverlayCarry::default());
        let outcome = self.accept_transferred_panes(vec![(extracted, overlay)], destination, false);
        if !outcome.moved.contains(&pane) {
            return false;
        }
        if let Some(tab) = self.tabs.get_mut(&destination) {
            tab.focus_pane_with_id(pane, false, false, client)
                .context("failed to focus mother")
                .non_fatal();
        }
        true
    }

    /// The plugin host launched `pane` as mother. Recorded as the pane is placed rather than
    /// from the palette's reply, which is dropped when the palette was dismissed while the agent
    /// started. `place_new_pane` refuses a launch that arrives while a mother is running, so
    /// the only one this can replace has exited.
    pub(super) fn mother_spawned(&mut self, pane: PaneId) -> Result<()> {
        let PaneId::Terminal(id) = pane else {
            return Ok(());
        };
        if let Some(previous) = self.mother.replace(id).filter(|previous| *previous != id) {
            self.close_pane_explicitly(PaneId::Terminal(previous), None);
        }
        let client = self.connected_clients.borrow().iter().next().copied();
        if let Some(tab) = self.tab_containing_pane_mut(pane) {
            if tab.pane_tabs_of(pane).is_none() {
                if let Some(client) = client {
                    use yinz_types::pane_tabs::{PaneTabDescriptor, PaneTabsLayout};
                    tab.mount_pane_tabs(
                        pane,
                        PaneTabsLayout {
                            active: 0,
                            tabs: ["Conversation", "Overview"]
                                .into_iter()
                                .map(|label| PaneTabDescriptor {
                                    label: label.into(),
                                    location: "yinz:mother-overview".into(),
                                    configuration: Default::default(),
                                    initial_cwd: None,
                                })
                                .collect(),
                        },
                        0,
                        client,
                    )
                    .context("mount Mother overview")
                    .non_fatal();
                }
            }
        }
        self.log_and_report_session_state()
            .context("failed to report session state after registering mother")
    }

    /// A visible mother goes with a client that leaves its tab, keeping the keyboard if it had
    /// it. A hidden or exited one stays where it is until the next toggle fetches it, and so
    /// does one that a client staying behind can still see.
    pub(super) fn mother_follows(
        &mut self,
        client: ClientId,
        from: TabId,
        to: TabId,
        old_focused_panes: &[(ClientId, Option<PaneId>)],
    ) {
        let Some(pane) = self
            .live_mother()
            .and_then(|pane| self.mother_presentation(pane))
        else {
            return;
        };
        let Some(source) = self.tabs.get(&from) else {
            return;
        };
        if !source.pane_is_overlay(pane) || source.overlay_carry_for(pane).hidden {
            return;
        }
        let watched = self
            .connected_clients
            .borrow()
            .iter()
            .any(|other| *other != client && source.has_connected_client(other));
        if watched || !self.carry_mother(pane, to, client) {
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

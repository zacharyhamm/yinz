use zellij_utils::errors::prelude::*;

use super::{direction_away_from_edge, global_pane_edge_for_direction, Screen};
use crate::global_panes::AllocatedGlobalPanes;
use crate::panes::PaneId;
use crate::route::NotificationEnd;
use crate::tab::Pane;
use crate::ClientId;
use zellij_utils::data::{Direction, GlobalPaneEdge, GlobalPanePlacement, ResizeStrategy};

impl Screen {
    /// Move the global panes to where they sit for `client_id`'s tab, so anything reading their
    /// geometry for that client - hit testing, relative mouse positions, a border drag - sees the
    /// positions that client is actually looking at rather than the last tab rendered. Returns the
    /// layout they were moved to.
    pub(super) fn position_global_panes_for_client(
        &mut self,
        client_id: ClientId,
    ) -> Option<AllocatedGlobalPanes> {
        let tab_id = self.active_tab_ids.get(&client_id).copied()?;
        let tab_size = self
            .tab_surface_size(tab_id)
            .unwrap_or_else(|| self.surface_size());
        let layout = self.global_layout_for(tab_size).ok()?;
        self.global_panes.set_positions(&layout);
        Some(layout)
    }

    pub(super) fn with_pane<R>(
        &mut self,
        pane_id: PaneId,
        f: impl FnOnce(&mut dyn Pane) -> R,
    ) -> Option<R> {
        if let Some(pane) = self.global_panes.pane_mut(pane_id) {
            return Some(f(pane));
        }
        for tab in self.tabs.values_mut() {
            if let Some(pane) = tab.get_pane_with_id_mut(pane_id) {
                return Some(f(pane.as_mut()));
            }
        }
        None
    }

    #[cfg(test)]
    pub(super) fn global_pane_placement(&self, pane_id: PaneId) -> Option<GlobalPanePlacement> {
        self.global_panes.placement(pane_id)
    }

    pub(super) fn reject_global_for_tab_operation(&self, pane_id: PaneId) -> Result<()> {
        if self.global_panes.pane(pane_id).is_some() {
            Err(anyhow!("global pane has no tab"))
        } else {
            Ok(())
        }
    }

    pub(super) fn reject_global_for_tab_operation_with_completion(
        &self,
        pane_id: PaneId,
        completion_tx: &mut Option<NotificationEnd>,
    ) -> bool {
        if self.reject_global_for_tab_operation(pane_id).is_err() {
            let error = "global pane has no tab";
            log::error!("{error}");
            if let Some(completion_tx) = completion_tx.as_mut() {
                completion_tx.set_exit_status(1);
                completion_tx.set_error_message(error.to_owned());
            }
            true
        } else {
            false
        }
    }

    pub(super) fn reject_focused_global_for_tab_operation_with_completion(
        &self,
        client_id: ClientId,
        completion_tx: &mut Option<NotificationEnd>,
    ) -> bool {
        self.global_panes
            .focused_pane_id(client_id)
            .map(|pane_id| {
                self.reject_global_for_tab_operation_with_completion(pane_id, completion_tx)
            })
            .unwrap_or(false)
    }

    pub(super) fn active_pane_id(&self, client_id: ClientId) -> Option<PaneId> {
        self.global_panes.focused_pane_id(client_id).or_else(|| {
            self.get_active_tab(client_id)
                .ok()
                .and_then(|tab| tab.get_active_pane_id(client_id))
        })
    }

    pub(super) fn focus_global_pane(&mut self, pane_id: PaneId, client_id: ClientId) -> Result<()> {
        self.global_panes.focus_pane(pane_id, client_id)?;
        if let Ok(tab) = self.get_active_tab_mut(client_id) {
            tab.set_force_render();
        }
        self.global_panes.set_force_render();
        self.sync_scroll_mode_on_focus(client_id).non_fatal();
        Ok(())
    }

    pub(super) fn defocus_global_pane(&mut self, client_id: ClientId) {
        if self.global_panes.focused_pane_id(client_id).is_none() {
            return;
        }
        self.global_panes.defocus_pane(client_id);
        if let Ok(tab) = self.get_active_tab_mut(client_id) {
            tab.set_force_render();
        }
        self.global_panes.set_force_render();
        self.sync_scroll_mode_on_focus(client_id).non_fatal();
    }

    pub(super) fn activate_overlay(&mut self, pane_id: PaneId) {
        let Some((tab_id, tab)) = self
            .tabs
            .iter()
            .find(|(_, tab)| tab.pane_is_overlay(pane_id))
        else {
            return;
        };
        let tab_id = *tab_id;
        let clients: Vec<_> = self
            .active_tab_ids
            .iter()
            .filter_map(|(client_id, active_tab_id)| {
                (*active_tab_id == tab_id && tab.get_active_pane_id(*client_id) == Some(pane_id))
                    .then_some(*client_id)
            })
            .collect();
        for client_id in clients {
            if let Some(global_pane_id) = self.global_panes.focused_pane_id(client_id) {
                self.suspended_global_focus
                    .insert(client_id, (tab_id, global_pane_id));
                self.defocus_global_pane(client_id);
            }
        }
    }

    pub(super) fn take_global_focus_after_overlay_close(
        &mut self,
        pane_id: PaneId,
    ) -> Vec<(ClientId, PaneId)> {
        let Some((tab_id, tab)) = self
            .tabs
            .iter()
            .find(|(_, tab)| tab.pane_is_overlay(pane_id))
        else {
            return vec![];
        };
        let tab_id = *tab_id;
        let mut restore = vec![];
        self.suspended_global_focus
            .retain(|client_id, (saved_tab, global_pane)| {
                if *saved_tab != tab_id {
                    return true;
                }
                // Only the clients this overlay was actually holding: the column hocus draws
                // is two overlays hidden one after the other, and taking every entry on the tab
                // would let the first hide consume the saved focus of a client sitting on the
                // second - which then has nothing to restore.
                if tab.get_active_pane_id(*client_id) == Some(pane_id) {
                    restore.push((*client_id, *global_pane));
                    return false;
                }
                true
            });
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.set_should_clear_display_before_rendering();
        }
        self.global_panes.set_force_render();
        restore
    }

    pub(super) fn restore_global_focus(&mut self, focus: Vec<(ClientId, PaneId)>) {
        for (client_id, pane_id) in focus {
            self.focus_global_pane(pane_id, client_id).non_fatal();
        }
    }

    /// The global pane to focus next when cycling, or `None` to hand the cycle back to the tab.
    /// Global panes form a segment appended to the end of the active tab's pane cycle, so the full
    /// loop is `tab panes... -> global panes... -> tab panes...`.
    fn next_global_pane_in_cycle(&mut self, client_id: ClientId, forward: bool) -> Option<PaneId> {
        let global_pane_ids = self.global_panes.focusable_pane_ids();
        if global_pane_ids.is_empty() {
            return None;
        }
        match self.global_panes.focused_pane_id(client_id) {
            Some(focused_pane_id) => {
                let position = global_pane_ids
                    .iter()
                    .position(|pane_id| *pane_id == focused_pane_id)?;
                if forward {
                    global_pane_ids.get(position + 1).copied()
                } else {
                    position
                        .checked_sub(1)
                        .and_then(|position| global_pane_ids.get(position).copied())
                }
            },
            None => {
                let tab = self.get_active_tab_mut(client_id).ok()?;
                // a tab with no selectable panes of its own is always "at the end of its cycle"
                let at_cycle_end = !tab.has_selectable_tiled_panes()
                    || if forward {
                        tab.is_last_pane_in_cycle(client_id)
                    } else {
                        tab.is_first_pane_in_cycle(client_id)
                    };
                if !at_cycle_end {
                    return None;
                }
                if forward {
                    global_pane_ids.first().copied()
                } else {
                    global_pane_ids.last().copied()
                }
            },
        }
    }

    pub(super) fn cycle_focus(&mut self, client_id: ClientId, forward: bool) {
        let client_id = if self.get_active_tab(client_id).is_ok() {
            Some(client_id)
        } else {
            self.get_first_client_id()
        };
        let Some(client_id) = client_id else {
            log::error!("No client ids in screen found");
            return;
        };
        match self.next_global_pane_in_cycle(client_id, forward) {
            Some(pane_id) => {
                if let Err(error) = self.focus_global_pane(pane_id, client_id) {
                    log::error!("{error}");
                }
            },
            None => {
                // either we were never on a global pane, or we just fell off the end of the global
                // segment - both mean "advance the tab", which also closes the loop back to its
                // first pane
                self.defocus_global_pane(client_id);
                if let Ok(tab) = self.get_active_tab_mut(client_id) {
                    if forward {
                        tab.focus_next_pane(client_id);
                    } else {
                        tab.focus_previous_pane(client_id);
                    }
                }
            },
        }
    }

    /// Focus the global pane sitting on the edge we're moving towards, if there is one. With
    /// several panes splitting that edge's strip, the one whose span overlaps the client's active
    /// tab pane wins, so the move lands where the eye expects.
    pub(super) fn enter_global_pane_in_direction(
        &mut self,
        direction: Direction,
        client_id: ClientId,
    ) -> bool {
        let edge = global_pane_edge_for_direction(direction);
        let cross_axis_pos = self
            .get_active_tab(client_id)
            .ok()
            .and_then(|tab| tab.get_active_pane(client_id))
            .map(|pane| {
                let geom = pane.position_and_size();
                match edge {
                    GlobalPaneEdge::Left | GlobalPaneEdge::Right => {
                        geom.y + geom.rows.as_usize() / 2
                    },
                    GlobalPaneEdge::Top | GlobalPaneEdge::Bottom => {
                        geom.x + geom.cols.as_usize() / 2
                    },
                }
            })
            .unwrap_or(0);
        let Some(pane_id) = self.global_panes.pane_on_edge_nearest(edge, cross_axis_pos) else {
            return false;
        };
        if let Err(error) = self.focus_global_pane(pane_id, client_id) {
            log::error!("{error}");
            return false;
        }
        true
    }

    /// Whether a modal in the client's active tab holds the keyboard. A directional focus move
    /// is then a no-op - never an escalation to a global pane, another tab, or the host session.
    pub(super) fn active_tab_modal_holds_focus(&self, client_id: ClientId) -> bool {
        self.get_active_tab(client_id)
            .map(|tab| !tab.lower_layer_may_take_focus(client_id))
            .unwrap_or(false)
    }

    /// Handle a directional focus move made from a focused global pane. Returns `true` when the
    /// client is on a global pane, in which case the caller must not fall through to the tab.
    pub(super) fn global_pane_handles_direction(
        &mut self,
        direction: Direction,
        client_id: ClientId,
    ) -> bool {
        let Some(focused_pane_id) = self.global_panes.focused_pane_id(client_id) else {
            return false;
        };
        if let Some(edge) = self
            .global_panes
            .placement(focused_pane_id)
            .map(|placement| placement.edge)
        {
            let along_strip = match edge {
                GlobalPaneEdge::Left | GlobalPaneEdge::Right => {
                    matches!(direction, Direction::Up | Direction::Down)
                },
                GlobalPaneEdge::Top | GlobalPaneEdge::Bottom => {
                    matches!(direction, Direction::Left | Direction::Right)
                },
            };
            if along_strip {
                // moving within the strip lands on the same-edge neighbor; off the strip's end,
                // fall through to the perpendicular hop below, same as before splits existed
                let towards_strip_end = matches!(direction, Direction::Down | Direction::Right);
                if let Some(neighbor_id) = self
                    .global_panes
                    .neighbor_in_strip(focused_pane_id, towards_strip_end)
                {
                    if let Err(error) = self.focus_global_pane(neighbor_id, client_id) {
                        log::error!("{error}");
                    }
                    return true;
                }
            }
            if global_pane_edge_for_direction(direction) == edge {
                // moving further outwards - this is the session boundary, same as a tab edge
                self.bubble_focus_to_host(client_id, direction);
            } else if direction == direction_away_from_edge(edge) {
                self.defocus_global_pane(client_id);
                if let Ok(tab) = self.get_active_tab_mut(client_id) {
                    tab.focus_pane_on_edge(direction, client_id);
                }
            } else {
                // perpendicular - move to the global pane on that edge if there is one
                self.enter_global_pane_in_direction(direction, client_id);
            }
        }
        true
    }

    /// Resize the global pane the client's active tab pane borders, if the resize was made towards
    /// it. Moving that shared border is the same gesture as moving one between two tab panes, only
    /// the pane on the other side of it is global - so growing the tab pane shrinks the global one.
    /// Returns whether the resize was handled here.
    pub(super) fn resize_global_pane_from_adjacent_tab_pane(
        &mut self,
        client_id: ClientId,
        strategy: ResizeStrategy,
    ) -> Result<bool> {
        // a directionless resize has no border to point at, so it stays tab-local
        let Some(direction) = strategy.direction else {
            return Ok(false);
        };
        let edge = global_pane_edge_for_direction(direction);
        let cross_axis_pos = self
            .get_active_tab(client_id)
            .ok()
            .and_then(|tab| tab.get_active_pane(client_id))
            .map(|pane| {
                let geom = pane.position_and_size();
                match edge {
                    GlobalPaneEdge::Left | GlobalPaneEdge::Right => {
                        geom.y + geom.rows.as_usize() / 2
                    },
                    GlobalPaneEdge::Top | GlobalPaneEdge::Bottom => {
                        geom.x + geom.cols.as_usize() / 2
                    },
                }
            })
            .unwrap_or(0);
        let Some(global_pane_id) = self.global_panes.pane_on_edge_nearest(edge, cross_axis_pos)
        else {
            return Ok(false);
        };
        let is_flush_with_global_pane = self
            .get_active_tab(client_id)
            .map(|tab| tab.active_pane_is_flush_with_edge(direction, client_id))
            .unwrap_or(false);
        if !is_flush_with_global_pane {
            return Ok(false);
        }
        let strategy = ResizeStrategy {
            resize: strategy.resize.invert(),
            ..strategy
        };
        self.resize_pane_with_id(strategy, global_pane_id)?;
        Ok(true)
    }

    /// Hide the whole strip `pane_id` belongs to - panes sharing an edge (a hocus-style sidebar
    /// and its agent list, say) hide and show as one unit.
    pub(super) fn hide_pane(&mut self, pane_id: PaneId) -> Result<()> {
        let edge = self
            .global_panes
            .placement(pane_id)
            .ok_or_else(|| anyhow!("global pane {pane_id:?} not found"))?
            .edge;
        self.global_panes.hide_edge(edge)?;
        self.reflow_global_panes()
    }

    /// Show the whole strip `pane_id` belongs to, restoring its split geometry.
    pub(super) fn show_pane(&mut self, pane_id: PaneId) -> Result<()> {
        let edge = self
            .global_panes
            .placement(pane_id)
            .ok_or_else(|| anyhow!("global pane {pane_id:?} not found"))?
            .edge;
        let surface = self.surface_size();
        self.global_panes.show_edge(edge, pane_id, surface.into())?;
        self.reflow_global_panes()
    }

    pub(super) fn set_global_pane_placement(
        &mut self,
        pane_id: PaneId,
        placement: GlobalPanePlacement,
    ) -> Result<()> {
        let surface = self.surface_size();
        self.global_panes
            .set_placement(pane_id, placement, surface.into())?;
        self.reflow_global_panes()
    }
}

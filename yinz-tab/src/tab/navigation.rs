//! Moving focus and panes between positions in their layers.

use super::PaneMove;
use super::PaneSubject;
use super::Tab;
use crate::panes::PaneId;
use crate::ClientId;
use std::time::Instant;

use yinz_runtime::errors::prelude::*;
use yinz_types::geometry::Direction;

impl Tab {
    pub(super) fn set_pane_active_at(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.set_active_at(Instant::now());
        }
    }
    /// Whether a lower layer may move this client's focus at all.
    ///
    /// `false` while a modal holds the keyboard: `get_active_pane_id` prefers the overlay over
    /// every other layer, so a move made anyway would be a move nothing could observe - input
    /// would go on reaching the modal, and the focus underneath would have been rearranged where
    /// the user cannot see it.
    ///
    /// This is the *moving* half of the rule. Opening something with focus is the other half, and
    /// answers differently: see `add_floating_pane` and `new_pane`, where a modal keeps the
    /// keyboard and the new pane is still created behind it.
    pub fn lower_layer_may_take_focus(&self, client_id: ClientId) -> bool {
        !self.overlay_panes.a_modal_holds_focus(client_id)
    }
    /// A lower layer has just taken this client's focus, so a dismissible overlay gives the
    /// keyboard up. The builtin panel is hidden in this same transition once no client focuses it.
    ///
    /// Called *after* the move rather than before it, and only when the move happened. A
    /// focus-move that found nothing in the direction asked for has changed nothing, and
    /// dismissing a popup on it would make a dead keypress close the thing the user was reading.
    pub fn release_overlay_focus_to_lower_layer(&mut self, client_id: ClientId) {
        self.defocus_overlay_pane(client_id, !self.floating_panes.panes_are_visible());
    }

    pub(super) fn defocus_overlay_pane(
        &mut self,
        client_id: ClientId,
        dismiss_panel: bool,
    ) -> bool {
        if !self.overlay_panes.defocus_pane_if_dismissible(client_id) {
            return false;
        }
        if dismiss_panel {
            // Floating focus can leave a panel visible after it has given up the keyboard.
            for pane in self.overlay_panes.rendered_pane_ids() {
                if self.overlay_panes.is_modal(&pane) {
                    continue;
                }
                let is_panel = self
                    .pane_tabs_of(pane)
                    .and_then(|id| self.pane_tabs.get(&id))
                    .is_some_and(|tabs| tabs.info.overlay_group);
                if is_panel && !self.overlay_panes.has_focused_member(&pane) {
                    // A plugin reacting to an older manifest could hide a subsequent reveal.
                    self.hide_overlay_pane(pane);
                }
            }
        }
        true
    }
    /// Whether an overlay - modal or not - holds this client's keyboard.
    pub fn overlay_holds_focus(&self, client_id: ClientId) -> bool {
        self.overlays_are_rendered() && self.overlay_panes.active_pane_id(client_id).is_some()
    }
    /// Move the client's keyboard across the divider of the multipane overlay holding it, when
    /// the other half lies in `direction`. The halves sit side by side on screen, so that is
    /// where a directional move goes, rather than to whatever is hidden under the overlay in
    /// that direction. Tried before `lower_layer_may_take_focus`: a modal multipane's halves may
    /// trade the keyboard between themselves. `false` when there is no such half to take it.
    fn move_focus_to_overlay_sibling(&mut self, direction: Direction, client_id: ClientId) -> bool {
        let Some(sibling) = self
            .overlay_panes
            .active_pane_id(client_id)
            .and_then(|active| self.overlay_panes.sibling_toward(&active, direction))
        else {
            return false;
        };
        self.overlay_panes.focus_pane(sibling, client_id);
        if self.overlay_panes.active_pane_id(client_id) != Some(sibling) {
            return false;
        }
        self.set_force_render();
        true
    }
    pub fn focus_next_pane(&mut self, client_id: ClientId) {
        if !self.has_selectable_panes() {
            return;
        }
        if !self.lower_layer_may_take_focus(client_id) {
            return;
        }
        let before = self.tiled_panes.get_active_pane_id(client_id);
        if self.tiled_panes.fullscreen_is_active() {
            self.switch_next_pane_fullscreen(client_id);
        } else {
            self.tiled_panes.focus_next_pane(client_id);
        }
        // release only after a move that happened: a cycle that lands where it started has
        // changed nothing (see release_overlay_focus_to_lower_layer)
        if self.tiled_panes.get_active_pane_id(client_id) != before {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
    }
    pub fn focus_previous_pane(&mut self, client_id: ClientId) {
        if !self.has_selectable_panes() {
            return;
        }
        if !self.lower_layer_may_take_focus(client_id) {
            return;
        }
        let before = self.tiled_panes.get_active_pane_id(client_id);
        if self.tiled_panes.fullscreen_is_active() {
            self.switch_prev_pane_fullscreen(client_id);
        } else {
            self.tiled_panes.focus_previous_pane(client_id);
        }
        // release only after a move that happened: a cycle that lands where it started has
        // changed nothing (see release_overlay_focus_to_lower_layer)
        if self.tiled_panes.get_active_pane_id(client_id) != before {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
    }
    pub fn focus_last_pane(&mut self, client_id: ClientId) {
        if !self.has_selectable_panes() {
            return;
        }
        if !self.lower_layer_may_take_focus(client_id) {
            return;
        }
        // release only after a move that happened: with no last pane recorded nothing moves,
        // and dismissing a popup on it would let a dead keypress close it
        if self.floating_panes.panes_are_visible() {
            let before = self.floating_panes.active_pane_id(client_id);
            self.floating_panes.focus_last_pane(client_id);
            if self.floating_panes.active_pane_id(client_id) != before {
                self.release_overlay_focus_to_lower_layer(client_id);
            }
        } else {
            let before = self.tiled_panes.get_active_pane_id(client_id);
            if self.tiled_panes.fullscreen_is_active() {
                self.switch_last_pane_fullscreen(client_id);
            } else {
                self.tiled_panes.focus_last_pane(client_id);
            }
            if self.tiled_panes.get_active_pane_id(client_id) != before {
                self.release_overlay_focus_to_lower_layer(client_id);
            }
        }
    }
    pub fn focus_pane_on_edge(&mut self, direction: Direction, client_id: ClientId) {
        if !self.lower_layer_may_take_focus(client_id) {
            return;
        }
        // release only after a move that happened: an edge with nothing on it moves nothing,
        // and dismissing a popup on it would let a dead keypress close it
        if self.floating_panes.panes_are_visible() {
            let before = self.floating_panes.active_pane_id(client_id);
            self.floating_panes.focus_pane_on_edge(direction, client_id);
            if self.floating_panes.active_pane_id(client_id) != before {
                self.release_overlay_focus_to_lower_layer(client_id);
            }
        } else if self.has_selectable_panes() && !self.tiled_panes.fullscreen_is_active() {
            let before = self.tiled_panes.get_active_pane_id(client_id);
            self.tiled_panes.focus_pane_on_edge(direction, client_id);
            if self.tiled_panes.get_active_pane_id(client_id) != before {
                self.release_overlay_focus_to_lower_layer(client_id);
            }
        }
    }
    pub fn focus_pane_adjacent_to(
        &mut self,
        pane_id: PaneId,
        direction: Direction,
        client_id: ClientId,
    ) -> Option<PaneId> {
        if !self.lower_layer_may_take_focus(client_id) {
            return None;
        }
        let focused = if self.floating_panes.panes_are_visible() {
            self.floating_panes
                .focus_pane_adjacent_to(pane_id, direction, client_id)
        } else if self.tiled_panes.fullscreen_is_active() {
            None
        } else {
            self.tiled_panes
                .focus_pane_adjacent_to(pane_id, direction, client_id)
        };
        if focused.is_some() {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
        focused
    }
    // returns a boolean that indicates whether the focus moved
    pub fn move_focus_left(&mut self, client_id: ClientId) -> Result<bool> {
        let err_context = || format!("failed to move focus left for client {}", client_id);

        if self.move_focus_to_overlay_sibling(Direction::Left, client_id) {
            return Ok(true);
        }
        if !self.lower_layer_may_take_focus(client_id) {
            return Ok(false);
        }
        let moved = if self.floating_panes.panes_are_visible() {
            self.floating_panes
                .move_focus(
                    client_id,
                    &self.connected_clients.borrow().iter().copied().collect(),
                    &Direction::Left,
                )
                .with_context(err_context)
        } else if !self.has_selectable_panes() {
            Ok(false)
        } else if self.tiled_panes.fullscreen_is_active() {
            Ok(self.focus_pane_left_fullscreen(client_id))
        } else {
            Ok(self.tiled_panes.move_focus_left(client_id))
        };
        // only now, and only if it happened: a move that found nothing in the direction asked for
        // has changed nothing, and dismissing a popup on it would let a dead keypress close the
        // thing the user was reading
        if matches!(moved, Ok(true)) {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
        moved
    }
    pub fn move_focus_down(&mut self, client_id: ClientId) -> Result<bool> {
        let err_context = || format!("failed to move focus down for client {}", client_id);

        if self.move_focus_to_overlay_sibling(Direction::Down, client_id) {
            return Ok(true);
        }
        if !self.lower_layer_may_take_focus(client_id) {
            return Ok(false);
        }
        if self.floating_panes.panes_are_visible()
            && self.move_focus_within_stack_list(client_id, true, false)
        {
            self.release_overlay_focus_to_lower_layer(client_id);
            return Ok(true);
        }
        let moved = if self.floating_panes.panes_are_visible() {
            let moved = self
                .floating_panes
                .move_focus(
                    client_id,
                    &self.connected_clients.borrow().iter().copied().collect(),
                    &Direction::Down,
                )
                .with_context(err_context)?;
            Ok(moved || self.move_focus_within_stack_list(client_id, true, true))
        } else if !self.has_selectable_panes() {
            Ok(false)
        } else if self.tiled_panes.fullscreen_is_active() {
            Ok(self.focus_pane_down_fullscreen(client_id))
        } else if self.stacked_pane_list_is_active()
            && self.move_focus_within_stack_list(client_id, true, false)
        {
            Ok(true)
        } else {
            Ok(self.tiled_panes.move_focus_down(client_id)
                || (self.stacked_pane_list_is_active()
                    && self.move_focus_within_stack_list(client_id, true, true)))
        };
        // only now, and only if it happened: a move that found nothing in the direction asked for
        // has changed nothing, and dismissing a popup on it would let a dead keypress close the
        // thing the user was reading
        if matches!(moved, Ok(true)) {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
        moved
    }
    pub fn move_focus_up(&mut self, client_id: ClientId) -> Result<bool> {
        let err_context = || format!("failed to move focus up for client {}", client_id);

        if self.move_focus_to_overlay_sibling(Direction::Up, client_id) {
            return Ok(true);
        }
        if !self.lower_layer_may_take_focus(client_id) {
            return Ok(false);
        }
        if self.floating_panes.panes_are_visible()
            && self.move_focus_within_stack_list(client_id, false, false)
        {
            self.release_overlay_focus_to_lower_layer(client_id);
            return Ok(true);
        }
        let moved = if self.floating_panes.panes_are_visible() {
            let moved = self
                .floating_panes
                .move_focus(
                    client_id,
                    &self.connected_clients.borrow().iter().copied().collect(),
                    &Direction::Up,
                )
                .with_context(err_context)?;
            Ok(moved || self.move_focus_within_stack_list(client_id, false, true))
        } else if !self.has_selectable_panes() {
            Ok(false)
        } else if self.tiled_panes.fullscreen_is_active() {
            Ok(self.focus_pane_up_fullscreen(client_id))
        } else if self.stacked_pane_list_is_active()
            && self.move_focus_within_stack_list(client_id, false, false)
        {
            Ok(true)
        } else {
            Ok(self.tiled_panes.move_focus_up(client_id)
                || (self.stacked_pane_list_is_active()
                    && self.move_focus_within_stack_list(client_id, false, true)))
        };
        // only now, and only if it happened: a move that found nothing in the direction asked for
        // has changed nothing, and dismissing a popup on it would let a dead keypress close the
        // thing the user was reading
        if matches!(moved, Ok(true)) {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
        moved
    }
    // returns a boolean that indicates whether the focus moved
    pub fn move_focus_right(&mut self, client_id: ClientId) -> Result<bool> {
        let err_context = || format!("failed to move focus right for client {}", client_id);

        if self.move_focus_to_overlay_sibling(Direction::Right, client_id) {
            return Ok(true);
        }
        if !self.lower_layer_may_take_focus(client_id) {
            return Ok(false);
        }
        let moved = if self.floating_panes.panes_are_visible() {
            self.floating_panes
                .move_focus(
                    client_id,
                    &self.connected_clients.borrow().iter().copied().collect(),
                    &Direction::Right,
                )
                .with_context(err_context)
        } else if !self.has_selectable_panes() {
            Ok(false)
        } else if self.tiled_panes.fullscreen_is_active() {
            Ok(self.focus_pane_right_fullscreen(client_id))
        } else {
            Ok(self.tiled_panes.move_focus_right(client_id))
        };
        // only now, and only if it happened: a move that found nothing in the direction asked for
        // has changed nothing, and dismissing a popup on it would let a dead keypress close the
        // thing the user was reading
        if matches!(moved, Ok(true)) {
            self.release_overlay_focus_to_lower_layer(client_id);
        }
        moved
    }
    /// Move a pane within its own layer. One dispatch for the thirteen public entry points
    /// below, so that they cannot disagree about the rules:
    ///
    /// - a floating move always marks the floating layout damaged and forces a render, so the
    ///   panes under the moved one repaint and nothing of it is left behind;
    /// - a tiled move is refused while nothing is selectable or a tiled pane is fullscreen, and
    ///   dissolves the stack lists first, being a classic mutation of the grid.
    ///
    /// A pane named by id moves in the layer it is in; a client's active pane moves in the
    /// floating layer while that layer is shown, and in the tiled one otherwise. An overlay
    /// (named, or holding the client's focus) moves one step in a direction and has no next
    /// slot. A pane suppressed behind another is left alone.
    pub(super) fn move_pane_in_layer(&mut self, subject: PaneSubject, movement: PaneMove) {
        let overlay = match subject {
            PaneSubject::Active(client_id) => self.overlay_panes.active_pane_id(client_id),
            PaneSubject::Pane(pane_id) => Some(pane_id).filter(|id| self.pane_is_overlay(*id)),
        };
        if let Some(pane_id) = overlay {
            if let PaneMove::Toward(direction) = movement {
                self.overlay_panes.move_pane(pane_id, direction);
                self.set_force_render();
            }
            return;
        }
        let on_floating_layer = match subject {
            PaneSubject::Active(_) => self.floating_panes.panes_are_visible(),
            PaneSubject::Pane(pane_id) => self.floating_panes.panes_contain(&pane_id),
        };
        if on_floating_layer {
            let pane_id = match subject {
                PaneSubject::Active(client_id) => self.floating_panes.get_active_pane_id(client_id),
                PaneSubject::Pane(pane_id) => Some(pane_id),
            };
            if let Some(pane_id) = pane_id {
                match movement {
                    PaneMove::Next { backwards } => {
                        self.floating_panes.move_pane(backwards, pane_id)
                    },
                    PaneMove::Toward(Direction::Down) => {
                        self.floating_panes.move_pane_down(pane_id)
                    },
                    PaneMove::Toward(Direction::Up) => self.floating_panes.move_pane_up(pane_id),
                    PaneMove::Toward(Direction::Left) => {
                        self.floating_panes.move_pane_left(pane_id)
                    },
                    PaneMove::Toward(Direction::Right) => {
                        self.floating_panes.move_pane_right(pane_id)
                    },
                }
            }
            self.swap_layouts.set_is_floating_damaged();
            self.set_force_render();
        } else {
            if !self.has_selectable_panes() || self.tiled_panes.fullscreen_is_active() {
                return;
            }
            self.dissolve_stack_lists_for_classic_mutation();
            // resolved after the dissolve, which can change which pane stands for a stack list
            let pane_id = match subject {
                PaneSubject::Active(client_id) => self.tiled_panes.get_active_pane_id(client_id),
                PaneSubject::Pane(pane_id) => Some(pane_id),
            };
            let Some(pane_id) = pane_id else {
                return;
            };
            if !self.tiled_panes.panes_contain(&pane_id) {
                // an overlay or a suppressed pane: in neither layer this dispatch moves, so
                // there is nothing to do
                return;
            }
            match movement {
                PaneMove::Next { backwards } => self.tiled_panes.move_pane(backwards, pane_id),
                PaneMove::Toward(Direction::Down) => self.tiled_panes.move_pane_down(pane_id),
                PaneMove::Toward(Direction::Up) => self.tiled_panes.move_pane_up(pane_id),
                PaneMove::Toward(Direction::Left) => self.tiled_panes.move_pane_left(pane_id),
                PaneMove::Toward(Direction::Right) => self.tiled_panes.move_pane_right(pane_id),
            }
        }
    }
    pub fn move_active_pane(&mut self, client_id: ClientId) {
        self.move_pane_in_layer(
            PaneSubject::Active(client_id),
            PaneMove::Next { backwards: false },
        );
    }
    pub fn move_pane(&mut self, pane_id: PaneId) {
        self.move_pane_in_layer(
            PaneSubject::Pane(pane_id),
            PaneMove::Next { backwards: false },
        );
    }
    pub fn move_active_pane_backwards(&mut self, client_id: ClientId) {
        self.move_pane_in_layer(
            PaneSubject::Active(client_id),
            PaneMove::Next { backwards: true },
        );
    }
    pub fn move_active_pane_down(&mut self, client_id: ClientId) {
        self.move_pane_in_layer(
            PaneSubject::Active(client_id),
            PaneMove::Toward(Direction::Down),
        );
    }
    pub fn move_pane_down(&mut self, pane_id: PaneId) {
        self.move_pane_in_layer(
            PaneSubject::Pane(pane_id),
            PaneMove::Toward(Direction::Down),
        );
    }
    pub fn move_active_pane_up(&mut self, client_id: ClientId) {
        self.move_pane_in_layer(
            PaneSubject::Active(client_id),
            PaneMove::Toward(Direction::Up),
        );
    }
    pub fn move_pane_up(&mut self, pane_id: PaneId) {
        self.move_pane_in_layer(PaneSubject::Pane(pane_id), PaneMove::Toward(Direction::Up));
    }
    pub fn move_active_pane_right(&mut self, client_id: ClientId) {
        self.move_pane_in_layer(
            PaneSubject::Active(client_id),
            PaneMove::Toward(Direction::Right),
        );
    }
    pub fn move_pane_right(&mut self, pane_id: PaneId) {
        self.move_pane_in_layer(
            PaneSubject::Pane(pane_id),
            PaneMove::Toward(Direction::Right),
        );
    }
    pub fn move_active_pane_left(&mut self, client_id: ClientId) {
        self.move_pane_in_layer(
            PaneSubject::Active(client_id),
            PaneMove::Toward(Direction::Left),
        );
    }
    pub fn move_pane_left(&mut self, pane_id: PaneId) {
        self.move_pane_in_layer(
            PaneSubject::Pane(pane_id),
            PaneMove::Toward(Direction::Left),
        );
    }
}

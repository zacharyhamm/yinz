use std::time::Instant;

use yinz_runtime::errors::prelude::*;

use super::Screen;
use crate::plugins::PluginInstruction;
use crate::ClientId;
use yinz_types::mouse::{MouseEvent, MouseEventType};
use {yinz_types::data::Event, yinz_types::data::EventType};

impl Screen {
    /// Copy this client's selection.
    pub(super) fn copy_selection(&mut self, client_id: ClientId) -> Result<()> {
        self.get_active_tab(client_id)?.copy_selection(client_id)
    }
    /// Tell the plugins this client did something. The status bar uses this to drop its "copied to
    /// clipboard" message, so the event that *caused* a copy must not send it.
    pub(super) fn notify_plugins_of_input(&mut self, client_id: ClientId) {
        let plugin_updates: Vec<_> = self
            .targeted_plugin_ids(client_id, EventType::InputReceived)
            .into_iter()
            .map(|pid| (Some(pid), Some(client_id), Event::InputReceived))
            .collect();
        if !plugin_updates.is_empty() {
            let _ = self
                .bus
                .senders
                .send_to_plugin(PluginInstruction::Update(plugin_updates));
        }
    }
    pub fn handle_mouse_event(&mut self, event: MouseEvent, client_id: ClientId) {
        if self.permission_modal_mouse(&event, client_id) {
            return;
        }
        if self.notification_mouse_captures.contains(&client_id) {
            if event.event_type == MouseEventType::Release && event.left {
                self.notification_mouse_captures.remove(&client_id);
            }
            return;
        }
        let is_bare_motion = event.event_type == MouseEventType::Motion
            && !event.left
            && !event.right
            && !event.middle
            && !event.wheel_up
            && !event.wheel_down;
        let active_pane_id_before = self.active_pane_id(client_id);
        let active_pane_was_scrolled = self.active_pane_is_scrolled(client_id);
        let session_surface = self
            .active_tab_ids
            .get(&client_id)
            .copied()
            .and_then(|tab_id| self.session_surface_for_tab(tab_id));
        // An open context menu owns the mouse outright - ahead of toasts and the tab. That is
        // what a menu is: while it is up, a click means "a row of this menu, or dismiss", and
        // never also "focus whatever happened to be underneath". The event is consumed either
        // way: with no surface there is nothing to hit-test against, but letting it fall through
        // to the pane under the menu is the one thing it must not do.
        if self.context_menu_is_open(client_id) {
            if let Some(surface) = session_surface {
                self.handle_context_menu_mouse_event(&event, surface, client_id);
            }
            return;
        }
        // sampled here rather than taken from the last painted frame: a click should resolve
        // against where the toast is now, which is at most one frame ahead of what was drawn
        let now = Instant::now();
        // A browsing client sees the browser in place of its toasts, so the browser is what gets
        // hit-tested - never the toasts it is covering.
        if self.notification_browser_is_visible(client_id) {
            if session_surface.is_some_and(|surface| {
                self.handle_notification_browser_mouse(&event, surface, client_id, now)
            }) {
                if event.event_type == MouseEventType::Press && event.left {
                    self.notification_mouse_captures.insert(client_id);
                }
                return;
            }
        } else if session_surface.is_some_and(|surface| {
            self.handle_pane_notification_click(&event, surface, client_id, now)
        }) {
            self.notification_mouse_captures.insert(client_id);
            return;
        }
        if self.command_palettes.contains_key(&client_id)
            && session_surface.is_some_and(|surface| {
                usize::try_from(event.position.line())
                    .ok()
                    .is_some_and(|line| {
                        self.notification_cell_rects(surface, now, client_id)
                            .iter()
                            .any(|rect| rect.contains(event.position.column(), line))
                    })
            })
        {
            if event.event_type == MouseEventType::Press && event.left {
                self.notification_mouse_captures.insert(client_id);
            }
            return;
        }
        if self.command_palette_handles_mouse(&event, client_id) {
            return;
        }
        // A client descended into a nested guest has handed that pane its keys; alt+mouse
        // belongs to the guest too, rather than to the host's pane-grouping gestures.
        let passthrough_pane_id = active_pane_id_before
            .filter(|pane_id| self.should_route_keys_to_pane(client_id, *pane_id));
        match self.get_active_tab_mut(client_id).and_then(|tab| {
            tab.handle_mouse_event_with_passthrough(&event, client_id, passthrough_pane_id)
        }) {
            Ok(mouse_effect) => {
                let mut should_render = false;
                if let Some(context) = mouse_effect.open_context_menu.clone() {
                    self.request_context_menu(*context, client_id);
                    should_render = true;
                }
                if let Some(pane_id) = mouse_effect.group_toggle {
                    if self.advanced_mouse_actions {
                        self.toggle_pane_id_in_group(pane_id, &client_id);
                        should_render = true;
                    }
                }
                if let Some(pane_id) = mouse_effect.group_add {
                    if self.advanced_mouse_actions {
                        self.add_pane_id_to_group(pane_id, &client_id);
                        should_render = true;
                    }
                }
                if mouse_effect.ungroup && self.advanced_mouse_actions {
                    self.clear_pane_group(&client_id);
                    should_render = true;
                }
                if mouse_effect.state_changed {
                    if !is_bare_motion {
                        let _ = self.log_and_report_session_state();
                    }
                    let active_pane_id_after = self
                        .get_active_tab(client_id)
                        .ok()
                        .and_then(|tab| tab.get_active_pane_id(client_id));
                    if active_pane_id_before.is_some()
                        && active_pane_id_before != active_pane_id_after
                    {
                        self.clear_bell_for_focused_pane(client_id);
                        if let (Some(old), Some(new)) =
                            (active_pane_id_before, active_pane_id_after)
                        {
                            self.report_key_passthrough_state(client_id, old, new);
                        }
                        // A mouse focus change (click, click-through, focus-follows-mouse)
                        // syncs the scroll mode too, same as a keyboard focus move.
                        self.sync_scroll_mode_on_focus(client_id).non_fatal();
                    }
                    should_render = true;
                }
                if !mouse_effect.leave_clipboard_message && !is_bare_motion {
                    self.notify_plugins_of_input(client_id);
                    should_render = true;
                }
                self.sync_scroll_mode_if_scroll_changed(client_id, active_pane_was_scrolled)
                    .non_fatal();
                if should_render {
                    self.render().non_fatal();
                }
            },
            Err(e) => {
                log::error!("Failed to process MouseEvent: {}", e);
            },
        }
    }
}

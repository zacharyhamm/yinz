use zellij_utils::errors::prelude::*;

use super::Screen;
use crate::global_panes::{GlobalBorderHit, GlobalPaneMouseEffect};
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::pty_writer::PtyWriteInstruction;
use crate::ClientId;
use zellij_utils::data::{Event, EventType};
use zellij_utils::input::mouse::{MouseEvent, MouseEventType};

impl Screen {
    /// Whether this client's tab takes the mouse at `event`'s position, ahead of any global pane
    /// there.
    ///
    /// Two cases. A floating pane or an overlay being dragged holds the mouse for the duration of
    /// the drag - it has to, or it could never be dragged across the tab's edge onto a global pane
    /// at all, and the drag would end wherever the pointer first left the tab. And a floating pane
    /// or overlay painted at that position owns it, because both draw over the global panes: what
    /// is under the pointer is what gets clicked.
    ///
    /// A global pane that has already captured this client's mouse for a selection keeps it, so a
    /// selection dragged under a floating pane still ends where it was released.
    ///
    /// The drag state is per-tab rather than per-client, so while one client drags a pane another
    /// client on the same tab loses global-pane mouse routing until the drag ends. Short lived, and
    /// not worth threading a client id through the drag bookkeeping for.
    fn tab_wins_mouse_event(&self, event: &MouseEvent, client_id: ClientId) -> bool {
        if self.global_panes.is_selecting_with_mouse(client_id) {
            return false;
        }
        self.get_active_tab(client_id)
            .map(|tab| {
                tab.a_pane_is_being_moved_with_mouse()
                    || tab.visible_floating_pane_at(&event.position).is_some()
            })
            .unwrap_or(false)
    }
    /// Dragging the border between a global pane and the tab area resizes the global pane.
    ///
    /// This runs before the mouse event is routed to a pane, because the border cell belongs to the
    /// global pane on one edge and to the tab pane on the other (see `GlobalPaneManager`'s
    /// `apply_layout`) - hit-testing the position instead of the pane makes all four edges behave
    /// the same. Returns whether the event was consumed by a drag.
    fn handle_global_pane_border_drag(
        &mut self,
        event: &MouseEvent,
        client_id: ClientId,
        tab_wins_mouse: bool,
    ) -> bool {
        if let Some(border) = self.global_panes.mouse_resize_border(client_id) {
            let surface = self.surface_size();
            match event.event_type {
                MouseEventType::Motion if event.left => {
                    let resized = match border {
                        GlobalBorderHit::Thickness(pane_id) => self
                            .global_panes
                            .resize_pane_to_position(pane_id, &event.position, surface.into()),
                        GlobalBorderHit::Split { earlier } => self
                            .global_panes
                            .resize_split_to_position(earlier, &event.position, surface.into()),
                    };
                    match resized {
                        // an over-far drag is simply not applied, the pane keeps its last valid size
                        Ok(false) | Err(_) => {},
                        Ok(true) => {
                            self.reflow_global_panes().non_fatal();
                            self.render(None).non_fatal();
                        },
                    }
                },
                _ => {
                    self.global_panes.stop_mouse_resize(client_id);
                    self.log_and_report_session_state().non_fatal();
                },
            }
            return true;
        }
        if event.event_type == MouseEventType::Press && event.left {
            // a ui pane (tab bar, status bar) beside a right or bottom global pane draws no
            // boundary on the cell they share, so there is nothing to drag there - the click is the
            // ui pane's. Same for a floating pane drawn over the border cell: it is what the user
            // sees there, and pressing it must not start a global pane resize behind it
            let is_on_tab_chrome = self
                .get_active_tab(client_id)
                .map(|tab| tab.position_is_on_ui_pane(&event.position))
                .unwrap_or(false);
            if !is_on_tab_chrome && !tab_wins_mouse {
                if let Some(border) = self.global_panes.border_hit_at(&event.position) {
                    self.global_panes.start_mouse_resize(border, client_id);
                    return true;
                }
            }
        }
        false
    }
    /// Copy this client's selection. Like `Tab::copy_selection` this only ever reads the focused
    /// pane, so a focused global pane with nothing selected copies nothing rather than falling
    /// back to whatever the tab has.
    pub(super) fn copy_selection(&mut self, client_id: ClientId) -> Result<()> {
        if let Some(global_pane_id) = self.global_panes.focused_pane_id(client_id) {
            let selected_text = self
                .global_panes
                .pane(global_pane_id)
                .and_then(|pane| pane.get_selected_text(client_id));
            if let Some(selected_text) = selected_text {
                self.get_active_tab(client_id)?
                    .copy_text_to_clipboard(&selected_text)?;
            }
            return Ok(());
        }
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
        // the panes are left wherever the last render group put them, which is another client's
        // tab as often as not - everything below hit-tests and takes relative positions against
        // their geometry, so it has to be this client's view of them
        let client_layout = self.position_global_panes_for_client(client_id);
        if client_layout.is_some_and(|layout| {
            self.handle_pane_notification_click(&event, layout.center, client_id)
        }) {
            self.notification_mouse_captures.insert(client_id);
            return;
        }
        let tab_wins_mouse = self.tab_wins_mouse_event(&event, client_id);
        if self.handle_global_pane_border_drag(&event, client_id, tab_wins_mouse) {
            return;
        }
        if tab_wins_mouse {
            // `pane_id_for_mouse_event` is being skipped, so do the one thing it would have done
            // besides answering: a fresh button press ends any global-pane selection
            if event.event_type == MouseEventType::Press
                && (event.left || event.right || event.middle)
            {
                self.global_panes.stop_selecting_with_mouse(client_id);
            }
        } else if let Some(global_pane_id) =
            self.global_panes.pane_id_for_mouse_event(&event, client_id)
        {
            let mouse_effect =
                self.global_panes
                    .handle_mouse_event(global_pane_id, &event, client_id);
            let left_clipboard_message =
                matches!(&mouse_effect, Some(GlobalPaneMouseEffect::Copy(_)))
                    && self.copy_options.copy_on_select;
            match mouse_effect {
                Some(GlobalPaneMouseEffect::WriteRaw(mouse_bytes)) => {
                    if let PaneId::Terminal(terminal_id) = global_pane_id {
                        self.bus
                            .senders
                            .send_to_pty_writer(PtyWriteInstruction::Write(
                                mouse_bytes,
                                terminal_id,
                                None,
                            ))
                            .non_fatal();
                    }
                },
                Some(GlobalPaneMouseEffect::WriteAdjusted(writes)) => {
                    for bytes in writes {
                        self.write_to_pane_id(
                            &None,
                            bytes,
                            false,
                            global_pane_id,
                            Some(client_id),
                            None,
                        )
                        .non_fatal();
                    }
                },
                Some(GlobalPaneMouseEffect::Copy(selected_text))
                    if self.copy_options.copy_on_select =>
                {
                    if let Ok(tab) = self.get_active_tab(client_id) {
                        tab.write_selection_to_clipboard(&selected_text).non_fatal();
                    }
                },
                _ => {},
            }
            if event.event_type == MouseEventType::Press
                && (event.left || event.right || event.middle)
            {
                if let Err(error) = self.focus_global_pane(global_pane_id, client_id) {
                    log::error!("{error}");
                }
            }
            if !is_bare_motion {
                self.log_and_report_session_state().non_fatal();
            }
            if !left_clipboard_message && !is_bare_motion {
                self.notify_plugins_of_input(client_id);
            }
            // A mouse focus change or wheel scroll on a global pane syncs the scroll mode
            // too, same as a keyboard focus move or tab-pane scroll.
            self.sync_scroll_mode_if_scroll_changed(client_id, active_pane_was_scrolled)
                .non_fatal();
            self.render(None).non_fatal();
            return;
        }
        if event.event_type == MouseEventType::Press && (event.left || event.right || event.middle)
        {
            self.defocus_global_pane(client_id);
        }
        match self
            .get_active_tab_mut(client_id)
            .and_then(|tab| tab.handle_mouse_event(&event, client_id))
        {
            Ok(mouse_effect) => {
                let mut should_render = false;
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
                if mouse_effect.ungroup {
                    if self.advanced_mouse_actions {
                        self.clear_pane_group(&client_id);
                        should_render = true;
                    }
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
                    self.render(None).non_fatal();
                }
            },
            Err(e) => {
                log::error!("Failed to process MouseEvent: {}", e);
            },
        }
    }
}

//! Text selection, search, mouse targeting, and clipboard delivery.

use super::clipboard::ClipboardProvider;
use super::mouse_handler::MouseEffect;
use super::mouse_handler::MouseHandler;
use super::Tab;
use crate::background_jobs::BackgroundJob;
use crate::output::Output;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::ClientId;
use crate::ServerInstruction;
use std::collections::HashSet;
use std::str;
use yinz_panes::pane::Pane;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::Event;
use yinz_types::geometry::Direction;
use yinz_types::internal_ids::TerminalId;
use yinz_types::mouse::MouseEvent;
use yinz_types::position::Position;

impl Tab {
    pub fn start_text_selection(&mut self, client_id: ClientId) {
        let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) else {
            return;
        };
        let pane_id = active_pane.pid();
        active_pane.start_text_selection(client_id);
        self.keyboard_text_selection.insert(client_id, pane_id);
    }

    /// The pane a keyboard text selection is live on: the one it was started on, which is not
    /// necessarily the focused one any more.
    fn text_selection_pane_mut(&mut self, client_id: ClientId) -> Option<&mut Box<dyn Pane>> {
        match self.keyboard_text_selection.get(&client_id).copied() {
            Some(pane_id) => self.get_pane_with_id_mut(pane_id),
            None => self.get_active_pane_or_floating_pane_mut(client_id),
        }
    }

    pub fn move_text_selection(&mut self, direction: Direction, client_id: ClientId) {
        if let Some(pane) = self.text_selection_pane_mut(client_id) {
            pane.move_text_selection(direction, client_id);
        }
    }

    pub fn swap_text_selection_ends(&mut self, client_id: ClientId) {
        if let Some(pane) = self.text_selection_pane_mut(client_id) {
            pane.swap_text_selection_ends(client_id);
        }
    }

    pub fn remember_copied_text_selection(&mut self, client_id: ClientId) {
        if let Some(pane) = self.text_selection_pane_mut(client_id) {
            pane.remember_copied_text_selection(client_id);
        }
    }

    pub fn finish_text_selection(&mut self, client_id: ClientId) {
        let Some(pane_id) = self.keyboard_text_selection.remove(&client_id) else {
            return;
        };
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.finish_text_selection(client_id);
        }
    }

    /// Copies the last completed command's output and schedules the end of its flash.
    pub fn copy_last_command_output(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || format!("failed to copy last command output for client {client_id}");
        let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) else {
            return Ok(());
        };
        let pane_id = active_pane.pid();
        let Some(output) = active_pane.copy_last_command_output() else {
            return Ok(());
        };
        self.copy_text_to_clipboard(&output)
            .with_context(err_context)?;
        self.senders
            .send_to_background_jobs(BackgroundJob::ClearCommandOutputFlash { pane_id })
            .with_context(err_context)?;
        Ok(())
    }

    pub fn clear_command_output_flash(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.clear_command_output_flash();
        }
    }

    pub fn scroll_active_terminal_to_top(&mut self, client_id: ClientId) -> Result<()> {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.clear_scroll();
            if let Some(size) = active_pane.get_line_number() {
                active_pane.scroll_up(size, Some(client_id));
            }
        }
        Ok(())
    }

    pub fn scroll_terminal_to_top(&mut self, terminal_pane_id: TerminalId) {
        if let Some(terminal_pane) = self.get_pane_with_id_mut(PaneId::Terminal(terminal_pane_id)) {
            terminal_pane.clear_scroll();
            if let Some(size) = terminal_pane.get_line_number() {
                // Terminal panes ignore the client ID required by the shared Pane interface.
                terminal_pane.scroll_up(size, None);
            }
        }
    }

    /// Handler for `ScreenInstruction::ClearScroll`. Returns whether anything was
    /// scrolled, so the caller can skip a render when nothing changed.
    pub fn clear_active_terminal_scroll(&mut self, client_id: ClientId) -> Result<bool> {
        let err_context =
            || format!("failed to clear scroll in active pane for client {client_id}");

        let mut was_scrolled = false;
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            was_scrolled = active_pane.is_scrolled();
            active_pane.clear_scroll();
            if !active_pane.is_scrolled()
                && let PaneId::Terminal(raw_fd) = active_pane.pid()
            {
                self.process_pending_vte_events(raw_fd)
                    .with_context(err_context)?;
            }
        }
        Ok(was_scrolled)
    }

    pub fn handle_scrollwheel_up(
        &mut self,
        point: &Position,
        lines: usize,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        MouseHandler::handle_scrollwheel_up(self, point, lines, client_id)
    }

    pub fn handle_scrollwheel_down(
        &mut self,
        point: &Position,
        lines: usize,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        MouseHandler::handle_scrollwheel_down(self, point, lines, client_id)
    }

    pub(super) fn get_pane_id_at(
        &mut self,
        point: &Position,
        search_selectable: bool,
    ) -> Result<Option<PaneId>> {
        let err_context = || format!("failed to get id of pane at position {point:?}");

        if self.tiled_panes.fullscreen_is_active()
            && (self.tiled_panes.fullscreen_covers_ui()
                || self
                    .is_position_inside_viewport(point)
                    .with_context(err_context)?)
        {
            return Ok(self.tiled_panes.fullscreen_pane_id());
        }

        let (stacked_pane_ids_under_flexible_pane, _stacked_pane_ids_over_flexible_pane) = {
            self.tiled_panes
                .stacked_pane_ids_under_and_over_flexible_panes()
                .with_context(err_context)?
        };
        let pane_contains_point = |p: &dyn Pane,
                                   point: &Position,
                                   stacked_pane_ids_under_flexible_pane: &HashSet<PaneId>|
         -> bool {
            let is_flexible_in_stack =
                p.render_geom().is_stacked() && !p.render_geom().rows.is_fixed();
            let is_stacked_under = stacked_pane_ids_under_flexible_pane.contains(&p.pid());
            let geom_to_compare_against =
                if is_stacked_under && !self.pane_frame_style.draws_full_frames() {
                    // these sort of panes are one-liner panes under a flexible pane in a stack when we
                    // don't draw pane frames - because the whole stack's content is offset to allow
                    // room for the boundary between panes, they are actually drawn 1 line above where
                    // they are
                    let mut geom = p.render_geom();
                    geom.y = geom.y.saturating_sub(p.get_content_offset().bottom);
                    geom
                } else if is_flexible_in_stack && !self.pane_frame_style.draws_full_frames() {
                    // these sorts of panes are flexible panes inside a stack when we don't draw pane
                    // frames - because the whole stack's content is offset to give room for the
                    // boundary between panes, we need to take this offset into account when figuring
                    // out whether the position is inside them
                    let mut geom = p.render_geom();
                    geom.rows.decrease_inner(p.get_content_offset().bottom);
                    geom
                } else {
                    p.render_geom()
                };
            geom_to_compare_against.contains(point)
        };

        let found_pane_id = if search_selectable {
            self.get_selectable_tiled_panes()
                .find(|(_, p)| {
                    pane_contains_point(p.as_ref(), point, &stacked_pane_ids_under_flexible_pane)
                })
                .map(|(&id, _)| id)
        } else {
            self.get_tiled_panes()
                .find(|(_, p)| {
                    pane_contains_point(p.as_ref(), point, &stacked_pane_ids_under_flexible_pane)
                })
                .map(|(&id, _)| id)
        };
        let resolved_pane_id = found_pane_id.map(|id| self.stack_list_member_at_point(id, point));
        if search_selectable {
            Ok(resolved_pane_id.filter(|id| {
                self.get_pane_with_id(*id)
                    .map(|p| p.selectable())
                    .unwrap_or(false)
            }))
        } else {
            Ok(resolved_pane_id)
        }
    }

    /// `handle_mouse_event_with_passthrough` for a client not descended into any pane.
    #[cfg(test)]
    pub fn handle_mouse_event(
        &mut self,
        event: &MouseEvent,
        client_id: ClientId,
    ) -> Result<MouseEffect> {
        self.handle_mouse_event_with_passthrough(event, client_id, None)
    }

    /// `passthrough_pane_id` is the pane the client is descended into, if any: alt+mouse over
    /// it is written to the pane when it tracks the mouse, instead of grouping host panes.
    pub fn handle_mouse_event_with_passthrough(
        &mut self,
        event: &MouseEvent,
        client_id: ClientId,
        passthrough_pane_id: Option<PaneId>,
    ) -> Result<MouseEffect> {
        if self.pane_tabs_mouse(event, client_id)? {
            return Ok(MouseEffect::state_changed());
        }
        MouseHandler::handle_mouse_event(self, event, client_id, passthrough_pane_id)
    }

    pub fn copy_selection(&self, client_id: ClientId) -> Result<()> {
        // a keyboard selection is copied from the pane it was started on, which is not
        // necessarily the focused one any more
        let selected_text = match self.keyboard_text_selection.get(&client_id).copied() {
            Some(pane_id) => self.get_pane_with_id(pane_id),
            None => self.get_active_pane(client_id),
        }
        .and_then(|p| p.get_selected_text(client_id));
        if let Some(selected_text) = selected_text {
            // `write_selection_to_clipboard` already tells the plugins how it went - and says
            // `SystemClipboardFailure` when it went badly, which a second unconditional
            // `CopyToClipboard` from here used to paper over.
            self.write_selection_to_clipboard(&selected_text)
                .with_context(|| {
                    format!("failed to write selection to clipboard for client {client_id}")
                })?;
        }
        Ok(())
    }
    pub fn copy_text_to_clipboard(&self, text: &str) -> Result<()> {
        self.write_selection_to_clipboard(text)
            .with_context(|| "failed to write text to clipboard".to_string())
    }

    pub fn write_selection_to_clipboard(&self, selection: &str) -> Result<()> {
        let err_context = || "failed to write selection to clipboard";

        let mut output = Output::default();
        let connected_clients: HashSet<ClientId> =
            { self.connected_clients.borrow().iter().copied().collect() };
        output.add_clients(&connected_clients, self.link_handler.clone(), None);
        let client_ids = connected_clients.iter().copied();
        let senders = self.senders.clone();
        let on_command_complete = move |result: Result<()>| {
            let event = match result {
                Ok(()) => Event::CopyToClipboard(yinz_types::data::CopyDestination::Command),
                Err(error) => {
                    Err::<(), _>(error)
                        .context("failed to write selection to clipboard")
                        .non_fatal();
                    Event::SystemClipboardFailure
                },
            };
            senders
                .send_to_plugin(PluginInstruction::Update(vec![(None, None, event)]))
                .context("failed to notify plugins about new clipboard event")
                .non_fatal();
        };
        let clipboard_event = match self.clipboard_provider.set_content(
            selection,
            &mut output,
            client_ids,
            on_command_complete,
        ) {
            Ok(()) if matches!(self.clipboard_provider, ClipboardProvider::Command(_)) => {
                return Ok(());
            },
            Ok(()) => output
                .serialize()
                .and_then(|serialized_output| {
                    if serialized_output.is_empty() {
                        return Ok(());
                    }
                    self.senders
                        .send_to_server(ServerInstruction::Render(Some(serialized_output)))
                })
                .map(|_| Event::CopyToClipboard(self.clipboard_provider.as_copy_destination()))
                .with_context(err_context)?,
            Err(err) => {
                Err::<(), _>(err).with_context(err_context).non_fatal();
                Event::SystemClipboardFailure
            },
        };
        self.senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                clipboard_event,
            )]))
            .context("failed to notify plugins about new clipboard event")
            .non_fatal();

        Ok(())
    }

    /// Input that is not text is dropped, the same as a control sequence: a keystroke is never a
    /// reason to fail the screen thread, so this cannot error.
    pub fn update_search_term(&mut self, buf: Vec<u8>, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            let Ok(s) = str::from_utf8(&buf) else {
                return;
            };
            // Accept text plus the existing reset, backspace and delete commands.
            let is_updatable = s
                .chars()
                .all(|c| !c.is_control() || matches!(c, '\0' | '\u{8}' | '\u{7f}'));
            if is_updatable {
                active_pane.update_search_term(s);
            }
        }
    }

    pub fn search_down(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.search_down();
        }
    }

    pub fn search_up(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.search_up();
        }
    }

    pub fn toggle_search_case_sensitivity(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.toggle_search_case_sensitivity();
        }
    }

    pub fn toggle_search_wrap(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.toggle_search_wrap();
        }
    }

    pub fn toggle_search_whole_words(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.toggle_search_whole_words();
        }
    }

    pub fn clear_search(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.clear_search();
        }
    }
}

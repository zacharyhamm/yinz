//! Scrollback navigation, capture, and editor panes.

use super::Tab;
use crate::panes::PaneId;
use crate::panes::TerminalPane;
use crate::pty::ClientTabIndexOrPaneId;
use crate::pty::PtyInstruction;
use crate::route::NotificationEnd;
use crate::ClientId;
use std::env::temp_dir;
use uuid::Uuid;
use yinz_panes::pane::Pane;
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TerminalId;
use yinz_types::pane_size::PaneGeom;
use yinz_types::position::Position;

impl Tab {
    pub fn clear_active_terminal_screen(&mut self, client_id: ClientId) -> Result<()> {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.clear_screen();
        }
        Ok(())
    }
    pub fn clear_screen_for_pane_id(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.clear_screen();
        }
    }
    pub fn dump_active_terminal_screen(
        &mut self,
        file: Option<String>,
        client_id: ClientId,
        full: bool,
    ) -> Result<()> {
        let err_context =
            || format!("failed to dump active terminal screen for client {client_id}");

        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            let dump = active_pane.dump_screen(full, Some(client_id));
            self.os_api
                .write_to_file(dump, file)
                .with_context(err_context)?;
        }
        Ok(())
    }
    pub fn dump_with_ansi_active_terminal_screen(
        &mut self,
        file: Option<String>,
        client_id: ClientId,
        full: bool,
    ) -> Result<()> {
        let err_context =
            || format!("failed to dump active terminal screen for client {client_id}");

        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            let dump = active_pane.dump_screen_with_ansi(full, Some(client_id));
            self.os_api
                .write_to_file(dump, file)
                .with_context(err_context)?;
        }
        Ok(())
    }
    pub fn dump_terminal_screen(
        &mut self,
        file: Option<String>,
        pane_id: PaneId,
        full: bool,
    ) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id(pane_id) {
            let dump = pane.dump_screen(full, None);
            self.os_api.write_to_file(dump, file).non_fatal()
        }
        Ok(())
    }
    pub fn dump_with_ansi_terminal_screen(
        &mut self,
        file: Option<String>,
        pane_id: PaneId,
        full: bool,
    ) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id(pane_id) {
            let dump = pane.dump_screen_with_ansi(full, None);
            self.os_api.write_to_file(dump, file).non_fatal()
        }
        Ok(())
    }
    pub fn get_dump_active_terminal_screen(&mut self, client_id: ClientId, full: bool) -> String {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.dump_screen(full, Some(client_id))
        } else {
            String::new()
        }
    }
    pub fn get_dump_with_ansi_active_terminal_screen(
        &mut self,
        client_id: ClientId,
        full: bool,
    ) -> String {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.dump_screen_with_ansi(full, Some(client_id))
        } else {
            String::new()
        }
    }
    pub fn get_dump_terminal_screen(&mut self, pane_id: PaneId, full: bool) -> Option<String> {
        self.get_pane_with_id(pane_id)
            .map(|pane| pane.dump_screen(full, None))
    }
    pub fn get_dump_with_ansi_terminal_screen(
        &mut self,
        pane_id: PaneId,
        full: bool,
    ) -> Option<String> {
        self.get_pane_with_id(pane_id)
            .map(|pane| pane.dump_screen_with_ansi(full, None))
    }
    pub fn edit_scrollback(
        &mut self,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        let err_context = || format!("failed to edit scrollback for client {client_id}");

        let mut file = temp_dir();
        file.push(format!("{}.dump", Uuid::new_v4()));
        self.dump_active_terminal_screen(
            Some(String::from(file.to_string_lossy())),
            client_id,
            true,
        )
        .with_context(err_context)?;
        let line_number = self
            .get_active_pane(client_id)
            .and_then(|a_t| a_t.get_line_number());
        self.senders
            .send_to_pty(PtyInstruction::OpenInPlaceEditor(
                file,
                line_number,
                ClientTabIndexOrPaneId::ClientId(client_id),
                completion_tx,
            ))
            .with_context(err_context)
    }
    pub fn edit_scrollback_raw(
        &mut self,
        client_id: ClientId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        let err_context = || format!("failed to edit scrollback for client {client_id}");

        let mut file = temp_dir();
        file.push(format!("{}.dump", Uuid::new_v4()));
        self.dump_with_ansi_active_terminal_screen(
            Some(String::from(file.to_string_lossy())),
            client_id,
            true,
        )
        .with_context(err_context)?;
        let line_number = self
            .get_active_pane(client_id)
            .and_then(|a_t| a_t.get_line_number());
        self.senders
            .send_to_pty(PtyInstruction::OpenInPlaceEditor(
                file,
                line_number,
                ClientTabIndexOrPaneId::ClientId(client_id),
                completion_tx,
            ))
            .with_context(err_context)
    }
    pub fn edit_scrollback_for_pane_with_id(
        &mut self,
        pane_id: PaneId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        if let PaneId::Terminal(_terminal_pane_id) = pane_id {
            self.make_hidden_stack_list_member_visible(pane_id);
            let mut file = temp_dir();
            file.push(format!("{}.dump", Uuid::new_v4()));
            self.dump_terminal_screen(Some(String::from(file.to_string_lossy())), pane_id, true)
                .non_fatal();
            let line_number = self
                .get_pane_with_id(pane_id)
                .and_then(|a_t| a_t.get_line_number());
            self.senders.send_to_pty(PtyInstruction::OpenInPlaceEditor(
                file,
                line_number,
                ClientTabIndexOrPaneId::PaneId(pane_id),
                completion_tx,
            ))
        } else {
            log::error!("Editing plugin pane scrollback is currently unsupported.");
            Ok(())
        }
    }
    pub fn edit_scrollback_raw_for_pane_with_id(
        &mut self,
        pane_id: PaneId,
        completion_tx: Option<NotificationEnd>,
    ) -> Result<()> {
        if let PaneId::Terminal(_terminal_pane_id) = pane_id {
            self.make_hidden_stack_list_member_visible(pane_id);
            let mut file = temp_dir();
            file.push(format!("{}.dump", Uuid::new_v4()));
            self.dump_with_ansi_terminal_screen(
                Some(String::from(file.to_string_lossy())),
                pane_id,
                true,
            )
            .non_fatal();
            let line_number = self
                .get_pane_with_id(pane_id)
                .and_then(|a_t| a_t.get_line_number());
            self.senders.send_to_pty(PtyInstruction::OpenInPlaceEditor(
                file,
                line_number,
                ClientTabIndexOrPaneId::PaneId(pane_id),
                completion_tx,
            ))
        } else {
            log::error!("Editing plugin pane scrollback is currently unsupported.");
            Ok(())
        }
    }
    pub fn scroll_active_terminal_up(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.scroll_up(1, Some(client_id));
        }
    }

    pub fn scroll_terminal_up(&mut self, terminal_pane_id: TerminalId) -> Result<()> {
        let pane_id = PaneId::Terminal(terminal_pane_id);
        let err_context = || format!("failed to scroll up pane {pane_id:?}");
        let synthetic_position = Position::new(0, 0);
        let (mouse_sgr, is_alt) = match self.get_pane_with_id(pane_id) {
            Some(pane) => (
                pane.mouse_scroll_up(&synthetic_position),
                pane.is_alternate_mode_active(),
            ),
            None => return Ok(()),
        };
        if let Some(sgr) = mouse_sgr {
            self.write_mouse_to_pane_id(sgr.into_bytes(), pane_id, None)
                .with_context(err_context)?;
        } else if is_alt {
            self.write_to_pane_id(&None, b"\x1b[A".to_vec(), false, pane_id, None, None)
                .with_context(err_context)?;
        } else if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.scroll_up(1, None);
        }
        Ok(())
    }

    pub fn scroll_active_terminal_down(&mut self, client_id: ClientId) -> Result<()> {
        let err_context = || format!("failed to scroll down active pane for client {client_id}");

        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.scroll_down(1, Some(client_id));
            if !active_pane.is_scrolled()
                && let PaneId::Terminal(raw_fd) = active_pane.pid()
            {
                self.process_pending_vte_events(raw_fd)
                    .with_context(err_context)?;
            }
        }
        Ok(())
    }

    pub fn scroll_terminal_down(&mut self, terminal_pane_id: TerminalId) -> Result<()> {
        let pane_id = PaneId::Terminal(terminal_pane_id);
        let err_context = || format!("failed to scroll down pane {pane_id:?}");
        let synthetic_position = Position::new(0, 0);
        let (mouse_sgr, is_alt) = match self.get_pane_with_id(pane_id) {
            Some(pane) => (
                pane.mouse_scroll_down(&synthetic_position),
                pane.is_alternate_mode_active(),
            ),
            None => return Ok(()),
        };
        if let Some(sgr) = mouse_sgr {
            self.write_mouse_to_pane_id(sgr.into_bytes(), pane_id, None)
                .with_context(err_context)?;
        } else if is_alt {
            self.write_to_pane_id(&None, b"\x1b[B".to_vec(), false, pane_id, None, None)
                .with_context(err_context)?;
        } else {
            let needs_flush = if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
                pane.scroll_down(1, None);
                !pane.is_scrolled()
            } else {
                false
            };
            if needs_flush {
                self.process_pending_vte_events(terminal_pane_id)
                    .with_context(err_context)?;
            }
        }
        Ok(())
    }

    pub fn scroll_active_terminal_up_page(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            // prevent overflow when row == 0
            let scroll_rows = active_pane.rows().max(1).saturating_sub(1);
            active_pane.scroll_up(scroll_rows, Some(client_id));
        }
    }

    pub fn scroll_terminal_page_up(&mut self, terminal_pane_id: TerminalId) {
        if let Some(terminal_pane) = self.get_pane_with_id_mut(PaneId::Terminal(terminal_pane_id)) {
            // Terminal panes ignore the client ID required by the shared Pane interface.
            // prevent overflow when row == 0
            let scroll_rows = terminal_pane.rows().max(1).saturating_sub(1);
            terminal_pane.scroll_up(scroll_rows, None);
        }
    }

    pub fn scroll_active_terminal_down_page(&mut self, client_id: ClientId) -> Result<()> {
        let err_context =
            || format!("failed to scroll down one page in active pane for client {client_id}");

        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            let scroll_rows = active_pane.get_content_rows();
            active_pane.scroll_down(scroll_rows, Some(client_id));
            if !active_pane.is_scrolled()
                && let PaneId::Terminal(raw_fd) = active_pane.pid()
            {
                self.process_pending_vte_events(raw_fd)
                    .with_context(err_context)?;
            }
        }
        Ok(())
    }

    pub fn scroll_terminal_page_down(&mut self, terminal_pane_id: TerminalId) {
        if let Some(terminal_pane) = self.get_pane_with_id_mut(PaneId::Terminal(terminal_pane_id)) {
            // Terminal panes ignore the client ID required by the shared Pane interface.
            let scroll_rows = terminal_pane.get_content_rows();
            terminal_pane.scroll_down(scroll_rows, None);
            if !terminal_pane.is_scrolled()
                && let PaneId::Terminal(raw_fd) = terminal_pane.pid()
            {
                self.process_pending_vte_events(raw_fd).non_fatal()
            }
        }
    }

    pub fn scroll_active_terminal_up_half_page(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            // prevent overflow when row == 0
            let scroll_rows = (active_pane.rows().max(1).saturating_sub(1)) / 2;
            active_pane.scroll_up(scroll_rows, Some(client_id));
        }
    }

    pub fn scroll_active_terminal_down_half_page(&mut self, client_id: ClientId) -> Result<()> {
        let err_context =
            || format!("failed to scroll down half a page in active pane for client {client_id}");

        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            let scroll_rows = (active_pane.rows().max(1) - 1) / 2;
            active_pane.scroll_down(scroll_rows, Some(client_id));
            if !active_pane.is_scrolled()
                && let PaneId::Terminal(raw_fd) = active_pane.pid()
            {
                self.process_pending_vte_events(raw_fd)
                    .with_context(err_context)?;
            }
        }
        Ok(())
    }

    pub fn scroll_active_terminal_to_bottom(&mut self, client_id: ClientId) -> Result<()> {
        let err_context =
            || format!("failed to scroll to bottom in active pane for client {client_id}");

        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.clear_scroll();
            if !active_pane.is_scrolled()
                && let PaneId::Terminal(raw_fd) = active_pane.pid()
            {
                self.process_pending_vte_events(raw_fd)
                    .with_context(err_context)?;
            }
        }
        Ok(())
    }

    pub fn scroll_terminal_to_bottom(&mut self, terminal_pane_id: TerminalId) {
        if let Some(terminal_pane) = self.get_pane_with_id_mut(PaneId::Terminal(terminal_pane_id)) {
            terminal_pane.clear_scroll();
            if !terminal_pane.is_scrolled()
                && let PaneId::Terminal(raw_fd) = terminal_pane.pid()
            {
                self.process_pending_vte_events(raw_fd).non_fatal();
            }
        }
    }

    pub fn scroll_active_terminal_to_previous_prompt(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.scroll_to_previous_prompt(client_id);
        }
    }

    pub fn scroll_active_terminal_to_next_prompt(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.scroll_to_next_prompt(client_id);
        }
    }

    pub fn select_command_at_scroll_position(&mut self, client_id: ClientId) {
        if let Some(active_pane) = self.get_active_pane_or_floating_pane_mut(client_id) {
            active_pane.select_command_at_scroll_position(client_id);
        }
    }

    pub(super) fn new_scrollback_editor_pane(&self, pid: TerminalId) -> TerminalPane {
        let next_terminal_position = self.get_next_terminal_position();
        let mut new_pane = TerminalPane::new(
            pid,
            PaneGeom::default(), // the initial size will be set later
            self.style,
            next_terminal_position,
            String::new(),
            self.link_handler.clone(),
            self.character_cell_size.clone(),
            self.sixel_image_store.clone(),
            self.kitty_image_store.clone(),
            self.terminal_emulator_colors.clone(),
            self.terminal_emulator_color_codes.clone(),
            None,
            None,
            self.debug,
            self.arrow_fonts,
            self.styled_underlines,
            self.osc8_hyperlinks,
            self.explicitly_disable_kitty_keyboard_protocol,
            None,
        );
        if let Some(supported) = self.kitty_host_support {
            new_pane.update_kitty_host_support(supported);
        }
        if let Some(supported) = self.sixel_host_support {
            new_pane.update_sixel_host_support(supported);
        }
        new_pane.update_name("EDITING SCROLLBACK"); // we do this here and not in the
                                                    // constructor so it won't be overrided
                                                    // by the editor
        new_pane
    }
    pub(super) fn insert_scrollback_editor_replaced_pane(
        &mut self,
        replaced_pane: Box<dyn Pane>,
        terminal_pane_id: TerminalId,
    ) {
        let is_scrollback_editor = true;
        let lent = false;
        self.suppress_pane_behind(
            replaced_pane,
            PaneId::Terminal(terminal_pane_id),
            is_scrollback_editor,
            lent,
        );
    }

    // --- Pane-targeting CLI methods ---
    pub fn scroll_up_by_pane_id(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.scroll_up(1, None);
        }
    }
    pub fn scroll_down_by_pane_id(&mut self, pane_id: PaneId) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.scroll_down(1, None);
            if !pane.is_scrolled()
                && let PaneId::Terminal(raw_fd) = pane.pid()
            {
                self.process_pending_vte_events(raw_fd)?;
            }
        }
        Ok(())
    }
    pub fn scroll_to_top_by_pane_id(&mut self, pane_id: PaneId) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.clear_scroll();
            if let Some(size) = pane.get_line_number() {
                pane.scroll_up(size, None);
            }
        }
        Ok(())
    }
    pub fn scroll_to_bottom_by_pane_id(&mut self, pane_id: PaneId) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            pane.clear_scroll();
            if !pane.is_scrolled()
                && let PaneId::Terminal(raw_fd) = pane.pid()
            {
                self.process_pending_vte_events(raw_fd)?;
            }
        }
        Ok(())
    }
    pub fn page_scroll_up_by_pane_id(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            let scroll_rows = pane.rows().max(1).saturating_sub(1);
            pane.scroll_up(scroll_rows, None);
        }
    }
    pub fn page_scroll_down_by_pane_id(&mut self, pane_id: PaneId) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            let scroll_rows = pane.get_content_rows();
            pane.scroll_down(scroll_rows, None);
            if !pane.is_scrolled()
                && let PaneId::Terminal(raw_fd) = pane.pid()
            {
                self.process_pending_vte_events(raw_fd)?;
            }
        }
        Ok(())
    }
    pub fn half_page_scroll_up_by_pane_id(&mut self, pane_id: PaneId) {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            let scroll_rows = (pane.rows().max(1).saturating_sub(1)) / 2;
            pane.scroll_up(scroll_rows, None);
        }
    }
    pub fn half_page_scroll_down_by_pane_id(&mut self, pane_id: PaneId) -> Result<()> {
        if let Some(pane) = self.get_pane_with_id_mut(pane_id) {
            let scroll_rows = (pane.rows().max(1).saturating_sub(1)) / 2;
            pane.scroll_down(scroll_rows, None);
            if !pane.is_scrolled()
                && let PaneId::Terminal(raw_fd) = pane.pid()
            {
                self.process_pending_vte_events(raw_fd)?;
            }
        }
        Ok(())
    }
}

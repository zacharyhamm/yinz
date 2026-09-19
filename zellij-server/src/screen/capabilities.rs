use super::Screen;
use crate::panes::alacritty_functions::xparse_color;
use crate::panes::kitty_graphics::KittyHostSupport;
use crate::panes::terminal_character::AnsiCode;
use crate::ClientId;
use zellij_ipc::ipc::PixelDimensions;
use zellij_runtime::errors::prelude::*;
use zellij_types::colors::PaletteColor;
use zellij_types::pane_size::SizeInPixels;

impl Screen {
    /// Returns whether the character cell size changed, so the caller can re-apply
    /// pty sizes: ptys created before the host reported its pixel dimensions were
    /// sized without them.
    pub fn update_pixel_dimensions(
        &mut self,
        client_id: ClientId,
        pixel_dimensions: PixelDimensions,
    ) -> bool {
        let previous_character_cell_size = *self.character_cell_size.borrow();
        self.pixel_dimensions.merge(pixel_dimensions);
        if let Some(character_cell_size) = self.pixel_dimensions.character_cell_size {
            *self.character_cell_size.borrow_mut() = Some(character_cell_size);
        } else if let Some(text_area_size) = self.pixel_dimensions.text_area_size {
            let Some(client_size) = self.client_sizes.get(&client_id).copied() else {
                return false;
            };
            if client_size.rows == 0 || client_size.cols == 0 {
                return false;
            }
            let character_cell_size = SizeInPixels {
                height: text_area_size.height / client_size.rows,
                width: text_area_size.width / client_size.cols,
            };
            *self.character_cell_size.borrow_mut() = Some(character_cell_size);
        }
        *self.character_cell_size.borrow() != previous_character_cell_size
    }

    /// Re-send every pane's pty size, so a character cell size learned after the
    /// panes were created reaches their ptys as pixel dimensions.
    pub fn resize_pty_all_panes(&mut self) -> Result<()> {
        for tab in self.tabs.values_mut() {
            tab.resize_pty_all_panes()
                .context("failed to re-apply pty sizes")?;
        }
        Ok(())
    }

    pub fn update_kitty_graphics_support(&mut self, client_id: ClientId, supported: bool) {
        // The probe reply can outlive its client. `remove_client` drops both this entry and the
        // `mode_info` one because client ids are recycled; re-creating them here would put the
        // departed terminal's capability back into the tab aggregate, and into every tab
        // created afterwards.
        if !self.connected_clients_contains(&client_id) {
            return;
        }
        self.kitty_probe_answers.insert(client_id, supported);
        let supported = supported && self.support_kitty_graphics_protocol;
        self.kitty_host_capabilities
            .borrow_mut()
            .insert(client_id, supported);
        self.push_kitty_host_support_to_tabs();
        self.push_kitty_capability_to_plugins(client_id, supported);
    }

    /// Re-derive every client's kitty graphics capability from its probe answer, after a reload
    /// changed `support_kitty_graphics_protocol`.
    pub(super) fn reapply_kitty_graphics_support(&mut self) {
        let answers: Vec<(ClientId, bool)> = self
            .kitty_probe_answers
            .iter()
            .map(|(client_id, answered)| (*client_id, *answered))
            .collect();
        for (client_id, answered) in answers {
            let supported = answered && self.support_kitty_graphics_protocol;
            self.kitty_host_capabilities
                .borrow_mut()
                .insert(client_id, supported);
            self.push_kitty_capability_to_plugins(client_id, supported);
        }
        self.push_kitty_host_support_to_tabs();
    }

    pub(super) fn kitty_host_support_aggregate(&self) -> Option<KittyHostSupport> {
        if !self.support_kitty_graphics_protocol {
            return Some(KittyHostSupport::ProtocolDisabled);
        }
        let capabilities = self.kitty_host_capabilities.borrow();
        if capabilities.is_empty() {
            None
        } else {
            Some(KittyHostSupport::from_host_capability(
                capabilities.values().any(|supported| *supported),
            ))
        }
    }

    pub(super) fn push_kitty_host_support_to_tabs(&mut self) {
        if let Some(aggregate) = self.kitty_host_support_aggregate() {
            for tab in self.tabs.values_mut() {
                tab.update_kitty_host_support(aggregate);
            }
        }
    }

    /// Plugins learn whether an image is worth drawing from `ModeInfo`, which - unlike the pane
    /// side above - is per client and cached in three places. The capability is not known when
    /// `ModeInfo` is built (`thread_main.rs`, from config, before anyone attaches); it arrives with
    /// the client's probe reply, so it has to be written back into each copy and re-fanned out.
    /// Same shape as the host-theme swap in `Screen::apply_host_terminal_theme_mode`.
    fn push_kitty_capability_to_plugins(&mut self, client_id: ClientId, supported: bool) {
        let default_for_new = self.default_mode_info.clone();
        let mode_info = self.mode_info.entry(client_id).or_insert(default_for_new);
        mode_info.capabilities.kitty_graphics = supported;
        let mode_info = mode_info.clone();
        // `update_input_modes` fans out `ModeUpdate` from the tabs' per-client map, not from
        // Screen's, so without this step plugins would keep being told the old answer.
        for tab in self.tabs.values_mut() {
            tab.change_mode_info(mode_info.clone(), client_id);
        }
        for tab in self.tabs.values_mut() {
            tab.update_input_modes()
                .context("failed to push kitty graphics support to plugins")
                .non_fatal();
        }
    }

    pub fn update_sixel_support(&mut self, client_id: ClientId, supported: bool) {
        self.sixel_host_capabilities
            .borrow_mut()
            .insert(client_id, supported);
        self.push_sixel_host_support_to_tabs();
    }

    /// Remember whether this client's host presents each frame atomically (DECSET 2026).
    ///
    /// Unguarded, unlike `update_kitty_graphics_support`: that one arrives long after attach and
    /// can outlive its client, so it has to check that the id still means what it meant. This one
    /// is sent by the client the moment it attaches, and takes the same route through the server's
    /// main loop as `AddClient` and `RemoveClient` do - so it cannot land on the wrong side of
    /// either, and there is nothing left for a guard here to catch. Watchers report too, and they
    /// are not in `connected_clients`, so a guard would in fact throw their answers away.
    pub fn update_synchronized_output_support(
        &mut self,
        client_id: ClientId,
        supported: bool,
    ) -> Result<()> {
        self.synchronized_output_capabilities
            .borrow_mut()
            .insert(client_id, supported);
        // The watchers share one byte stream, hidden according to what every one of their hosts
        // can do (see `Screen::render_to_clients`), so one watcher's answer changes the frame the
        // others get. Rare enough to just repaint them all - and to ask for that repaint here,
        // since this answer arrives on its own and a session with nothing else to say would
        // otherwise leave the request armed and unspent.
        if self.watcher_clients.contains_key(&client_id) {
            for watcher_state in self.watcher_clients.values_mut() {
                watcher_state.set_force_render();
            }
            self.render(None)
                .context("failed to repaint the watchers after a synchronized-output answer")?;
        }
        Ok(())
    }

    pub(super) fn sixel_host_support_aggregate(&self) -> Option<bool> {
        let capabilities = self.sixel_host_capabilities.borrow();
        if capabilities.is_empty() {
            None
        } else {
            Some(capabilities.values().any(|supported| *supported))
        }
    }

    pub(super) fn push_sixel_host_support_to_tabs(&mut self) {
        if let Some(aggregate) = self.sixel_host_support_aggregate() {
            for tab in self.tabs.values_mut() {
                tab.update_sixel_host_support(aggregate);
            }
        }
    }

    pub fn update_terminal_background_color(&mut self, background_color_instruction: String) {
        if let Some(AnsiCode::RgbCode((r, g, b))) =
            xparse_color(background_color_instruction.as_bytes())
        {
            let bg_palette_color = PaletteColor::Rgb((r, g, b));
            self.terminal_emulator_colors.borrow_mut().bg = bg_palette_color;
        }
    }

    pub fn update_terminal_foreground_color(&mut self, foreground_color_instruction: String) {
        if let Some(AnsiCode::RgbCode((r, g, b))) =
            xparse_color(foreground_color_instruction.as_bytes())
        {
            let fg_palette_color = PaletteColor::Rgb((r, g, b));
            self.terminal_emulator_colors.borrow_mut().fg = fg_palette_color;
        }
    }

    pub fn update_terminal_color_registers(&mut self, color_registers: Vec<(usize, String)>) {
        let mut terminal_emulator_color_codes = self.terminal_emulator_color_codes.borrow_mut();
        for (color_register, color_sequence) in color_registers {
            terminal_emulator_color_codes.insert(color_register, color_sequence);
        }
    }
}

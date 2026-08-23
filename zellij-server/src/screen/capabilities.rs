use super::Screen;
use crate::panes::alacritty_functions::xparse_color;
use crate::panes::kitty_graphics::KittyHostSupport;
use crate::panes::terminal_character::AnsiCode;
use crate::ClientId;
use zellij_utils::data::PaletteColor;
use zellij_utils::ipc::PixelDimensions;
use zellij_utils::pane_size::SizeInPixels;

impl Screen {
    pub fn update_pixel_dimensions(
        &mut self,
        client_id: ClientId,
        pixel_dimensions: PixelDimensions,
    ) {
        self.pixel_dimensions.merge(pixel_dimensions);
        if let Some(character_cell_size) = self.pixel_dimensions.character_cell_size {
            *self.character_cell_size.borrow_mut() = Some(character_cell_size);
        } else if let Some(text_area_size) = self.pixel_dimensions.text_area_size {
            let Some(client_size) = self.client_sizes.get(&client_id).copied() else {
                return;
            };
            if client_size.rows == 0 || client_size.cols == 0 {
                return;
            }
            let character_cell_size = SizeInPixels {
                height: text_area_size.height / client_size.rows,
                width: text_area_size.width / client_size.cols,
            };
            *self.character_cell_size.borrow_mut() = Some(character_cell_size);
        }
    }

    pub fn update_kitty_graphics_support(&mut self, client_id: ClientId, supported: bool) {
        let supported = supported && self.support_kitty_graphics_protocol;
        self.kitty_host_capabilities
            .borrow_mut()
            .insert(client_id, supported);
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

    pub fn update_sixel_support(&mut self, client_id: ClientId, supported: bool) {
        self.sixel_host_capabilities
            .borrow_mut()
            .insert(client_id, supported);
        self.push_sixel_host_support_to_tabs();
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

use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;
use yinz_types::keys::KeyWithModifier;
use yinz_types::mouse::{MouseEvent, MouseEventType};
use yinz_types::pane_size::Viewport;

use super::Screen;
use crate::output::Output;
use crate::ui::command_palette::CommandPalette;
use crate::ui::damage::CellRect;
use crate::ClientId;

pub(super) struct CommandPaletteState {
    widget: CommandPalette,
    tab_id: TabId,
    painted: Option<CellRect>,
}

impl Screen {
    pub(crate) fn toggle_command_palette(&mut self, client_id: ClientId) {
        if self.close_command_palette(client_id) {
            return;
        }
        let Some(tab_id) = self.active_tab_ids.get(&client_id).copied() else {
            return;
        };
        self.close_context_menu(client_id);
        self.command_palettes.insert(
            client_id,
            CommandPaletteState {
                widget: CommandPalette::default(),
                tab_id,
                painted: None,
            },
        );
    }

    pub(crate) fn close_command_palette(&mut self, client_id: ClientId) -> bool {
        if self.command_palettes.remove(&client_id).is_none() {
            return false;
        }
        // Also restores clipped graphics and any cells exposed by a tab switch.
        for tab in self.tabs.values_mut() {
            tab.set_should_clear_display_before_rendering();
            tab.set_force_render();
        }
        true
    }

    pub(super) fn dismiss_stale_command_palettes(&mut self) {
        let stale: Vec<_> = self
            .command_palettes
            .iter()
            .filter(|(client, state)| self.active_tab_ids.get(client) != Some(&state.tab_id))
            .map(|(client, _)| *client)
            .collect();
        for client in stale {
            self.close_command_palette(client);
        }
    }

    fn command_palette_surface(&self, client_id: ClientId) -> Option<Viewport> {
        let tab_id = *self.active_tab_ids.get(&client_id)?;
        let mut surface = self.session_surface_for_tab(tab_id)?;
        if let Some(size) = self.client_sizes.get(&client_id) {
            surface.cols = surface.cols.min(size.cols);
            surface.rows = surface.rows.min(size.rows);
        }
        Some(surface)
    }

    /// This check runs before keybindings, plugin intercepts, writes, and paste delivery.
    pub(crate) fn command_palette_handles_input(
        &mut self,
        key: &Option<KeyWithModifier>,
        bytes: &[u8],
        toggle: bool,
        client_id: ClientId,
    ) -> bool {
        let Some(state) = self.command_palettes.get_mut(&client_id) else {
            return false;
        };
        let close = toggle
            || if let Some(key) = key {
                state.widget.handle_key(key.clone())
            } else {
                // Explicit writes can still include bracketed-paste markers.
                if bytes != b"\x1b[200~" && bytes != b"\x1b[201~" {
                    state.widget.insert_text(&String::from_utf8_lossy(bytes));
                }
                false
            };
        if close {
            self.close_command_palette(client_id);
        }
        for tab in self.tabs.values_mut() {
            tab.pending_agent_escapes
                .retain(|(client, _), _| *client != client_id);
        }
        self.render().non_fatal();
        true
    }

    pub(crate) fn command_palette_handles_mouse(
        &mut self,
        event: &MouseEvent,
        client_id: ClientId,
    ) -> bool {
        if self.command_palette_mouse_captures.contains(&client_id) {
            if event.event_type == MouseEventType::Release {
                self.command_palette_mouse_captures.remove(&client_id);
            }
            return true;
        }
        let Some(surface) = self.command_palette_surface(client_id) else {
            return self.command_palettes.contains_key(&client_id);
        };
        let Some(state) = self.command_palettes.get_mut(&client_id) else {
            return false;
        };
        let layout = state.widget.layout(surface);
        if event.event_type == MouseEventType::Press && (event.left || event.right || event.middle)
        {
            let inside = usize::try_from(event.position.line())
                .ok()
                .is_some_and(|y| layout.rect.contains(event.position.column(), y));
            if !inside {
                self.close_command_palette(client_id);
                self.command_palette_mouse_captures.insert(client_id);
            } else if event.left {
                state.widget.click(&layout, event.position);
            }
            self.render().non_fatal();
        }
        true
    }

    pub(super) fn prepare_command_palette_render(&mut self, output: &mut Output) {
        output.set_input_overlay_clients(self.command_palettes.keys().copied());
        let layouts: Vec<_> = self
            .command_palettes
            .iter()
            .filter_map(|(client, state)| {
                let surface = self.command_palette_surface(*client)?;
                Some((*client, state.widget.layout(surface).rect))
            })
            .collect();
        for (client, rect) in layouts {
            let Some(state) = self.command_palettes.get_mut(&client) else {
                continue;
            };
            let geometry_changed = state
                .painted
                .replace(rect)
                .is_some_and(|previous| previous != rect);
            // ponytail: repaint intersecting panes per frame; cache cells only if measured cost warrants it.
            if let Some(tab) = self.tabs.get_mut(&state.tab_id) {
                if geometry_changed {
                    tab.set_should_clear_display_before_rendering();
                    tab.set_force_render();
                } else {
                    tab.force_render_panes_intersecting(&[rect]);
                }
            }
        }
    }

    /// Called after pane cursors: the palette's input cursor is the final instruction.
    pub(super) fn render_command_palettes(
        &self,
        output: &mut Output,
        clients: impl Iterator<Item = ClientId>,
    ) -> Result<()> {
        for client in clients {
            let Some(state) = self.command_palettes.get(&client) else {
                continue;
            };
            let Some(surface) = self.command_palette_surface(client) else {
                continue;
            };
            let layout = state.widget.layout(surface);
            if layout.rect.is_empty() {
                continue;
            }
            output
                .add_translucent_overlay(
                    client,
                    layout.rect,
                    75,
                    state.widget.chunks(&layout, &self.style),
                    *self.terminal_emulator_colors.borrow(),
                    self.style.colors.text_unselected.background,
                    &self.terminal_emulator_color_codes.borrow(),
                )
                .context("failed to render command palette")?;
            if let Some((x, y)) = state.widget.cursor(&layout) {
                output.add_post_vte_instruction_to_client(
                    client,
                    &format!("\u{1b}[{};{}H\u{1b}[0m\u{1b}[2 q\u{1b}[?25h", y + 1, x + 1),
                );
                output.set_cursor_coordinates(client, x, y);
            }
        }
        Ok(())
    }
}

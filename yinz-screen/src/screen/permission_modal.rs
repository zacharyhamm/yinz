//! Host permissions have no pane, focus, input-mode, or layout ownership.
use super::Screen;
use crate::output::Output;
use crate::plugins::PluginInstruction;
use crate::ui::context_menu::{
    context_menu_chunks, context_menu_item_at, context_menu_layout, context_menu_text_width,
    ContextMenuLayout,
};
use crate::ui::damage::CellRect;
use unicode_width::UnicodeWidthChar;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{ContextMenuItem, PaneId, PermissionStatus, PluginPermission};
use yinz_types::internal_ids::{ClientId, PluginId};
use yinz_types::keys::{BareKey, KeyModifier, KeyWithModifier};
use yinz_types::mouse::{MouseEvent, MouseEventType};
use yinz_types::pane_size::Viewport;
use yinz_types::position::Position;

pub(super) struct PermissionModal {
    request: PluginPermission,
    client: Option<ClientId>,
    allow: bool,
    scroll: usize,
    painted: Option<CellRect>,
    pressed: Option<usize>,
}

impl PermissionModal {
    fn items(&self, viewport: Viewport) -> Vec<ContextMenuItem> {
        let mut items = vec![
            ContextMenuItem {
                label: "Plugin permissions".into(),
                disabled: true,
                ..Default::default()
            },
            ContextMenuItem {
                label: self.request.name.clone(),
                disabled: true,
                ..Default::default()
            },
        ];
        items.extend(
            self.request
                .permissions
                .iter()
                .map(|permission| ContextMenuItem {
                    label: permission.to_string(),
                    disabled: true,
                    ..Default::default()
                }),
        );
        items.push(
            ContextMenuItem::new("Deny", vec![]).with_shortcut(if self.allow {
                "Esc"
            } else {
                "Esc / Enter"
            }),
        );
        let mut allow = ContextMenuItem::new("Allow", vec![]);
        if self.allow {
            allow = allow.with_shortcut("Enter");
        }
        items.push(allow);
        let width = context_menu_text_width(&items, viewport);
        let mut identity_rows = vec![];
        let mut label = String::new();
        let mut used = 0;
        for character in self.request.name.chars() {
            let character_width = character.width().unwrap_or(0);
            if used + character_width > width && !label.is_empty() {
                identity_rows.push(ContextMenuItem {
                    label: std::mem::take(&mut label),
                    disabled: true,
                    ..Default::default()
                });
                used = 0;
            }
            label.push(character);
            used += character_width;
        }
        identity_rows.push(ContextMenuItem {
            label,
            disabled: true,
            ..Default::default()
        });
        items.splice(1..2, identity_rows);
        items
    }
    pub(super) fn layout(&self, viewport: Viewport) -> ContextMenuLayout {
        let items = self.items(viewport);
        // The layout helper follows a row at the bottom of the requested viewport.
        let bottom = self.scroll + viewport.rows.saturating_sub(3);
        let mut layout = context_menu_layout(&items, Position::new(0, 0), viewport, Some(bottom));
        layout.x = viewport.x + viewport.cols.saturating_sub(layout.width) / 2;
        layout.y = viewport.y + viewport.rows.saturating_sub(layout.height()) / 2;
        layout
    }
    fn set_scroll(&mut self, scroll: usize, viewport: Viewport) {
        let previous = self.scroll;
        self.scroll = scroll;
        self.scroll = self.layout(viewport).window_start;
        if self.scroll != previous {
            // Invalidate the hit while retaining capture of the matching release.
            self.pressed = self.pressed.map(|_| usize::MAX);
        }
    }
    fn show_selection(&mut self, viewport: Viewport) {
        let rows = viewport.rows.saturating_sub(2);
        let selected = self.selected(viewport);
        let scroll = self
            .scroll
            .min(selected)
            .max(selected.saturating_add(1).saturating_sub(rows));
        self.set_scroll(scroll, viewport);
    }
    fn selected(&self, viewport: Viewport) -> usize {
        self.items(viewport).len() - if self.allow { 1 } else { 2 }
    }
}

impl Screen {
    pub(super) fn permission_modal_clients(&self) -> impl Iterator<Item = ClientId> + '_ {
        self.permission_modals
            .iter()
            .filter_map(|modal| modal.client)
    }
    pub(super) fn permission_modal_open(&self, client: ClientId) -> bool {
        self.permission_modals
            .iter()
            .any(|modal| modal.client == Some(client))
    }
    pub(super) fn open_permission_modal(&mut self, _plugin: PluginId, request: PluginPermission) {
        if self
            .permission_modals
            .iter()
            .any(|modal| modal.request.request_id == request.request_id)
        {
            return;
        }
        self.permission_modals.push(PermissionModal {
            request,
            client: None,
            allow: false,
            scroll: 0,
            painted: None,
            pressed: None,
        });
        self.assign_permission_modals();
    }
    pub(super) fn assign_permission_modals(&mut self) {
        let clients: Vec<_> = self
            .active_tab_ids
            .keys()
            .filter(|id| {
                self.connected_clients.borrow().contains(id)
                    && !self.watcher_clients.contains_key(id)
            })
            .copied()
            .collect();
        let fallback = self
            .permission_last_client
            .filter(|id| clients.contains(id))
            .or_else(|| clients.first().copied());
        let focused: Vec<_> = clients
            .iter()
            .map(|id| (*id, self.get_active_pane_id(id)))
            .collect();
        let mut presented = vec![];
        for modal in &mut self.permission_modals {
            if modal.client.is_some_and(|client| clients.contains(&client)) {
                continue;
            }
            modal.client = modal
                .request
                .caller
                .and_then(|caller| {
                    focused
                        .iter()
                        .find(|(_, pane)| *pane == Some(PaneId::Terminal(caller)))
                        .map(|(id, _)| *id)
                })
                .or_else(|| modal.request.client_id.filter(|id| clients.contains(id)))
                .or(fallback);
            modal.allow = false;
            modal.scroll = 0;
            modal.pressed = None;
            modal.painted = None;
            if let Some(client) = modal.client {
                presented.push(client);
            }
        }
        for client in presented {
            self.close_command_palette(client);
            self.close_context_menu(client);
        }
    }
    pub(super) fn cancel_permission_modal(&mut self, token: u64) {
        let old_len = self.permission_modals.len();
        for modal in &self.permission_modals {
            if modal.request.request_id == token
                && modal.pressed.is_some()
                && let Some(client) = modal.client
            {
                self.permission_mouse_captures.insert(client);
            }
        }
        self.permission_modals
            .retain(|modal| modal.request.request_id != token);
        if old_len != self.permission_modals.len() {
            self.force_render_under_context_menu();
        }
    }
    fn answer_permission_modal(&mut self, client: ClientId, granted: bool) {
        let Some(token) = self
            .permission_modals
            .iter()
            .find(|modal| modal.client == Some(client))
            .map(|modal| modal.request.request_id)
        else {
            return;
        };
        self.cancel_permission_modal(token);
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::PermissionAnswer {
                request_id: token,
                status: if granted {
                    PermissionStatus::Granted
                } else {
                    PermissionStatus::Denied
                },
                cache_path: None,
            })
            .context("answer plugin permission request")
            .non_fatal();
    }
    pub(super) fn permission_modal_input(
        &mut self,
        key: &Option<KeyWithModifier>,
        client: ClientId,
    ) -> bool {
        if !self.watcher_clients.contains_key(&client) {
            self.permission_last_client = Some(client);
        }
        let surface = self.command_palette_surface(client);
        let Some(modal) = self
            .permission_modals
            .iter_mut()
            .find(|modal| modal.client == Some(client))
        else {
            return false;
        };
        // None is paste or an explicit write. Text never grants permissions.
        if let Some(key) = key.as_ref().filter(|key| {
            key.key_modifiers.is_empty()
                || (key.bare_key == BareKey::Tab && key.has_only_modifiers(&[KeyModifier::Shift]))
        }) {
            let answer = match key.bare_key {
                BareKey::Esc => Some(false),
                BareKey::Enter => surface
                    .filter(|surface| {
                        modal
                            .layout(*surface)
                            .row_of_item(modal.selected(*surface))
                            .is_some()
                    })
                    .map(|_| modal.allow),
                BareKey::Tab | BareKey::Up | BareKey::Down => {
                    modal.allow = match key.bare_key {
                        BareKey::Up => false,
                        BareKey::Down => true,
                        _ => !modal.allow,
                    };
                    if let Some(surface) = surface {
                        modal.show_selection(surface);
                    }
                    None
                },
                BareKey::PageUp | BareKey::PageDown | BareKey::Home | BareKey::End => {
                    if let Some(surface) = surface {
                        let page = surface.rows.saturating_sub(2).max(1);
                        let scroll = match key.bare_key {
                            BareKey::PageUp => modal.scroll.saturating_sub(page),
                            BareKey::PageDown => modal.scroll + page,
                            BareKey::Home => 0,
                            _ => modal.items(surface).len(),
                        };
                        modal.set_scroll(scroll, surface);
                    }
                    None
                },
                _ => None,
            };
            if let Some(granted) = answer {
                self.answer_permission_modal(client, granted);
            }
            self.render().non_fatal();
        }
        true
    }
    pub(super) fn permission_modal_mouse(&mut self, event: &MouseEvent, client: ClientId) -> bool {
        if self.permission_mouse_captures.contains(&client) {
            if event.event_type == MouseEventType::Release {
                self.permission_mouse_captures.remove(&client);
            }
            return true;
        }
        if !self.watcher_clients.contains_key(&client) {
            self.permission_last_client = Some(client);
        }
        let surface = self.command_palette_surface(client);
        let Some(modal) = self
            .permission_modals
            .iter_mut()
            .find(|modal| modal.client == Some(client))
        else {
            return false;
        };
        if event.wheel_up || event.wheel_down {
            if let Some(surface) = surface {
                let scroll = if event.wheel_up {
                    modal.scroll.saturating_sub(1)
                } else {
                    modal.scroll + 1
                };
                modal.set_scroll(scroll, surface);
            }
            self.render().non_fatal();
            return true;
        }
        let hit = surface
            .and_then(|surface| context_menu_item_at(&modal.layout(surface), event.position));
        if event.event_type == MouseEventType::Press {
            // Retain capture even outside the controls if a keyboard answer closes the modal.
            modal.pressed = Some(if event.left {
                hit.unwrap_or(usize::MAX)
            } else {
                usize::MAX
            });
        }
        if event.event_type == MouseEventType::Release {
            let pressed = modal.pressed.take();
            if let (Some(hit), Some(surface)) = (hit, surface)
                && event.left
                && Some(hit) == pressed
            {
                let item_count = modal.items(surface).len();
                let allow = hit == item_count - 1;
                if allow || hit == item_count - 2 {
                    self.answer_permission_modal(client, allow);
                    self.render().non_fatal();
                }
            }
        }
        true
    }
    pub(super) fn prepare_permission_modal_render(&mut self) {
        let clients: Vec<_> = self.permission_modal_clients().collect();
        for client in clients {
            let Some(surface) = self.command_palette_surface(client) else {
                continue;
            };
            let Some(modal) = self
                .permission_modals
                .iter_mut()
                .find(|modal| modal.client == Some(client))
            else {
                continue;
            };
            modal.set_scroll(modal.scroll, surface);
            let layout = modal.layout(surface);
            let rect = CellRect {
                x: layout.x,
                y: layout.y,
                cols: layout.width,
                rows: layout.height(),
            };
            if modal.painted.replace(rect).is_some_and(|old| old != rect) {
                modal.pressed = modal.pressed.map(|_| usize::MAX);
                self.force_render_under_context_menu();
            }
        }
    }
    pub(super) fn render_permission_modals(&self, output: &mut Output) -> Result<()> {
        let previous_stack = output.set_floating_panes_stack(None);
        let result = (|| -> Result<()> {
            let mut clients = std::collections::HashSet::new();
            for modal in &self.permission_modals {
                let Some(client) = modal.client.filter(|client| clients.insert(*client)) else {
                    continue;
                };
                let Some(surface) = self.command_palette_surface(client) else {
                    continue;
                };
                let layout = modal.layout(surface);
                output.add_character_chunks_to_client(
                    client,
                    context_menu_chunks(&layout, Some(modal.selected(surface)), &self.style),
                    None,
                )?;
                output.add_post_vte_instruction_to_client(client, "\u{1b}[?25l");
            }
            Ok(())
        })();
        output.set_floating_panes_stack(previous_stack);
        result
    }
}

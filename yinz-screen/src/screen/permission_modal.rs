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
    // The outer option retains ownership of the release after its hit is invalidated.
    pressed: Option<Option<(usize, HitGeometry)>>,
}

#[derive(PartialEq, Eq)]
struct HitGeometry {
    origin: (usize, usize),
    x: usize,
    y: usize,
    width: usize,
    window_start: usize,
    rows: Vec<Option<usize>>,
}

impl HitGeometry {
    fn new(layout: &ContextMenuLayout, origin: (usize, usize)) -> Self {
        Self {
            origin,
            x: layout.x,
            y: layout.y,
            width: layout.width,
            window_start: layout.window_start,
            rows: layout
                .rows
                .iter()
                .map(|row| (!row.disabled && !row.separator).then_some(row.item_index))
                .collect(),
        }
    }
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
        self.layout_with_selection(viewport).0
    }

    fn layout_with_selection(&self, viewport: Viewport) -> (ContextMenuLayout, usize) {
        let items = self.items(viewport);
        let selected = items.len() - if self.allow { 1 } else { 2 };
        // The layout helper follows a row at the bottom of the requested viewport.
        let bottom = self.scroll + viewport.rows.saturating_sub(3);
        let mut layout = context_menu_layout(&items, Position::new(0, 0), viewport, Some(bottom));
        layout.x = viewport.x + viewport.cols.saturating_sub(layout.width) / 2;
        layout.y = viewport.y + viewport.rows.saturating_sub(layout.height()) / 2;
        (layout, selected)
    }
    fn revalidate_press(&mut self, surface: Viewport, origin: (usize, usize)) {
        if let Some(Some((_, previous))) = &self.pressed
            && *previous != HitGeometry::new(&self.layout(surface), origin)
        {
            self.pressed = Some(None);
        }
    }

    fn set_scroll(&mut self, scroll: usize, viewport: Viewport) {
        let previous = self.scroll;
        self.scroll = scroll;
        self.scroll = self.layout(viewport).window_start;
        if self.scroll != previous {
            // Invalidate the hit while retaining capture of the matching release.
            self.pressed = self.pressed.as_ref().map(|_| None);
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
    pub(super) fn open_permission_modal(&mut self, _plugin: PluginId, request: PluginPermission) {
        if self.permissions.open(request) {
            self.assign_permission_modals();
        }
    }
    pub(super) fn assign_permission_modals(&mut self) {
        if self.permissions.permission_modals.is_empty() {
            return;
        }
        let clients: Vec<_> = self
            .core
            .active_tab_ids
            .keys()
            .filter(|id| {
                self.core.connected_clients.borrow().contains(id)
                    && !self.core.watcher_clients.contains_key(id)
            })
            .copied()
            .collect();
        let focused: Vec<_> = clients
            .iter()
            .map(|id| (*id, self.get_active_pane_id(id)))
            .collect();
        let presented = self.permissions.assign(&clients, &focused);
        for client in presented {
            self.close_command_palette(client);
            // The modal owns the interrupted gesture, even if it closes before release.
            if self.palette.command_palette_mouse_captures.remove(&client) {
                self.permissions.permission_mouse_captures.insert(client);
            }
            self.close_context_menu(client);
        }
    }
    pub(super) fn cancel_permission_modal(&mut self, token: u64) {
        if self.permissions.cancel(token) {
            self.force_render_under_context_menu();
        }
    }
    fn answer_permission_modal(&mut self, client: ClientId, granted: bool) {
        let Some(token) = self
            .permissions
            .permission_modals
            .iter()
            .find(|modal| modal.client == Some(client))
            .map(|modal| modal.request.request_id)
        else {
            return;
        };
        self.cancel_permission_modal(token);
        self.core
            .bus
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
    fn permission_surface(&self, client: ClientId) -> Option<(Viewport, (usize, usize))> {
        let surface = self.command_palette_surface(client)?;
        let origin = self
            .core
            .client_sizes
            .get(&client)
            .map_or((0, 0), |terminal| {
                crate::output::Presentation::centered_offset(
                    *terminal,
                    yinz_types::pane_size::Size {
                        cols: surface.cols,
                        rows: surface.rows,
                    },
                )
            });
        Some((surface, origin))
    }

    pub(super) fn permission_modal_input(
        &mut self,
        key: &Option<KeyWithModifier>,
        client: ClientId,
    ) -> bool {
        let surface = self.permission_surface(client);
        let interactive = !self.core.watcher_clients.contains_key(&client);
        let action = self.permissions.key(client, key, interactive, surface);
        self.apply_permission_input(client, action)
    }
    pub(super) fn permission_modal_mouse(&mut self, event: &MouseEvent, client: ClientId) -> bool {
        // Mouse coordinates were translated through the last delivered presentation.
        let origin = self
            .core
            .client_presentations
            .get(&client)
            .map_or((0, 0), |(_, presentation)| presentation.offset());
        let surface = self
            .command_palette_surface(client)
            .map(|surface| (surface, origin));
        let interactive = !self.core.watcher_clients.contains_key(&client);
        let action = self.permissions.mouse(client, event, interactive, surface);
        self.apply_permission_input(client, action)
    }

    fn apply_permission_input(&mut self, client: ClientId, action: ModalInput) -> bool {
        match action {
            ModalInput::Ignored => return false,
            ModalInput::Consumed => {},
            ModalInput::Redraw => {
                self.render().non_fatal();
            },
            ModalInput::Answer(granted) => {
                self.answer_permission_modal(client, granted);
                self.render().non_fatal();
            },
        }
        true
    }
    pub(super) fn permission_surfaces_changed(&mut self) {
        let clients: Vec<_> = self.permissions.permission_modal_clients().collect();
        for client in clients {
            let surface = self.permission_surface(client);
            self.permissions.surface_changed(client, surface);
        }
    }
    pub(super) fn prepare_permission_modal_render(
        &mut self,
    ) -> Vec<(ClientId, ContextMenuLayout, usize)> {
        let clients: std::collections::BTreeSet<_> =
            self.permissions.permission_modal_clients().collect();
        let mut frames = Vec::with_capacity(clients.len());
        for client in clients {
            let Some(surface) = self.command_palette_surface(client) else {
                continue;
            };
            let Some(modal) = self
                .permissions
                .permission_modals
                .iter_mut()
                .find(|modal| modal.client == Some(client))
            else {
                continue;
            };
            let (layout, selected) = modal.layout_with_selection(surface);
            let rect = CellRect {
                x: layout.x,
                y: layout.y,
                cols: layout.width,
                rows: layout.height(),
            };
            if modal.painted.replace(rect).is_some_and(|old| old != rect) {
                self.force_render_under_context_menu();
            }
            frames.push((client, layout, selected));
        }
        frames
    }
    pub(super) fn render_permission_modals(
        &self,
        output: &mut Output,
        frames: Vec<(ClientId, ContextMenuLayout, usize)>,
    ) -> Result<()> {
        let previous_stack = output.set_floating_panes_stack(None);
        let result = (|| -> Result<()> {
            for (client, layout, selected) in frames {
                output.add_character_chunks_to_client(
                    client,
                    context_menu_chunks(&layout, Some(selected), &self.core.style),
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

impl crate::screen::PermissionModalController {
    pub(in crate::screen) fn permission_modal_clients(
        &self,
    ) -> impl Iterator<Item = ClientId> + '_ {
        self.permission_modals
            .iter()
            .filter_map(|modal| modal.client)
    }
    pub(in crate::screen) fn permission_modal_open(&self, client: ClientId) -> bool {
        self.permission_modals
            .iter()
            .any(|modal| modal.client == Some(client))
    }
}

impl super::PermissionModalController {
    pub(super) fn presentation_changed(&mut self, client: ClientId, origin: (usize, usize)) {
        for modal in self
            .permission_modals
            .iter_mut()
            .filter(|m| m.client == Some(client))
        {
            if let Some(Some((_, geometry))) = &modal.pressed
                && geometry.origin != origin
            {
                modal.pressed = Some(None);
            }
        }
    }

    pub(super) fn on_client_removed(&mut self, client_id: ClientId) {
        self.permission_mouse_captures.remove(&client_id);
    }
}

impl super::PermissionModalController {
    fn assign(
        &mut self,
        clients: &[ClientId],
        focused: &[(ClientId, Option<PaneId>)],
    ) -> Vec<ClientId> {
        let fallback = self
            .permission_last_client
            .filter(|id| clients.contains(id))
            .or_else(|| clients.first().copied());
        let mut presented = vec![];
        for modal in &mut self.permission_modals {
            if modal.client.is_some_and(|client| clients.contains(&client)) {
                continue;
            }
            if modal.pressed.take().is_some()
                && let Some(client) = modal.client
            {
                self.permission_mouse_captures.insert(client);
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
        presented
    }
}

impl super::PermissionModalController {
    fn surface_changed(&mut self, client: ClientId, surface: Option<(Viewport, (usize, usize))>) {
        for modal in self
            .permission_modals
            .iter_mut()
            .filter(|modal| modal.client == Some(client))
        {
            if let Some((surface, origin)) = surface {
                modal.set_scroll(modal.scroll, surface);
                modal.revalidate_press(surface, origin);
            } else if modal.pressed.is_some() {
                modal.pressed = Some(None);
            }
        }
    }
}

enum ModalInput {
    Ignored,
    Consumed,
    Redraw,
    Answer(bool),
}

impl super::PermissionModalController {
    fn open(&mut self, request: PluginPermission) -> bool {
        if self
            .permission_modals
            .iter()
            .any(|modal| modal.request.request_id == request.request_id)
        {
            return false;
        }
        self.permission_modals.push(PermissionModal {
            request,
            client: None,
            allow: false,
            scroll: 0,
            painted: None,
            pressed: None,
        });
        true
    }

    fn cancel(&mut self, token: u64) -> bool {
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
        old_len != self.permission_modals.len()
    }

    fn key(
        &mut self,
        client: ClientId,
        key: &Option<KeyWithModifier>,
        interactive: bool,
        surface: Option<(Viewport, (usize, usize))>,
    ) -> ModalInput {
        if interactive {
            self.permission_last_client = Some(client);
        }
        let Some(modal) = self
            .permission_modals
            .iter_mut()
            .find(|modal| modal.client == Some(client))
        else {
            return ModalInput::Ignored;
        };
        // None is paste or an explicit write. Text never grants permissions.
        if let Some(key) = key.as_ref().filter(|key| {
            key.key_modifiers.is_empty()
                || (key.bare_key == BareKey::Tab && key.has_only_modifiers(&[KeyModifier::Shift]))
        }) {
            let answer = match key.bare_key {
                BareKey::Esc => Some(false),
                BareKey::Enter => surface
                    .filter(|(surface, _)| {
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
                    if let Some((surface, origin)) = surface {
                        modal.show_selection(surface);
                        modal.revalidate_press(surface, origin);
                    }
                    None
                },
                BareKey::PageUp | BareKey::PageDown | BareKey::Home | BareKey::End => {
                    if let Some((surface, _)) = surface {
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
            return answer.map_or(ModalInput::Redraw, ModalInput::Answer);
        }
        ModalInput::Consumed
    }

    fn mouse(
        &mut self,
        client: ClientId,
        event: &MouseEvent,
        interactive: bool,
        surface: Option<(Viewport, (usize, usize))>,
    ) -> ModalInput {
        if self.permission_mouse_captures.contains(&client) {
            if event.event_type == MouseEventType::Release {
                self.permission_mouse_captures.remove(&client);
            }
            return ModalInput::Consumed;
        }
        if interactive {
            self.permission_last_client = Some(client);
        }
        let Some(modal) = self
            .permission_modals
            .iter_mut()
            .find(|modal| modal.client == Some(client))
        else {
            return ModalInput::Ignored;
        };
        if event.wheel_up || event.wheel_down {
            if let Some((surface, _)) = surface {
                let scroll = if event.wheel_up {
                    modal.scroll.saturating_sub(1)
                } else {
                    modal.scroll + 1
                };
                modal.set_scroll(scroll, surface);
            }
            return ModalInput::Redraw;
        }
        let origin = surface.map_or((0, 0), |(_, origin)| origin);
        let layout = surface.map(|(surface, _)| modal.layout(surface));
        let hit = layout
            .as_ref()
            .and_then(|layout| context_menu_item_at(layout, event.position));
        if event.event_type == MouseEventType::Press {
            // Retain capture even outside the controls if a keyboard answer closes the modal.
            modal.pressed = Some(
                hit.zip(layout.as_ref())
                    .filter(|_| event.left)
                    .map(|(hit, layout)| (hit, HitGeometry::new(layout, origin))),
            );
        }
        if event.event_type == MouseEventType::Release {
            let pressed = modal.pressed.take();
            if let (Some(hit), Some((surface, origin)), Some(layout)) =
                (hit, surface, layout.as_ref())
                && event.left
                && pressed.flatten().is_some_and(|(index, pressed)| {
                    index == hit && pressed == HitGeometry::new(layout, origin)
                })
            {
                let item_count = modal.items(surface).len();
                let allow = hit == item_count - 1;
                if allow || hit == item_count - 2 {
                    return ModalInput::Answer(allow);
                }
            }
        }
        ModalInput::Consumed
    }
}

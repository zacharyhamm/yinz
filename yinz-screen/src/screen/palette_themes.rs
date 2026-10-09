use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use super::{Builtin, EntryKind, Screen};
use crate::ClientId;
use yinz_bus::ServerInstruction;
use yinz_runtime::errors::prelude::*;
use yinz_types::style::{HostTerminalThemeMode, Styling};

pub(crate) struct Preview {
    pub(crate) client: ClientId,
    original: Styling,
    saving: Option<uuid::Uuid>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl Screen {
    pub(super) fn open_theme_picker(&mut self, client: ClientId) {
        if self.theme_preview.is_some() {
            if let Some(state) = self.command_palettes.get_mut(&client) {
                state
                    .widget
                    .set_error("Theme picker is open in another client".into());
            }
            return;
        }
        let configured =
            if self.host_theme_dark_styling.is_some() && self.host_theme_light_styling.is_some() {
                match self.host_terminal_theme_mode {
                    Some(HostTerminalThemeMode::Light) => self.theme_light_name.as_deref(),
                    _ => self.theme_dark_name.as_deref(),
                }
            } else {
                self.theme_name.as_deref().or(Some("default"))
            };
        let active = configured
            .filter(|name| self.themes.get(*name) == Some(&self.style.colors))
            .or_else(|| {
                self.themes
                    .iter()
                    .find(|(_, theme)| **theme == self.style.colors)
                    .map(|(name, _)| name.as_str())
            })
            .map(|name| format!("theme:{name}"));
        if let Some(state) = self.command_palettes.get_mut(&client) {
            state.widget.open_picker(EntryKind::Theme);
            if let Some(active) = active {
                state.widget.select_id(&active);
            }
            self.theme_preview = Some(Preview {
                client,
                original: self.style.colors,
                saving: None,
                cancelled: Arc::new(AtomicBool::new(false)),
            });
            self.refresh_plugin_palette(client, false);
            self.refresh_theme_preview(client);
        }
    }

    pub(super) fn cancel_theme_preview(&mut self, client: ClientId) {
        if self
            .theme_preview
            .as_ref()
            .is_none_or(|preview| preview.client != client)
        {
            return;
        }
        if let Some(preview) = self.theme_preview.take() {
            self.repaint_theme(preview.original).non_fatal();
            if let Some(mode) = self.host_terminal_theme_mode {
                self.repaint_theme_mode(mode).non_fatal();
            }
        }
    }

    pub(crate) fn refresh_theme_preview(&mut self, client: ClientId) {
        let Some(preview) = self
            .theme_preview
            .as_ref()
            .filter(|preview| preview.client == client)
        else {
            return;
        };
        let Some(state) = self
            .command_palettes
            .get(&client)
            .filter(|state| state.widget.theme_picker())
        else {
            self.cancel_theme_preview(client);
            return;
        };
        let selected = state
            .widget
            .selected_entry()
            .and_then(|entry| state.actions.get(&entry.id));
        let theme = match selected {
            Some(Builtin::Theme(name)) => self.themes.get(name).copied(),
            _ => None,
        };
        let normal =
            if self.host_theme_dark_styling.is_some() && self.host_theme_light_styling.is_some() {
                match self.host_terminal_theme_mode {
                    Some(HostTerminalThemeMode::Dark) => self.host_theme_dark_styling,
                    Some(HostTerminalThemeMode::Light) => self.host_theme_light_styling,
                    None => None,
                }
            } else {
                None
            };
        let theme = theme.or(normal).unwrap_or(preview.original);
        if self.style.colors != theme {
            self.repaint_theme(theme).non_fatal();
        }
    }

    pub(super) fn save_palette_theme(&mut self, client: ClientId, name: String) {
        let Some(preview) = self
            .theme_preview
            .as_mut()
            .filter(|preview| preview.client == client && preview.saving.is_none())
        else {
            return;
        };
        let request = uuid::Uuid::new_v4();
        preview.saving = Some(request);
        if let Some(state) = self.command_palettes.get_mut(&client) {
            state.widget.busy = true;
        }
        if let Err(error) = self
            .bus
            .senders
            .send_to_server(ServerInstruction::SavePaletteTheme {
                client,
                request,
                name,
                cancelled: preview.cancelled.clone(),
            })
        {
            self.palette_theme_saved(client, request, Err(error.to_string()));
        }
    }

    pub(crate) fn palette_theme_saved(
        &mut self,
        client: ClientId,
        request: uuid::Uuid,
        result: Result<(), String>,
    ) {
        if self
            .theme_preview
            .as_ref()
            .is_none_or(|preview| preview.client != client || preview.saving != Some(request))
        {
            return;
        }
        match result {
            Ok(()) => {
                self.theme_preview = None;
                self.close_command_palette(client);
            },
            Err(error) => {
                if let Some(preview) = self.theme_preview.as_mut() {
                    preview.saving = None;
                }
                if let Some(state) = self.command_palettes.get_mut(&client) {
                    state.widget.busy = false;
                    state.widget.set_error(error);
                }
            },
        }
    }
}

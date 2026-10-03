use std::collections::HashMap;
use yinz_bus::screen::ScreenInstruction;
use yinz_bus::ServerInstruction;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{InputMode, KeybindsVec, NewPanePlacement, PaneId};
use yinz_types::geometry::Direction;
use yinz_types::input::actions::Action;
use yinz_types::input::command::{RunCommandAction, TerminalAction};
use yinz_types::input::keybinds::shortcut_for_action;
use yinz_types::internal_ids::{SessionName, TabId, TabNumber};
use yinz_types::keys::KeyWithModifier;
use yinz_types::mouse::{MouseEvent, MouseEventType};
use yinz_types::pane_size::Viewport;

use super::Screen;
use crate::output::Output;
use crate::ui::command_palette::{CommandPalette, Entry, EntryKind, Field, Form, Outcome};
use crate::ui::damage::CellRect;
use crate::ClientId;

pub(super) struct CommandPaletteState {
    widget: CommandPalette,
    tab_id: TabId,
    pane_id: PaneId,
    actions: HashMap<String, Builtin>,
    painted: Option<CellRect>,
    shortcut_keybinds: KeybindsVec,
    shortcuts: HashMap<String, CachedShortcut>,
    #[cfg(test)]
    pub(super) shortcut_lookups: usize,
    #[cfg(test)]
    pub(super) shortcut_keymap_filters: usize,
}

struct CachedShortcut {
    original: Option<Action>,
    target: Option<Action>,
    hint: String,
}

impl Screen {
    pub(crate) fn toggle_command_palette(&mut self, client_id: ClientId) {
        if self.close_command_palette(client_id) {
            return;
        }
        let Some(tab_id) = self.active_tab_ids.get(&client_id).copied() else {
            return;
        };
        let Some(pane_id) = self.get_active_pane_id(&client_id) else {
            return;
        };
        self.close_context_menu(client_id);
        self.bus
            .senders
            .send_to_server(ServerInstruction::SetCommandPaletteOpen(client_id, true))
            .context("failed to lock input when opening command palette")
            .non_fatal();
        self.command_palettes.insert(
            client_id,
            CommandPaletteState {
                widget: CommandPalette::default(),
                tab_id,
                pane_id,
                actions: HashMap::new(),
                painted: None,
                shortcut_keybinds: Vec::new(),
                shortcuts: HashMap::new(),
                #[cfg(test)]
                shortcut_lookups: 0,
                #[cfg(test)]
                shortcut_keymap_filters: 0,
            },
        );
        self.refresh_command_palettes();
        if self.command_palettes.len() == 1 {
            self.bus
                .senders
                .send_to_background_jobs(
                    yinz_bus::background_jobs::BackgroundJob::WatchCommandPaletteSessions(true),
                )
                .non_fatal();
        }
    }

    pub(crate) fn close_command_palette(&mut self, client_id: ClientId) -> bool {
        if self.command_palettes.remove(&client_id).is_none() {
            return false;
        }
        self.bus
            .senders
            .send_to_server(ServerInstruction::SetCommandPaletteOpen(client_id, false))
            .context("failed to restore input mode when closing command palette")
            .non_fatal();
        if self.command_palettes.is_empty() {
            self.bus
                .senders
                .send_to_background_jobs(
                    yinz_bus::background_jobs::BackgroundJob::WatchCommandPaletteSessions(false),
                )
                .non_fatal();
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
        let outcome = if toggle {
            Outcome::Dismiss
        } else if let Some(key) = key {
            state.widget.handle_key(key.clone())
        } else {
            // Explicit writes can still include bracketed-paste markers.
            if bytes != b"\x1b[200~" && bytes != b"\x1b[201~" {
                state.widget.insert_text(&String::from_utf8_lossy(bytes));
            }
            Outcome::None
        };
        self.handle_palette_outcome(client_id, outcome);
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
        self.refresh_command_palettes();
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
            } else {
                output.add_post_vte_instruction_to_client(client, "\u{1b}[?25l");
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
enum Builtin {
    Action(Action),
    Picker(EntryKind),
    Form(FormKind),
}

#[derive(Clone, Copy)]
enum FormKind {
    NewPane,
    NewTab,
    NewSession,
    RenamePane,
    RenameTab,
    RenameSession,
    ClosePane,
    CloseTab,
}

impl Screen {
    fn palette_entries(
        &self,
        pane_id: PaneId,
        tab_id: TabId,
        client: ClientId,
        state: &mut CommandPaletteState,
    ) -> Vec<(Entry, Builtin)> {
        use Action::*;
        let mut entries = Vec::new();
        let mut command = |id: &str, label: &str, aliases: &[&str], activation: Builtin| {
            entries.push((
                Entry {
                    id: id.into(),
                    label: label.into(),
                    shortcut: String::new(),
                    description: String::new(),
                    aliases: aliases.iter().map(|s| (*s).into()).collect(),
                    kind: EntryKind::Command,
                },
                activation,
            ));
        };
        command(
            "new-pane",
            "New pane",
            &["create terminal"],
            Builtin::Action(new_pane(
                None,
                None,
                None,
                self.tabs
                    .get(&tab_id)
                    .is_some_and(|tab| tab.are_floating_panes_visible()),
            )),
        );
        command(
            "new-tab",
            "New tab",
            &["create tab"],
            Builtin::Action(new_tab(None, None)),
        );
        command(
            "focus-pane",
            "Focus pane…",
            &["find pane"],
            Builtin::Picker(EntryKind::Pane),
        );
        command(
            "switch-tab",
            "Switch tab…",
            &["find tab"],
            Builtin::Picker(EntryKind::Tab),
        );
        command(
            "switch-session",
            "Switch session…",
            &["find session"],
            Builtin::Picker(EntryKind::Session),
        );
        command(
            "floating",
            "New floating pane",
            &["create floating terminal"],
            Builtin::Action(new_pane(None, None, None, true)),
        );
        command(
            "split-right",
            "Split right",
            &["vertical split"],
            Builtin::Action(new_pane(Some(Direction::Right), None, None, false)),
        );
        command(
            "split-down",
            "Split down",
            &["horizontal split"],
            Builtin::Action(new_pane(Some(Direction::Down), None, None, false)),
        );
        for (id, label, form) in [
            ("new-pane-form", "New pane…", FormKind::NewPane),
            ("new-tab-form", "New tab…", FormKind::NewTab),
            ("new-session", "New session…", FormKind::NewSession),
            ("rename-pane", "Rename pane…", FormKind::RenamePane),
            ("rename-tab", "Rename tab…", FormKind::RenameTab),
            ("rename-session", "Rename session…", FormKind::RenameSession),
            ("close-pane", "Close pane…", FormKind::ClosePane),
            ("close-tab", "Close tab…", FormKind::CloseTab),
        ] {
            command(id, label, &[], Builtin::Form(form));
        }
        for (id, label, action) in [
            (
                "fullscreen",
                "Fullscreen/restore pane",
                ToggleFocusFullscreenByPaneId { pane_id },
            ),
            (
                "float",
                "Float/embed pane",
                TogglePaneEmbedOrFloatingByPaneId { pane_id },
            ),
            (
                "floating-visible",
                "Show/hide floating panes",
                ToggleFloatingPanesByTabId { id: tab_id },
            ),
            ("move-pane-new", "Move pane to new tab", BreakPane),
            (
                "move-pane-previous",
                "Move pane to previous tab",
                BreakPaneLeft,
            ),
            ("move-pane-next", "Move pane to next tab", BreakPaneRight),
            (
                "move-tab-left",
                "Move tab left",
                MoveTabByTabId {
                    id: tab_id,
                    direction: Direction::Left,
                },
            ),
            (
                "move-tab-right",
                "Move tab right",
                MoveTabByTabId {
                    id: tab_id,
                    direction: Direction::Right,
                },
            ),
            ("save-session", "Save session", SaveSession),
            ("detach", "Detach", Detach),
        ] {
            command(id, label, &[], Builtin::Action(action));
        }
        let mut tabs: Vec<_> = self.tabs.values().collect();
        tabs.sort_by_key(|tab| tab.position);
        for tab in tabs {
            let panes = tab.pane_infos();
            let tab_label = format!("{} (tab {})", tab.name, tab.position.get() + 1);
            entries.push((
                Entry {
                    id: format!("tab:{}", tab.id),
                    label: tab_label.clone(),
                    shortcut: String::new(),
                    description: format!("{} panes", panes.len()),
                    aliases: vec![tab.name.clone()],
                    kind: EntryKind::Tab,
                },
                Builtin::Action(GoToTabById { id: tab.id }),
            ));
            for pane in panes
                .into_iter()
                .filter(|pane| !pane.is_suppressed && pane.is_selectable)
            {
                entries.push((
                    Entry {
                        id: format!("pane:{}", pane.id),
                        label: format!("{} ({})", pane.title, pane.id),
                        shortcut: String::new(),
                        description: tab_label.clone(),
                        aliases: vec![pane.title],
                        kind: EntryKind::Pane,
                    },
                    Builtin::Action(FocusPaneByPaneId { pane_id: pane.id }),
                ));
            }
        }
        let mut sessions: Vec<_> = self.peer_sessions_cache.values().collect();
        sessions.sort_by_key(|session| (session.creation_time, &session.name));
        for session in sessions {
            let seconds = session.creation_time.as_secs();
            let age = std::time::Duration::from_secs(if seconds >= 60 {
                seconds / 60 * 60
            } else {
                seconds
            });
            entries.push((
                Entry {
                    id: format!("session:{}", session.name),
                    label: session.name.to_string(),
                    shortcut: String::new(),
                    description: format!(
                        "up {} · {} connected client{}",
                        humantime::format_duration(age),
                        session.connected_clients,
                        if session.connected_clients == 1 {
                            ""
                        } else {
                            "s"
                        },
                    ),
                    aliases: vec![],
                    kind: EntryKind::Session,
                },
                Builtin::Action(switch_session(session.name.clone(), None)),
            ));
        }
        let mut exited: Vec<_> = self
            .resurrectable_sessions_cache
            .iter()
            .filter(|(name, _)| !self.peer_sessions_cache.contains_key(*name))
            .collect();
        exited.sort_by_key(|(name, age)| (*age, *name));
        for (name, _) in exited {
            entries.push((
                Entry {
                    id: format!("session:{name}"),
                    label: name.to_string(),
                    shortcut: String::new(),
                    description: "Exited · Enter to restore".into(),
                    aliases: vec![],
                    kind: EntryKind::ExitedSession,
                },
                Builtin::Action(switch_session(name.clone(), None)),
            ));
        }
        let mode_info = self
            .mode_info
            .get(&client)
            .unwrap_or(&self.default_mode_info);
        if state.shortcut_keybinds != mode_info.keybinds {
            state.shortcuts.clear();
            state.shortcut_keybinds.clone_from(&mode_info.keybinds);
        }
        let mut regular_keybinds = None;
        for (entry, builtin) in &mut entries {
            let builtin = &*builtin;
            let target = match builtin {
                Builtin::Action(ToggleFocusFullscreenByPaneId { .. }) => {
                    Some(ToggleFocusFullscreen)
                },
                Builtin::Action(TogglePaneEmbedOrFloatingByPaneId { .. }) => {
                    Some(TogglePaneEmbedOrFloating)
                },
                Builtin::Action(ToggleFloatingPanesByTabId { .. }) => Some(ToggleFloatingPanes),
                Builtin::Action(MoveTabByTabId { direction, .. }) => Some(MoveTab {
                    direction: *direction,
                }),
                Builtin::Action(GoToTabById { id }) => self
                    .tabs
                    .get(id)
                    .and_then(|tab| TabNumber::try_from(tab.position).ok())
                    .map(|index| GoToTab { index }),
                Builtin::Action(NewTiledPane {
                    direction,
                    command: None,
                    pane_name,
                    ..
                }) => Some(NewPane {
                    direction: *direction,
                    pane_name: pane_name.clone(),
                    start_suppressed: false,
                }),
                Builtin::Action(NewFloatingPane {
                    command: None,
                    pane_name,
                    ..
                }) if self
                    .tabs
                    .get(&tab_id)
                    .is_some_and(|tab| tab.are_floating_panes_visible()) =>
                {
                    Some(NewPane {
                        direction: None,
                        pane_name: pane_name.clone(),
                        start_suppressed: false,
                    })
                },
                Builtin::Action(action) => Some(action.clone()),
                Builtin::Form(FormKind::ClosePane) => Some(CloseFocus),
                Builtin::Form(FormKind::CloseTab) => Some(CloseTab),
                Builtin::Form(FormKind::RenamePane) => Some(SwitchToMode {
                    input_mode: InputMode::RenamePane,
                }),
                Builtin::Form(FormKind::RenameTab) => Some(SwitchToMode {
                    input_mode: InputMode::RenameTab,
                }),
                _ => None,
            };
            let original = match builtin {
                Builtin::Action(action) => Some(action.clone()),
                _ => None,
            };
            if let Some(cached) = state.shortcuts.get(&entry.id) {
                if cached.original == original && cached.target == target {
                    entry.shortcut.clone_from(&cached.hint);
                    continue;
                }
            }
            #[cfg(test)]
            {
                state.shortcut_lookups += 1;
            }
            // Prefer regular modes even when a Tmux route is shorter. Cache hits never
            // build this keymap, and all misses in this refresh share it.
            let regular_keybinds = regular_keybinds.get_or_insert_with(|| {
                #[cfg(test)]
                {
                    state.shortcut_keymap_filters += 1;
                }
                mode_info
                    .keybinds
                    .iter()
                    .filter(|(mode, _)| *mode != InputMode::Tmux)
                    .cloned()
                    .collect()
            });
            let is_target = |action: &Action| {
                target.as_ref() == Some(action) || original.as_ref() == Some(action)
            };
            entry.shortcut = shortcut_for_action(regular_keybinds, InputMode::Locked, is_target)
                .or_else(|| shortcut_for_action(&mode_info.keybinds, InputMode::Locked, is_target))
                .map(|keys| {
                    keys.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" → ")
                })
                .unwrap_or_default();
            state.shortcuts.insert(
                entry.id.clone(),
                CachedShortcut {
                    original,
                    target,
                    hint: entry.shortcut.clone(),
                },
            );
        }
        entries
    }

    fn refresh_command_palettes(&mut self) {
        let clients: Vec<_> = self
            .command_palettes
            .iter()
            .map(|(client, state)| (*client, state.pane_id, state.tab_id))
            .collect();
        for (client, pane, tab) in clients {
            if let Some(mut state) = self.command_palettes.remove(&client) {
                let entries = self.palette_entries(pane, tab, client, &mut state);
                state
                    .widget
                    .set_entries(entries.iter().map(|(entry, _)| entry.clone()).collect());
                state.actions = entries
                    .into_iter()
                    .map(|(entry, action)| (entry.id, action))
                    .collect();
                state
                    .shortcuts
                    .retain(|id, _| state.actions.contains_key(id));
                self.command_palettes.insert(client, state);
            }
        }
    }

    fn palette_target_exists(&self, pane: PaneId, tab: TabId, action: &Action) -> bool {
        if !self.tabs.contains_key(&tab) || self.tab_containing_pane(pane).is_none() {
            return false;
        }
        match action {
            Action::FocusPaneByPaneId { pane_id } => self.tab_containing_pane(*pane_id).is_some(),
            Action::GoToTabById { id } => self.tabs.contains_key(id),
            Action::SwitchSession { name, .. } => {
                if self.peer_sessions_cache.contains_key(name) {
                    *name == self.session_name
                        || yinz_runtime::consts::YINZ_SOCK_DIR.join(name).exists()
                } else {
                    self.resurrectable_sessions_cache.contains_key(name)
                        && yinz_runtime::session_checkpoint::SessionCheckpoint::read(
                            &yinz_runtime::consts::YINZ_SESSION_INFO_CACHE_DIR.join(name),
                        )
                        .is_ok_and(|checkpoint| checkpoint.layout_path().is_file())
                }
            },
            _ => true,
        }
    }

    fn validate_palette_split(
        &self,
        pane: PaneId,
        tab: TabId,
        action: &Action,
    ) -> std::result::Result<(), String> {
        if matches!(
            action,
            Action::NewTiledPane {
                direction: Some(_),
                ..
            }
        ) && !self.tabs.get(&tab).is_some_and(|tab| {
            !tab.are_floating_panes_visible() && tab.get_tiled_pane_ids().contains(&pane)
        }) {
            return Err(
                "Directional splits require a tiled pane with floating panes hidden.".into(),
            );
        }
        Ok(())
    }

    fn handle_palette_outcome(&mut self, client: ClientId, outcome: Outcome) {
        if outcome == Outcome::Dismiss {
            self.close_command_palette(client);
            return;
        }
        let Some(state) = self.command_palettes.get(&client) else {
            return;
        };
        let pane = state.pane_id;
        let tab = state.tab_id;
        let (id, values) = match outcome {
            Outcome::Activate(id) => (id, None),
            Outcome::Submit { id, values } => (id, Some(values)),
            _ => return,
        };
        let Some(builtin) = state.actions.get(&id).cloned() else {
            if let Some(state) = self.command_palettes.get_mut(&client) {
                state
                    .widget
                    .set_error("Destination no longer exists".into());
            }
            return;
        };
        let create_session = matches!(builtin, Builtin::Form(FormKind::NewSession));
        let result = match builtin {
            Builtin::Picker(kind) => {
                if let Some(state) = self.command_palettes.get_mut(&client) {
                    state.widget.open_picker(kind);
                }
                return;
            },
            Builtin::Form(kind) => {
                if let Some(values) = values {
                    self.palette_form_action(kind, pane, tab, values)
                } else {
                    match self.palette_form(id, kind, pane, tab) {
                        Ok(form) => {
                            if let Some(state) = self.command_palettes.get_mut(&client) {
                                state.widget.open_form(form);
                            }
                        },
                        Err(error) => {
                            if let Some(state) = self.command_palettes.get_mut(&client) {
                                state.widget.set_error(error);
                            }
                        },
                    }
                    return;
                }
            },
            Builtin::Action(action) => {
                if self.palette_target_exists(pane, tab, &action) {
                    Ok(action)
                } else {
                    Err("Destination no longer exists".into())
                }
            },
        };
        match result.and_then(|action| {
            self.validate_palette_split(pane, tab, &action)?;
            Ok(action)
        }) {
            Ok(action) => {
                self.close_command_palette(client);
                self.bus
                    .senders
                    .send_to_screen(ScreenInstruction::ActivateCommandPaletteAction {
                        pane_id: pane,
                        tab_id: tab,
                        action,
                        create_session,
                        client_id: client,
                    })
                    .non_fatal();
            },
            Err(error) => {
                if let Some(state) = self.command_palettes.get_mut(&client) {
                    state.widget.set_error(error);
                }
            },
        }
    }

    fn palette_form(
        &self,
        id: String,
        kind: FormKind,
        pane: PaneId,
        tab: TabId,
    ) -> std::result::Result<Form, String> {
        let tab = self.tabs.get(&tab).ok_or("Tab no longer exists")?;
        let pane = tab
            .pane_infos()
            .into_iter()
            .find(|info| info.id == pane)
            .ok_or("Pane no longer exists")?;
        let (title, fields, submit, confirmation) = match kind {
            FormKind::NewPane => (
                "New pane".into(),
                vec![
                    Field::text("Name", ""),
                    Field::text("Directory", ""),
                    Field::choice(
                        "Placement",
                        &["Automatic tiled", "Right", "Down", "Floating"],
                    ),
                ],
                "Create",
                false,
            ),
            FormKind::NewTab => (
                "New tab".into(),
                vec![Field::text("Name", ""), Field::text("Directory", "")],
                "Create",
                false,
            ),
            FormKind::NewSession => (
                "New session".into(),
                vec![Field::text("Name", ""), Field::text("Directory", "")],
                "Create and switch",
                false,
            ),
            FormKind::RenamePane => (
                "Rename pane".into(),
                vec![Field::text("Name", &pane.title)],
                "Rename",
                false,
            ),
            FormKind::RenameTab => (
                "Rename tab".into(),
                vec![Field::text("Name", &tab.name)],
                "Rename",
                false,
            ),
            FormKind::RenameSession => (
                "Rename session".into(),
                vec![Field::text("Name", self.session_name.as_str())],
                "Rename",
                false,
            ),
            FormKind::ClosePane => (
                format!("Close {} ({})?", pane.title, pane.id),
                vec![],
                "Close pane",
                true,
            ),
            FormKind::CloseTab => (
                format!(
                    "Close {} (tab {}, {} panes)?",
                    tab.name,
                    tab.position.get() + 1,
                    tab.pane_infos().len()
                ),
                vec![],
                "Close tab",
                true,
            ),
        };
        Ok(Form::new(id, title, fields, submit, confirmation))
    }

    fn palette_form_action(
        &self,
        kind: FormKind,
        pane: PaneId,
        tab: TabId,
        values: Vec<String>,
    ) -> std::result::Result<Action, String> {
        if !self.tabs.contains_key(&tab) || self.tab_containing_pane(pane).is_none() {
            return Err("Target no longer exists".into());
        }
        let name = values.first().cloned().unwrap_or_default();
        let cwd = values
            .get(1)
            .filter(|value| !value.is_empty())
            .map(|value| yinz_runtime::shared::expand_path(value))
            .transpose()?;
        if cwd.as_ref().is_some_and(|directory| !directory.is_dir()) {
            return Err("Directory does not exist or is not accessible".into());
        }
        if matches!(kind, FormKind::NewSession | FormKind::RenameSession) {
            yinz_session::sessions::validate_session_name(&name)?;
            let session_name = SessionName::new(&name);
            if matches!(kind, FormKind::RenameSession) && session_name == self.session_name {
                return Ok(Action::NoOp);
            }
            if self.peer_sessions_cache.contains_key(&session_name)
                || self
                    .resurrectable_sessions_cache
                    .contains_key(&session_name)
                || yinz_runtime::consts::YINZ_SOCK_DIR
                    .join(&session_name)
                    .try_exists()
                    .map_err(|error| format!("Could not check session name: {error}"))?
                || yinz_runtime::consts::YINZ_SESSION_INFO_CACHE_DIR
                    .join(&session_name)
                    .try_exists()
                    .map_err(|error| format!("Could not check saved session name: {error}"))?
            {
                return Err("A live or saved session with that name already exists".into());
            }
        }
        Ok(match kind {
            FormKind::NewPane => {
                let command = cwd.map(|cwd| {
                    let mut command = match self.default_shell_action.clone() {
                        Some(TerminalAction::RunCommand(command)) => {
                            RunCommandAction::from(command)
                        },
                        _ => RunCommandAction {
                            command: yinz_bus::pty::get_default_shell(),
                            ..Default::default()
                        },
                    };
                    command.cwd = Some(cwd);
                    command
                });
                let placement = values.get(2).map(String::as_str).unwrap_or_default();
                let direction = match placement {
                    "Right" => Some(Direction::Right),
                    "Down" => Some(Direction::Down),
                    _ => None,
                };
                new_pane(
                    direction,
                    optional_name(name),
                    command,
                    placement == "Floating",
                )
            },
            FormKind::NewTab => new_tab(optional_name(name), cwd),
            FormKind::NewSession => switch_session(SessionName::new(name), cwd),
            FormKind::RenamePane => Action::RenamePaneByPaneId {
                pane_id: Some(pane),
                name: name.into_bytes(),
            },
            FormKind::RenameTab => Action::RenameTabById { id: tab, name },
            FormKind::RenameSession => Action::RenameSession {
                name: SessionName::new(name),
            },
            FormKind::ClosePane => Action::CloseFocusByPaneId { pane_id: pane },
            FormKind::CloseTab => Action::CloseTabById { id: tab },
        })
    }

    pub(crate) fn dispatch_command_palette_action(
        &mut self,
        pane: PaneId,
        tab: TabId,
        action: Action,
        create_session: bool,
        client: ClientId,
    ) {
        let target_exists = if create_session {
            self.tabs.contains_key(&tab)
                && self.tab_containing_pane(pane).is_some()
                && matches!(&action, Action::SwitchSession { name, .. } if !self.peer_sessions_cache.contains_key(name) && !self.resurrectable_sessions_cache.contains_key(name))
        } else {
            self.palette_target_exists(pane, tab, &action)
        };
        if !self.active_tab_ids.contains_key(&client) || !target_exists {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::Notify(
                    "Command palette target changed or no longer exists".into(),
                    None,
                    None,
                    None,
                ))
                .non_fatal();
            return;
        }
        if let Err(error) = self.validate_palette_split(pane, tab, &action) {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::Notify(error, None, None, None))
                .non_fatal();
            return;
        }
        self.run_overlay_actions(pane, vec![action], client, Some(tab));
    }

    pub(crate) fn move_command_palette_pane(
        &mut self,
        pane: PaneId,
        tab: TabId,
        direction: Option<Direction>,
        client: ClientId,
    ) -> Result<()> {
        if !self
            .tabs
            .get(&tab)
            .is_some_and(|tab| tab.has_pane_with_pid(&pane))
        {
            return Ok(());
        }
        if let Some(direction) = direction {
            let mut tabs: Vec<_> = self
                .tabs
                .values()
                .map(|tab| (tab.position, tab.id))
                .collect();
            tabs.sort();
            if tabs.len() < 2 {
                return Ok(());
            }
            let Some(index) = tabs.iter().position(|(_, id)| *id == tab) else {
                return Ok(());
            };
            let offset = if direction == Direction::Left {
                tabs.len() - 1
            } else {
                1
            };
            let destination = tabs[(index + offset) % tabs.len()].1;
            let outcome = self
                .break_multiple_panes_to_tab_with_id(vec![pane], destination, true, client)
                .context("failed to move palette pane")?;
            if outcome.moved.contains(&pane) {
                self.focus_pane_with_id(pane, false, false, client)
                    .context("failed to focus moved palette pane")?;
                self.clear_bell_for_pane_id(pane, client);
                self.sync_scroll_mode_on_focus(client)
                    .context("failed to sync scroll mode for moved palette pane")?;
                self.log_and_report_session_state()
                    .context("failed to report moved palette pane focus")?;
            }
        } else {
            self.break_multiple_panes_to_new_tab(
                vec![pane],
                self.default_shell_action.clone(),
                true,
                None,
                client,
            )
            .context("failed to move palette pane to new tab")?;
        }
        Ok(())
    }
}

fn optional_name(name: String) -> Option<String> {
    (!name.is_empty()).then_some(name)
}

fn new_pane(
    direction: Option<Direction>,
    pane_name: Option<String>,
    command: Option<RunCommandAction>,
    floating: bool,
) -> Action {
    if floating {
        Action::NewFloatingPane {
            command,
            pane_name,
            placement: NewPanePlacement::Floating(None),
            near_current_pane: true,
            no_focus: false,
            tab_id: None,
        }
    } else {
        Action::NewTiledPane {
            direction,
            command,
            pane_name,
            near_current_pane: true,
            no_focus: false,
            borderless: None,
            tab_id: None,
        }
    }
}

fn new_tab(tab_name: Option<String>, cwd: Option<std::path::PathBuf>) -> Action {
    Action::NewTab {
        layout_source: None,
        tiled_layout: None,
        floating_layouts: vec![],
        swap_tiled_layouts: None,
        swap_floating_layouts: None,
        tab_name,
        should_change_focus_to_new_tab: true,
        cwd,
        initial_panes: None,
        first_pane_unblock_condition: None,
    }
}

fn switch_session(name: SessionName, cwd: Option<std::path::PathBuf>) -> Action {
    Action::SwitchSession {
        name,
        tab_position: None,
        pane_id: None,
        layout: None,
        cwd,
    }
}

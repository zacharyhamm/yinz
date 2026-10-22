use yinz_panes::ui::command_palette::escape_literal;
#[path = "palette_agents.rs"]
pub(super) mod agents;
#[path = "palette_history.rs"]
pub(super) mod history;
#[path = "palette_plugins.rs"]
pub(super) mod plugins;

#[path = "palette_themes.rs"]
pub(super) mod themes;

use std::collections::HashMap;
use yinz_bus::screen::ScreenInstruction;
use yinz_bus::ServerInstruction;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{InputMode, KeybindsVec, NewPanePlacement, PaneId, PaneInfo};
use yinz_types::geometry::Direction;
use yinz_types::input::actions::Action;
use yinz_types::input::command::{RunCommandAction, TerminalAction};
use yinz_types::input::keybinds::shortcut_for_action;
use yinz_types::internal_ids::{SessionName, TabId, TabNumber};
use yinz_types::keys::KeyWithModifier;
use yinz_types::mouse::{MouseEvent, MouseEventType};
use yinz_types::pane_size::Viewport;

use super::Screen;
use crate::output::{Output, TranslucentOverlayBacking};
use crate::ui::command_palette::{CommandPalette, Entry, EntryKind, Field, Form, Outcome};
use crate::ui::damage::CellRect;
use crate::ClientId;

pub(super) struct CommandPaletteState {
    widget: CommandPalette,
    suspended: bool,
    plugins: plugins::State,
    agents: agents::State,
    context: PaletteContext,
    actions: HashMap<String, Builtin>,
    warp_source: Option<SessionName>,
    painted: Option<CellRect>,
    backing: TranslucentOverlayBacking,
    watcher_backing: TranslucentOverlayBacking,
    backing_style: yinz_types::style::Style,
    shortcut_keybinds: KeybindsVec,
    shortcuts: HashMap<String, CachedShortcut>,
    #[cfg(test)]
    pub(super) shortcut_lookups: usize,
    #[cfg(test)]
    pub(super) shortcut_keymap_filters: usize,
}

struct PaletteContext {
    tab_id: TabId,
    pane_id: PaneId,
    current_project: Option<String>,
}

#[derive(Default)]
pub(super) struct ClientProject {
    pub(super) current: Option<String>,
    focused: Option<PaneId>,
    focused_project: Option<String>,
}

struct CachedShortcut {
    original: Option<Action>,
    target: Option<Action>,
    hint: String,
}

fn pane_label(pane: &PaneInfo) -> String {
    let id = match pane.id {
        PaneId::Terminal(id) => id.to_string(),
        PaneId::Plugin(_) => pane.id.to_string(),
    };
    format!(
        "[{id}] {}",
        pane.display_title.as_deref().unwrap_or(&pane.title)
    )
}

impl Screen {
    pub(super) fn reconcile_project_contexts(&mut self) {
        for client in self.active_tab_ids.keys().copied().collect::<Vec<_>>() {
            let focused = self.get_active_pane_id(&client);
            let project = focused
                .and_then(|pane| self.pane_projects.get(&pane))
                .cloned();
            let state = self.client_projects.entry(client).or_default();
            if (state.focused != focused || state.focused_project != project) && project.is_some() {
                if state.current != project
                    && let Some(name) = &project
                {
                    self.last_project.remember(name);
                }
                state.current.clone_from(&project);
            }
            if state
                .current
                .as_ref()
                .is_none_or(|name| !self.projects.contains(name))
            {
                state.current = project.clone().or_else(|| {
                    self.last_project
                        .name()
                        .filter(|name| self.projects.contains(*name))
                        .cloned()
                        .or_else(|| self.projects.first().cloned())
                });
            }
            state.focused = focused;
            state.focused_project = project;
        }
    }

    pub(crate) fn toggle_command_palette(&mut self, client_id: ClientId) {
        if self.permission_modal_open(client_id) {
            return;
        }
        if self.close_command_palette(client_id) {
            return;
        }
        let Some(tab_id) = self.active_tab_ids.get(&client_id).copied() else {
            return;
        };
        let Some(pane_id) = self.get_active_pane_id(&client_id) else {
            return;
        };
        let Some(source) = self
            .tabs
            .get(&tab_id)
            .and_then(|tab| tab.pane_infos().into_iter().find(|pane| pane.id == pane_id))
        else {
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
                suspended: false,
                plugins: plugins::State::default(),
                agents: agents::State::new(source),
                context: PaletteContext {
                    tab_id,
                    pane_id,
                    current_project: None,
                },
                actions: HashMap::new(),
                warp_source: None,
                painted: None,
                backing: TranslucentOverlayBacking::default(),
                watcher_backing: TranslucentOverlayBacking::default(),
                backing_style: self.style,
                shortcut_keybinds: Vec::new(),
                shortcuts: HashMap::new(),
                #[cfg(test)]
                shortcut_lookups: 0,
                #[cfg(test)]
                shortcut_keymap_filters: 0,
            },
        );
        self.refresh_command_palettes();
        self.open_plugin_palette(client_id);
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
        self.take_command_palette(client_id).is_some()
    }

    pub(super) fn command_palette_is_open(&self, client_id: ClientId) -> bool {
        self.command_palettes.contains_key(&client_id)
    }

    pub(super) fn palette_owns_input(&self, client_id: ClientId) -> bool {
        self.command_palettes
            .get(&client_id)
            .is_some_and(|s| !s.suspended)
    }

    fn release_palette_input(&self, client_id: ClientId) {
        if !self.command_palette_is_open(client_id) {
            self.bus
                .senders
                .send_to_server(ServerInstruction::SetCommandPaletteOpen(client_id, false))
                .context("failed to restore input mode when closing command palette")
                .non_fatal();
        }
    }

    fn take_command_palette(&mut self, client_id: ClientId) -> Option<CommandPaletteState> {
        let mut state = self.command_palettes.remove(&client_id)?;
        self.cancel_theme_preview(client_id);
        state.backing = TranslucentOverlayBacking::default();
        state.watcher_backing = TranslucentOverlayBacking::default();
        state.painted = None;
        state.agents.cancel();
        self.dismiss_plugin_palette(client_id, &mut state);
        self.release_palette_input(client_id);
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
        Some(state)
    }

    pub(super) fn dismiss_stale_command_palettes(&mut self) {
        let stale: Vec<_> = self
            .command_palettes
            .iter()
            .filter(|(client, state)| {
                self.active_tab_ids.get(client) != Some(&state.context.tab_id)
            })
            .map(|(client, _)| *client)
            .collect();
        for client in stale {
            self.close_command_palette(client);
        }
    }

    pub(super) fn command_palette_surface(&self, client_id: ClientId) -> Option<Viewport> {
        if self
            .command_palettes
            .get(&client_id)
            .is_some_and(|s| s.suspended)
        {
            return None;
        }
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
        if self.permission_modal_input(key, client_id) {
            return true;
        }
        let Some(state) = self.command_palettes.get_mut(&client_id) else {
            return false;
        };
        if state.suspended {
            return false;
        }
        state.agents.save_form(&state.widget);
        let before = (
            state.widget.view,
            state.widget.query().to_owned(),
            state.widget.is_root(),
        );
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
        let changed = before
            != (
                state.widget.view,
                state.widget.query().to_owned(),
                state.widget.is_root(),
            );
        let query_changed = before.1 != state.widget.query();
        let history_query_changed = query_changed && before.0 == state.widget.view;
        if changed {
            self.refresh_palette_history(client_id, history_query_changed);
            self.refresh_plugin_palette(client_id, query_changed);
        }
        self.handle_palette_outcome(client_id, outcome);
        self.refresh_theme_preview(client_id);
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
            return self.palette_owns_input(client_id);
        };
        let Some(state) = self.command_palettes.get_mut(&client_id) else {
            return false;
        };
        let layout = state.widget.layout(surface);
        if (event.wheel_up || event.wheel_down) && state.widget.scroll_agent(event.wheel_down) {
            self.render().non_fatal();
            return true;
        }
        if event.event_type == MouseEventType::Press && (event.left || event.right || event.middle)
        {
            let inside = usize::try_from(event.position.line())
                .ok()
                .is_some_and(|y| layout.rect.contains(event.position.column(), y));
            if !inside {
                self.close_command_palette(client_id);
                self.command_palette_mouse_captures.insert(client_id);
            } else if event.left && !state.widget.busy {
                state.widget.click(&layout, event.position);
            }
            self.refresh_theme_preview(client_id);
            self.render().non_fatal();
        }
        true
    }

    pub(super) fn invalidate_command_palette_backing(
        &mut self,
        client: ClientId,
        damage: &[CellRect],
    ) {
        if let Some(state) = self.command_palettes.get_mut(&client) {
            state.backing.invalidate(damage);
        }
    }

    pub(super) fn prepare_command_palette_render(&mut self) {
        self.remove_unavailable_palette_providers();
        self.refresh_command_palettes();
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
            if state.backing_style != self.style {
                state.backing = TranslucentOverlayBacking::default();
                state.watcher_backing = TranslucentOverlayBacking::default();
                state.backing_style = self.style;
            }
            let mut needs_backing = state.backing.prepare(
                rect,
                self.background_opacity,
                *self.terminal_emulator_colors.borrow(),
                self.style.colors.text_unselected.background,
                &self.terminal_emulator_color_codes.borrow(),
            );
            if self.followed_client_id == Some(client) && !self.watcher_clients.is_empty() {
                needs_backing |= state.watcher_backing.prepare(
                    rect,
                    self.background_opacity,
                    *self.terminal_emulator_colors.borrow(),
                    self.style.colors.text_unselected.background,
                    &self.terminal_emulator_color_codes.borrow(),
                );
            }
            if let Some(tab) = self.tabs.get_mut(&state.context.tab_id) {
                if geometry_changed {
                    tab.set_should_clear_display_before_rendering();
                    tab.set_force_render();
                } else if needs_backing {
                    tab.force_render_panes_intersecting(&[rect]);
                }
            }
        }
    }

    /// Called after pane cursors: the palette's input cursor is the final instruction.
    pub(super) fn render_command_palettes(
        &mut self,
        output: &mut Output,
        clients: impl Iterator<Item = ClientId>,
        watcher: bool,
    ) -> Result<()> {
        for client in clients {
            let Some(surface) = self.command_palette_surface(client) else {
                continue;
            };
            let Some(state) = self.command_palettes.get_mut(&client) else {
                continue;
            };
            let layout = state.widget.layout(surface);
            if layout.rect.is_empty() {
                continue;
            }
            let cursor = state
                .widget
                .cursor(&layout)
                .filter(|(x, y)| !output.cursor_is_covered_by_notification(*x, *y, client));
            output
                .add_translucent_overlay(
                    client,
                    layout.rect,
                    self.background_opacity,
                    state.widget.chunks(&layout, &self.style),
                    cursor,
                    *self.terminal_emulator_colors.borrow(),
                    self.style.colors.text_unselected.background,
                    &self.terminal_emulator_color_codes.borrow(),
                    if watcher {
                        &mut state.watcher_backing
                    } else {
                        &mut state.backing
                    },
                )
                .context("failed to render command palette")?;
            if let Some((x, y)) = cursor {
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
    Theme(String),
    Inspect(history::LiveAgent),
    Archive,
    AgentFilters,
    Agent(yinz_bus::command_palette::AgentAction),
    Action(Action),
    Picker(EntryKind),
    Project(String),
    Form(FormKind),
    WarpSource(SessionName, EntryKind),
    Warp(SessionName, WarpTarget),
}

#[derive(Clone)]
enum WarpTarget {
    Pane(PaneId),
    Tab(TabId),
    AllTabs,
}

#[derive(Clone, Copy)]
enum FormKind {
    NewPane,
    NewTab,
    RenameTabGroup(u32),
    CloseTabGroup(u32),
    RenamePane,
    RenameTab,
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
            "switch-project",
            "Switch current project…",
            &["project context"],
            Builtin::Picker(EntryKind::Project),
        );
        command(
            "agents",
            "Agents…",
            &["find live agents"],
            Builtin::Picker(EntryKind::Agent),
        );
        command(
            "archived-agents",
            "Archived agents…",
            &["history resume"],
            Builtin::Archive,
        );
        command(
            "agent-filters",
            "Agent filters…",
            &["filter agents"],
            Builtin::AgentFilters,
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
            ("rename-pane", "Rename pane…", FormKind::RenamePane),
            ("rename-tab", "Rename tab…", FormKind::RenameTab),
            ("close-pane", "Close pane…", FormKind::ClosePane),
            ("close-tab", "Close tab…", FormKind::CloseTab),
        ] {
            command(id, label, &[], Builtin::Form(form));
        }
        if self.tab_groups_enabled() {
            command(
                "new-tab-group",
                "New tab group",
                &["create group workspace"],
                Builtin::Action(NewTabGroup),
            );
            command(
                "switch-tab-group",
                "Switch tab group…",
                &["workspace"],
                Builtin::Picker(EntryKind::TabGroup),
            );
            if let Some(group) = self.tab_group_ids.get(&tab_id) {
                command(
                    "rename-tab-group",
                    "Rename tab group…",
                    &[],
                    Builtin::Form(FormKind::RenameTabGroup(*group)),
                );
                command(
                    "close-tab-group",
                    "Close tab group…",
                    &[],
                    Builtin::Form(FormKind::CloseTabGroup(*group)),
                );
            }
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
            ("detach", "Detach", Detach),
        ] {
            command(id, label, &[], Builtin::Action(action));
        }
        let offered_agents = yinz_runtime::agent::launch::offered();
        for kind in &offered_agents {
            for (suffix, label, aliases, action) in [
                (
                    "project",
                    format!("Spawn {kind} agent in project…"),
                    format!("{kind} project"),
                    yinz_bus::command_palette::AgentAction::Projects { kind: kind.clone() },
                ),
                (
                    "worktree",
                    format!("Spawn {kind} agent in worktree…"),
                    format!("{kind} worktree"),
                    yinz_bus::command_palette::AgentAction::Start {
                        kind: kind.clone(),
                        project: state.context.current_project.clone(),
                    },
                ),
                (
                    "directory",
                    format!("Spawn {kind} agent in current directory"),
                    format!("{kind} current directory cwd"),
                    yinz_bus::command_palette::AgentAction::Current { kind: kind.clone() },
                ),
            ] {
                command(
                    &format!("agent:{kind}:{suffix}"),
                    &escape_literal(&label),
                    &[&aliases],
                    Builtin::Agent(action),
                );
            }
        }
        command(
            "agent:duplicate",
            "Duplicate focused agent",
            &["duplicate agent"],
            Builtin::Agent(yinz_bus::command_palette::AgentAction::Duplicate { kind: None }),
        );
        command(
            "agent:fork",
            "Fork focused agent…",
            &["fork agent"],
            Builtin::Agent(yinz_bus::command_palette::AgentAction::Fork),
        );
        command(
            "change-theme",
            "Change theme…",
            &["colors appearance"],
            Builtin::Picker(EntryKind::Theme),
        );
        for (id, label, kind) in [
            ("warp-tab", "Warp tab…", EntryKind::WarpTabSession),
            ("warp-pane", "Warp pane…", EntryKind::WarpPaneSession),
        ] {
            entries.push((
                Entry {
                    id: id.into(),
                    label: label.into(),
                    description: "Bring from another session into this session".into(),
                    shortcut: String::new(),
                    aliases: vec![],
                    kind: EntryKind::Command,
                },
                Builtin::Picker(kind),
            ));
        }
        for id in self.tab_groups.keys().filter(|_| self.tab_groups_enabled()) {
            entries.push((
                Entry {
                    id: format!("tab-group:{id}"),
                    label: escape_literal(&self.tab_group_name(*id).unwrap_or_default()),
                    description: if self.tab_group_ids.get(&tab_id) == Some(id) {
                        "Current".into()
                    } else {
                        String::new()
                    },
                    shortcut: String::new(),
                    aliases: vec![],
                    kind: EntryKind::TabGroup,
                },
                Builtin::Action(Action::SwitchTabGroup { id: *id }),
            ));
        }
        entries.extend(self.warp_palette_entries(state.warp_source.as_ref()));
        for name in &self.projects {
            let source = self
                .project_sources
                .get(name)
                .map(String::as_str)
                .unwrap_or_default();
            let description = if state.context.current_project.as_ref() == Some(name) {
                if source.is_empty() {
                    "Current".into()
                } else {
                    format!("Current · {source}")
                }
            } else {
                source.to_owned()
            };
            entries.push((
                Entry {
                    id: format!("project:{name}"),
                    label: escape_literal(name),
                    description: escape_literal(&description),
                    shortcut: String::new(),
                    aliases: vec![],
                    kind: EntryKind::Project,
                },
                Builtin::Project(name.clone()),
            ));
        }
        let live = self.live_palette_agents();
        state.widget.agent_metadata = live.iter().map(|a| (a.id(), a.metadata.clone())).collect();
        entries.extend(live.into_iter().map(|agent| {
            (
                Entry {
                    id: agent.id(),
                    label: escape_literal(&pane_label(&agent.pane)),
                    description: escape_literal(&agent.description()),
                    shortcut: String::new(),
                    aliases: vec![
                        agent.pane.title.clone(),
                        agent.pane.id.to_string(),
                        agent.pane.id.legacy_parts().0.to_string(),
                        agent
                            .metadata
                            .values()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" "),
                    ],
                    kind: EntryKind::Agent,
                },
                Builtin::Inspect(agent),
            )
        }));
        for name in self.themes.keys() {
            entries.push((
                Entry {
                    id: format!("theme:{name}"),
                    label: escape_literal(name),
                    description: String::new(),
                    shortcut: String::new(),
                    aliases: vec![],
                    kind: EntryKind::Theme,
                },
                Builtin::Theme(name.clone()),
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
            if original.is_none() && target.is_none() {
                continue;
            }
            if let Some(cached) = state.shortcuts.get(&entry.id)
                && cached.original == original
                && cached.target == target
            {
                entry.shortcut.clone_from(&cached.hint);
                continue;
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
            entry.shortcut = shortcut_for_action(regular_keybinds, InputMode::Normal, is_target)
                .or_else(|| shortcut_for_action(&mode_info.keybinds, InputMode::Normal, is_target))
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

    fn warp_palette_entries(&self, source: Option<&SessionName>) -> Vec<(Entry, Builtin)> {
        let mut entries = Vec::new();
        let mut add = |id, label: String, description: String, kind, action| {
            let aliases = match &action {
                Builtin::Warp(_, WarpTarget::Pane(id)) => {
                    vec![id.to_string(), id.legacy_parts().0.to_string()]
                },
                _ => vec![],
            };
            entries.push((
                Entry {
                    id,
                    label: escape_literal(&label),
                    description: escape_literal(&description),
                    kind,
                    shortcut: String::new(),
                    aliases,
                },
                action,
            ));
        };
        for session in self
            .peer_sessions_cache
            .values()
            .filter(|s| s.name != self.session_name)
        {
            for (prefix, kind, target) in [
                ("warp-tab", EntryKind::WarpTabSession, EntryKind::WarpTab),
                ("warp-pane", EntryKind::WarpPaneSession, EntryKind::WarpPane),
            ] {
                add(
                    format!("{prefix}:session:{}", session.name),
                    session.name.to_string(),
                    String::new(),
                    kind,
                    Builtin::WarpSource(session.name.clone(), target),
                );
            }
            if source != Some(&session.name) {
                continue;
            }
            let mut tabs: Vec<_> = session.tabs.iter().collect();
            tabs.sort_by_key(|tab| tab.position);
            for tab in &tabs {
                add(
                    format!("warp-tab:tab:{}:{}", session.name, tab.tab_id),
                    format!("{} (tab {})", tab.name, tab.position.get() + 1),
                    session.name.to_string(),
                    EntryKind::WarpTab,
                    Builtin::Warp(session.name.clone(), WarpTarget::Tab(tab.tab_id)),
                );
                for pane in session
                    .panes
                    .panes
                    .get(&tab.position)
                    .into_iter()
                    .flatten()
                    .filter(|pane| pane.is_selectable && !pane.is_suppressed)
                {
                    add(
                        format!("warp-pane:pane:{}:{}", session.name, pane.id),
                        format!("{} · {}", pane_label(pane), tab.name),
                        session.name.to_string(),
                        EntryKind::WarpPane,
                        Builtin::Warp(session.name.clone(), WarpTarget::Pane(pane.id)),
                    );
                }
            }
            if !tabs.is_empty() {
                add(
                    format!("warp-tab:all:{}", session.name),
                    "All tabs".into(),
                    session.name.to_string(),
                    EntryKind::WarpTab,
                    Builtin::Warp(session.name.clone(), WarpTarget::AllTabs),
                );
            }
        }
        entries
    }

    fn run_palette_warp(&mut self, client: ClientId, source: SessionName, target: WarpTarget) {
        use yinz_bus::warp::{warp_pane_from_session, warp_tab_from_session, WARP_PULL_TIMEOUT};
        let Some(session) = self
            .peer_sessions_cache
            .get(&source)
            .filter(|s| s.name != self.session_name)
        else {
            if let Some(state) = self.command_palettes.get_mut(&client) {
                state
                    .widget
                    .set_error("Source session no longer exists".into());
                notify(&self.bus.senders, "Source session no longer exists".into());
            }
            return;
        };
        let mut tabs: Vec<_> = session.tabs.iter().collect();
        tabs.sort_by_key(|tab| tab.position);
        let valid = match target {
            WarpTarget::Tab(id) => tabs.iter().any(|tab| tab.tab_id == id),
            WarpTarget::AllTabs => !tabs.is_empty(),
            WarpTarget::Pane(id) => tabs.iter().any(|tab| {
                session
                    .panes
                    .panes
                    .get(&tab.position)
                    .into_iter()
                    .flatten()
                    .any(|pane| pane.id == id && pane.is_selectable && !pane.is_suppressed)
            }),
        };
        if !valid {
            if let Some(state) = self.command_palettes.get_mut(&client) {
                state
                    .widget
                    .set_error("Warp target no longer exists".into());
                notify(&self.bus.senders, "Warp target no longer exists".into());
            }
            return;
        }
        let tab_ids: Vec<_> = match target {
            WarpTarget::Tab(id) => vec![id],
            WarpTarget::AllTabs => tabs.iter().map(|tab| tab.tab_id).collect(),
            WarpTarget::Pane(_) => vec![],
        };
        let destination = self.session_name.clone();
        let senders = self.bus.senders.clone();
        self.close_command_palette(client);
        let spawned = std::thread::Builder::new()
            .name("palette_warp".into())
            .spawn(move || {
                let result = if let WarpTarget::Pane(id) = target {
                    warp_pane_from_session(
                        &destination,
                        &source,
                        id,
                        yinz_types::data::WarpPlacement::NewTab,
                        WARP_PULL_TIMEOUT,
                    ).map(|_| format!("Brought pane from session ‘{source}’"))
                } else {
                    let total = tab_ids.len();
                    tab_ids
                        .into_iter()
                        .enumerate()
                        .try_for_each(|(completed, id)| {
                            warp_tab_from_session(&destination, &source, id, WARP_PULL_TIMEOUT)
                                .map(|_| ())
                                .map_err(|error| {
                                    format!(
                                        "Warp tab {id} failed ({completed}/{total} completed): {error}"
                                    )
                                })
                        })
                        .map(|()| format!("Brought {total} tab(s) from session '{source}'"))
                };
                let message = result.unwrap_or_else(|error| error);
                senders
                    .send_to_screen(ScreenInstruction::Notify(message, None, None, None))
                    .context("failed to report palette warp result")
                    .non_fatal();
            });
        if let Err(error) = spawned {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::Notify(
                    format!("Failed to start palette warp: {error}"),
                    None,
                    None,
                    None,
                ))
                .context("failed to report palette warp worker failure")
                .non_fatal();
        }
    }

    fn refresh_command_palettes(&mut self) {
        self.reconcile_project_contexts();
        let clients: Vec<_> = self
            .command_palettes
            .iter()
            .map(|(client, state)| (*client, state.context.pane_id, state.context.tab_id))
            .collect();
        for (client, pane, tab) in clients {
            if let Some(mut state) = self.command_palettes.remove(&client) {
                state.context.current_project = self
                    .client_projects
                    .get(&client)
                    .and_then(|state| state.current.clone());
                state
                    .widget
                    .current_project
                    .clone_from(&state.context.current_project);
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
            Action::SwitchTabGroup { id }
            | Action::RenameTabGroup { id, .. }
            | Action::CloseTabGroup { id } => self.tab_groups.contains_key(id),
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
        if outcome == Outcome::Cancel
            && self
                .theme_preview
                .as_ref()
                .is_some_and(|preview| preview.client == client)
        {
            self.close_command_palette(client);
            return;
        }
        if self.history_palette_outcome(client, &outcome) {
            return;
        }
        if self.agent_palette_outcome(client, &outcome) {
            return;
        }
        if self.plugin_palette_outcome(client, &outcome) {
            return;
        }
        if outcome == Outcome::Dismiss {
            self.close_command_palette(client);
            return;
        }
        let Some(state) = self.command_palettes.get(&client) else {
            return;
        };
        let pane = state.context.pane_id;
        let tab = state.context.tab_id;
        let (id, values) = match outcome {
            Outcome::Activate(id) => (id, None),
            Outcome::Submit { id, values } => (id, Some(values)),
            _ => return,
        };
        let label = state.widget.action_label(&id);
        let Some(builtin) = state.actions.get(&id).cloned() else {
            if let Some(state) = self.command_palettes.get_mut(&client) {
                state
                    .widget
                    .set_error("Destination no longer exists".into());
                notify(&self.bus.senders, "Destination no longer exists".into());
            }
            return;
        };
        let result = match builtin {
            Builtin::Theme(name) => {
                if let Some(values) = values {
                    self.save_palette_theme(
                        client,
                        name,
                        values.first().is_some_and(|v| v == "Save permanently"),
                    );
                } else if let Some(state) = self.command_palettes.get_mut(&client) {
                    state.widget.open_form(Form::new(
                        id,
                        escape_literal(&format!("Apply theme {name}")),
                        vec![Field::choice(
                            "Destination",
                            &["Apply to session", "Save permanently"],
                        )],
                        "Apply",
                        false,
                    ));
                }
                return;
            },
            Builtin::Picker(EntryKind::Theme) => {
                self.open_theme_picker(client);
                return;
            },
            Builtin::Inspect(agent) => {
                self.open_palette_history(client, Some(agent), None);
                return;
            },
            Builtin::Archive => {
                self.open_palette_history(client, None, None);
                return;
            },
            Builtin::AgentFilters => {
                self.open_agent_filters(client);
                return;
            },
            Builtin::Agent(action) => {
                self.request_palette_agent(client, action, values.unwrap_or_default());
                return;
            },
            Builtin::WarpSource(source, kind) => {
                let valid =
                    source != self.session_name && self.peer_sessions_cache.contains_key(&source);
                if let Some(state) = self.command_palettes.get_mut(&client) {
                    if valid {
                        state.warp_source = Some(source);
                        state.widget.open_picker(kind);
                    } else {
                        state
                            .widget
                            .set_error("Source session no longer exists".into());
                        notify(&self.bus.senders, "Source session no longer exists".into());
                    }
                }
                self.refresh_command_palettes();
                self.refresh_plugin_palette(client, false);
                return;
            },
            Builtin::Warp(source, target) => {
                self.run_palette_warp(client, source, target);
                return;
            },
            Builtin::Project(name) => {
                if self.projects.contains(&name) {
                    notify(&self.bus.senders, format!("Current project: {name}"));
                    self.last_project.remember(&name);
                    self.client_projects.entry(client).or_default().current = Some(name);
                    if let Some(state) = self.command_palettes.get_mut(&client) {
                        state.widget.back();
                        state.widget.clear_searches();
                    }
                } else if let Some(state) = self.command_palettes.get_mut(&client) {
                    state.widget.set_error("Project no longer exists".into());
                    notify(&self.bus.senders, "Project no longer exists".into());
                }
                self.refresh_plugin_palette(client, false);
                return;
            },
            Builtin::Picker(kind) => {
                if let Some(state) = self.command_palettes.get_mut(&client) {
                    state.widget.open_picker(kind);
                }
                self.refresh_plugin_palette(client, false);
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
                                state.widget.set_error(error.clone());
                                notify(
                                    &self.bus.senders,
                                    format!("Could not {}: {error}", label.to_lowercase()),
                                );
                            }
                        },
                    }
                    self.refresh_plugin_palette(client, false);
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
                        label,
                        pane_id: pane,
                        tab_id: tab,
                        action,
                        client_id: client,
                    })
                    .non_fatal();
            },
            Err(error) => {
                if let Some(state) = self.command_palettes.get_mut(&client) {
                    state.widget.set_error(error.clone());
                    notify(
                        &self.bus.senders,
                        format!("Could not {}: {error}", label.to_lowercase()),
                    );
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
            FormKind::RenameTabGroup(id) => (
                "Rename tab group".into(),
                vec![Field::text(
                    "Name",
                    &self
                        .tab_groups
                        .get(&id)
                        .ok_or("Group no longer exists")?
                        .name,
                )],
                "Rename",
                false,
            ),
            FormKind::CloseTabGroup(id) => {
                let name = self.tab_group_name(id).ok_or("Group no longer exists")?;
                let tabs = self.group_tabs(id);
                let panes: usize = tabs
                    .iter()
                    .filter_map(|id| self.tabs.get(id))
                    .map(|tab| tab.pane_infos().len())
                    .sum();
                (
                    format!("Close {name} ({} tabs, {panes} panes)?", tabs.len()),
                    vec![],
                    "Close group",
                    true,
                )
            },
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
            FormKind::ClosePane => (
                format!("Close {}?", pane_label(&pane)),
                vec![],
                "Close pane",
                true,
            ),
            FormKind::CloseTab => (
                format!(
                    "Close {} (tab {}, {} panes)?",
                    self.plugin_tab_names()[&tab.id],
                    tab.position.get() + 1,
                    tab.pane_infos().len()
                ),
                vec![],
                "Close tab",
                true,
            ),
        };
        Ok(Form::new(
            id,
            escape_literal(&title),
            fields,
            submit,
            confirmation,
        ))
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
        Ok(match kind {
            FormKind::RenameTabGroup(id) => {
                if !self.tab_groups.contains_key(&id) {
                    return Err("Group no longer exists".into());
                }
                Action::RenameTabGroup { id, name }
            },
            FormKind::CloseTabGroup(id) => {
                if !self.tab_groups.contains_key(&id) {
                    return Err("Group no longer exists".into());
                }
                Action::CloseTabGroup { id }
            },
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
            FormKind::RenamePane => Action::RenamePaneByPaneId {
                pane_id: Some(pane),
                name: name.into_bytes(),
            },
            FormKind::RenameTab => Action::RenameTabById { id: tab, name },
            FormKind::ClosePane => Action::CloseFocusByPaneId { pane_id: pane },
            FormKind::CloseTab => Action::CloseTabById { id: tab },
        })
    }

    pub(crate) fn dispatch_command_palette_action(
        &mut self,
        pane: PaneId,
        tab: TabId,
        action: Action,
        client: ClientId,
        label: String,
    ) {
        let target_exists = self.palette_target_exists(pane, tab, &action);
        if !self.active_tab_ids.contains_key(&client) || !target_exists {
            self.bus
                .senders
                .send_to_screen(ScreenInstruction::Notify(
                    format!(
                        "Could not {}: destination no longer exists",
                        label.to_lowercase()
                    ),
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
        if matches!(&action, Action::MoveTabByTabId { .. }) && self.tabs.len() < 2 {
            notify(&self.bus.senders, "Tab was not moved: no other tab".into());
            return;
        }
        let departure = match &action {
            Action::Detach => Some("Detaching…".to_owned()),
            _ => None,
        };
        if let Some(message) = departure {
            notify(&self.bus.senders, message);
        }
        self.run_overlay_actions(pane, vec![action], client, Some((tab, label)));
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
            anyhow::bail!("Pane is no longer available");
        }
        if let Some(direction) = direction {
            let destination = self
                .adjacent_group_tab(tab, direction != Direction::Left)
                .and_then(|position| self.get_tab_id_at_position(position));
            let Some(destination) = destination.filter(|destination| *destination != tab) else {
                notify(&self.bus.senders, "Pane was not moved: no other tab".into());
                return Ok(());
            };
            let outcome = self
                .break_multiple_panes_to_tab_with_id(vec![pane], destination, true, client)
                .context("failed to move palette pane")?;
            if !outcome.moved.contains(&pane) {
                notify(&self.bus.senders, "Pane was not moved".into());
                return Ok(());
            }
            self.focus_pane_with_id(pane, false, false, client)
                .context("failed to focus moved palette pane")?;
            self.clear_bell_for_pane_id(pane, client);
            self.sync_scroll_mode_on_focus(client)
                .context("failed to sync scroll mode for moved palette pane")?;
            self.log_and_report_session_state()
                .context("failed to report moved palette pane focus")?;
        } else {
            let destination = self
                .break_multiple_panes_to_new_tab(
                    vec![pane],
                    self.default_shell_action.clone(),
                    true,
                    None,
                    client,
                )
                .context("failed to move palette pane to new tab")?;
            if !self
                .tabs
                .get(&destination)
                .is_some_and(|tab| tab.has_pane_with_pid(&pane))
            {
                notify(&self.bus.senders, "Pane was not moved".into());
                return Ok(());
            }
        }
        let label = match direction {
            Some(Direction::Left) => "Move pane to previous tab",
            Some(_) => "Move pane to next tab",
            None => "Move pane to new tab",
        };
        notify(&self.bus.senders, format!("Completed: {label}"));
        Ok(())
    }
}

pub(super) fn notify(senders: &yinz_bus::thread_bus::ThreadSenders, message: String) {
    senders
        .send_to_screen(ScreenInstruction::Notify(message, None, None, None))
        .context("notifying palette result")
        .non_fatal();
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

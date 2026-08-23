use zellij_utils::errors::prelude::*;

use super::Screen;
use crate::background_jobs::BackgroundJob;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::tab::{pane_info_for_pane, Tab};
use crate::ClientId;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};
use zellij_utils::consts::ZELLIJ_SOCK_DIR;
use zellij_utils::data::{
    Event, EventType, LayoutInfo, LayoutWithError, ListPanesResponse, ListTabsResponse, PaneInfo,
    PaneListEntry, PaneManifest, SessionInfo, TabInfo,
};
#[cfg(not(test))]
use zellij_utils::input::layout::Layout;
use zellij_utils::ipc::{
    MobileActivePanePayload, MobilePanePayload, MobileSessionPayload, MobileSizePayload,
    MobileStatePayload, MobileTabPayload, ServerToClientMsg,
};

impl Screen {
    pub fn generate_and_report_tab_state(&mut self) -> Result<Vec<TabInfo>> {
        let mut plugin_updates = vec![];
        let mut tab_infos_for_screen_state = BTreeMap::new();
        for tab in self.tabs.values() {
            let all_focused_clients: Vec<ClientId> = self
                .active_tab_ids
                .iter()
                .filter(|(_c_id, tab_position)| **tab_position == tab.id)
                .map(|(c_id, _)| c_id)
                .copied()
                .collect();
            let (active_swap_layout_name, is_swap_layout_dirty) = tab.swap_layout_info();
            let tab_viewport = tab.get_viewport();
            let tab_display_area = tab.get_display_area();
            let selectable_tiled_panes_count = tab.get_selectable_tiled_panes_count();
            let selectable_floating_panes_count = tab.get_selectable_floating_panes_count();
            let tab_info_for_screen = TabInfo {
                position: tab.position,
                name: tab.name.clone(),
                active: self.active_tab_ids.values().any(|i| i == &tab.id),
                panes_to_hide: tab.panes_to_hide_count(),
                is_fullscreen_active: tab.is_fullscreen_active(),
                is_sync_panes_active: tab.is_sync_panes_active(),
                are_floating_panes_visible: tab.are_floating_panes_visible(),
                other_focused_clients: all_focused_clients,
                active_swap_layout_name,
                is_swap_layout_dirty,
                viewport_rows: tab_viewport.rows,
                viewport_columns: tab_viewport.cols,
                display_area_rows: tab_display_area.rows,
                display_area_columns: tab_display_area.cols,
                selectable_tiled_panes_count,
                selectable_floating_panes_count,
                tab_id: tab.id,
                has_bell_notification: tab.tab_has_pending_bell
                    && !self.active_tab_ids.values().any(|i| i == &tab.id),
                is_flashing_bell: tab.tab_bell_flash
                    && !self.active_tab_ids.values().any(|i| i == &tab.id),
            };
            tab_infos_for_screen_state.insert(tab.position, tab_info_for_screen);
        }
        let plugin_tab_names = self.plugin_tab_names();
        for (client_id, active_tab_index) in self.active_tab_ids.iter() {
            let mut plugin_tab_updates = vec![];
            for tab in self.tabs.values() {
                let other_focused_clients: Vec<ClientId> = if self.session_is_mirrored {
                    vec![]
                } else {
                    self.active_tab_ids
                        .iter()
                        .filter(|(c_id, tab_position)| {
                            **tab_position == tab.id && *c_id != client_id
                        })
                        .map(|(c_id, _)| c_id)
                        .copied()
                        .collect()
                };
                let (active_swap_layout_name, is_swap_layout_dirty) = tab.swap_layout_info();
                let tab_viewport = tab.get_viewport();
                let tab_display_area = tab.get_display_area();
                let selectable_tiled_panes_count = tab.get_selectable_tiled_panes_count();
                let selectable_floating_panes_count = tab.get_selectable_floating_panes_count();
                let tab_info_for_plugins = TabInfo {
                    position: tab.position,
                    name: plugin_tab_names
                        .get(&tab.id)
                        .cloned()
                        .unwrap_or_else(|| tab.name.clone()),
                    active: *active_tab_index == tab.id,
                    panes_to_hide: tab.panes_to_hide_count(),
                    is_fullscreen_active: tab.is_fullscreen_active(),
                    is_sync_panes_active: tab.is_sync_panes_active(),
                    are_floating_panes_visible: tab.are_floating_panes_visible(),
                    other_focused_clients,
                    active_swap_layout_name,
                    is_swap_layout_dirty,
                    viewport_rows: tab_viewport.rows,
                    viewport_columns: tab_viewport.cols,
                    display_area_rows: tab_display_area.rows,
                    display_area_columns: tab_display_area.cols,
                    selectable_tiled_panes_count,
                    selectable_floating_panes_count,
                    tab_id: tab.id,
                    has_bell_notification: tab.tab_has_pending_bell && *active_tab_index != tab.id,
                    is_flashing_bell: tab.tab_bell_flash && *active_tab_index != tab.id,
                };
                plugin_tab_updates.push(tab_info_for_plugins);
            }
            plugin_tab_updates.sort_by_key(|a| a.position);
            let target_plugin_ids = self.targeted_plugin_ids(*client_id, EventType::TabUpdate);
            for plugin_id in target_plugin_ids {
                plugin_updates.push((
                    Some(plugin_id),
                    Some(*client_id),
                    Event::TabUpdate(plugin_tab_updates.clone()),
                ));
            }
        }
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::Update(plugin_updates))
            .context("failed to update tabs")?;
        Ok(tab_infos_for_screen_state.values().cloned().collect())
    }
    pub(super) fn generate_and_report_pane_state(&mut self) -> Result<PaneManifest> {
        let mut pane_manifest = PaneManifest::default();
        for tab in self.tabs.values() {
            pane_manifest.panes.insert(tab.position, tab.pane_infos());
        }
        let current_pane_group = HashMap::new();
        pane_manifest.global_panes = self
            .global_panes
            .panes()
            .map(|(pane, placement, is_hidden)| {
                let pane_id = pane.pid();
                let mut pane_info = pane_info_for_pane(&pane_id, pane, &current_pane_group);
                pane_info.is_focused = self.active_tab_ids.keys().any(|client_id| {
                    self.global_panes.focused_pane_id(*client_id) == Some(pane_id)
                });
                pane_info.global_placement = Some(placement);
                pane_info.is_hidden = is_hidden;
                pane_info
            })
            .collect();
        // One place, after both halves of the manifest are built. A plugin pane
        // is not a process in a folder, so it is left without one.
        for pane_info in pane_manifest
            .panes
            .values_mut()
            .flatten()
            .chain(pane_manifest.global_panes.iter_mut())
            .filter(|pane_info| !pane_info.is_plugin)
        {
            pane_info.cwd = self
                .terminal_cwds
                .get(&pane_info.id)
                .map(|cwd| cwd.to_string_lossy().into_owned());
            pane_info.host = self.terminal_hosts.get(&pane_info.id).cloned();
        }
        let mut plugin_updates = vec![];
        let client_ids: Vec<ClientId> = self.active_tab_ids.keys().copied().collect();
        for client_id in client_ids {
            let target_plugin_ids = self.targeted_plugin_ids(client_id, EventType::PaneUpdate);
            for plugin_id in target_plugin_ids {
                plugin_updates.push((
                    Some(plugin_id),
                    Some(client_id),
                    Event::PaneUpdate(pane_manifest.clone()),
                ));
            }
        }
        if !plugin_updates.is_empty() {
            self.bus
                .senders
                .send_to_plugin(PluginInstruction::Update(plugin_updates))
                .context("failed to update pane state")?;
        }

        Ok(pane_manifest)
    }

    pub(super) fn collect_pane_list(&self, show_all: bool) -> Result<ListPanesResponse> {
        fn should_include_pane(pane_info: &PaneInfo, show_all: bool) -> bool {
            pane_info.is_selectable || show_all
        }

        fn create_pane_list_entry(pane_info: PaneInfo, tab: &crate::tab::Tab) -> PaneListEntry {
            PaneListEntry {
                pane_info,
                tab_id: tab.id,
                tab_position: tab.position,
                tab_name: tab.name.clone(),
                pane_command: None,
                pane_cwd: None,
            }
        }

        fn sort_panes_by_tab_and_type(pane_entries: &mut [PaneListEntry]) {
            pane_entries.sort_by_key(|e| (e.tab_position, !e.pane_info.is_plugin, e.pane_info.id));
        }

        let mut pane_entries = Vec::new();

        for tab in self.tabs.values() {
            let pane_infos = tab.pane_infos();

            for pane_info in pane_infos {
                if should_include_pane(&pane_info, show_all) {
                    pane_entries.push(create_pane_list_entry(pane_info, tab));
                }
            }
        }

        sort_panes_by_tab_and_type(&mut pane_entries);
        Ok(pane_entries)
    }

    pub(super) fn collect_tab_list(&self, _client_id: ClientId) -> Result<ListTabsResponse> {
        let mut tab_infos = Vec::new();

        for tab in self.tabs.values() {
            if let Some(tab_info) = self.get_tab_info(tab.id) {
                tab_infos.push(tab_info);
            }
        }

        // Sort by position (display order)
        tab_infos.sort_by_key(|t| t.position);

        Ok(tab_infos)
    }

    pub(super) fn get_current_tab_info(&self, client_id: ClientId) -> Result<Option<TabInfo>> {
        match self.active_tab_ids.get(&client_id) {
            Some(active_tab_id) => Ok(self.get_tab_info(*active_tab_id)),
            None => Ok(None),
        }
    }

    /// The name each tab shows a plugin, which is not always the name it holds.
    ///
    /// Three things can name a tab, and they are asked in this order:
    ///
    /// 1. A lone pane that lends its own title (see [`Tab::single_pane_tab_name`]).
    /// 2. The project the tab is working on, when a plugin has told us of one.
    /// 3. The tab's own name — what the user typed, or the "Tab #N" it was born with.
    ///
    /// Only the third is stored on the tab. The other two are worked out afresh
    /// each time, so a tab the user names keeps that name, and a tab goes back to
    /// its own the moment the thing that was lending it one is gone. Nothing here
    /// reaches session serialization, which wants the name the user would
    /// recognise as theirs.
    pub(super) fn plugin_tab_names(&self) -> HashMap<usize, String> {
        // Positions, because the `#1`/`#2` a collision earns are read off the tab
        // bar left to right. Ids would number by age instead, which is invisible.
        let mut by_position: Vec<&Tab> = self.tabs.values().collect();
        by_position.sort_by_key(|tab| tab.position);

        let mut project_counts: HashMap<&str, usize> = HashMap::new();
        for tab in &by_position {
            if let Some(project) = self.tab_project_name(tab) {
                *project_counts.entry(project).or_default() += 1;
            }
        }

        let mut seen: HashMap<&str, usize> = HashMap::new();
        let mut names = HashMap::new();
        for tab in by_position {
            let name = match self.tab_project_name(tab) {
                // A project only one tab is on reads better without a "#1" that
                // implies a "#2" somewhere. Two or more, and every one of them is
                // numbered — a bare name beside a numbered one would look like the
                // original and its copies.
                Some(project) if project_counts.get(project).copied().unwrap_or(0) > 1 => {
                    let ordinal = seen.entry(project).or_default();
                    *ordinal += 1;
                    format!("{} #{}", project, ordinal)
                },
                Some(project) => project.to_owned(),
                None => tab
                    .single_pane_tab_name()
                    .unwrap_or_else(|| tab.name.clone()),
            };
            names.insert(tab.id, name);
        }
        names
    }

    /// The project name a tab shows, before any `#1`/`#2` is worked out.
    ///
    /// `None` for a tab that has something better to show. A name the user typed
    /// is never spoken over, and neither is a title a lone pane is lending: the
    /// pane's title is the more specific answer to "what is in this tab", and it
    /// changes as the work does, where the project is the same all session.
    fn tab_project_name(&self, tab: &Tab) -> Option<&str> {
        if !tab.tab_name_is_default() {
            return None;
        }
        // `single_pane_tab_name` answers with the tab's own name when a lone pane
        // has no title to lend, so what makes a pane win is having *changed* it.
        if tab
            .single_pane_tab_name()
            .is_some_and(|borrowed| borrowed != tab.name)
        {
            return None;
        }
        self.tab_projects.get(&tab.id).map(String::as_str)
    }

    pub(super) fn update_plugin_tab_names(&mut self) -> bool {
        let current = self.plugin_tab_names();
        if current != self.last_plugin_tab_names {
            self.last_plugin_tab_names = current;
            true
        } else {
            false
        }
    }
    pub(crate) fn convert_pane_metadata(
        pane_manifest: &PaneManifest,
        tab_infos: &[TabInfo],
    ) -> Vec<zellij_utils::session_recording::PaneMetadata> {
        use zellij_utils::session_recording::PaneMetadata;
        let mut result = Vec::new();
        // Build a map from tab position to TabInfo for quick lookup
        let tab_by_position: std::collections::HashMap<usize, &TabInfo> =
            tab_infos.iter().map(|t| (t.position, t)).collect();
        for (tab_position_key, pane_infos) in &pane_manifest.panes {
            if let Some(tab_info) = tab_by_position.get(tab_position_key) {
                for pane in pane_infos {
                    if pane.is_plugin {
                        continue;
                    }
                    result.push(PaneMetadata {
                        terminal_id: pane.id,
                        tab_id: tab_info.tab_id,
                        tab_position: tab_info.position,
                        tab_name: tab_info.name.clone(),
                        title: pane.title.clone(),
                        cols: pane.pane_content_columns,
                        rows: pane.pane_content_rows,
                    });
                }
            }
        }
        // Global terminal panes are not tied to a tab. Use a distinct
        // `tab_position` (9999) and `tab_name` "global" so they sort after
        // normal tabs (`ORDER BY tab_position ASC`) and do not collide with
        // tab 0. `tab_id` stays 0 as it is not meaningful for globals.
        // See REVIEW_CHECKLIST.md 2.12.
        for pane in &pane_manifest.global_panes {
            if pane.is_plugin {
                continue;
            }
            result.push(PaneMetadata {
                terminal_id: pane.id,
                tab_id: 0,
                tab_position: 9999,
                tab_name: "global".to_string(),
                title: pane.title.clone(),
                cols: pane.pane_content_columns,
                rows: pane.pane_content_rows,
            });
        }
        result
    }

    pub(super) fn log_and_report_session_state(&mut self) -> Result<()> {
        let err_context = || "Failed to log and report session state".to_string();

        self.update_active_pane_ids();
        self.revert_fit_disabled_without_reference_client()
            .with_context(err_context)?;
        self.reconcile_all_single_pane_focus();
        // generate own session info
        let pane_manifest = self.generate_and_report_pane_state()?;
        let tab_infos = self.generate_and_report_tab_state()?;
        // send pane metadata to session recorder if enabled
        {
            let metadata = Self::convert_pane_metadata(&pane_manifest, &tab_infos);
            if let Some(recorder) = &self.bus.senders.session_recorder {
                recorder.pane_metadata(metadata);
            }
        }

        // Lazy-load layouts on first call if cache is empty
        // After that, cache is updated by watcher via UpdateAvailableLayouts instruction
        if self.cached_layouts.is_empty() {
            #[cfg(not(test))]
            {
                let (layouts, errors) = Layout::list_available_layouts(
                    self.layout_dir.clone(),
                    &self.default_layout_name,
                );
                self.cached_layouts = layouts;
                self.cached_layout_errors = errors;
            }
            #[cfg(test)]
            {
                self.cached_layouts = vec![];
                self.cached_layout_errors = vec![];
            }
        }
        let available_layouts = self.cached_layouts.clone();
        let creation_time = {
            let sock_path = ZELLIJ_SOCK_DIR.join(&self.session_name);
            std::fs::metadata(&sock_path)
                .ok()
                .and_then(|f| f.created().ok().or_else(|| f.modified().ok()))
                .and_then(|d| d.elapsed().ok())
                .map(|d| Duration::from_secs(d.as_secs()))
                .unwrap_or_default()
        };
        let session_info = SessionInfo {
            name: self.session_name.clone(),
            tabs: tab_infos,
            panes: pane_manifest,
            connected_clients: self.active_tab_ids.keys().len(),
            is_current_session: true,
            available_layouts,
            web_clients_allowed: self.web_sharing.web_clients_allowed(),
            web_client_count: self
                .connected_clients
                .borrow()
                .iter()
                .filter(|(_client_id, is_web_client)| **is_web_client)
                .count(),
            plugins: Default::default(), // these are filled in by the wasm thread
            tab_history: self.tab_history.clone(),
            pane_history: self
                .pane_history
                .iter()
                .map(|(k, v)| (*k, v.iter().map(|v| (*v).into()).collect()))
                .collect(),
            creation_time,
        };
        self.bus
            .senders
            .send_to_background_jobs(BackgroundJob::ReportSessionInfo(
                self.session_name.to_owned(),
                session_info.clone(),
            ))
            .with_context(err_context)?;

        self.peer_sessions_cache
            .insert(self.session_name.clone(), session_info);
        let mut live_sessions: Vec<SessionInfo> =
            self.peer_sessions_cache.values().cloned().collect();
        for info in live_sessions.iter_mut() {
            info.is_current_session = info.name == self.session_name;
        }
        let resurrectable_sessions: Vec<(String, Duration)> = self
            .resurrectable_sessions_cache
            .iter()
            .map(|(n, d)| (n.clone(), *d))
            .collect();
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                Event::SessionUpdate(live_sessions, resurrectable_sessions),
            )]))
            .with_context(err_context)?;

        self.bus
            .senders
            .send_to_background_jobs(BackgroundJob::QueryZellijWebServerStatus)
            .with_context(err_context)?;

        self.report_mobile_state();
        Ok(())
    }
    fn report_mobile_state(&mut self) {
        let web_client_ids: Vec<ClientId> = self
            .connected_clients
            .borrow()
            .iter()
            .filter(|(_client_id, is_web_client)| **is_web_client)
            .map(|(client_id, _)| *client_id)
            .collect();
        if web_client_ids.is_empty() {
            self.last_mobile_state_sent
                .retain(|client_id, _| self.connected_clients.borrow().contains_key(client_id));
            return;
        }

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let now = Instant::now();
        let mut tabs: Vec<MobileTabPayload> = Vec::new();
        let mut panes: Vec<MobilePanePayload> = Vec::new();
        for tab in self.tabs.values() {
            tabs.push(MobileTabPayload {
                position: tab.position,
                name: tab.name.clone(),
                active: self.active_tab_ids.values().any(|i| i == &tab.id),
            });
            let activity = tab.pane_last_activity();
            for pane_info in tab.pane_infos() {
                if pane_info.is_suppressed || !pane_info.is_selectable {
                    continue;
                }
                let pane_id = if pane_info.is_plugin {
                    PaneId::Plugin(pane_info.id)
                } else {
                    PaneId::Terminal(pane_info.id)
                };
                let last_activity_secs_ago = self
                    .pane_output_activity
                    .get(&pane_id)
                    .map(|instant| now.saturating_duration_since(*instant).as_secs())
                    .or_else(|| activity.get(&pane_id).copied())
                    .unwrap_or_default();
                panes.push(MobilePanePayload {
                    tab_position: tab.position,
                    pane_id: pane_info.id,
                    is_plugin: pane_info.is_plugin,
                    title: pane_info.title.clone(),
                    is_floating: pane_info.is_floating,
                    last_activity_secs_ago,
                });
            }
        }
        tabs.sort_by_key(|t| t.position);

        {
            let live_pane_ids: HashSet<PaneId> = self
                .tabs
                .values()
                .flat_map(|tab| tab.get_all_pane_ids())
                .collect();
            self.pane_output_activity
                .retain(|pane_id, _| live_pane_ids.contains(pane_id));
        }

        let sessions: Vec<MobileSessionPayload> = self
            .peer_sessions_cache
            .values()
            .map(|info| MobileSessionPayload {
                name: info.name.clone(),
                web_clients_allowed: info.web_clients_allowed,
                tab_count: info.tabs.len(),
                pane_count: info.panes.panes.values().map(|p| p.len()).sum(),
                connected_clients: info.connected_clients,
                creation_secs_ago: info.creation_time.as_secs(),
            })
            .collect();

        for client_id in web_client_ids {
            let active_pane = self
                .get_active_tab(client_id)
                .ok()
                .and_then(|tab| {
                    tab.get_active_pane_id_or_first_selectable(client_id)
                        .map(|pane_id| (tab.position, pane_id))
                })
                .map(|(tab_position, pane_id)| {
                    let (id, is_plugin) = match pane_id {
                        PaneId::Terminal(id) => (id, false),
                        PaneId::Plugin(id) => (id, true),
                    };
                    MobileActivePanePayload {
                        pane_id: id,
                        is_plugin,
                        tab_position,
                    }
                });

            let is_welcome_screen = active_pane
                .as_ref()
                .map(|ap| {
                    ap.is_plugin
                        && panes
                            .iter()
                            .find(|p| p.pane_id == ap.pane_id && p.is_plugin)
                            .map(|p| p.title == "welcome-screen")
                            .unwrap_or(false)
                })
                .unwrap_or(false);

            let prefs = self.mobile_web_prefs.get(&client_id).copied();
            let active_pane_is_fullscreen = self
                .get_active_tab(client_id)
                .ok()
                .map(|tab| tab.is_fullscreen_active())
                .unwrap_or(false);
            // Defaults must describe what the server has actually applied: a client that never
            // sent preferences has nothing fullscreened, and participates in the tab min-size
            // rule. Reporting an unapplied `single_pane` desynchronises the browser, which then
            // asserts the stale value on its next unrelated preference change.
            let render_prefs = zellij_utils::ipc::MobileRenderPrefsPayload {
                single_pane: prefs.map(|p| p.single_pane).unwrap_or(false),
                fit: prefs.map(|p| p.fit).unwrap_or(true),
                active_pane_is_fullscreen,
            };

            let desktop_client_connected = self.has_reference_client(client_id);
            let desktop_size =
                self.reference_size_for_client(client_id)
                    .map(|s| MobileSizePayload {
                        cols: s.cols,
                        rows: s.rows,
                    });

            let payload = MobileStatePayload {
                session_name: self.session_name.clone(),
                now_secs,
                is_welcome_screen,
                desktop_client_connected,
                desktop_size,
                active_pane,
                tabs: tabs.clone(),
                panes: panes.clone(),
                sessions: sessions.clone(),
                render_prefs,
            };

            let mut comparable = payload.clone();
            comparable.now_secs = 0;
            for pane in comparable.panes.iter_mut() {
                pane.last_activity_secs_ago = 0;
            }
            if self.last_mobile_state_sent.get(&client_id) == Some(&comparable) {
                continue;
            }

            if let Some(os_input) = &self.bus.os_input {
                let _ =
                    os_input.send_to_client(client_id, ServerToClientMsg::MobileState { payload });
            }
            self.last_mobile_state_sent.insert(client_id, comparable);
        }

        let connected = self.connected_clients.borrow();
        self.last_mobile_state_sent
            .retain(|client_id, _| connected.contains_key(client_id));
    }
    pub(super) fn dump_layout_to_hd(&mut self) -> Result<()> {
        let err_context = || "Failed to log and report session state".to_string();
        let session_layout_metadata =
            self.get_layout_metadata(Some(self.default_shell.clone()), None);
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::LogLayoutToHd(session_layout_metadata))
            .with_context(err_context)?;

        Ok(())
    }
    pub fn update_session_infos(
        &mut self,
        new_session_infos: BTreeMap<String, SessionInfo>,
        resurrectable_sessions: BTreeMap<String, Duration>,
    ) -> Result<()> {
        self.peer_sessions_cache = new_session_infos;
        self.resurrectable_sessions_cache = resurrectable_sessions;
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                Event::SessionUpdate(
                    self.peer_sessions_cache.values().cloned().collect(),
                    self.resurrectable_sessions_cache
                        .iter()
                        .map(|(n, c)| (n.clone(), *c))
                        .collect(),
                ),
            )]))
            .context("failed to update session info")?;
        self.report_mobile_state();
        Ok(())
    }

    pub fn update_available_layouts(
        &mut self,
        layouts: Vec<LayoutInfo>,
        errors: Vec<LayoutWithError>,
    ) {
        self.cached_layouts = layouts;
        self.cached_layout_errors = errors;
    }
}

use crate::TerminalId;
use unicode_width::UnicodeWidthStr;
use zellij_runtime::errors::prelude::*;
use zellij_tile_utils::ellipsize;

use super::Screen;
use crate::background_jobs::BackgroundJob;
use crate::plugins::{PluginInstruction, CLI_EVENTS_ONLY_TARGET};
use crate::tab::Tab;
use crate::ClientId;
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;
use zellij_types::command::PaneTarget;
use zellij_utils::consts::ZELLIJ_SOCK_DIR;
use zellij_utils::data::{
    Event, EventType, LayoutInfo, LayoutWithError, ListPanesResponse, ListTabsResponse, PaneInfo,
    PaneListEntry, PaneManifest, SessionInfo, TabInfo,
};
#[cfg(not(test))]
use zellij_utils::input::layout::Layout;

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
                other_focused_clients: all_focused_clients.into_iter().collect(),
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
                    other_focused_clients: other_focused_clients.into_iter().collect(),
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
            if self.cli_event_subscribers_present && target_plugin_ids.is_empty() {
                plugin_updates.push((
                    Some(CLI_EVENTS_ONLY_TARGET),
                    None,
                    Event::TabUpdate(plugin_tab_updates.clone()),
                ));
            }
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
        // A plugin pane is not a process in a folder, so it is left without one.
        for pane_info in pane_manifest
            .panes
            .values_mut()
            .flatten()
            .filter(|pane_info| !pane_info.is_plugin)
        {
            pane_info.cwd = self
                .terminal_cwds
                .get(&TerminalId(pane_info.id))
                .map(|cwd| cwd.to_string_lossy().into_owned());
            pane_info.host = self.terminal_hosts.get(&TerminalId(pane_info.id)).cloned();
            // The worktree the pane's VM actually mirrors, for a pane that named none of
            // its own: `pane_info.target` is what the pane was *asked* for, and a pane
            // that joined an existing VM asked for a VM by name and nothing else. Only
            // filled in, never overridden - a pane that named a worktree was ensured with
            // that one.
            if let (
                Some(worktree),
                PaneTarget::Firecracker {
                    worktree: named, ..
                },
            ) = (
                self.terminal_firecracker_worktrees
                    .get(&TerminalId(pane_info.id)),
                &mut pane_info.target,
            ) {
                if named.is_none() {
                    *named = Some(worktree.clone());
                }
            }
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
        // No plugin wants the manifest but a `zellij events` client does: address it to no
        // plugin, and the plugin thread forwards it without loading anything.
        if self.cli_event_subscribers_present && plugin_updates.is_empty() {
            plugin_updates.push((
                Some(CLI_EVENTS_ONLY_TARGET),
                None,
                Event::PaneUpdate(pane_manifest.clone()),
            ));
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
    /// 1. A lone pane that lends its own title (see [`Tab::lent_pane_title`]).
    /// 2. The project the tab is working on, when a plugin has told us of one.
    /// 3. The tab's own name — what the user typed, or the "Tab #N" it was born with.
    ///
    /// Only the third is stored on the tab. The other two are worked out afresh
    /// each time, so a tab the user names keeps that name, and a tab goes back to
    /// its own the moment the thing that was lending it one is gone. Nothing here
    /// reaches session serialization, which wants the name the user would
    /// recognise as theirs.
    ///
    /// Whichever of the three wins then carries the exit marker of a lone pane
    /// that has stopped ([`Tab::exit_marker`]). That is not a fourth name and does
    /// not compete with the others: it says the pane is finished, which is worth
    /// saying about a tab called "hocus" as much as about one called "vim README".
    ///
    /// A tab down to one pane leads whichever name won with that pane's id
    /// ([`Tab::lone_pane_id_marker`]). Same story as the marker: not a name, just
    /// the label the pane's own frame wears, said where the eye already is.
    ///
    /// The decorations are charged to [`Tab::tab_name_budget`] and the winning name is cut
    /// to what is left, so the finished string fits the cap the bars apply anyway
    /// (`MAX_TAB_NAME_WIDTH`) and they have nothing to take off the end - which is where
    /// the exit marker lives, and the part a reader could not put back.
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
            // The "#1" is a decoration too, not part of the name: charged to the
            // budget rather than truncated away, or two tabs on a long shared
            // project would both end in an ellipsis and neither would say which
            // is which.
            let (base, suffix) = match self.tab_project_name(tab) {
                // A project only one tab is on reads better without a "#1" that
                // implies a "#2" somewhere. Two or more, and every one of them is
                // numbered — a bare name beside a numbered one would look like the
                // original and its copies.
                Some(project) if project_counts.get(project).copied().unwrap_or(0) > 1 => {
                    let ordinal = seen.entry(project).or_default();
                    *ordinal += 1;
                    (project.to_owned(), format!(" #{}", ordinal))
                },
                Some(project) => (project.to_owned(), String::new()),
                None => (
                    tab.lent_pane_title().unwrap_or_else(|| tab.name.clone()),
                    String::new(),
                ),
            };
            // The decorations go on whichever of the three won, and the name is cut
            // to what they leave. They say the pane has stopped and which pane it
            // is — as worth saying about a tab wearing a project's name as about one
            // wearing the title that pane lent it, and neither reconstructable from
            // what a bar would leave if it did the cutting itself.
            let budget = tab.tab_name_budget().saturating_sub(suffix.width());
            names.insert(
                tab.id,
                format!(
                    "{}{}{}{}",
                    tab.lone_pane_id_marker(),
                    ellipsize(&base, budget),
                    suffix,
                    tab.exit_marker()
                ),
            );
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
        // Only an actual loan outranks the project. A lone pane that set no title
        // of its own lends nothing, and the exit marker it may still be carrying
        // is not a name — it goes on whatever name wins, in `plugin_tab_names`.
        if tab.lent_pane_title().is_some() {
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
    ) -> Vec<zellij_storage::session_recording::PaneMetadata> {
        use zellij_storage::session_recording::PaneMetadata;
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
                        terminal_id: zellij_types::internal_ids::TerminalId::new(pane.id),
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
        result
    }

    /// The pane half of `log_and_report_session_state`, without rebuilding tab state.
    ///
    /// An agent record lives on a pane, so a changed record does not need a
    /// `TabUpdate`. It does need to update `SessionInfo`, which embeds the same
    /// pane manifest and is read by the session list and `SessionUpdate`
    /// subscribers.
    pub(super) fn report_pane_state(&mut self) -> Result<()> {
        self.update_active_pane_ids();
        let pane_manifest = self.generate_and_report_pane_state()?;
        let mut session_info = self
            .peer_sessions_cache
            .get(&self.session_name)
            .cloned()
            .context("current session missing from peer sessions cache")?;
        session_info.panes = pane_manifest;
        self.report_session_info(session_info)
    }

    pub(super) fn log_and_report_session_state(&mut self) -> Result<()> {
        self.update_active_pane_ids();
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
            plugins: Default::default(), // these are filled in by the wasm thread
            tab_history: self
                .tab_history
                .iter()
                .map(|(id, history)| (*id, history.clone()))
                .collect(),
            pane_history: self
                .pane_history
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect(),
            creation_time,
        };
        self.report_session_info(session_info)
    }

    fn report_session_info(&mut self, session_info: SessionInfo) -> Result<()> {
        let err_context = || "Failed to report session info".to_string();

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

        Ok(())
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

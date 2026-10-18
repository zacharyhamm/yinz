//! Opening plugin panes: the instructions that launch a plugin, focus one that is already
//! open, or place a loaded plugin's pane.
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;

use super::thread_main::PendingEvents;
use super::{
    active_tab_and_connected_client_id, replay_pending_plugin_events, Screen, ScreenInstruction,
};
use crate::panes::PaneId;
use crate::plugins::PluginId;
use crate::plugins::PluginInstruction;
use crate::pty::{ClientTabIndexOrPaneId, PtyInstruction};
use crate::route::NotificationEnd;
use crate::tab::Tab;
use crate::ClientId;
use std::path::PathBuf;
use yinz_types::data::{NewPanePlacement, OverlayPlacement};
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::input::layout::{Run, RunPluginOrAlias};
use yinz_types::pane_size::Size;
use yinz_types::pane_tabs::{PaneTabsId, PaneTabsLayout};

pub(super) struct PendingOverlayTabs {
    id: PaneTabsId,
    layout: PaneTabsLayout,
    first: usize,
    skip_cache: bool,
    first_client: ClientId,
    focus_client: ClientId,
    tab: TabId,
    launches: Vec<(usize, ClientId, Option<NotificationEnd>)>,
}

impl Screen {
    /// The tab a plugin launched by id, rather than for a client, goes into: the one named,
    /// else the first active one. Reports a non-fatal error when there is none.
    fn tab_for_plugin_launch(&self, explicit_tab_id: Option<TabId>) -> Option<TabId> {
        let tab_index = explicit_tab_id.or_else(|| self.active_tab_ids.values().next().copied());
        if tab_index.is_none() {
            Err::<(), _>(anyhow!(
                "Could not find an active tab - is there at least 1 connected user?"
            ))
            .non_fatal();
        }
        tab_index
    }

    /// Asks the pty thread to resolve `run_plugin`'s cwd and load it into `tab_index`, replacing
    /// `pane_id_to_replace` when one is named. The pty thread answers with `AddPlugin`.
    fn fill_plugin_cwd(
        &self,
        should_float: bool,
        should_open_in_place: bool,
        close_replaced_pane: bool,
        run_plugin: RunPluginOrAlias,
        tab_index: TabId,
        pane_id_to_replace: Option<PaneId>,
        client_id: ClientId,
        skip_cache: bool,
        cwd: Option<PathBuf>,
        no_focus: bool,
        completion_tx: Option<NotificationEnd>,
        overlay_placement: Option<OverlayPlacement>,
    ) -> Result<()> {
        self.bus.senders.send_to_pty(PtyInstruction::FillPluginCwd(
            Some(should_float),
            should_open_in_place,
            close_replaced_pane,
            None,
            run_plugin,
            tab_index,
            pane_id_to_replace,
            client_id,
            Size::default(),
            skip_cache,
            cwd,
            if no_focus { Some(false) } else { None },
            None,
            completion_tx,
            overlay_placement,
        ))
    }

    /// Asks the pty thread to resolve a cwd for a plugin and load it, replacing `pane_id_to_replace`
    /// when one is named and otherwise opening beside the client's active pane.
    pub(super) fn launch_plugin(
        &mut self,
        mut run_plugin: RunPluginOrAlias,
        should_float: bool,
        should_open_in_place: bool,
        close_replaced_pane: bool,
        pane_id_to_replace: Option<PaneId>,
        skip_cache: bool,
        cwd: Option<PathBuf>,
        client_id: ClientId,
        mut completion_tx: Option<NotificationEnd>,
        explicit_tab_id: Option<TabId>,
        no_focus: bool,
        overlay_placement: Option<OverlayPlacement>,
    ) -> Result<()> {
        yinz_runtime::plugin_location::refresh_file_identity(&mut run_plugin);
        let (tab_index, client_id) = if should_open_in_place && pane_id_to_replace.is_some() {
            match self.tab_for_plugin_launch(explicit_tab_id) {
                Some(tab_index) => (tab_index, client_id),
                None => return Ok(()),
            }
        } else {
            match self.resolve_tab_and_client(client_id, explicit_tab_id) {
                Some(resolved) => resolved,
                None => {
                    Err::<(), _>(anyhow!(
                        "No connected clients found - cannot load or focus plugin"
                    ))
                    .non_fatal();
                    return Ok(());
                },
            }
        };
        if !should_open_in_place
            && explicit_tab_id.is_none()
            && !no_focus
            && self.launch_overlay_tab(
                &run_plugin,
                cwd.clone(),
                skip_cache,
                client_id,
                &mut completion_tx,
            )?
        {
            return Ok(());
        }
        self.fill_plugin_cwd(
            should_float,
            should_open_in_place,
            close_replaced_pane,
            run_plugin,
            tab_index,
            pane_id_to_replace,
            client_id,
            skip_cache,
            cwd,
            no_focus,
            completion_tx,
            overlay_placement,
        )
    }

    /// Focuses the plugin's pane if one is already open, otherwise asks the pty thread to
    /// resolve a cwd for it and load it. Replacing a pane in place skips the focus attempt.
    pub(super) fn launch_or_focus_plugin(
        &mut self,
        mut run_plugin: RunPluginOrAlias,
        should_float: bool,
        move_to_focused_tab: bool,
        should_open_in_place: bool,
        close_replaced_pane: bool,
        pane_id_to_replace: Option<PaneId>,
        skip_cache: bool,
        client_id: ClientId,
        mut completion_tx: Option<NotificationEnd>,
        explicit_tab_id: Option<TabId>,
    ) -> Result<()> {
        yinz_runtime::plugin_location::refresh_file_identity(&mut run_plugin);
        match pane_id_to_replace {
            Some(pane_id_to_replace) if should_open_in_place => {
                if let Some(tab_index) = self.tab_for_plugin_launch(explicit_tab_id) {
                    self.fill_plugin_cwd(
                        should_float,
                        should_open_in_place,
                        close_replaced_pane,
                        run_plugin,
                        tab_index,
                        Some(pane_id_to_replace),
                        client_id,
                        skip_cache,
                        None,
                        false,
                        completion_tx,
                        None,
                    )?;
                }
            },
            _ => {
                let Some((tab_index, client_id)) =
                    self.resolve_tab_and_client(client_id, explicit_tab_id)
                else {
                    Err::<(), _>(anyhow!(
                        "No connected clients found - cannot load or focus plugin"
                    ))
                    .non_fatal();
                    return Ok(());
                };
                if !should_open_in_place
                    && explicit_tab_id.is_none()
                    && self.launch_overlay_tab(
                        &run_plugin,
                        None,
                        skip_cache,
                        client_id,
                        &mut completion_tx,
                    )?
                {
                    return Ok(());
                }
                let found_plugin = match self.focus_plugin_pane(
                    &run_plugin,
                    should_float,
                    move_to_focused_tab,
                    should_open_in_place,
                    client_id,
                    &mut completion_tx,
                ) {
                    Ok(found) => found,
                    Err(error) => {
                        if let Some(completion) = completion_tx.as_mut() {
                            completion.set_exit_status(1);
                            completion.set_error_message(format!("{error:#}"));
                        }
                        Err::<(), _>(error).non_fatal();
                        return Ok(());
                    },
                };
                if found_plugin {
                    self.render()?;
                    self.log_and_report_session_state()?;
                } else {
                    self.fill_plugin_cwd(
                        should_float,
                        should_open_in_place,
                        close_replaced_pane,
                        run_plugin,
                        tab_index,
                        None,
                        client_id,
                        skip_cache,
                        None,
                        false,
                        completion_tx,
                        None,
                    )?;
                }
            },
        }
        Ok(())
    }

    /// A plugin has loaded: work out where its pane goes (overlay, floating, tiled or in place of
    /// another pane) and open it there, then replay whatever was waiting for that pane.
    ///
    /// Returns `false` when there is no client and no tab to place it in yet; the instruction is
    /// parked in `pending` and the caller must not report.
    pub(super) fn add_plugin(
        &mut self,
        should_float: Option<bool>,
        should_be_in_place: bool,
        close_replaced_pane: bool,
        run_plugin_or_alias: RunPluginOrAlias,
        pane_title: Option<String>,
        tab_index: Option<TabId>,
        plugin_id: PluginId,
        pane_id_to_replace: Option<PaneId>,
        cwd: Option<PathBuf>,
        start_suppressed: bool,
        floating_pane_coordinates: Option<FloatingPaneCoordinates>,
        should_focus_plugin: Option<bool>,
        client_id: Option<ClientId>,
        mut completion_tx: Option<NotificationEnd>,
        stated_overlay_placement: Option<OverlayPlacement>,
        pending: &mut PendingEvents,
    ) -> Result<bool> {
        let mut new_pane_placement = NewPanePlacement::default();
        let maybe_should_float = should_float;
        let should_float = maybe_should_float.unwrap_or(false);
        // Two spellings of "this is an overlay" meet here. A launcher that states an
        // `OverlayPlacement` outright wins; the plugin's own `overlay`/`modal`
        // configuration is the compatibility shim for everything written before
        // placements existed, and this is the one place it is read. Coordinates the
        // caller passed are kept either way - they say *where* on the surface, not
        // *whether*.
        let overlay_placement = stated_overlay_placement
            .clone()
            .or_else(|| run_plugin_or_alias.overlay_placement())
            .map(|placement| OverlayPlacement {
                coordinates: floating_pane_coordinates
                    .clone()
                    .or(placement.coordinates.clone()),
                ..placement
            });
        let floating_pane_coordinates = floating_pane_coordinates.or_else(|| {
            overlay_placement
                .as_ref()
                .map(|placement| placement.as_floating_pane_coordinates())
        });
        let should_be_tiled =
            maybe_should_float.map(|f| !f).unwrap_or(false) && floating_pane_coordinates.is_none();
        if let Some(overlay_placement) = overlay_placement {
            new_pane_placement = NewPanePlacement::Overlay(overlay_placement);
        } else if floating_pane_coordinates.is_some() || should_float {
            new_pane_placement =
                NewPanePlacement::with_floating_pane_coordinates(floating_pane_coordinates.clone());
        }
        if should_be_tiled {
            new_pane_placement = NewPanePlacement::Tiled {
                direction: None,
                borderless: None,
            };
        }
        if should_be_in_place {
            new_pane_placement =
                NewPanePlacement::with_pane_id_to_replace(pane_id_to_replace, close_replaced_pane);
        }
        if self.active_tab_ids.is_empty() && tab_index.is_none() {
            pending
                .waiting_for_client
                .push(ScreenInstruction::AddPlugin(
                    maybe_should_float,
                    should_be_in_place,
                    close_replaced_pane,
                    run_plugin_or_alias,
                    pane_title,
                    tab_index,
                    plugin_id,
                    pane_id_to_replace,
                    cwd,
                    start_suppressed,
                    floating_pane_coordinates,
                    should_focus_plugin,
                    client_id,
                    completion_tx,
                    stated_overlay_placement,
                ));
            return Ok(false);
        }
        let pane_title = pane_title.unwrap_or_else(|| {
            format!(
                "({}) - {}",
                cwd.map(|cwd| cwd.display().to_string())
                    .unwrap_or(".".to_owned()),
                run_plugin_or_alias.location_string()
            )
        });
        let run_plugin = Run::Plugin(run_plugin_or_alias);

        // Set affected pane ID for CLI client output
        if let Some(ref mut completion) = completion_tx {
            completion.set_affected_pane_id(PaneId::Plugin(plugin_id));
        }

        if should_be_in_place {
            if let Some(pane_id_to_replace) = pane_id_to_replace {
                let client_tab_index_or_pane_id =
                    ClientTabIndexOrPaneId::PaneId(pane_id_to_replace);
                self.replace_pane(
                    PaneId::Plugin(plugin_id),
                    None,
                    Some(run_plugin),
                    Some(pane_title),
                    close_replaced_pane,
                    client_tab_index_or_pane_id,
                )?;
            } else if let Some(client_id) = client_id {
                let client_tab_index_or_pane_id = ClientTabIndexOrPaneId::ClientId(client_id);
                self.replace_pane(
                    PaneId::Plugin(plugin_id),
                    None,
                    Some(run_plugin),
                    Some(pane_title),
                    close_replaced_pane,
                    client_tab_index_or_pane_id,
                )?;
            } else {
                Err::<(), _>(anyhow!(
                    "Must have pane id to replace or connected client_id if replacing a pane"
                ))
                .non_fatal();
            }
        } else if let Some(active_tab) =
            tab_index.and_then(|tab_index| self.tabs.get_mut(&tab_index))
        {
            // An explicit tab remains usable after its CLI client disconnects.
            active_tab.new_pane(
                PaneId::Plugin(plugin_id),
                Some(pane_title),
                Some(run_plugin),
                start_suppressed,
                should_focus_plugin.unwrap_or(true),
                new_pane_placement,
                client_id,
                None,
            )?;
        } else if let Some(client_id) = client_id {
            active_tab_and_connected_client_id!(self, client_id, |active_tab: &mut Tab, _client_id: ClientId| {
            active_tab.new_pane(
                PaneId::Plugin(plugin_id),
                Some(pane_title),
                Some(run_plugin),
                start_suppressed,
                should_focus_plugin.unwrap_or(true),
                new_pane_placement,
                Some(client_id),
                None,
            )
        }, ?);
        } else {
            Err::<(), _>(anyhow!("Tab index not found: {:?}", tab_index)).non_fatal();
        }
        let pane_id = PaneId::Plugin(plugin_id);
        let plugin_exists = self.tabs.values().any(|tab| tab.has_plugin(plugin_id));
        if plugin_exists {
            if let Some(pending_events) = pending.waiting_for_pane.remove(&pane_id) {
                replay_pending_plugin_events(self, pending_events)?;
            }
        }
        Ok(true)
    }

    /// Asks the pty thread to resolve a cwd for, and then load, a plugin that opens floating
    /// (or as the overlay `placement` names) in the given tab or the first active one.
    pub(super) fn new_floating_plugin_pane(
        &mut self,
        run_plugin: RunPluginOrAlias,
        pane_title: Option<String>,
        skip_cache: bool,
        cwd: Option<PathBuf>,
        placement: NewPanePlacement,
        client_id: ClientId,
        mut completion_tx: Option<NotificationEnd>,
        explicit_tab_id: Option<TabId>,
        no_focus: bool,
    ) -> Result<()> {
        if explicit_tab_id.is_none()
            && !no_focus
            && self.launch_overlay_tab(
                &run_plugin,
                cwd.clone(),
                skip_cache,
                client_id,
                &mut completion_tx,
            )?
        {
            return Ok(());
        }
        let Some(tab_index) = self.tab_for_plugin_launch(explicit_tab_id) else {
            return Ok(());
        };
        let size = Size::default();
        let should_float = Some(true);
        let should_be_opened_in_place = false;
        let should_focus_plugin = if no_focus { Some(false) } else { None };
        let overlay_placement = match &placement {
            NewPanePlacement::Overlay(overlay) => Some(overlay.clone()),
            _ => None,
        };
        self.bus.senders.send_to_pty(PtyInstruction::FillPluginCwd(
            should_float,
            should_be_opened_in_place,
            false, // close_replaced_pane
            pane_title,
            run_plugin,
            tab_index,
            None,
            client_id,
            size,
            skip_cache,
            cwd,
            should_focus_plugin,
            placement.floating_pane_coordinates(),
            completion_tx,
            overlay_placement,
        ))?;
        Ok(())
    }
}

impl Screen {
    pub(super) fn launch_overlay_tab(
        &mut self,
        requested: &RunPluginOrAlias,
        cwd: Option<PathBuf>,
        skip_cache: bool,
        client: ClientId,
        completion: &mut Option<NotificationEnd>,
    ) -> Result<bool> {
        let configuration = match requested {
            RunPluginOrAlias::RunPlugin(run) => Some(run.configuration.inner().clone()),
            RunPluginOrAlias::Alias(alias) => {
                alias.configuration.as_ref().map(|c| c.inner().clone())
            },
        };
        let explicit_cwd = match requested {
            RunPluginOrAlias::RunPlugin(run) => run.initial_cwd.clone(),
            RunPluginOrAlias::Alias(alias) => alias.initial_cwd.clone(),
        };
        let mut requested = requested.clone();
        requested.populate_run_plugin_if_needed(&self.plugin_aliases);
        let Some(run) = requested.get_run_plugin() else {
            return Ok(false);
        };
        let matches_location = |location: &str| {
            yinz_runtime::plugin_location::parse_plugin_location(location, None)
                .is_ok_and(|location| location == run.location)
        };
        let existing = self.tabs.iter().find_map(|(tab, value)| {
            value
                .pane_tabs_infos()
                .find(|info| info.overlay_group)
                .map(|info| (*tab, info))
        });
        if let Some((source, info)) = existing {
            let Some(index) = info
                .tabs
                .iter()
                .position(|slot| matches_location(&slot.descriptor.location))
            else {
                // A reload cannot change the descriptors of a running group.
                return Ok(false);
            };
            let Some((destination, client)) = self.resolve_tab_and_client(client, None) else {
                return Ok(false);
            };
            if source != destination {
                self.break_multiple_panes_to_tab_with_id(
                    vec![info.pane_id],
                    destination,
                    false,
                    client,
                )?;
            }
            let tab = self
                .tabs
                .get_mut(&destination)
                .context("overlay destination disappeared")?;
            tab.select_pane_tab(info.id, index, client)?;
            tab.focus_pane_tabs(info.id, false, client)?;
            if let Some(mut completion) = completion.take() {
                if let Some(plugin) = tab
                    .pane_tabs_info(info.id)
                    .context("overlay disappeared")?
                    .tabs[index]
                    .plugin_id
                {
                    completion.set_affected_pane_id(PaneId::Plugin(plugin));
                } else {
                    self.overlay_tab_completions
                        .push((info.id, index, completion));
                }
            }
            self.log_and_report_session_state()?;
            self.render()?;
            return Ok(true);
        }
        if let Some(pending) = self.pending_overlay_tabs.as_mut() {
            let Some(index) = pending
                .layout
                .tabs
                .iter()
                .position(|tab| matches_location(&tab.location))
            else {
                return Ok(false);
            };
            pending.layout.active = index;
            pending.focus_client = client;
            pending.launches.push((index, client, completion.take()));
            return Ok(true);
        }
        let Some(index) = self
            .overlay_tab_plugins
            .iter()
            .position(|tab| matches_location(&tab.location))
        else {
            return Ok(false);
        };
        let Some((tab, client)) = self.resolve_tab_and_client(client, None) else {
            return Ok(false);
        };
        let mut tabs = self.overlay_tab_plugins.clone();
        if let Some(configuration) = configuration {
            tabs[index].configuration.extend(configuration);
        }
        if explicit_cwd.is_some() {
            tabs[index].initial_cwd = explicit_cwd;
        }
        let cwd = cwd.or_else(|| {
            tabs[index]
                .initial_cwd
                .is_none()
                .then(|| requested.get_initial_cwd())
                .flatten()
        });
        let id = Tab::reserve_pane_tabs_id();
        self.bus
            .senders
            .send_to_pty(PtyInstruction::ResolveOverlayTabCwd {
                id,
                client_id: client,
                cwd,
            })
            .context("failed to resolve overlay tab working directory")?;
        self.pending_overlay_tabs = Some(PendingOverlayTabs {
            id,
            layout: PaneTabsLayout {
                tabs,
                active: index,
            },
            first: index,
            skip_cache,
            first_client: client,
            focus_client: client,
            tab,
            launches: vec![(index, client, completion.take())],
        });
        Ok(true)
    }

    pub(super) fn overlay_tab_cwd(
        &mut self,
        id: PaneTabsId,
        cwd: Option<PathBuf>,
        caller_cwd: Option<PathBuf>,
        caller_terminal_pane_id: Option<yinz_types::internal_ids::TerminalId>,
    ) -> Result<()> {
        let Some(pending) = self
            .pending_overlay_tabs
            .as_mut()
            .filter(|pending| pending.id == id)
        else {
            return Ok(());
        };
        for descriptor in &mut pending.layout.tabs {
            if let Some(cwd) = &cwd {
                descriptor.add_cwd(cwd);
            }
            if let Some(caller_cwd) = &caller_cwd {
                descriptor
                    .configuration
                    .entry("caller_cwd".into())
                    .or_insert_with(|| caller_cwd.display().to_string());
            }
            if let Some(pane_id) = caller_terminal_pane_id {
                descriptor
                    .configuration
                    .entry("caller_terminal_pane_id".into())
                    .or_insert_with(|| pane_id.to_string());
            }
        }
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::LoadPaneTab {
                id,
                index: pending.first,
                generation: 0,
                descriptor: pending.layout.tabs[pending.first].clone(),
                client_id: pending.first_client,
                tab_id: pending.tab,
                size: Size::default(),
                skip_cache: pending.skip_cache,
            })
            .context("failed to launch overlay tab group")
    }

    pub(super) fn pending_overlay_tab_loaded(
        &mut self,
        id: PaneTabsId,
        result: &std::result::Result<(PluginId, RunPluginOrAlias), String>,
    ) -> Result<bool> {
        if !self
            .pending_overlay_tabs
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            return Ok(false);
        }
        let mut pending = self
            .pending_overlay_tabs
            .take()
            .context("pending overlay disappeared")?;
        let (plugin, run) = match result {
            Ok(loaded) => loaded,
            Err(error) => {
                for completion in pending
                    .launches
                    .iter_mut()
                    .filter_map(|(_, _, c)| c.as_mut())
                {
                    completion.set_exit_status(1);
                    completion.set_error_message(error.clone());
                }
                log::error!("Failed to load overlay tab: {error}");
                return Ok(true);
            },
        };
        let destination = self
            .active_tab_ids
            .get(&pending.focus_client)
            .copied()
            .filter(|tab| self.tabs.contains_key(tab))
            .or_else(|| self.tabs.contains_key(&pending.tab).then_some(pending.tab));
        let Some(tab) = destination.and_then(|id| self.tabs.get_mut(&id)) else {
            self.bus
                .senders
                .send_to_plugin(PluginInstruction::Unload(*plugin))
                .context("failed to unload abandoned overlay")?;
            for completion in pending
                .launches
                .iter_mut()
                .filter_map(|(_, _, c)| c.as_mut())
            {
                completion.set_exit_status(1);
                completion.set_error_message("Overlay destination closed".into());
            }
            return Ok(true);
        };
        let pane = PaneId::Plugin(*plugin);
        tab.new_pane(
            pane,
            Some(pending.layout.tabs[pending.first].label.clone()),
            Some(Run::Plugin(run.clone())),
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(pending.focus_client),
            None,
        )?;
        let active = pending.layout.active;
        // Replay launches before selecting the final slot so each load keeps its client.
        pending.layout.active = pending.first;
        let info =
            tab.mount_pane_tabs(pane, pending.layout, pending.first, pending.focus_client)?;
        tab.mark_overlay_tab_group(info.id)?;
        // Each queued launch must load its slot even if a later launch selected another.
        for (index, client, completion) in pending.launches {
            tab.select_pane_tab(info.id, index, client)?;
            if let Some(completion) = completion {
                self.overlay_tab_completions
                    .push((info.id, index, completion));
            }
        }
        tab.select_pane_tab(info.id, active, pending.focus_client)?;
        tab.focus_pane_tabs(info.id, false, pending.focus_client)?;
        self.report_pane_tabs()?;
        self.log_and_report_session_state()?;
        Ok(true)
    }
}

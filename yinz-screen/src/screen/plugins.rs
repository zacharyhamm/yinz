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
use crate::pty::{ClientTabIndexOrPaneId, PtyInstruction, ReplacedPane};
use crate::route::NotificationEnd;
use crate::tab::Tab;
use crate::ClientId;
use std::collections::{BTreeMap, HashMap};
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
    /// A replacement's tab wins over an explicit destination; otherwise use the acting client.
    pub(super) fn tab_for_plugin_launch(
        &self,
        client_id: ClientId,
        explicit_tab_id: Option<TabId>,
        pane_id_to_replace: Option<PaneId>,
        completion_tx: &mut Option<NotificationEnd>,
    ) -> Option<TabId> {
        let destination = if let Some(pane_id) = pane_id_to_replace {
            self.tab_id_containing_pane(pane_id)
                .with_context(|| format!("Pane with id {pane_id:?} not found"))
        } else if let Some(tab_id) = explicit_tab_id {
            self.tabs
                .contains_key(&tab_id)
                .then_some(tab_id)
                .with_context(|| format!("Tab with id {tab_id} not found"))
        } else {
            self.client_to_act_for(client_id)
                .and_then(|client_id| self.active_tab_ids.get(&client_id).copied())
                .context("No active tab found - cannot launch plugin")
        };
        match destination {
            Ok(tab_id) => Some(tab_id),
            Err(error) => {
                if let Some(completion) = completion_tx.as_mut() {
                    completion.set_exit_status(1);
                    completion.set_error_message(format!("{error:#}"));
                }
                Err::<(), _>(error).non_fatal();
                None
            },
        }
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
        let replacement = pane_id_to_replace.filter(|_| should_open_in_place);
        let Some(tab_index) =
            self.tab_for_plugin_launch(client_id, explicit_tab_id, replacement, &mut completion_tx)
        else {
            return Ok(());
        };
        let client_id = if replacement.is_some() {
            client_id
        } else {
            self.client_to_act_for(client_id).unwrap_or(client_id)
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
                if let Some(tab_index) = self.tab_for_plugin_launch(
                    client_id,
                    explicit_tab_id,
                    Some(pane_id_to_replace),
                    &mut completion_tx,
                ) {
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
                let Some(tab_index) = self.tab_for_plugin_launch(
                    client_id,
                    explicit_tab_id,
                    None,
                    &mut completion_tx,
                ) else {
                    return Ok(());
                };
                let client_id = self.client_to_act_for(client_id).unwrap_or(client_id);
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
                } else if let Some(plugin_id) = self.running_headless(&run_plugin) {
                    // The instance runs without a pane: there is nothing to focus, and loading
                    // another would run it twice. It hears it was asked for again instead.
                    if let Some(completion) = completion_tx.as_mut() {
                        completion.set_affected_pane_id(PaneId::Plugin(plugin_id));
                        completion.set_stdout_message(format!(
                            "{} is already running headless",
                            run_plugin.location_string()
                        ));
                    }
                    self.relaunch_headless_plugin(plugin_id, client_id);
                } else if let Some(plugin_id) = self.resolving_plugin(&run_plugin) {
                    // Still resolving its repository, so it may yet get a pane: loading another
                    // would run it twice.
                    if let Some(completion) = completion_tx.as_mut() {
                        completion.set_affected_pane_id(PaneId::Plugin(plugin_id));
                        completion.set_stdout_message(format!(
                            "{} is still loading",
                            run_plugin.location_string()
                        ));
                    }
                } else {
                    // A headless instance that failed or crashed runs no more: this launch
                    // replaces it rather than leaving it behind.
                    for plugin_id in self.failed_headless(&run_plugin) {
                        self.unload_paneless_plugin(plugin_id);
                    }
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

    /// The screen's half of a plugin's `OpenPluginPaneInPlaceOfPaneId`: refuse a target that
    /// holds no slot before the plugin thread loads anything, so that a refusal never starts
    /// a plugin only to stop it again. `add_plugin` checks once more when the pane arrives,
    /// since the target can go in between.
    pub(super) fn open_plugin_pane_in_place_of_pane_id(
        &mut self,
        pane_id: PaneId,
        plugin_url: String,
        configuration: BTreeMap<String, String>,
        cwd: PathBuf,
        client_id: ClientId,
        mut completion: NotificationEnd,
    ) {
        let tab_id = match self.tab_containing_pane(pane_id) {
            Some(tab) if tab.has_non_suppressed_pane_with_pid(&pane_id) => Ok(tab.id),
            Some(tab) => Err(tab.no_slot_error(pane_id)),
            None => Err(anyhow!("pane {pane_id:?} not found")),
        };
        let tab_id = match tab_id {
            Ok(tab_id) => tab_id,
            Err(error) => {
                completion.set_exit_status(1);
                completion.set_error_message(format!("{error:#}"));
                return;
            },
        };
        // A failed send drops the completion, which answers the caller unconfirmed.
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::OpenPluginPaneInPlaceOfPaneId {
                pane_id,
                plugin_url,
                configuration,
                cwd,
                tab_id,
                client_id,
                completion,
            })
            .context("failed to reach the plugin thread")
            .non_fatal();
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
        replaced: ReplacedPane,
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
        // Plugin ids are never reused, so this is a late answer to a load that is already gone
        // (a git plugin can resolve after its unload).
        if self.unloaded_plugins.contains(&plugin_id) {
            if let Some(completion) = completion_tx.as_mut() {
                completion.set_exit_status(1);
                completion.set_error_message("plugin unloaded before its pane was placed".into());
            }
            return Ok(false);
        }
        // Clear headless state before parking so the reload's initialization can wait for its pane.
        let was_headless = self.paneless_plugins.remove(&plugin_id).is_some();
        if was_headless {
            // A pane in a tab must no longer hear every other tab through the background registry.
            self.background_plugin_subscriptions
                .retain(|(subscriber, _), _| *subscriber != plugin_id);
        }
        self.failed_paneless_plugins.remove(&plugin_id);
        self.resolving_plugins.remove(&plugin_id);
        self.destroyed_panes.remove(&PaneId::Plugin(plugin_id));
        let restored_pane_tab = match self.tab_containing_pane_mut(PaneId::Plugin(plugin_id)) {
            Some(tab) if was_headless => tab
                .restore_pane_tab_plugin(plugin_id)
                .context("failed to restore the plugin's pane tab")?,
            _ => false,
        };
        let client_id = client_id.and_then(|client_id| self.client_to_act_for(client_id));
        // The launch tab may be gone, and a replay no longer has headless state to fall back on.
        let tab_index = if client_id.is_none() && (was_headless || tab_index.is_none()) {
            tab_index
                .filter(|tab| self.tabs.contains_key(tab))
                .or_else(|| self.tabs.keys().next().copied())
        } else {
            tab_index
        };
        if !restored_pane_tab && self.active_tab_ids.is_empty() && tab_index.is_none() {
            pending
                .waiting_for_client
                .push(ScreenInstruction::AddPlugin(
                    should_float,
                    should_be_in_place,
                    replaced,
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
        if restored_pane_tab {
            // The surviving pane tab takes precedence over the reload's floating placement.
            let pane_id = PaneId::Plugin(plugin_id);
            if let Some(completion) = completion_tx.as_mut() {
                completion.set_affected_pane_id(pane_id);
            }
            if let Some(events) = pending.waiting_for_pane.remove(&pane_id) {
                replay_pending_plugin_events(self, events)?;
            }
            return Ok(true);
        }
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
                NewPanePlacement::with_pane_id_to_replace(pane_id_to_replace, replaced.closes());
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
                    replaced,
                    client_tab_index_or_pane_id,
                    &mut completion_tx,
                );
            } else if let Some(client_id) = client_id {
                let client_tab_index_or_pane_id = ClientTabIndexOrPaneId::ClientId(client_id);
                self.replace_pane(
                    PaneId::Plugin(plugin_id),
                    None,
                    Some(run_plugin),
                    Some(pane_title),
                    replaced,
                    client_tab_index_or_pane_id,
                    &mut completion_tx,
                );
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
        if plugin_exists && let Some(pending_events) = pending.waiting_for_pane.remove(&pane_id) {
            replay_pending_plugin_events(self, pending_events)?;
        }
        Ok(true)
    }

    /// Asks the pty thread to resolve a cwd for, and then load, a plugin that opens floating
    /// (or as the overlay `placement` names) in the given tab or the acting client's tab.
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
        let Some(tab_index) =
            self.tab_for_plugin_launch(client_id, explicit_tab_id, None, &mut completion_tx)
        else {
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

/// Plugins without a pane: those that declare none and run headless, and those the plugin thread
/// has unloaded. Neither may hold a pane record, so each loses whatever record it has without a
/// `PtyInstruction::ClosePane`: that would unload a headless plugin, or unload a gone one again.
impl Screen {
    /// Whether `plugin_id` can no longer get a pane: it runs headless or it is gone.
    pub(super) fn plugin_has_no_pane(&self, plugin_id: PluginId) -> bool {
        self.paneless_plugins.contains_key(&plugin_id) || self.unloaded_plugins.contains(&plugin_id)
    }

    /// The running headless instance of `run_plugin`, matched the way `Tab::find_plugin`
    /// matches a pane's plugin. The lowest id wins, so the answer does not depend on hash order.
    fn running_headless(&self, run_plugin: &RunPluginOrAlias) -> Option<PluginId> {
        instances_of(&self.paneless_plugins, run_plugin)
            .filter(|plugin_id| !self.failed_paneless_plugins.contains(plugin_id))
            .min()
    }

    /// The headless instances of `run_plugin` that failed or crashed.
    fn failed_headless(&self, run_plugin: &RunPluginOrAlias) -> Vec<PluginId> {
        instances_of(&self.paneless_plugins, run_plugin)
            .filter(|plugin_id| self.failed_paneless_plugins.contains(plugin_id))
            .collect()
    }

    /// An instance of `run_plugin` still resolving its repository, matched as
    /// `running_headless` matches.
    fn resolving_plugin(&self, run_plugin: &RunPluginOrAlias) -> Option<PluginId> {
        instances_of(&self.resolving_plugins, run_plugin).min()
    }

    /// A launch-or-focus found `plugin_id` running headless, with nothing to focus: it hears so
    /// through a private pipe message named [`RELAUNCH_PIPE`], from `client_id`, and may answer
    /// by bringing back whatever it shows (the file finder refocuses its finder or editor).
    fn relaunch_headless_plugin(&self, plugin_id: PluginId, client_id: ClientId) {
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::KeybindPipe {
                name: RELAUNCH_PIPE.to_owned(),
                payload: None,
                plugin: None,
                args: None,
                configuration: None,
                floating: None,
                should_open_in_place: false,
                pane_id_to_replace: None,
                pane_title: None,
                cwd: None,
                skip_cache: false,
                cli_client_id: client_id,
                plugin_and_client_id: Some((plugin_id, client_id)),
                notification_end: None,
            })
            .context("failed to tell a headless plugin it was launched again")
            .non_fatal();
    }

    /// Removes the plugin's pane record, if it has one, and forgets the per-pane state kept for
    /// it. Unless a record stays, the pane ends up marked destroyed, so whatever waits in
    /// `PendingEvents::waiting_for_pane` for it is dropped at the end of the instruction.
    /// Returns whether a pane was removed; the caller reports and renders.
    pub(super) fn remove_plugin_pane_quietly(&mut self, plugin_id: PluginId) -> bool {
        let pane_id = PaneId::Plugin(plugin_id);
        // Closing the shown plugin tab of a pane tabs container keeps its pane as a blank slot
        // that owns no guest; it is the container's pane from then on, not the plugin's.
        let blank_pane_tab = |tab: &Tab| {
            tab.pane_tabs_infos().any(|info| {
                info.pane_id == pane_id
                    && info.tabs.iter().all(|slot| slot.pane_id() != Some(pane_id))
            })
        };
        let blank = self
            .tab_containing_pane(pane_id)
            .is_some_and(blank_pane_tab);
        if !blank {
            self.repaint_tab_of_closing_overlay(pane_id);
        }
        let removed = match self.tab_containing_pane_mut(pane_id) {
            Some(_) if blank => false,
            Some(tab) => {
                // A pane tab child leaves its pane tab as closing it would, but without the unload
                // that comes with that: any plugin pane can be mounted into pane tabs, and one
                // that reloads without its declaration keeps running headless. A multipane child
                // leaves its container to the sibling: `close_pane` keeps a member in place.
                if !tab.detach_pane_tab_plugin(plugin_id) && !tab.remove_multipane_member(pane_id) {
                    tab.close_pane(pane_id, false, None);
                }
                let kept = tab.has_pane_with_pid(&pane_id);
                if kept && !blank_pane_tab(tab) {
                    log::warn!("plugin {plugin_id} has no pane but keeps its composed pane record");
                }
                !kept
            },
            None => false,
        };
        // An ordinary close already cleaned up before the plugin thread reported the unload,
        // and a record that stays is still a pane.
        if removed
            || (!self.destroyed_panes.contains(&pane_id)
                && self.tab_containing_pane(pane_id).is_none())
        {
            self.cleanup_closed_pane(pane_id);
        }
        self.render_blocker.remove_blocking_plugin(plugin_id);
        self.animating_plugin_loads.remove(&plugin_id);
        removed
    }

    /// `remove_plugin_pane_quietly`, then what a closed pane gets: pane groups, scroll modes, a
    /// state report when a pane went, and a render.
    pub(super) fn detach_plugin_pane(&mut self, plugin_id: PluginId) -> Result<()> {
        if self.remove_plugin_pane_quietly(plugin_id) {
            self.retain_only_existing_panes_in_pane_groups();
            let clients: Vec<ClientId> = self.active_tab_ids.keys().copied().collect();
            for client_id in clients {
                self.sync_scroll_mode_on_focus(client_id)?;
            }
            self.log_and_report_session_state()?;
        }
        self.render()
    }

    /// The plugin thread found that `plugin_id` declares no pane: it runs headless from now on.
    /// A record it already has (a layout slot, or the pane of a load that declared one before a
    /// reload) is removed, and so is an `AddPlugin` still parked for a client. A close that
    /// waited for its pane closes the plugin itself, which for a headless plugin means
    /// unloading it.
    ///
    /// `completion` answers the launch that started it, once the plugin is recorded: whatever
    /// the launcher does next finds it running headless.
    pub(super) fn plugin_paneless(
        &mut self,
        plugin_id: PluginId,
        run: RunPluginOrAlias,
        completion: Option<NotificationEnd>,
        pending: &mut PendingEvents,
    ) -> Result<()> {
        // A git plugin declares from its repository thread, so this can trail the unload. The
        // unload already removed its record, and a gone plugin must not count as running.
        if self.unloaded_plugins.contains(&plugin_id) {
            if let Some(mut completion) = completion {
                completion.set_exit_status(1);
                completion.set_error_message(format!("plugin {plugin_id} was unloaded"));
            }
            return Ok(());
        }
        self.resolving_plugins.remove(&plugin_id);
        self.failed_paneless_plugins.remove(&plugin_id);
        let pane_id = PaneId::Plugin(plugin_id);
        for mut completion in take_parked_add_plugins(plugin_id, pending) {
            completion.set_affected_pane_id(pane_id);
            completion.set_stdout_message(format!(
                "{} runs headless: it declares no pane",
                run.location_string()
            ));
        }
        // A reload that leaves the plugin headless: it runs again, and has no record to remove.
        if let Some(recorded) = self.paneless_plugins.get_mut(&plugin_id) {
            *recorded = run;
            drop(completion);
            return Ok(());
        }
        let parked = pending
            .waiting_for_pane
            .remove(&pane_id)
            .unwrap_or_default();
        self.paneless_plugins.insert(plugin_id, run);
        // Recorded, so the launch can be answered.
        drop(completion);
        let mut close_requested = false;
        for event in parked {
            if matches!(event, ScreenInstruction::ClosePane(..)) {
                close_requested = true;
            } else {
                // What waited for the pane is answered now: a request that needs one is
                // refused as for any headless plugin, and the plugin's own output is dropped.
                drop(self.settle_headless_target(event));
            }
        }
        if close_requested {
            self.unload_paneless_plugin(plugin_id);
        }
        self.detach_plugin_pane(plugin_id)
    }

    /// The plugin thread has unloaded `plugin_id`: remove its pane record if one is left, and
    /// everything the screen keeps on its behalf. What waited for its pane is dropped; a close
    /// among it is done. An `AddPlugin` parked for a client is answered now rather than when
    /// a client attaches.
    pub(super) fn plugin_unloaded(
        &mut self,
        plugin_id: PluginId,
        keybind_intercepts: &mut HashMap<ClientId, PluginId>,
        pending: &mut PendingEvents,
    ) -> Result<()> {
        for mut completion in take_parked_add_plugins(plugin_id, pending) {
            completion.set_exit_status(1);
            completion.set_error_message("plugin unloaded before its pane was placed".into());
        }
        pending.waiting_for_pane.remove(&PaneId::Plugin(plugin_id));
        self.paneless_plugins.remove(&plugin_id);
        self.failed_paneless_plugins.remove(&plugin_id);
        self.resolving_plugins.remove(&plugin_id);
        self.unloaded_plugins.insert(plugin_id);
        self.background_plugin_subscriptions
            .retain(|(subscriber, _), _| *subscriber != plugin_id);
        keybind_intercepts.retain(|_, interceptor| *interceptor != plugin_id);
        // Headless plugins already detached their panes, so pane cleanup may not run again.
        self.drop_quit_notice(plugin_id);
        self.detach_plugin_pane(plugin_id)
    }

    /// A headless plugin has no pane to close: closing it unloads it, which the pty thread's
    /// `ClosePane` does for any plugin id. It stops counting as running at once, so a launch
    /// sent before `PluginUnloaded` arrives starts a new instance instead of naming this one.
    pub(super) fn unload_paneless_plugin(&mut self, plugin_id: PluginId) {
        self.paneless_plugins.remove(&plugin_id);
        self.failed_paneless_plugins.remove(&plugin_id);
        self.resolving_plugins.remove(&plugin_id);
        self.bus
            .senders
            .send_to_pty(PtyInstruction::ClosePane(PaneId::Plugin(plugin_id), None))
            .context("failed to unload a headless plugin")
            .non_fatal();
    }

    /// The plugin thread is resolving `plugin_id`'s repository before it can read its
    /// declaration: record it as loading, and answer the launch that started it.
    pub(super) fn plugin_resolving(
        &mut self,
        plugin_id: PluginId,
        run: RunPluginOrAlias,
        completion: Option<NotificationEnd>,
    ) {
        if self.unloaded_plugins.contains(&plugin_id) {
            if let Some(mut completion) = completion {
                completion.set_exit_status(1);
                completion.set_error_message(format!("plugin {plugin_id} was unloaded"));
            }
            return;
        }
        self.resolving_plugins.insert(plugin_id, run);
        drop(completion);
    }

    /// `plugin_id` failed to load or crashed, with no pane to show it: it runs no more. A
    /// launch of it starts another instance, and one still loading is forgotten.
    pub(super) fn plugin_failed(&mut self, plugin_id: PluginId) {
        self.resolving_plugins.remove(&plugin_id);
        if self.paneless_plugins.contains_key(&plugin_id) {
            self.failed_paneless_plugins.insert(plugin_id);
        }
    }

    /// Whether closing `plugin_id` means unloading it: it runs headless, or it is still
    /// resolving its repository and has no pane yet.
    pub(super) fn closes_by_unloading(&self, plugin_id: PluginId) -> bool {
        (self.paneless_plugins.contains_key(&plugin_id)
            || self.resolving_plugins.contains_key(&plugin_id))
            && !self.holds_pane_record(plugin_id)
    }

    /// Why a request for `plugin_id`'s pane cannot be met, when the plugin runs headless.
    pub(super) fn headless_plugin_error(&self, plugin_id: PluginId) -> Option<String> {
        (self.paneless_plugins.contains_key(&plugin_id) && !self.holds_pane_record(plugin_id))
            .then(|| format!("plugin {plugin_id} runs headless: it declares no pane"))
    }

    /// Whether a tab still holds a pane under `plugin_id`'s id. A headless plugin normally has
    /// none, but a pane tabs container that showed it keeps its id as a blank slot: that pane is
    /// the container's, and an instruction naming the id is for it rather than for the plugin.
    pub(super) fn holds_pane_record(&self, plugin_id: PluginId) -> bool {
        self.tab_containing_pane(PaneId::Plugin(plugin_id))
            .is_some()
    }
}

/// The private pipe message a headless plugin receives when a `LaunchOrFocusPlugin` finds it
/// running, in place of the focus a plugin with a pane would get.
pub(super) const RELAUNCH_PIPE: &str = "launch_or_focus";

/// The plugins of `plugins` loaded from what `run_plugin` names, matched the way
/// `Tab::find_plugin` matches a pane's plugin.
fn instances_of<'a>(
    plugins: &'a HashMap<PluginId, RunPluginOrAlias>,
    run_plugin: &'a RunPluginOrAlias,
) -> impl Iterator<Item = PluginId> + 'a {
    plugins
        .iter()
        .filter(|(_, run)| run_plugin.is_equivalent_to_run(&Some(Run::Plugin((*run).clone()))))
        .map(|(plugin_id, _)| *plugin_id)
}

/// Removes every `AddPlugin` for `plugin_id` still parked until a client attaches, and hands
/// back their completions for the caller to answer.
fn take_parked_add_plugins(
    plugin_id: PluginId,
    pending: &mut PendingEvents,
) -> Vec<NotificationEnd> {
    let mut parked = vec![];
    pending.waiting_for_client.retain_mut(|event| match event {
        ScreenInstruction::AddPlugin(.., id, _, _, _, _, _, _, completion, _)
            if *id == plugin_id =>
        {
            parked.extend(completion.take());
            false
        },
        _ => true,
    });
    parked
}

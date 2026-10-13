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
use crate::pty::{ClientTabIndexOrPaneId, PtyInstruction};
use crate::route::NotificationEnd;
use crate::tab::Tab;
use crate::ClientId;
use std::path::PathBuf;
use yinz_types::data::{NewPanePlacement, OverlayPlacement};
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::input::layout::{Run, RunPluginOrAlias};
use yinz_types::pane_size::Size;

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
        completion_tx: Option<NotificationEnd>,
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
                if self.focus_plugin_pane(
                    &run_plugin,
                    should_float,
                    move_to_focused_tab,
                    should_open_in_place,
                    client_id,
                    &mut completion_tx,
                )? {
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
        completion_tx: Option<NotificationEnd>,
        explicit_tab_id: Option<TabId>,
        no_focus: bool,
    ) -> Result<()> {
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

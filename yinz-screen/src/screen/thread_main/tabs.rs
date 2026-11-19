use super::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use yinz_bus::plugins::PluginId;
use yinz_bus::route::NotificationEnd;
use yinz_bus::screen::InitialTab;
use yinz_bus::ClientId;
use yinz_bus::PaneId;
use yinz_types::geometry::Direction;
use yinz_types::input::command::TerminalAction;
use yinz_types::input::layout::RunPluginOrAlias;
use yinz_types::input::layout::SwapFloatingLayout;
use yinz_types::input::layout::SwapTiledLayout;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;

pub(super) fn close_tab(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    screen.close_tab(client_id)?;
    let connected_client_ids: Vec<ClientId> = screen.core.active_tab_ids.keys().copied().collect();
    for connected_client_id in connected_client_ids {
        screen.sync_scroll_mode_on_focus(connected_client_id)?;
    }
    screen.render()?;
    Ok(())
}

pub(super) fn open_initial_tabs(
    screen: &mut Screen,
    cwd: Option<PathBuf>,
    default_shell: Option<TerminalAction>,
    tabs: Vec<InitialTab>,
    swap_tiled: Vec<SwapTiledLayout>,
    swap_floating: Vec<SwapFloatingLayout>,
    focused: TabPosition,
    client_id: ClientId,
) -> Result<()> {
    for (index, tab) in tabs.into_iter().enumerate() {
        screen.open_new_tab(
            cwd.clone(),
            default_shell.clone(),
            tab.tiled,
            tab.floating,
            tab.name,
            Some(swap_tiled.clone()),
            Some(swap_floating.clone()),
            None,
            false,
            yinz_types::internal_ids::TabPosition::new(index) == focused,
            client_id,
            None,
        )?;
    }
    Ok(())
}

pub(super) fn go_to_tab(
    state: &mut EventLoopState,
    screen: &mut Screen,
    tab_index: TabNumber,
    client_id: Option<ClientId>,
) -> Result<()> {
    let client_id_to_switch = match client_id {
        None => None,
        Some(client_id) if screen.core.active_tab_ids.contains_key(&client_id) => Some(client_id),
        Some(_) => screen.core.active_tab_ids.keys().next().copied(),
    };
    match client_id_to_switch {
        // we must make sure no tab is pending because otherwise we cannot be
        // sure this instruction is applied at the right time (eg. we might have a
        // pending tab that will become not-pending after this instruction and change
        // the client focus, which should have happened before this instruction and not
        // after)
        Some(client_id) if !screen.has_pending_tabs() => {
            screen.go_to_tab(tab_index, client_id)?;
            screen.render()?;
        },
        _ => {
            if let Some(client_id) = client_id
                && let Some(tab_id) = screen.get_tab_id_at_position(tab_index.position())
            {
                state.pending.tab_switches.insert((tab_id, client_id));
            }
        },
    }
    Ok(())
}

pub(super) fn go_to_tab_name(
    screen: &mut Screen,
    tab_name: String,
    default_shell: Option<TerminalAction>,
    create: bool,
    client_id: Option<ClientId>,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    screen.go_to_or_create_tab_by_name(
        tab_name,
        default_shell,
        create,
        client_id,
        completion_tx,
    )?;
    Ok(())
}

pub(super) fn update_tab_name(
    screen: &mut Screen,
    c: Vec<u8>,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if screen.has_no_client_to_act_for(client_id) {
        screen.report_no_client_to_act_for(
                        client_id,
                        "Cannot rename the focused tab: no client is attached to this session. Target a tab explicitly with --tab-id.".to_owned(),
                        completion_tx,
                    );
    } else {
        // the action ends here, dropping completion_tx will release anything
        // waiting for it
        screen
            .update_active_tab_name(c, client_id)
            .context("failed to update the tab name")
            .non_fatal();
        screen.render()?;
    }
    Ok(())
}

pub(super) fn move_tab_left(
    state: &mut EventLoopState,
    screen: &mut Screen,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if !screen.has_pending_tabs() {
        screen.move_active_tab_to_left(client_id)?;
        screen.render()?;
    } else {
        // Defer execution, forward completion_tx
        state
            .pending
            .waiting_for_tab
            .push(ScreenInstruction::MoveTabLeft(client_id, completion_tx));
    }
    Ok(())
}

pub(super) fn move_tab_right(
    state: &mut EventLoopState,
    screen: &mut Screen,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if !screen.has_pending_tabs() {
        screen.move_active_tab_to_right(client_id)?;
        screen.render()?;
    } else {
        // Defer execution, forward completion_tx
        state
            .pending
            .waiting_for_tab
            .push(ScreenInstruction::MoveTabRight(client_id, completion_tx));
    }
    Ok(())
}

pub(super) fn toggle_active_sync_tab(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(
        screen,
        client_id,
        |tab: &mut Tab, _client_id: ClientId| tab.toggle_sync_panes_is_active()
    );
    screen.log_and_report_session_state()?;
    screen.render()?;
    Ok(())
}

pub(super) fn query_tab_names(
    screen: &mut Screen,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    let tab_names = screen
        .get_tabs_mut()
        .values()
        .map(|tab| tab.name.clone())
        .collect::<Vec<String>>();
    screen
        .core
        .bus
        .senders
        .send_to_server(ServerInstruction::Log(tab_names, client_id, completion_tx))?;
    Ok(())
}

pub(super) fn rename_tab(
    screen: &mut Screen,
    tab_index: TabNumber,
    new_name: Vec<u8>,
) -> Result<()> {
    // tab_index here is 1-based user input representing display position
    let tab_position = tab_index.position();

    match screen.get_tab_by_position_mut(tab_position) {
        Some(tab) => {
            tab.rename_by_user(String::from_utf8_lossy(&new_name).to_string());
        },
        None => {
            log::error!("Failed to find tab at position: {}", tab_position);
        },
    }
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn go_to_tab_with_id(
    screen: &mut Screen,
    tab_id: TabId,
    client_id: Option<ClientId>,
) -> Result<()> {
    let client_id_to_switch = client_id
        .and_then(|cid| {
            if screen.core.active_tab_ids.contains_key(&cid) {
                Some(cid)
            } else {
                screen.core.active_tab_ids.keys().next().copied()
            }
        })
        .or_else(|| screen.core.active_tab_ids.keys().next().copied());

    if let Some(client_id) = client_id_to_switch {
        // Get the position from the ID
        if let Some(tab_position) = screen.get_tab_position_by_id(tab_id) {
            // Navigation uses a 0-based display position.
            screen.navigate_to_tab(tab_position, None, true, client_id)?;
            screen.render()?;

            screen
                .core
                .tab_history
                .entry(client_id)
                .or_default()
                .push(tab_id);
        } else {
            log::error!("Tab with ID {} not found", tab_id);
        }
    }
    Ok(())
}

pub(super) fn rename_tab_with_id(
    screen: &mut Screen,
    tab_id: TabId,
    new_name: Vec<u8>,
) -> Result<()> {
    // Use get_tab_by_id_mut() helper method
    if let Some(tab) = screen.get_tab_by_id_mut(tab_id) {
        tab.rename_by_user(String::from_utf8_lossy(&new_name).to_string());
        screen.log_and_report_session_state()?;
    } else {
        log::error!("Failed to find tab with ID: {}", tab_id);
    }
    Ok(())
}

pub(super) fn set_tab_projects(
    screen: &mut Screen,
    tab_projects: BTreeMap<TabId, String>,
) -> Result<()> {
    // Whole map each time, so a tab that lost its project loses the
    // name with it. Tabs we have never heard of are kept: a plugin can
    // know about a tab before we have finished making it, and the next
    // pass drops whatever is genuinely gone.
    let tab_projects: HashMap<TabId, String> = tab_projects.into_iter().collect();
    if tab_projects != screen.core.tab_projects {
        screen.core.tab_projects = tab_projects;
        if screen.update_plugin_tab_names() {
            screen.log_and_report_session_state()?;
        }
    }
    Ok(())
}

pub(super) fn close_tab_with_id(
    state: &mut EventLoopState,
    screen: &mut Screen,
    tab_id: TabId,
) -> Result<()> {
    if screen.get_tab_by_id(tab_id).is_some_and(Tab::is_pending) {
        screen.close_tab_by_id(tab_id).non_fatal();
        if !screen.has_pending_tabs() {
            screen
                .apply_deferred_tab_switches(&mut state.pending.tab_switches)
                .non_fatal();
        }
    } else if screen.get_tab_by_id(tab_id).is_some() {
        screen.request_close_tab(tab_id, None).non_fatal();
    } else {
        log::error!("Failed to find tab with ID: {}", tab_id);
    }
    Ok(())
}

pub(super) fn break_panes_to_tab_with_id(
    screen: &mut Screen,
    pane_ids: Vec<PaneId>,
    tab_id: TabId,
    should_change_focus_to_target_tab: bool,
    client_id: ClientId,
    mut completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    // tab_id is a stable id and the callee is id-space, so nothing to convert
    let outcome = screen.break_multiple_panes_to_tab_with_id(
        pane_ids,
        tab_id,
        should_change_focus_to_target_tab,
        client_id,
    )?;
    match outcome.affected_tab_id() {
        Some(affected_tab_id) => {
            if let Some(c) = completion_tx.as_mut() {
                c.set_affected_tab_id(affected_tab_id)
            }
            let pane_group = screen.get_client_pane_group(&client_id);
            if !pane_group.is_empty() {
                let _ = screen.core.bus.senders.send_to_background_jobs(
                    BackgroundJob::HighlightPanesWithMessage(
                        pane_group.iter().copied().collect(),
                        "BROKEN OUT".to_owned(),
                    ),
                );
            }
            screen.clear_pane_group(&client_id);
        },
        // leaving affected_tab_id None is what signals failure to the plugin
        None => {
            log::error!("Failed to move panes to tab with id {tab_id}: {outcome:?}")
        },
    }
    Ok(())
}

pub(super) fn close_tab_with_index(screen: &mut Screen, tab_index: TabPosition) -> Result<()> {
    // tab_index here means display position (0-based) per API convention
    // Resolve the display position before the shared close preflight.

    if let Some(tab_id) = screen.get_tab_id_at_position(tab_index) {
        screen.request_close_tab(tab_id, None).non_fatal();
    } else {
        log::error!("Failed to find tab at position: {}", tab_index);
    }
    Ok(())
}

pub(super) fn break_panes_to_new_tab(
    screen: &mut Screen,
    pane_ids: Vec<PaneId>,
    default_shell: Option<TerminalAction>,
    should_change_focus_to_new_tab: bool,
    new_tab_name: Option<String>,
    client_id: ClientId,
    mut completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    let tab_id = screen.break_multiple_panes_to_new_tab(
        pane_ids,
        default_shell,
        should_change_focus_to_new_tab,
        new_tab_name,
        client_id,
    )?;
    // Set affected tab ID for plugin API return value
    if let Some(c) = completion_tx.as_mut() {
        c.set_affected_tab_id(tab_id)
    }
    // Not a race: everything here runs on the screen thread, and the
    // background job re-enters through the same serialised instruction
    // queue. The only hazard is a stale clear if the panes gain another
    // frame-colour override before the flash ends.
    let pane_group = screen.get_client_pane_group(&client_id);
    if !pane_group.is_empty() {
        let _ = screen.core.bus.senders.send_to_background_jobs(
            BackgroundJob::HighlightPanesWithMessage(
                pane_group.iter().copied().collect(),
                "BROKEN OUT".to_owned(),
            ),
        );
    }
    screen.clear_pane_group(&client_id);
    Ok(())
}

pub(super) fn break_panes_to_tab_with_index(
    screen: &mut Screen,
    pane_ids: Vec<PaneId>,
    tab_index: TabPosition,
    should_change_focus_to_new_tab: bool,
    client_id: ClientId,
    mut completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    // tab_index here means display position (0-based) per API convention,
    // and the callee is id-space, so it must be mapped to a stable ID -
    // the same conversion CloseTabWithIndex does
    let Some(tab_id) = screen.get_tab_id_at_position(tab_index) else {
        log::error!("Failed to find tab at position: {}", tab_index);
        return Ok(()); // affected_tab_id stays None, signalling failure
    };
    let outcome = screen.break_multiple_panes_to_tab_with_id(
        pane_ids,
        tab_id,
        should_change_focus_to_new_tab,
        client_id,
    )?;
    match outcome.affected_tab_id() {
        Some(affected_tab_id) => {
            if let Some(c) = completion_tx.as_mut() {
                c.set_affected_tab_id(affected_tab_id)
            }
            let pane_group = screen.get_client_pane_group(&client_id);
            if !pane_group.is_empty() {
                let _ = screen.core.bus.senders.send_to_background_jobs(
                    BackgroundJob::HighlightPanesWithMessage(
                        pane_group.iter().copied().collect(),
                        "BROKEN OUT".to_owned(),
                    ),
                );
            }
            screen.clear_pane_group(&client_id);
        },
        None => log::error!(
            "Failed to move panes to tab at position {tab_index} (id {tab_id}): {outcome:?}"
        ),
    }
    Ok(())
}

pub(super) fn pane_tab_loaded(
    state: &mut EventLoopState,
    screen: &mut Screen,
    id: yinz_types::pane_tabs::PaneTabsId,
    index: usize,
    generation: u64,
    result: Result<(PluginId, RunPluginOrAlias), String>,
) -> Result<()> {
    let plugin = result.as_ref().ok().map(|(plugin, _)| *plugin);
    screen.pane_tab_loaded(id, index, generation, result)?;
    if let Some(plugin) = plugin {
        let events = state
            .pending
            .waiting_for_pane
            .remove(&PaneId::Plugin(plugin))
            .unwrap_or_default();
        if screen.tab_containing_pane(PaneId::Plugin(plugin)).is_some() {
            crate::screen::replay_pending_plugin_events(screen, events)?;
        } else {
            screen.cleanup_closed_pane(PaneId::Plugin(plugin));
        }
    }
    screen.render()?;
    Ok(())
}

pub(super) fn undo_rename_tab_with_tab_id(
    screen: &mut Screen,
    tab_id: TabId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.core.tabs.get_mut(&tab_id) {
        if tab.undo_rename() {
            screen.log_and_report_session_state()?;
        }
    } else {
        log::error!("Tab with id {} not found", tab_id);
        if let Some(ref mut c) = _completion_tx {
            c.set_exit_status(1);
            c.set_error_message(format!("Tab with id {} not found", tab_id));
        }
    }
    screen.render()?;
    Ok(())
}

pub(super) fn toggle_active_sync_tab_with_tab_id(
    screen: &mut Screen,
    tab_id: TabId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.core.tabs.get_mut(&tab_id) {
        tab.toggle_sync_panes_is_active();
    } else {
        log::error!("Tab with id {} not found", tab_id);
        if let Some(ref mut c) = _completion_tx {
            c.set_exit_status(1);
            c.set_error_message(format!("Tab with id {} not found", tab_id));
        }
    }
    screen.log_and_report_session_state()?;
    screen.render()?;
    Ok(())
}

pub(super) fn toggle_floating_panes_with_tab_id(
    screen: &mut Screen,
    tab_id: TabId,
    default_shell: Option<TerminalAction>,
    mut completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.core.tabs.get_mut(&tab_id) {
        // Pass None as client_id so that if a new floating pane must be spawned,
        // it targets this tab (via TabIndex) rather than the focused tab of some client.
        tab.toggle_floating_panes(None, default_shell, completion_tx)
            .non_fatal();
    } else {
        log::error!("Tab with id {} not found", tab_id);
        if let Some(ref mut c) = completion_tx {
            c.set_exit_status(1);
            c.set_error_message(format!("Tab with id {} not found", tab_id));
        }
        drop(completion_tx);
    }
    screen.log_and_report_session_state()?;
    screen.render()?;
    Ok(())
}

pub(super) fn move_tab_with_tab_id(
    state: &mut EventLoopState,
    screen: &mut Screen,
    tab_id: TabId,
    direction: Direction,
    _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if !screen.has_pending_tabs() {
        screen.move_tab_by_id(tab_id, direction)?;
        screen.render()?;
    } else {
        state
            .pending
            .waiting_for_tab
            .push(ScreenInstruction::MoveTabWithTabId(
                tab_id,
                direction,
                _completion_tx,
            ));
    }
    Ok(())
}

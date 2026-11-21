use super::*;
use std::path::PathBuf;
use yinz_bus::route::NotificationEnd;
use yinz_bus::screen::{HoldForCommand, TabOverrideResult};
use yinz_bus::ClientId;
use yinz_bus::PaneId;
use yinz_types::agent::AgentInfo;
use yinz_types::geometry::ResizeStrategy;
use yinz_types::input::command::RunCommand;
use yinz_types::input::command::TerminalAction;
use yinz_types::input::layout::FloatingPaneLayout;
use yinz_types::input::layout::TabLayoutInfo;
use yinz_types::input::layout::TiledPaneLayout;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TerminalId;
use yinz_types::pane_size::Size;

pub(super) fn resize(
    screen: &mut Screen,
    client_id: ClientId,
    strategy: ResizeStrategy,
) -> Result<()> {
    active_tab_and_connected_client_id!(
        screen,
        client_id,
        |tab: &mut Tab, client_id: ClientId| tab.resize(client_id, strategy),
        ?
    );
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn dump_layout(
    screen: &mut Screen,
    default_shell: Option<PathBuf>,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    let err_context = || "Failed to dump layout".to_string();
    let session_layout_metadata = screen.get_layout_metadata(default_shell, None);
    screen
        .core
        .bus
        .senders
        .send_to_plugin(PluginInstruction::DumpLayout(
            session_layout_metadata,
            client_id,
            completion_tx,
        ))
        .with_context(err_context)?;
    Ok(())
}

pub(super) fn swap_text_selection_ends(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.swap_text_selection_ends(client_id)
    });
    screen.render()?;
    Ok(())
}

pub(super) fn apply_layout(
    state: &mut EventLoopState,
    screen: &mut Screen,
    layout: TiledPaneLayout,
    floating_panes_layout: Vec<FloatingPaneLayout>,
    new_pane_pids: Vec<(TerminalId, HoldForCommand)>,
    new_floating_pane_pids: Vec<(TerminalId, HoldForCommand)>,
    adopted_terminals: Vec<(TerminalId, RunCommand, Option<AgentInfo>)>,
    new_plugin_ids: yinz_bus::plugins::LayoutPlugins,
    tab_id: TabId,
    should_change_focus_to_new_tab: bool,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
    blocking_terminal: Option<(TerminalId, NotificationEnd)>,
) -> Result<()> {
    let startup_started = std::time::Instant::now();
    log::info!("startup: ApplyLayout tab={tab_id:?} client={client_id} begin");
    screen.land_layout(
        layout,
        floating_panes_layout,
        new_pane_pids,
        new_floating_pane_pids,
        adopted_terminals,
        new_plugin_ids,
        tab_id,
        should_change_focus_to_new_tab,
        client_id,
        completion_tx,
        blocking_terminal,
        &mut state.pending,
    )?;
    log::info!(
        "startup: ApplyLayout tab={tab_id:?} client={client_id} complete at {:?}",
        startup_started.elapsed()
    );
    screen.render()?;
    Ok(())
}

pub(super) fn layout_spawn_failed(
    state: &mut EventLoopState,
    screen: &mut Screen,
    tab_id: TabId,
    destination: Option<yinz_bus::route::AgentTabPlacement>,
) -> Result<()> {
    if destination.is_some_and(|target| {
        !screen.core.tabs.get(&tab_id).is_some_and(|tab| {
            target
                .lifetime
                .ptr_eq(&std::sync::Arc::downgrade(&tab.agent_tab_lifetime))
        })
    }) {
        return Ok(());
    }
    screen.recover_pending_tab(tab_id, None).non_fatal();
    screen.replay_events_waiting_for_layout(&mut state.pending);
    if !screen.has_pending_tabs() {
        screen
            .apply_deferred_tab_switches(&mut state.pending.tab_switches)
            .non_fatal();
    }
    screen.render()?;
    Ok(())
}

pub(super) fn recompute_tab_size(
    screen: &mut Screen,
    client_id: ClientId,
    new_size: Size,
) -> Result<()> {
    let previous_tab_size = screen.size_of_tab_of_client(client_id);
    screen
        .set_client_size(client_id, new_size)
        .context("failed to set client size for tab recomputation")?;
    // the server sends this while setting the session up, before any tab exists
    if screen.core.tabs.is_empty() {
        return Ok(());
    }
    // Reflow each tab against its viewers, then report and render. An incomplete
    // reflow retries on that frame, so it must not take the session down.
    screen
        .resize_to_clients()
        .context("failed to resize tabs for a client")
        .non_fatal();
    if screen.size_of_tab_of_client(client_id) == previous_tab_size {
        // Another viewer can pin this tab's size, so its layout does not change,
        // but this client's terminal did, and it needs the
        // unchanged layout painted over its new viewport
        if let Ok(tab) = screen.get_active_tab_mut(client_id) {
            tab.set_force_render();
        }
        screen.render()?;
    }
    Ok(())
}

pub(super) fn previous_swap_layout(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(
        screen,
        client_id,
        |tab: &mut Tab, _client_id: ClientId| tab.previous_swap_layout(),
        ?
    );
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn next_swap_layout(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(
        screen,
        client_id,
        |tab: &mut Tab, _client_id: ClientId| tab.next_swap_layout(),
        ?
    );
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn override_layout(
    screen: &mut Screen,
    cwd: Option<PathBuf>,
    default_shell: Option<TerminalAction>,
    tab_layouts: Vec<TabLayoutInfo>,
    retain_existing_terminal_panes: bool,
    retain_existing_plugin_panes: bool,
    apply_only_to_focused_tab: bool,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    screen.override_layout(
        cwd,
        default_shell,
        tab_layouts,
        retain_existing_terminal_panes,
        retain_existing_plugin_panes,
        apply_only_to_focused_tab,
        client_id,
        completion_tx,
    )?;
    Ok(())
}

pub(super) fn override_layout_complete(
    state: &mut EventLoopState,
    screen: &mut Screen,
    tab_results: Vec<TabOverrideResult>,
    failed_tabs: Vec<TabId>,
    tabs_to_close: Vec<TabId>,
    retain_existing_terminal_panes: bool,
    retain_existing_plugin_panes: bool,
    client_id: ClientId,
    mut completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Err(error) = screen.complete_override_layout(
        tab_results,
        failed_tabs,
        tabs_to_close,
        retain_existing_terminal_panes,
        retain_existing_plugin_panes,
        client_id,
        &mut state.pending,
    ) {
        screen.report_layout_error(format!("{error:#}"), client_id, &mut completion_tx);
    }
    screen.log_and_report_session_state()?;
    let _ = screen.render();
    drop(completion_tx); // action ends here, notify the action initiator
    Ok(())
}

pub(super) fn resize_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    strategy: ResizeStrategy,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.resize_by_pane_id(pane_id, strategy);
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn previous_swap_layout_with_tab_id(
    screen: &mut Screen,
    tab_id: TabId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.core.tabs.get_mut(&tab_id) {
        tab.previous_swap_layout().non_fatal();
    } else {
        log::error!("Tab with id {} not found", tab_id);
        if let Some(ref mut c) = _completion_tx {
            c.set_exit_status(1);
            c.set_error_message(format!("Tab with id {} not found", tab_id));
        }
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn next_swap_layout_with_tab_id(
    screen: &mut Screen,
    tab_id: TabId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.core.tabs.get_mut(&tab_id) {
        tab.next_swap_layout().non_fatal();
    } else {
        log::error!("Tab with id {} not found", tab_id);
        if let Some(ref mut c) = _completion_tx {
            c.set_exit_status(1);
            c.set_error_message(format!("Tab with id {} not found", tab_id));
        }
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

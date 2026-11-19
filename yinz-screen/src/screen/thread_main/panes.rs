use super::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use yinz_bus::plugins::PluginId;
use yinz_bus::pty::ClientTabIndexOrPaneId;
use yinz_bus::pty::ReplacedPane;
use yinz_bus::route::NotificationEnd;
use yinz_bus::screen::{HoldForCommand, InitialTitle};
use yinz_bus::ClientId;
use yinz_bus::PaneId;
use yinz_types::data::NewPanePlacement;
use yinz_types::geometry::Direction;
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::input::command::RunCommand;
use yinz_types::input::command::TerminalAction;
use yinz_types::input::layout::Run;
use yinz_types::input::options::PaneFrameStyle;
use yinz_types::internal_ids::TerminalId;

pub(super) fn new_pane(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pid: PaneId,
    initial_pane_title: Option<InitialTitle>,
    hold_for_command: HoldForCommand,
    invoked_with: Option<Run>,
    new_pane_placement: NewPanePlacement,
    start_suppressed: bool,
    client_or_tab_index: ClientTabIndexOrPaneId,
    completion_tx: Option<NotificationEnd>,
    set_blocking: bool,
) -> Result<()> {
    // A placement that fails has already been reported on its completion and
    // the pane forgotten; an in-place target without a slot is one such
    // request, and must not take the screen thread down with it.
    screen
        .place_new_pane(
            pid,
            initial_pane_title,
            hold_for_command,
            invoked_with,
            new_pane_placement,
            start_suppressed,
            client_or_tab_index,
            completion_tx,
            set_blocking,
            &mut state.pending,
        )
        .non_fatal();
    screen.log_and_report_session_state()?;

    screen.render()?;
    Ok(())
}

pub(super) fn open_in_place_editor(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pid: PaneId,
    client_tab_index_or_pane_id: ClientTabIndexOrPaneId,
) -> Result<()> {
    match client_tab_index_or_pane_id {
        ClientTabIndexOrPaneId::ClientId(client_id)
        | ClientTabIndexOrPaneId::ClientIdNoFocus(client_id) => {
            // An editor that finds no pane to replace is closed, as in the
            // pane-id case below, so its process and scrollback dump go too.
            let placed = screen
                .get_active_tab_mut(client_id)
                .and_then(|tab| tab.replace_active_pane_with_editor_pane(pid, client_id));
            if let Err(error) = placed {
                screen.forget_unplaced_pane(pid, format!("{error:#}"));
            }
        },
        ClientTabIndexOrPaneId::TabIndex(_tab_index)
        | ClientTabIndexOrPaneId::TabIndexNoFocus(_tab_index)
        | ClientTabIndexOrPaneId::TabIndexForClient(_tab_index, _) => {
            screen.forget_unplaced_pane(
                pid,
                "cannot open an in-place editor with a tab index".to_owned(),
            );
        },
        ClientTabIndexOrPaneId::PaneId(pane_id_to_replace)
        | ClientTabIndexOrPaneId::PaneIdOrClient(pane_id_to_replace, _) => {
            // As in `replace_pane`: there is nothing to edit in place of a pane the
            // caller does not have, so no client fallback here.
            let placed = match screen.tab_containing_pane_mut(pane_id_to_replace) {
                Some(tab) => tab.replace_pane_with_editor_pane(pid, pane_id_to_replace),
                None => Err(anyhow!("pane {pane_id_to_replace:?} to replace not found")),
            };
            if let Err(error) = placed {
                screen.forget_unplaced_pane(pid, format!("{error:#}"));
            }
        },
    }

    if let Some(pending_events) = state.pending.waiting_for_pane.remove(&pid) {
        for event in pending_events {
            replay_pending_pane_event(screen, event, &mut state.pending).non_fatal();
        }
    }

    screen.log_and_report_session_state()?;
    screen.render()?;
    Ok(())
}

pub(super) fn toggle_floating_panes(
    screen: &mut Screen,
    client_id: ClientId,
    default_shell: Option<TerminalAction>,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| tab
                    .toggle_floating_panes(Some(client_id), default_shell, completion_tx), ?);
    screen.clear_bell_for_focused_pane(client_id);
    screen.sync_scroll_mode_on_focus(client_id)?;
    screen.log_and_report_session_state()?;

    screen.render()?;
    Ok(())
}

pub(super) fn clear_screen(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(
        screen,
        client_id,
        |tab: &mut Tab, client_id: ClientId| tab.clear_active_terminal_screen(
            client_id,
        ),
        ?
    );
    screen.render()?;
    Ok(())
}

pub(super) fn dump_screen(
    screen: &mut Screen,
    file: Option<String>,
    client_id: ClientId,
    full: bool,
    pane_id: Option<PaneId>,
    completion_tx: Option<NotificationEnd>,
    cli_client_id: Option<ClientId>,
    ansi: bool,
) -> Result<()> {
    screen.dump_screen(
        file,
        client_id,
        full,
        pane_id,
        completion_tx,
        cli_client_id,
        ansi,
    )?;
    Ok(())
}

pub(super) fn move_pane(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.move_active_pane(client_id)
    });
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn move_pane_backwards(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.move_active_pane_backwards(client_id)
    });
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn move_pane_down(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.move_active_pane_down(client_id)
    });
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn move_pane_up(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.move_active_pane_up(client_id)
    });
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn move_pane_right(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.move_active_pane_right(client_id)
    });
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn move_pane_left(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.move_active_pane_left(client_id)
    });
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn move_text_selection(
    screen: &mut Screen,
    direction: Direction,
    client_id: ClientId,
) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.move_text_selection(direction, client_id)
    });
    screen.render()?;
    Ok(())
}

pub(super) fn set_selectable(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pid: PaneId,
    selectable: bool,
) -> Result<()> {
    if !screen.set_pane_selectable(pid, selectable) {
        // the pane's tab may simply not exist yet - retry once one does
        state
            .pending
            .waiting_for_tab
            .push(ScreenInstruction::SetSelectable(pid, selectable));
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn retire_picker(
    state: &mut EventLoopState,
    screen: &mut Screen,
    finder: PluginId,
    pane: Option<PaneId>,
    client: ClientId,
) -> Result<()> {
    if let Some(pane) = pane {
        screen.close_pane_from_instruction(pane, Some(client), None, None, &mut state.pending)?;
    }
    screen.render()?;
    screen
        .core
        .bus
        .senders
        .send_to_plugin(PluginInstruction::PickRetired(finder))
        .context("acknowledge picker retirement")?;
    Ok(())
}

pub(super) fn close_pane(
    state: &mut EventLoopState,
    screen: &mut Screen,
    id: PaneId,
    client_id: Option<ClientId>,
    completion_tx: Option<NotificationEnd>,
    exit_status: Option<i32>,
) -> Result<()> {
    screen.close_pane_from_instruction(
        id,
        client_id,
        completion_tx,
        exit_status,
        &mut state.pending,
    )?;
    screen.log_and_report_session_state().non_fatal();
    screen.retain_only_existing_panes_in_pane_groups();
    screen.render()?;
    Ok(())
}

pub(super) fn restore_held_pane(
    state: &mut EventLoopState,
    screen: &mut Screen,
    id: PaneId,
    held: yinz_types::data::HeldTerminal,
) -> Result<()> {
    if !screen.restore_held_pane(id, &held) {
        state
            .pending
            .waiting_for_pane
            .entry(id)
            .or_default()
            .push(ScreenInstruction::RestoreHeldPane(id, held));
    }
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn hold_pane(
    state: &mut EventLoopState,
    screen: &mut Screen,
    id: PaneId,
    exit_status: Option<i32>,
    run_command: RunCommand,
) -> Result<()> {
    screen.invalidate_agent_lifecycle(id, "held");
    let is_first_run = false;
    let mut found = false;
    let mut pane_is_agent = false;
    let mut queued_respawn = None;
    {
        for tab in screen.core.tabs.values_mut() {
            if tab.get_all_pane_ids().contains(&id) {
                queued_respawn = tab.hold_pane(id, exit_status, is_first_run, run_command.clone());
                found = true;
                pane_is_agent = tab.pane_is_agent(id);
                break;
            }
        }
    }
    if found {
        screen.refresh_agent_worktrees();
        // The observer first, then the pane-replacing hook, so the plugin thread
        // hears of the exit before it covers the pane.
        screen.fire_pane_exit_hook(id, exit_status, &run_command, true);
        screen.fire_agent_exit_hook(id, exit_status, &run_command, pane_is_agent);
        // Only after the exit hooks have seen this exit does a queued replacement
        // start, and it starts through Screen so agent_start fires for it too.
        if let Some((terminal_pane_id, run_command, keep_scrollback)) = queued_respawn {
            screen.run_command_in_pane_with_id(terminal_pane_id, run_command, keep_scrollback);
        }
    }
    if !found {
        state
            .pending
            .waiting_for_pane
            .entry(id)
            .or_default()
            .push(ScreenInstruction::HoldPane(id, exit_status, run_command));
    }
    screen.log_and_report_session_state()?;
    screen.render()?;
    Ok(())
}

pub(super) fn set_pane_suspended(screen: &mut Screen, id: PaneId, suspended: bool) -> Result<()> {
    // Unlike a hold this is not buffered for a pane that does not
    // exist yet: a pane cannot suspend before it has been created,
    // so a miss here means the pane is already gone and the news is
    // stale rather than early.
    for tab in screen.core.tabs.values_mut() {
        if tab.get_all_pane_ids().contains(&id) {
            tab.set_pane_suspended(id, suspended)?;
            break;
        }
    }
    screen.render()?;
    Ok(())
}

pub(super) fn update_pane_name(screen: &mut Screen, c: Vec<u8>, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.update_active_pane_name(c, client_id)
            .context("failed to update the pane name")
            .non_fatal();
    });
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn update_pane_foreground_command(
    state: &mut EventLoopState,
    screen: &mut Screen,
    terminal_id: TerminalId,
    command: Vec<String>,
    program: Option<String>,
) -> Result<()> {
    let pane_id = PaneId::Terminal(terminal_id);
    if apply_pane_foreground_command(screen, terminal_id, command.clone(), program.clone()) {
        screen.render()?;
    } else {
        state
            .pending
            .waiting_for_pane
            .entry(pane_id)
            .or_default()
            .push(ScreenInstruction::UpdatePaneForegroundCommand(
                terminal_id,
                command,
                program,
            ));
    }
    Ok(())
}

pub(super) fn update_pane_cwd(
    screen: &mut Screen,
    terminal_id: TerminalId,
    cwd: Option<PathBuf>,
) -> Result<()> {
    // Only pty says a pane moved, and it only says so on a change,
    // so there is nothing to compare against here. Pruning cannot be
    // done from the manifest instead: pty reports the folder a pane
    // starts in *before* the layout that puts the pane in a tab, so
    // an unknown id is as likely to be arriving as departing.
    match cwd {
        Some(cwd) => screen.core.terminal_cwds.insert(terminal_id, cwd),
        None => screen.core.terminal_cwds.remove(&terminal_id),
    };
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn set_pane_subtitle(
    screen: &mut Screen,
    pane_id: PaneId,
    subtitle: Option<String>,
) -> Result<()> {
    if let Some(tab) = screen
        .core
        .tabs
        .values_mut()
        .find(|tab| tab.get_pane_with_id(pane_id).is_some())
    {
        let resized = tab.set_pane_subtitle(pane_id, subtitle)?;
        screen.render()?;
        if resized {
            screen.log_and_report_session_state()?;
        }
    }
    Ok(())
}

pub(super) fn set_pane_title_prefix(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pane_id: PaneId,
    prefix: Option<String>,
) -> Result<()> {
    // No `log_and_report_session_state`: the prefix is a decoration on
    // the frame and never enters `current_title`, so nothing a plugin
    // can see has changed — and the plugin that sets this is reading
    // that very manifest to decide what to set.
    if screen
        .with_pane(pane_id, |pane| pane.set_title_prefix(prefix.clone()))
        .is_some()
    {
        screen.render()?;
    } else {
        state
            .pending
            .waiting_for_pane
            .entry(pane_id)
            .or_default()
            .push(ScreenInstruction::SetPaneTitlePrefix(pane_id, prefix));
    }
    Ok(())
}

pub(super) fn set_pane_caption(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pane_id: PaneId,
    caption: String,
) -> Result<()> {
    // `set_title`, not a rename: the caption is a default, and a default is not
    // lent to the tab. Buffered like a hold, for a pane still being built.
    if screen
        .with_pane(pane_id, |pane| pane.set_title(caption.clone()))
        .is_some()
    {
        screen.render()?;
        screen.log_and_report_session_state()?;
    } else {
        state
            .pending
            .waiting_for_pane
            .entry(pane_id)
            .or_default()
            .push(ScreenInstruction::SetPaneCaption(pane_id, caption));
    }
    Ok(())
}

pub(super) fn toggle_active_terminal_fullscreen(
    screen: &mut Screen,
    client_id: ClientId,
) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.toggle_active_pane_fullscreen(client_id)
    });
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn toggle_active_terminal_no_ui_fullscreen(
    screen: &mut Screen,
    client_id: ClientId,
) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.toggle_active_pane_no_ui_fullscreen(client_id)
    });
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn toggle_pane_frames(screen: &mut Screen) -> Result<()> {
    screen.core.pane_frame_style = match screen.core.pane_frame_style {
        PaneFrameStyle::Full => PaneFrameStyle::Titles,
        PaneFrameStyle::Titles => PaneFrameStyle::None,
        PaneFrameStyle::None => PaneFrameStyle::Full,
    };
    for tab in screen.core.tabs.values_mut() {
        tab.set_pane_frames(screen.core.pane_frame_style);
        tab.update_input_modes()?;
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn set_pane_frame_style(
    screen: &mut Screen,
    pane_frame_style: PaneFrameStyle,
) -> Result<()> {
    screen.core.pane_frame_style = pane_frame_style;
    for tab in screen.core.tabs.values_mut() {
        tab.set_pane_frames(screen.core.pane_frame_style);
        tab.update_input_modes()?;
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn add_highlight_pane_frame_color_override(
    screen: &mut Screen,
    pane_ids: Vec<PaneId>,
    error_text: Option<String>,
) -> Result<()> {
    for pane_id in pane_ids {
        if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
            tab.add_highlight_pane_frame_color_override(pane_id, error_text.clone(), None);
        }
    }
    screen.render()?;
    Ok(())
}

pub(super) fn flash_pane_frame_color(screen: &mut Screen, pane_ids: Vec<PaneId>) -> Result<()> {
    let now = Instant::now();
    for pane_id in pane_ids {
        if let Some(tab) = screen.tab_containing_pane_mut(pane_id)
            && tab.flash_pane_frame_color_override(pane_id, now)
        {
            screen.core.fading_frame_panes.insert(pane_id);
        }
    }
    screen.sync_animation_clock(now)?;
    screen.render()?;
    Ok(())
}

pub(super) fn suppress_pane(
    screen: &mut Screen,
    pane_id: PaneId,
    client_id: ClientId,
) -> Result<()> {
    // Before the tab loop: an overlay is in none of the collections
    // `has_non_suppressed_pane_with_pid` walks, so without this arm hiding one is a
    // silent no-op. It is also not `suppress_pane`, which extracts the pane and would
    // lose the kind and coordinates that make it an overlay.
    if screen.tab_containing_overlay(pane_id).is_some() {
        if let Some(tab) = screen.tab_containing_overlay_mut(pane_id) {
            tab.hide_overlay_pane(pane_id);
        }
        screen.render()?;
        screen.log_and_report_session_state()?;
        return Ok(());
    }
    let all_tabs = screen.get_tabs_mut();
    for tab in all_tabs.values_mut() {
        if tab.has_non_suppressed_pane_with_pid(&pane_id) {
            tab.suppress_pane(pane_id, Some(client_id));
            drop(screen.render());
            break;
        }
    }
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn unsuppress_pane(
    screen: &mut Screen,
    pane_id: PaneId,
    should_float_if_hidden: bool,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_overlay_mut(pane_id) {
        tab.show_overlay_pane(pane_id);
        screen.render()?;
        screen.log_and_report_session_state()?;
        return Ok(());
    }
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.unsuppress_pane(pane_id, should_float_if_hidden);
        drop(screen.render());
    }
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn unsuppress_or_expand_pane(
    screen: &mut Screen,
    pane_id: PaneId,
    should_float_if_hidden: bool,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_overlay_mut(pane_id) {
        tab.show_overlay_pane(pane_id);
        screen.render()?;
        screen.log_and_report_session_state()?;
        return Ok(());
    }
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.unsuppress_or_expand_pane(pane_id, should_float_if_hidden);
        drop(screen.render());
    }
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn rename_pane(screen: &mut Screen, pane_id: PaneId, new_name: Vec<u8>) -> Result<()> {
    let mut renamed = false;
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        match tab.rename_pane(new_name, pane_id) {
            Ok(changed) => {
                renamed = changed;
                if changed {
                    drop(screen.render());
                }
            },
            Err(e) => log::error!("Failed to rename pane: {:?}", e),
        }
    }
    // Nothing moved, so there is nothing to repaint and nothing new
    // to tell the plugins. Reporting the session state anyway
    // rebuilds every tab's pane manifest and pushes a
    // `PaneUpdate`/`TabUpdate` at every subscribed plugin — real
    // work, on the same threads and queues a keystroke has to cross.
    // See `Tab::rename_pane`.
    if renamed {
        screen.log_and_report_session_state()?;
    }
    Ok(())
}

pub(super) fn rename_active_pane(
    screen: &mut Screen,
    new_name: Vec<u8>,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if screen.has_no_client_to_act_for(client_id) {
        screen.report_no_client_to_act_for(
                        client_id,
                        "Cannot rename the focused pane: no client is attached to this session. Target a pane explicitly with --pane-id.".to_owned(),
                        completion_tx,
                    );
    } else {
        // the action ends here, dropping completion_tx will release anything
        // waiting for it
        active_tab_and_connected_client_id!(
            screen,
            client_id,
            |tab: &mut Tab, client_id: ClientId| tab
                .rename_active_pane(new_name, client_id),
            ?
        );
        screen.render()?;
        screen.log_and_report_session_state()?;
    }
    Ok(())
}

pub(super) fn set_project_context(
    screen: &mut Screen,
    projects: Vec<String>,
    panes: Vec<(PaneId, String)>,
    sources: BTreeMap<String, String>,
) -> Result<()> {
    screen.core.projects = projects.into_iter().collect();
    screen.core.project_sources = sources;
    screen.core.pane_projects = panes
        .into_iter()
        .filter(|(_, project)| screen.core.projects.contains(project))
        .collect();
    if !screen.migration_blocks_normal_work() {
        screen.reconcile_project_contexts();
    }
    screen.log_and_report_session_state()?;
    screen.render()?;
    Ok(())
}

pub(super) fn replace_pane(
    state: &mut EventLoopState,
    screen: &mut Screen,
    new_pane_id: PaneId,
    hold_for_command: HoldForCommand,
    pane_title: Option<InitialTitle>,
    invoked_with: Option<Run>,
    replaced: ReplacedPane,
    client_id_tab_index_or_pane_id: ClientTabIndexOrPaneId,
    mut completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    let placed = screen.replace_pane(
        new_pane_id,
        hold_for_command,
        invoked_with,
        pane_title,
        replaced,
        client_id_tab_index_or_pane_id,
        &mut completion_tx,
    );
    let waiting = state.pending.waiting_for_pane.remove(&new_pane_id);
    if placed {
        if let Some(c) = completion_tx.as_mut() {
            c.set_affected_pane_id(new_pane_id)
        }
        // A plugin can open an agent pane in place, the same as anywhere else.
        screen.fire_agent_start_for_new_pane(new_pane_id);
        // Its reader can produce output before the replacement is placed, just
        // as it can before NewPane. Agent-start precedes the buffered events.
        for event in waiting.into_iter().flatten() {
            replay_pending_pane_event(screen, event, &mut state.pending).non_fatal();
        }
    }

    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn run_command_in_pane(
    screen: &mut Screen,
    terminal_pane_id: TerminalId,
    run_command: RunCommand,
    keep_scrollback: bool,
) -> Result<()> {
    screen.run_command_in_pane_with_id(terminal_pane_id, run_command, keep_scrollback);
    screen.log_and_report_session_state()?;
    screen.render()?;
    Ok(())
}

pub(super) fn move_pane_with_pane_id_in_direction(
    screen: &mut Screen,
    pane_id: PaneId,
    direction: Direction,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        match direction {
            Direction::Down => tab.move_pane_down(pane_id),
            Direction::Up => tab.move_pane_up(pane_id),
            Direction::Left => tab.move_pane_left(pane_id),
            Direction::Right => tab.move_pane_right(pane_id),
        }
    }
    screen.render()?;
    Ok(())
}

pub(super) fn stack_panes(
    screen: &mut Screen,
    pane_ids_to_stack: Vec<PaneId>,
    client_id: ClientId,
) -> Result<()> {
    if let Some(last_pane_id) = screen.stack_panes(pane_ids_to_stack) {
        let _ = screen.focus_pane_with_id(last_pane_id, false, false, client_id);
        let _ = screen.render();
        let pane_group = screen.get_client_pane_group(&client_id);
        if !pane_group.is_empty() {
            let _ = screen.core.bus.senders.send_to_background_jobs(
                BackgroundJob::HighlightPanesWithMessage(
                    pane_group.iter().copied().collect(),
                    "STACKED".to_owned(),
                ),
            );
        }
        screen.clear_pane_group(&client_id);
    }
    Ok(())
}

pub(super) fn change_overlay_coordinates(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
    override_user_placement: bool,
) -> Result<()> {
    for (pane_id, coordinates) in
        screen.change_overlay_coordinates(pane_ids_and_coordinates, override_user_placement)
    {
        state
            .pending
            .waiting_for_pane
            .entry(pane_id)
            .or_default()
            .push(ScreenInstruction::ChangeOverlayCoordinates(
                vec![(pane_id, coordinates)],
                override_user_placement,
                None,
            ));
    }
    let _ = screen.render();
    Ok(())
}

pub(super) fn group_and_ungroup_panes(
    screen: &mut Screen,
    pane_ids_to_group: Vec<PaneId>,
    pane_ids_to_ungroup: Vec<PaneId>,
    for_all_clients: bool,
    client_id: ClientId,
) -> Result<()> {
    screen.group_and_ungroup_panes(
        pane_ids_to_group,
        pane_ids_to_ungroup,
        for_all_clients,
        client_id,
    );
    let _ = screen.log_and_report_session_state();
    Ok(())
}

pub(super) fn highlight_and_unhighlight_panes(
    screen: &mut Screen,
    pane_ids_to_highlight: Vec<PaneId>,
    pane_ids_to_unhighlight: Vec<PaneId>,
    client_id: ClientId,
) -> Result<()> {
    {
        for pane_id in pane_ids_to_highlight {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                tab.add_highlight_pane_frame_color_override(pane_id, None, Some(client_id));
            }
        }
        for pane_id in pane_ids_to_unhighlight {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                tab.clear_pane_frame_color_override(pane_id, Some(client_id));
            }
        }
        screen.render()?;
    }
    let _ = screen.log_and_report_session_state();
    Ok(())
}

pub(super) fn float_multiple_panes(
    screen: &mut Screen,
    pane_ids_to_float: Vec<PaneId>,
    client_id: ClientId,
) -> Result<()> {
    if screen.selection_has_multipane(&pane_ids_to_float) {
        Err::<(), _>(anyhow!(
            "cannot detach a multipane child from its container"
        ))
        .non_fatal();
        return Ok(());
    }
    {
        let mut ejected_panes_in_group = vec![];
        for pane_id in pane_ids_to_float {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                if !tab.pane_id_is_floating(&pane_id) {
                    ejected_panes_in_group.push(pane_id);
                    tab.toggle_pane_embed_or_floating_for_pane_id(pane_id, Some(client_id))
                        .non_fatal();
                }
                tab.show_floating_panes();
            }
        }
        screen.render()?;
        if !ejected_panes_in_group.is_empty() {
            let _ = screen.core.bus.senders.send_to_background_jobs(
                BackgroundJob::HighlightPanesWithMessage(
                    ejected_panes_in_group,
                    "EJECTED".to_owned(),
                ),
            );
        }
    }
    let _ = screen.log_and_report_session_state();
    Ok(())
}

pub(super) fn embed_multiple_panes(
    screen: &mut Screen,
    pane_ids_to_float: Vec<PaneId>,
    client_id: ClientId,
) -> Result<()> {
    if screen.selection_has_multipane(&pane_ids_to_float) {
        Err::<(), _>(anyhow!(
            "cannot detach a multipane child from its container"
        ))
        .non_fatal();
        return Ok(());
    }
    {
        let mut embedded_panes_in_group = vec![];
        for pane_id in pane_ids_to_float {
            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                if tab.pane_id_is_floating(&pane_id) {
                    embedded_panes_in_group.push(pane_id);
                    tab.toggle_pane_embed_or_floating_for_pane_id(pane_id, Some(client_id))
                        .non_fatal();
                }
                tab.hide_floating_panes();
            }
        }
        screen.render()?;
        if !embedded_panes_in_group.is_empty() {
            let _ = screen.core.bus.senders.send_to_background_jobs(
                BackgroundJob::HighlightPanesWithMessage(
                    embedded_panes_in_group,
                    "EMBEDDED".to_owned(),
                ),
            );
        }
    }
    let _ = screen.log_and_report_session_state();
    Ok(())
}

pub(super) fn move_pane_with_pane_id_cli(
    screen: &mut Screen,
    pane_id: PaneId,
    direction: Option<Direction>,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.move_pane_by_pane_id(pane_id, direction);
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn move_pane_backwards_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.move_pane_backwards_by_pane_id(pane_id);
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn toggle_fullscreen_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.toggle_fullscreen_by_pane_id(pane_id);
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn toggle_no_ui_fullscreen_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.toggle_no_ui_fullscreen_by_pane_id(pane_id);
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn toggle_pane_embed_or_floating_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    mut completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        if let Err(error) = tab.toggle_pane_embed_or_floating_for_pane_id(pane_id, None) {
            log::error!("{error:#}");
            if let Some(completion) = completion_tx.as_mut() {
                completion.set_error_message(format!("{error:#}"));
                completion.set_exit_status(1);
            }
        }
    } else {
        Screen::report_pane_not_found(pane_id, &mut completion_tx);
    }
    screen.log_and_report_session_state()?;
    screen.render()?;
    Ok(())
}

pub(super) fn rename_pane_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    name: Vec<u8>,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.rename_pane_by_pane_id(pane_id, name).non_fatal();
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

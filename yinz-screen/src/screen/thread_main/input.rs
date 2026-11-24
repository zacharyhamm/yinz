use super::*;
use yinz_bus::plugins::PluginId;
use yinz_bus::route::NotificationEnd;
use yinz_bus::ClientId;
use yinz_bus::PaneId;
use yinz_types::data::GetFocusedPaneInfoResponse;
use yinz_types::data::InputMode;
use yinz_types::data::PaneScrollbackResponse;
use yinz_types::keys::KeyWithModifier;
use yinz_types::position::Position;

pub(super) fn context_menu_handle_key_if_open(
    screen: &mut Screen,
    key: Option<KeyWithModifier>,
    toggle_command_palette: bool,
    open_agents: bool,
    client_id: ClientId,
    completion: Option<NotificationEnd>,
) -> Result<()> {
    let handled = if open_agents && screen.palette.command_palette_is_open(client_id) {
        screen.open_agents(client_id);
        screen.render()?;
        true
    } else {
        screen.command_palette_handles_input(&key, &[], toggle_command_palette, client_id)
            || screen.context_menu_handles_key(&key, client_id)
    };
    if let Some(mut completion) = completion {
        completion.set_stdout_message(handled.to_string());
    }
    Ok(())
}

pub(super) fn write_character(
    state: &mut EventLoopState,
    screen: &mut Screen,
    key_with_modifier: Option<KeyWithModifier>,
    raw_bytes: Vec<u8>,
    is_kitty_keyboard_protocol: bool,
    client_id: ClientId,
    completion: Option<NotificationEnd>,
) -> Result<()> {
    if screen.write_character(
        key_with_modifier,
        raw_bytes,
        is_kitty_keyboard_protocol,
        client_id,
        &state.keybind_intercepts,
        completion,
    )? {
        screen.render()?;
    }
    Ok(())
}

pub(super) fn switch_focus(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    let old_pane_id = screen.get_active_pane_id(&client_id);
    screen.cycle_focus(client_id, true);
    let new_pane_id = screen.get_active_pane_id(&client_id);
    if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
        screen.report_key_passthrough_state(client_id, old, new);
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn focus_next_pane(
    screen: &mut Screen,
    client_id: ClientId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if screen.get_first_client_id().is_none() {
        log::error!("No connected clients to change focus for");
        if let Some(ref mut c) = _completion_tx {
            c.set_exit_status(1);
            c.set_error_message("No connected clients to change focus for".to_string());
        }
    } else {
        let old_pane_id = screen.get_active_pane_id(&client_id);
        screen.cycle_focus(client_id, true);
        let new_pane_id = screen.get_active_pane_id(&client_id);
        if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
            screen.report_key_passthrough_state(client_id, old, new);
        }
        screen.sync_scroll_mode_on_focus(client_id)?;
        screen.render()?;
        screen.log_and_report_session_state()?;
    }
    Ok(())
}

pub(super) fn focus_previous_pane(
    screen: &mut Screen,
    client_id: ClientId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if screen.get_first_client_id().is_none() {
        log::error!("No connected clients to change focus for");
        if let Some(ref mut c) = _completion_tx {
            c.set_exit_status(1);
            c.set_error_message("No connected clients to change focus for".to_string());
        }
    } else {
        let old_pane_id = screen.get_active_pane_id(&client_id);
        screen.cycle_focus(client_id, false);
        let new_pane_id = screen.get_active_pane_id(&client_id);
        if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
            screen.report_key_passthrough_state(client_id, old, new);
        }
        screen.sync_scroll_mode_on_focus(client_id)?;
        screen.render()?;
        screen.log_and_report_session_state()?;
    }
    Ok(())
}

pub(super) fn focus_last_pane(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    let old_pane_id = screen.get_active_pane_id(&client_id);
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.focus_last_pane(client_id)
    });
    let new_pane_id = screen.get_active_pane_id(&client_id);
    if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
        screen.report_key_passthrough_state(client_id, old, new);
    }
    screen.sync_scroll_mode_on_focus(client_id)?;
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn move_focus_left(
    screen: &mut Screen,
    client_id: ClientId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if screen.get_first_client_id().is_none() {
        log::error!("No connected clients to move focus for");
        if let Some(ref mut c) = _completion_tx {
            c.set_exit_status(1);
            c.set_error_message("No connected clients to move focus for".to_string());
        }
    } else {
        screen.move_focus_in_direction_or_escalate(Direction::Left, client_id)?;
    }
    Ok(())
}

pub(super) fn move_focus_left_or_previous_tab(
    screen: &mut Screen,
    client_id: ClientId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if screen.get_first_client_id().is_none() {
        log::error!("No connected clients to move focus for");
        if let Some(ref mut c) = _completion_tx {
            c.set_exit_status(1);
            c.set_error_message("No connected clients to move focus for".to_string());
        }
    } else {
        let old_pane_id = screen.get_active_pane_id(&client_id);
        screen.move_focus_left_or_previous_tab(client_id)?;
        let new_pane_id = screen.get_active_pane_id(&client_id);
        if let (Some(old), Some(new)) = (old_pane_id, new_pane_id)
            && old != new
        {
            screen.report_key_passthrough_state_with_direction(
                client_id,
                old,
                new,
                Some(Direction::Left),
            );
        }
        screen.clear_bell_for_focused_pane(client_id);
        screen.add_active_pane_to_group_if_marking(&client_id);
        screen.sync_scroll_mode_on_focus(client_id)?;
        screen.render()?;
        screen.log_and_report_session_state()?;
    }
    Ok(())
}

pub(super) fn move_focus_down(
    screen: &mut Screen,
    client_id: ClientId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if screen.get_first_client_id().is_none() {
        log::error!("No connected clients to move focus for");
        if let Some(ref mut c) = _completion_tx {
            c.set_exit_status(1);
            c.set_error_message("No connected clients to move focus for".to_string());
        }
    } else {
        screen.move_focus_in_direction_or_escalate(Direction::Down, client_id)?;
    }
    Ok(())
}

pub(super) fn move_focus_right(
    screen: &mut Screen,
    client_id: ClientId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if screen.get_first_client_id().is_none() {
        log::error!("No connected clients to move focus for");
        if let Some(ref mut c) = _completion_tx {
            c.set_exit_status(1);
            c.set_error_message("No connected clients to move focus for".to_string());
        }
    } else {
        screen.move_focus_in_direction_or_escalate(Direction::Right, client_id)?;
    }
    Ok(())
}

pub(super) fn move_focus_right_or_next_tab(
    screen: &mut Screen,
    client_id: ClientId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if screen.get_first_client_id().is_none() {
        log::error!("No connected clients to move focus for");
        if let Some(ref mut c) = _completion_tx {
            c.set_exit_status(1);
            c.set_error_message("No connected clients to move focus for".to_string());
        }
    } else {
        let old_pane_id = screen.get_active_pane_id(&client_id);
        screen.move_focus_right_or_next_tab(client_id)?;
        let new_pane_id = screen.get_active_pane_id(&client_id);
        if let (Some(old), Some(new)) = (old_pane_id, new_pane_id)
            && old != new
        {
            screen.report_key_passthrough_state_with_direction(
                client_id,
                old,
                new,
                Some(Direction::Right),
            );
        }
        screen.clear_bell_for_focused_pane(client_id);
        screen.add_active_pane_to_group_if_marking(&client_id);
        screen.sync_scroll_mode_on_focus(client_id)?;
        screen.render()?;
        screen.log_and_report_session_state()?;
    }
    Ok(())
}

pub(super) fn move_focus_up(
    screen: &mut Screen,
    client_id: ClientId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if screen.get_first_client_id().is_none() {
        log::error!("No connected clients to move focus for");
        if let Some(ref mut c) = _completion_tx {
            c.set_exit_status(1);
            c.set_error_message("No connected clients to move focus for".to_string());
        }
    } else {
        screen.move_focus_in_direction_or_escalate(Direction::Up, client_id)?;
    }
    Ok(())
}

pub(super) fn get_focused_pane_info(
    screen: &mut Screen,
    client_id: ClientId,
    response_channel: crossbeam::channel::Sender<GetFocusedPaneInfoResponse>,
) -> Result<()> {
    let response = match screen.core.active_tab_ids.get(&client_id) {
        Some(&focused_tab_id) => match screen.get_active_pane_id(&client_id) {
            Some(focused_pane_id) => GetFocusedPaneInfoResponse::Ok {
                tab_id: focused_tab_id,
                pane_id: focused_pane_id,
            },
            None => GetFocusedPaneInfoResponse::Err(format!(
                "No active pane found for client {:?}",
                client_id
            )),
        },
        None => GetFocusedPaneInfoResponse::Err(format!(
            "Client {:?} not found in active_tab_indices",
            client_id
        )),
    };
    let _ = response_channel.send(response);
    Ok(())
}

pub(super) fn edit_scrollback(
    screen: &mut Screen,
    client_id: ClientId,
    ansi: bool,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        // A dump that cannot be written (for example, an unsafe dump
        // folder) or an editor that cannot be requested fails this one
        // edit, not the session.
        if ansi {
            tab.edit_scrollback_raw(client_id, completion_tx)
        } else {
            tab.edit_scrollback(client_id, completion_tx)
        }
        .non_fatal();
    });
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn get_pane_scrollback(
    screen: &mut Screen,
    pane_id: PaneId,
    client_id: ClientId,
    get_full_scrollback: bool,
    response_channel: crossbeam::channel::Sender<PaneScrollbackResponse>,
) -> Result<()> {
    let last_output_ms_ago = screen.core.pane_output_activity.get(&pane_id).map(|last| {
        yinz_types::units::ElapsedMillis::try_from(
            Instant::now().saturating_duration_since(*last).as_millis(),
        )
        .unwrap_or_else(|_| yinz_types::units::ElapsedMillis::new(u64::MAX))
    });
    let pane_contents = screen.with_pane(pane_id, |pane| {
        pane.pane_contents(Some(client_id), get_full_scrollback, None)
    });
    // Send response back through channel
    let response = match pane_contents {
        Some(mut contents) => {
            contents.last_output_ms_ago = last_output_ms_ago;
            PaneScrollbackResponse::Ok(contents)
        },
        None => {
            log::warn!(
                "Plugin requested scrollback for pane {:?} but pane was not found",
                pane_id
            );
            PaneScrollbackResponse::Err(format!("Pane {:?} not found", pane_id))
        },
    };
    if response_channel.send(response).is_err() {
        // the plugin likely timed out and dropped the receiver
        log::debug!(
            "Plugin timed out before pane scrollback response was sent for pane {:?}",
            pane_id
        );
    }
    Ok(())
}

pub(super) fn scroll_up(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    let was_scrolled = screen.active_pane_is_scrolled(client_id);
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.scroll_active_terminal_up(client_id)
    });
    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
    screen.render()?;
    Ok(())
}

pub(super) fn scroll_up_at(
    screen: &mut Screen,
    point: Position,
    client_id: ClientId,
) -> Result<()> {
    if !screen.command_palette_handles_mouse(
        &yinz_types::mouse::MouseEvent::new_scroll_up_event(point),
        client_id,
    ) {
        active_tab_and_connected_client_id!(
            screen,
            client_id,
            |tab: &mut Tab, client_id: ClientId| tab
                .handle_scrollwheel_up(&point, 3, client_id), ?
        );
        screen.render()?;
    }
    Ok(())
}

pub(super) fn scroll_down(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    let was_scrolled = screen.active_pane_is_scrolled(client_id);
    active_tab_and_connected_client_id!(
        screen,
        client_id,
        |tab: &mut Tab, client_id: ClientId| tab.scroll_active_terminal_down(client_id), ?
    );
    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
    screen.render()?;
    Ok(())
}

pub(super) fn scroll_down_at(
    screen: &mut Screen,
    point: Position,
    client_id: ClientId,
) -> Result<()> {
    if !screen.command_palette_handles_mouse(
        &yinz_types::mouse::MouseEvent::new_scroll_down_event(point),
        client_id,
    ) {
        active_tab_and_connected_client_id!(
            screen,
            client_id,
            |tab: &mut Tab, client_id: ClientId| tab
                .handle_scrollwheel_down(&point, 3, client_id), ?
        );
        screen.render()?;
    }
    Ok(())
}

pub(super) fn scroll_to_bottom(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    let was_scrolled = screen.active_pane_is_scrolled(client_id);
    active_tab_and_connected_client_id!(
        screen,
        client_id,
        |tab: &mut Tab, client_id: ClientId| tab
            .scroll_active_terminal_to_bottom(client_id), ?
    );
    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
    screen.render()?;
    Ok(())
}

pub(super) fn scroll_to_top(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    let was_scrolled = screen.active_pane_is_scrolled(client_id);
    active_tab_and_connected_client_id!(
        screen,
        client_id,
        |tab: &mut Tab, client_id: ClientId| tab
            .scroll_active_terminal_to_top(client_id), ?
    );
    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
    screen.render()?;
    Ok(())
}
// The prompt jumps deliberately do not sync scroll mode: they are bound in normal
// mode too, and jumping to a prompt from there must not switch modes

pub(super) fn scroll_to_previous_prompt(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.scroll_active_terminal_to_previous_prompt(client_id)
    });
    screen.render()?;
    Ok(())
}

pub(super) fn scroll_to_next_prompt(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.scroll_active_terminal_to_next_prompt(client_id)
    });
    screen.render()?;
    Ok(())
}

pub(super) fn select_command_at_scroll_position(
    screen: &mut Screen,
    client_id: ClientId,
) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.select_command_at_scroll_position(client_id)
    });
    screen.render()?;
    Ok(())
}

pub(super) fn copy_last_command_output(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(
        screen,
        client_id,
        |tab: &mut Tab, client_id: ClientId| tab.copy_last_command_output(client_id),
        ?
    );
    screen.render()?;
    Ok(())
}

pub(super) fn page_scroll_up(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    let was_scrolled = screen.active_pane_is_scrolled(client_id);
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.scroll_active_terminal_up_page(client_id)
    });
    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
    screen.render()?;
    Ok(())
}

pub(super) fn page_scroll_down(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    let was_scrolled = screen.active_pane_is_scrolled(client_id);
    active_tab_and_connected_client_id!(
        screen,
        client_id,
        |tab: &mut Tab, client_id: ClientId| tab
            .scroll_active_terminal_down_page(client_id), ?
    );
    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
    screen.render()?;
    Ok(())
}

pub(super) fn half_page_scroll_up(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    let was_scrolled = screen.active_pane_is_scrolled(client_id);
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.scroll_active_terminal_up_half_page(client_id)
    });
    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
    screen.render()?;
    Ok(())
}

pub(super) fn half_page_scroll_down(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    let was_scrolled = screen.active_pane_is_scrolled(client_id);
    active_tab_and_connected_client_id!(
        screen,
        client_id,
        |tab: &mut Tab, client_id: ClientId| tab
            .scroll_active_terminal_down_half_page(client_id), ?
    );
    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
    screen.render()?;
    Ok(())
}
// Writes captured by the palette must not scroll the underlying pane either

pub(super) fn clear_scroll(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    let was_scrolled = screen.active_pane_is_scrolled(client_id);
    let mut scroll_was_cleared = was_scrolled;
    active_tab_and_connected_client_id!(
        screen,
        client_id,
        |tab: &mut Tab, client_id: ClientId| -> Result<()> {
            scroll_was_cleared = tab.clear_active_terminal_scroll(client_id)?;
            Ok(())
        }, ?
    );
    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
    if scroll_was_cleared {
        screen.render()?;
    }
    Ok(())
}

pub(super) fn close_focused_pane(
    screen: &mut Screen,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    let old_pane_id = screen.close_focused_pane(client_id, completion_tx)?;
    let new_pane_id = screen.get_active_pane_id(&client_id);
    if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
        screen.report_key_passthrough_state(client_id, old, new);
    }
    screen.sync_scroll_mode_on_focus(client_id)?;
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn set_mouse_selection_support(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pid: PaneId,
    selection_support: bool,
) -> Result<()> {
    if screen
        .with_pane(pid, |pane| {
            if let Some(plugin) = pane.plugin_control_mut() {
                plugin.set_mouse_selection_support(selection_support);
            }
        })
        .is_none()
    {
        state
            .pending
            .waiting_for_tab
            .push(ScreenInstruction::SetMouseSelectionSupport(
                pid,
                selection_support,
            ));
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

pub(super) fn change_mode(
    state: &mut EventLoopState,
    screen: &mut Screen,
    input_mode: InputMode,
    base_mode: Option<InputMode>,
    client_id: ClientId,
) -> Result<()> {
    let trace_started_at = state
        .attach_traces_waiting_for_first_render
        .get(&client_id)
        .copied();
    if let Some(started_at) = trace_started_at {
        log::info!(
            "attach trace: screen client_id={client_id} attach ChangeMode begin after {:?}",
            started_at.elapsed()
        );
    }
    screen.change_mode(input_mode, base_mode, client_id)?;
    if let Some(started_at) = trace_started_at {
        log::info!(
            "attach trace: screen client_id={client_id} attach ChangeMode complete after {:?}",
            started_at.elapsed()
        );
    }
    screen.render()?;
    Ok(())
}

pub(super) fn copy(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    let selecting = screen
        .core
        .mode_info
        .get(&client_id)
        .is_some_and(|mode_info| mode_info.mode == InputMode::Select);
    screen.copy_selection(client_id)?;
    if selecting {
        active_tab_and_connected_client_id!(
            screen,
            client_id,
            |tab: &mut Tab, client_id: ClientId| tab.remember_copied_text_selection(client_id)
        );
    }
    screen.render()?;
    Ok(())
}

pub(super) fn search_toggle_case_sensitivity(
    screen: &mut Screen,
    client_id: ClientId,
) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.toggle_search_case_sensitivity(client_id)
    });
    screen.render()?;
    Ok(())
}

pub(super) fn search_toggle_whole_word(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| {
        tab.toggle_search_whole_words(client_id)
    });
    screen.render()?;
    Ok(())
}

pub(super) fn focus_pane_with_id(
    state: &mut EventLoopState,
    screen: &mut Screen,
    pane_id: PaneId,
    should_float_if_hidden: bool,
    should_be_in_place_if_hidden: bool,
    client_id: ClientId,
    completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    screen.focus_pane_by_request(
        pane_id,
        should_float_if_hidden,
        should_be_in_place_if_hidden,
        client_id,
        completion_tx,
        &mut state.pending,
    )?;
    Ok(())
}

pub(super) fn paste(
    screen: &mut Screen,
    bytes: Vec<u8>,
    pane_id: Option<PaneId>,
    client_id: ClientId,
    _completion: Option<NotificationEnd>,
) -> Result<()> {
    match pane_id {
        None if screen.command_palette_handles_input(&None, &bytes, false, client_id) => {},
        Some(pane_id) => {
            screen
                .paste_to_pane_id(bytes, pane_id, Some(client_id), _completion)
                .non_fatal();
        },
        None => {
            active_tab_and_connected_client_id!(
                screen,
                client_id,
                |tab: &mut Tab, _client_id: ClientId| {
                    tab.paste_to_active_terminal(bytes, client_id, _completion)
                        .non_fatal();
                }
            );
        },
    }
    screen.render()?;
    Ok(())
}

pub(super) fn write_key_to_pane_id(
    screen: &mut Screen,
    key_with_modifier: Option<KeyWithModifier>,
    bytes: Vec<u8>,
    is_kitty: bool,
    pane_id: PaneId,
    _completion: Option<NotificationEnd>,
) -> Result<()> {
    screen
        .write_to_pane_id(
            &key_with_modifier,
            bytes,
            is_kitty,
            pane_id,
            None,
            _completion,
        )
        .non_fatal();
    screen.render()?;
    Ok(())
}

pub(super) fn copy_text_to_clipboard(
    screen: &mut Screen,
    text: String,
    plugin_id: PluginId,
) -> Result<()> {
    let plugin_pane_id = PaneId::Plugin(plugin_id);
    if let Some(tab) = screen.tab_containing_pane_mut(plugin_pane_id) {
        tab.copy_text_to_clipboard(&text)
            .with_context(|| format!("failed to copy text to clipboard from plugin {}", plugin_id))
            .non_fatal();
    }
    screen.render()?;
    Ok(())
}

pub(super) fn scroll_up_in_pane_id(screen: &mut Screen, pane_id: PaneId) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        if let PaneId::Terminal(terminal_pane_id) = pane_id {
            tab.scroll_terminal_up(terminal_pane_id)?;
        } else {
            // this is because to do this with plugins, we need the client_id -
            // which we do not have (yet?) in this context...
            log::error!("Currently only terminal panes are supported for scrolling up");
        }
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn scroll_down_in_pane_id(screen: &mut Screen, pane_id: PaneId) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        if let PaneId::Terminal(terminal_pane_id) = pane_id {
            tab.scroll_terminal_down(terminal_pane_id)?;
        } else {
            // this is because to do this with plugins, we need the client_id -
            // which we do not have (yet?) in this context...
            log::error!("Currently only terminal panes are supported for scrolling down");
        }
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn scroll_to_top_in_pane_id(screen: &mut Screen, pane_id: PaneId) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        if let PaneId::Terminal(terminal_pane_id) = pane_id {
            tab.scroll_terminal_to_top(terminal_pane_id);
        } else {
            // this is because to do this with plugins, we need the client_id -
            // which we do not have (yet?) in this context...
            log::error!("Currently only terminal panes are supported for scrolling to top");
        }
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn scroll_to_bottom_in_pane_id(screen: &mut Screen, pane_id: PaneId) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        if let PaneId::Terminal(terminal_pane_id) = pane_id {
            tab.scroll_terminal_to_bottom(terminal_pane_id);
        } else {
            // this is because to do this with plugins, we need the client_id -
            // which we do not have (yet?) in this context...
            log::error!("Currently only terminal panes are supported for scrolling to bottom");
        }
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn page_scroll_up_in_pane_id(screen: &mut Screen, pane_id: PaneId) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        if let PaneId::Terminal(terminal_pane_id) = pane_id {
            tab.scroll_terminal_page_up(terminal_pane_id);
        } else {
            // this is because to do this with plugins, we need the client_id -
            // which we do not have (yet?) in this context...
            log::error!("Currently only terminal panes are supported for scrolling");
        }
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn page_scroll_down_in_pane_id(screen: &mut Screen, pane_id: PaneId) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        if let PaneId::Terminal(terminal_pane_id) = pane_id {
            tab.scroll_terminal_page_down(terminal_pane_id);
        } else {
            // this is because to do this with plugins, we need the client_id -
            // which we do not have (yet?) in this context...
            log::error!("Currently only terminal panes are supported for scrolling");
        }
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn scroll_up_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.scroll_up_by_pane_id(pane_id);
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn scroll_down_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.scroll_down_by_pane_id(pane_id).non_fatal();
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn scroll_to_top_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.scroll_to_top_by_pane_id(pane_id).non_fatal();
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn scroll_to_bottom_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.scroll_to_bottom_by_pane_id(pane_id).non_fatal();
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn page_scroll_up_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.page_scroll_up_by_pane_id(pane_id);
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn page_scroll_down_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.page_scroll_down_by_pane_id(pane_id).non_fatal();
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn half_page_scroll_up_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.half_page_scroll_up_by_pane_id(pane_id);
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn half_page_scroll_down_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    mut _completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        tab.half_page_scroll_down_by_pane_id(pane_id).non_fatal();
    } else {
        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
    }
    screen.sync_scroll_mode_for_pane_id(pane_id)?;
    screen.render()?;
    Ok(())
}

pub(super) fn edit_scrollback_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    ansi: bool,
    mut completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
        if ansi {
            tab.edit_scrollback_raw_for_pane_with_id(pane_id, completion_tx)
                .non_fatal();
        } else {
            tab.edit_scrollback_for_pane_with_id(pane_id, completion_tx)
                .non_fatal();
        }
    } else {
        Screen::report_pane_not_found(pane_id, &mut completion_tx);
    }
    screen.render()?;
    Ok(())
}

pub(super) fn close_focus_with_pane_id(
    screen: &mut Screen,
    pane_id: PaneId,
    mut completion_tx: Option<NotificationEnd>,
) -> Result<()> {
    // A headless plugin was unloaded by `settle_headless_target`. A pane that is
    // already gone is closed; one that never existed is an error, not a success.
    if screen.tab_containing_pane(pane_id).is_some() {
        screen.close_pane_explicitly(pane_id, completion_tx);
    } else if !screen.pane_is_gone(pane_id) {
        Screen::report_pane_not_found(pane_id, &mut completion_tx);
    }
    let client_ids: Vec<_> = screen.core.active_tab_ids.keys().copied().collect();
    for client_id in client_ids {
        screen.sync_scroll_mode_on_focus(client_id)?;
    }
    screen.render()?;
    screen.log_and_report_session_state()?;
    Ok(())
}

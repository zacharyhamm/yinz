mod plugin_launch;

pub(crate) use super::agent_reports::apply_pane_agent_report;
pub(super) use super::agent_reports::{apply_pane_foreground_command, apply_queued_agent_report};
pub(super) use super::pending_events::{replay_pending_pane_event, PendingEvents};

use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;

use super::{active_tab, active_tab_and_connected_client_id, Screen, ScreenInstruction};
use crate::background_jobs::BackgroundJob;
use crate::notifications::denormalize_notification_response;
use crate::os_input_output::ResizeCache;
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
use crate::pty::{ClientTabIndexOrPaneId, PtyInstruction};
use crate::tab::Tab;
use crate::thread_bus::Bus;
use crate::{ClientId, ServerInstruction};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use yinz_config::config::Config;
use yinz_config::screen_config::ScreenConfig;
use yinz_ipc::get_mode_info;
use yinz_ipc::ipc::ClientAttributes;
use yinz_ipc::nested_session;
use yinz_ipc::nested_session::NestedSessionMessage;
use yinz_types::diagnostics::ContextType;
use yinz_types::geometry::Direction;
use yinz_types::input::layout::Layout;
use yinz_types::input::options::PaneFrameStyle;
use yinz_types::style::HostTerminalThemeMode;
use {
    yinz_types::data::EventType, yinz_types::data::GetFocusedPaneInfoResponse,
    yinz_types::data::InputMode, yinz_types::data::PaneScrollbackResponse,
    yinz_types::data::PluginCapabilities,
};

/// Whether `raw_bytes` is text a tab name, a pane name or a search term can take.
///
/// A key that only has an escape encoding is not something anyone means to type into a name:
/// `clean_string_from_control_and_linebreak` strips the ESC and leaves the rest of the CSI
/// sequence behind as literal characters, so `Alt+t` in rename mode would append `[116;3u`. Under
/// the Kitty keyboard flags `report_modifier_keys` pushes, every printable key arrives as a CSI
/// sequence and is resolved back to a character before it gets here - but a host that ignores
/// `REPORT_ALTERNATE_KEYS` can still leave one unresolved, and that must not be typed either.
///
/// The bracketed-paste markers this used to name explicitly are covered by the same check: both
/// begin with ESC. Real pastes use `PasteFromClient` and update names/search directly.
pub(super) fn is_typable_text(raw_bytes: &[u8]) -> bool {
    !raw_bytes.starts_with(&[0x1b])
}

pub fn screen_thread_main(
    bus: Bus<ScreenInstruction>,
    max_panes: Option<usize>,
    client_attributes: ClientAttributes,
    config: Config,
    debug: bool,
    config_file_path: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    default_layout: Box<Layout>,
) -> Result<()> {
    let mut screen_config = ScreenConfig::from_config(&config);
    screen_config.layout_dir = yinz_runtime::home::layout_dir_or_default(
        config.options.layout_dir.clone(),
        config_dir.clone(),
    );
    let explicit_theme_hue = screen_config.explicit_theme_hue;
    let default_mode = screen_config.default_mode;
    let mode_info = get_mode_info(
        default_mode.unwrap_or_default(),
        &client_attributes,
        PluginCapabilities {
            //  ¯\_(ツ)_/¯
            arrow_fonts: screen_config.simplified_ui,
            // Not known until a client's startup probe answers; see
            // `Screen::update_kitty_graphics_support`.
            kitty_graphics: false,
        },
        &config.keybinds,
        default_mode,
    );
    let thread_senders = bus.senders.clone();
    let mut screen = Screen::new(
        bus,
        &client_attributes,
        max_panes,
        mode_info,
        debug,
        default_layout,
        screen_config,
    );
    screen.config_file_path = config_file_path;
    screen.config_dir = config_dir;
    if let Some(data_dir) = data_dir {
        screen.last_project = yinz_runtime::last_project::LastProject::load(data_dir);
    }
    if screen.config_file_path.is_some() {
        screen.restart_layout_watcher();
    }
    // Resolve the session's hue before the first render: an explicit pin
    // wins; otherwise a dark/light pair defaults to dark until the host
    // terminal reports.
    if explicit_theme_hue.is_some() {
        screen
            .apply_configured_explicit_theme_hue(explicit_theme_hue)
            .non_fatal();
    } else {
        screen.resolve_default_theme_mode().non_fatal();
    }

    let mut pending = PendingEvents::default();
    let mut attach_traces_waiting_for_first_render: HashMap<ClientId, Instant> = HashMap::new();
    let mut keybind_intercepts = HashMap::new();
    let mut escape_focus = HashMap::new();
    let mut pending_escape_count = 0;
    'instructions: loop {
        screen.refresh_agent_controls();
        if screen.agent_events.dirty {
            screen.refresh_agent_events();
            screen.refresh_agent_event_projects();
        }
        screen.retry_agent_event_projects(Instant::now());
        screen.flush_agent_events();
        if screen.agent_double_escape {
            // Observe every screen transition, including programmatic focus changes. Keeping the
            // previous focus/mode catches leaving and returning before the next key arrives.
            let focus: HashMap<_, _> = screen
                .connected_clients
                .borrow()
                .iter()
                .map(|client| {
                    (
                        *client,
                        (
                            screen.get_active_pane_id(client),
                            screen.get_client_input_mode(*client),
                        ),
                    )
                })
                .collect();
            for tab in screen.tabs.values_mut() {
                tab.agent_double_escape = screen.agent_double_escape;
                tab.pending_agent_escapes.retain(|(client, _), _| {
                    focus.contains_key(client) && focus.get(client) == escape_focus.get(client)
                });
            }
            escape_focus = focus;
        }
        // Run before receiving, so even input handlers that `continue` repaint cancellations.
        let pending_now: usize = screen
            .tabs
            .values()
            .map(|tab| tab.pending_agent_escapes.len())
            .sum();
        if pending_escape_count != pending_now {
            pending_escape_count = pending_now;
            screen.render()?;
            screen.sync_animation_clock(Instant::now())?;
        }
        screen.expire_notification_requests(Instant::now())?;
        let deadline = [
            screen.agent_control_deadline(),
            screen.notification_deadline(),
        ]
        .into_iter()
        .flatten()
        .min();
        let (event, mut err_ctx) = if let Some(timeout) = deadline {
            match screen.bus.recv_timeout(timeout) {
                Ok(event) => event,
                Err(yinz_runtime::channels::RecvTimeoutError::Timeout) => continue,
                Err(error) => return Err(error).context("receive agent control event"),
            }
        } else {
            screen
                .bus
                .recv()
                .context("failed to receive event on channel")?
        };
        yinz_runtime::errors::add_call(&mut err_ctx, ContextType::Screen((&event).into()));
        // here we start caching resizes, so that we'll send them in bulk at the end of each event
        // when this cache is Dropped, for more information, see the comments in PtyWriter
        let _resize_cache = ResizeCache::new(thread_senders.clone());

        if screen.agent_double_escape {
            let reset_client = match &event {
                ScreenInstruction::MouseEvent(_, client, ..)
                | ScreenInstruction::HostTerminalFocusChanged(client, _)
                | ScreenInstruction::RemoveClient(client)
                | ScreenInstruction::ChangeMode(_, _, client, _) => Some(*client),
                ScreenInstruction::WriteCharacter(key, _, _, client, _)
                    if key.as_ref().is_none_or(|key| {
                        key.bare_key != yinz_types::keys::BareKey::Esc || !key.has_no_modifiers()
                    }) || keybind_intercepts.contains_key(client) =>
                {
                    Some(*client)
                },
                _ => None,
            };
            if let Some(client_id) = reset_client {
                for tab in screen.tabs.values_mut() {
                    tab.pending_agent_escapes
                        .retain(|(client, _), _| *client != client_id);
                }
            }
        }

        // The same conversion applies to exits replayed after layout application.
        let event = match event {
            ScreenInstruction::CloseSelf(plugin, client) => {
                screen.close_self_instruction(plugin, client)
            },
            ScreenInstruction::ClosePane(id, None, None, status) => {
                match screen.replacement_exit_command(id) {
                    Some(command) => ScreenInstruction::HoldPane(id, status, command),
                    None => ScreenInstruction::ClosePane(id, None, None, status),
                }
            },
            event => event,
        };
        'instruction: {
            // An instruction naming a headless plugin's pane is answered here: it has none.
            let Some(event) = screen.settle_headless_target(event) else {
                break 'instruction;
            };
            match event {
                ScreenInstruction::CloseSelf(..) => {
                    unreachable!("close_self is resolved before dispatch")
                },
                ScreenInstruction::SubscribePaneOutput(plugin, client, context, generation) => {
                    let subscriber = (plugin, client, generation);
                    screen.pane_output_subscriptions.remove(&subscriber);
                    if let Some(context) = context {
                        screen.pane_output_subscriptions.insert(subscriber, context);
                    }
                },
                ScreenInstruction::UnsubscribePaneOutput(plugin, generation) => {
                    screen
                        .pane_output_subscriptions
                        .retain(|(id, _, registered), _| {
                            *id != plugin || (generation.is_some() && *registered != generation)
                        });
                },
                ScreenInstruction::PtyBytes(pid, vte_bytes) => {
                    screen.handle_pty_bytes(pid, vte_bytes, &mut pending)?;
                    let _ = screen
                        .bus
                        .senders
                        .send_to_background_jobs(BackgroundJob::RenderToClients);
                },
                ScreenInstruction::RemotePaneReady(pid) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(PaneId::Terminal(pid)) {
                        tab.remote_pane_ready(pid)
                            .context("failed to prepare remote pane")?;
                    }
                    screen.render()?;
                },
                ScreenInstruction::PluginBytes(plugin_render_assets) => {
                    for plugin_render_asset in
                        screen.handle_plugin_render_assets(plugin_render_assets)?
                    {
                        let plugin_id = plugin_render_asset.plugin_id;
                        pending
                            .waiting_for_pane
                            .entry(PaneId::Plugin(plugin_id))
                            .or_default()
                            .push(ScreenInstruction::PluginBytes(vec![plugin_render_asset]));
                    }
                },
                ScreenInstruction::Render => {
                    screen.render()?;
                },
                ScreenInstruction::RenderWriterReady => {
                    screen.resume_deferred_render();
                },
                ScreenInstruction::RenderToClients => {
                    // render_blocker.can_render() returning true means that either all pending plugins
                    // (only those waiting for a new tab layout to be applied!) have been rendered or
                    // that a 100ms timeout has been reached (more info in the RenderBlocker comment)
                    let tracing_attach = !attach_traces_waiting_for_first_render.is_empty();
                    if screen.render_blocker.can_render() {
                        let render_started_at = Instant::now();
                        if tracing_attach {
                            log::info!(
                                "attach trace: screen first render begin clients={:?}",
                                attach_traces_waiting_for_first_render
                                    .keys()
                                    .copied()
                                    .collect::<Vec<_>>()
                            );
                        }
                        let result = if tracing_attach {
                            screen.render_to_clients_traced(true)
                        } else {
                            screen.render_to_clients()
                        };
                        if tracing_attach {
                            log::info!(
                                "attach trace: screen first render finished after {:?} ({})",
                                render_started_at.elapsed(),
                                if result.is_ok() { "ok" } else { "error" }
                            );
                            if result.is_ok() {
                                attach_traces_waiting_for_first_render.clear();
                            }
                        }
                        result?;
                    } else {
                        if !attach_traces_waiting_for_first_render.is_empty() {
                            log::info!(
                            "attach trace: screen first render deferred by render blocker clients={:?}",
                            attach_traces_waiting_for_first_render
                                .keys()
                                .copied()
                                .collect::<Vec<_>>()
                        );
                        }
                        screen.render()?;
                    }
                },
                ScreenInstruction::NewPane(
                    pid,
                    initial_pane_title,
                    hold_for_command,
                    invoked_with,
                    new_pane_placement,
                    start_suppressed,
                    client_or_tab_index,
                    completion_tx,
                    set_blocking,
                ) => {
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
                            &mut pending,
                        )
                        .non_fatal();
                    screen.log_and_report_session_state()?;

                    screen.render()?;
                },
                ScreenInstruction::OpenInPlaceEditor(pid, client_tab_index_or_pane_id) => {
                    match client_tab_index_or_pane_id {
                        ClientTabIndexOrPaneId::ClientId(client_id)
                        | ClientTabIndexOrPaneId::ClientIdNoFocus(client_id) => {
                            active_tab!(screen, client_id, |tab: &mut Tab| tab
                            .replace_active_pane_with_editor_pane(pid, client_id), ?);
                        },
                        ClientTabIndexOrPaneId::TabIndex(_tab_index)
                        | ClientTabIndexOrPaneId::TabIndexNoFocus(_tab_index)
                        | ClientTabIndexOrPaneId::TabIndexForClient(_tab_index, _) => {
                            log::error!("Cannot OpenInPlaceEditor with a TabIndex");
                        },
                        ClientTabIndexOrPaneId::PaneId(pane_id_to_replace)
                        | ClientTabIndexOrPaneId::PaneIdOrClient(pane_id_to_replace, _) => {
                            // As in `replace_pane`: there is nothing to edit in place of a pane the
                            // caller does not have, so no client fallback here.
                            let placed = match screen.tab_containing_pane_mut(pane_id_to_replace) {
                                Some(tab) => {
                                    tab.replace_pane_with_editor_pane(pid, pane_id_to_replace)
                                },
                                None => {
                                    Err(anyhow!("pane {pane_id_to_replace:?} to replace not found"))
                                },
                            };
                            if let Err(error) = placed {
                                screen.forget_unplaced_pane(pid, format!("{error:#}"));
                            }
                        },
                    }

                    if let Some(pending_events) = pending.waiting_for_pane.remove(&pid) {
                        for event in pending_events {
                            replay_pending_pane_event(&mut screen, event, &mut pending).non_fatal();
                        }
                    }

                    screen.log_and_report_session_state()?;
                    screen.render()?;
                },
                ScreenInstruction::TogglePaneEmbedOrFloating(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| tab
                    .toggle_pane_embed_or_floating(client_id), ?);
                    screen.log_and_report_session_state()?;
                    screen.render()?;
                },
                ScreenInstruction::ToggleFloatingPanes(client_id, default_shell, completion_tx) => {
                    active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| tab
                    .toggle_floating_panes(Some(client_id), default_shell, completion_tx), ?);
                    screen.clear_bell_for_focused_pane(client_id);
                    screen.sync_scroll_mode_on_focus(client_id)?;
                    screen.log_and_report_session_state()?;

                    screen.render()?;
                },
                ScreenInstruction::ShowFloatingPanes {
                    client_id,
                    tab_id,
                    completion,
                } => {
                    screen.show_floating_panes_in_tab(client_id, tab_id, completion)?;
                    screen.sync_scroll_mode_on_focus(client_id)?;
                    screen.log_and_report_session_state()?;
                    screen.render()?;
                },
                ScreenInstruction::HideFloatingPanes {
                    client_id,
                    tab_id,
                    completion,
                } => {
                    screen.hide_floating_panes_in_tab(client_id, tab_id, completion)?;
                    screen.sync_scroll_mode_on_focus(client_id)?;
                    screen.log_and_report_session_state()?;
                    screen.render()?;
                },
                ScreenInstruction::AreFloatingPanesVisible {
                    client_id,
                    tab_id,
                    completion,
                } => {
                    screen.are_floating_panes_visible_in_tab(client_id, tab_id, completion)?;
                },
                ScreenInstruction::ContextMenuHandleKeyIfOpen {
                    key,
                    toggle_command_palette,
                    client_id,
                    completion,
                } => {
                    let handled = screen.command_palette_handles_input(
                        &key,
                        &[],
                        toggle_command_palette,
                        client_id,
                    ) || screen.context_menu_handles_key(&key, client_id);
                    if let Some(mut completion) = completion {
                        completion.set_stdout_message(handled.to_string());
                    }
                },
                ScreenInstruction::PaletteThemeSaved {
                    client,
                    request,
                    result,
                } => {
                    screen.palette_theme_saved(client, request, result);
                    screen.render()?;
                },
                ScreenInstruction::CommandPalette(instruction) => {
                    if screen.plugin_palette_instruction(instruction) {
                        screen.render()?;
                    }
                },
                ScreenInstruction::ToggleCommandPalette(client_id, _completion) => {
                    screen.toggle_command_palette(client_id);
                    screen.render()?;
                },
                ScreenInstruction::ResetAgentEscape(client_id) => {
                    for tab in screen.tabs.values_mut() {
                        tab.pending_agent_escapes
                            .retain(|(client, _), _| *client != client_id);
                    }
                },
                ScreenInstruction::PasteFromClient(text, client_id, completion) => {
                    screen.paste_from_client(text, client_id, &keybind_intercepts, completion)?;
                },
                ScreenInstruction::WriteCharacter(
                    key_with_modifier,
                    raw_bytes,
                    is_kitty_keyboard_protocol,
                    client_id,
                    completion,
                ) => {
                    if screen.write_character(
                        key_with_modifier,
                        raw_bytes,
                        is_kitty_keyboard_protocol,
                        client_id,
                        &keybind_intercepts,
                        completion,
                    )? {
                        screen.render()?;
                    }
                },
                ScreenInstruction::Resize(
                    client_id,
                    strategy,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.resize(client_id, strategy),
                        ?
                    );
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::SwitchFocus(client_id, _completion_tx) => {
                    let old_pane_id = screen.get_active_pane_id(&client_id);
                    screen.cycle_focus(client_id, true);
                    let new_pane_id = screen.get_active_pane_id(&client_id);
                    if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                        screen.report_key_passthrough_state(client_id, old, new);
                    }
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::FocusNextPane(client_id, mut _completion_tx) => {
                    if screen.get_first_client_id().is_none() {
                        log::error!("No connected clients to change focus for");
                        if let Some(ref mut c) = _completion_tx {
                            c.set_exit_status(1);
                            c.set_error_message(
                                "No connected clients to change focus for".to_string(),
                            );
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
                },
                ScreenInstruction::FocusPreviousPane(client_id, mut _completion_tx) => {
                    if screen.get_first_client_id().is_none() {
                        log::error!("No connected clients to change focus for");
                        if let Some(ref mut c) = _completion_tx {
                            c.set_exit_status(1);
                            c.set_error_message(
                                "No connected clients to change focus for".to_string(),
                            );
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
                },
                ScreenInstruction::FocusLastPane(client_id, _completion_tx) => {
                    let old_pane_id = screen.get_active_pane_id(&client_id);
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.focus_last_pane(client_id)
                    );
                    let new_pane_id = screen.get_active_pane_id(&client_id);
                    if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                        screen.report_key_passthrough_state(client_id, old, new);
                    }
                    screen.sync_scroll_mode_on_focus(client_id)?;
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::MoveFocusLeft(client_id, mut _completion_tx) => {
                    if screen.get_first_client_id().is_none() {
                        log::error!("No connected clients to move focus for");
                        if let Some(ref mut c) = _completion_tx {
                            c.set_exit_status(1);
                            c.set_error_message(
                                "No connected clients to move focus for".to_string(),
                            );
                        }
                    } else {
                        screen.move_focus_in_direction_or_escalate(Direction::Left, client_id)?;
                    }
                },
                ScreenInstruction::MoveFocusLeftOrPreviousTab(client_id, mut _completion_tx) => {
                    if screen.get_first_client_id().is_none() {
                        log::error!("No connected clients to move focus for");
                        if let Some(ref mut c) = _completion_tx {
                            c.set_exit_status(1);
                            c.set_error_message(
                                "No connected clients to move focus for".to_string(),
                            );
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
                },
                ScreenInstruction::MoveFocusDown(client_id, mut _completion_tx) => {
                    if screen.get_first_client_id().is_none() {
                        log::error!("No connected clients to move focus for");
                        if let Some(ref mut c) = _completion_tx {
                            c.set_exit_status(1);
                            c.set_error_message(
                                "No connected clients to move focus for".to_string(),
                            );
                        }
                    } else {
                        screen.move_focus_in_direction_or_escalate(Direction::Down, client_id)?;
                    }
                },
                ScreenInstruction::MoveFocusRight(client_id, mut _completion_tx) => {
                    if screen.get_first_client_id().is_none() {
                        log::error!("No connected clients to move focus for");
                        if let Some(ref mut c) = _completion_tx {
                            c.set_exit_status(1);
                            c.set_error_message(
                                "No connected clients to move focus for".to_string(),
                            );
                        }
                    } else {
                        screen.move_focus_in_direction_or_escalate(Direction::Right, client_id)?;
                    }
                },
                ScreenInstruction::MoveFocusRightOrNextTab(client_id, mut _completion_tx) => {
                    if screen.get_first_client_id().is_none() {
                        log::error!("No connected clients to move focus for");
                        if let Some(ref mut c) = _completion_tx {
                            c.set_exit_status(1);
                            c.set_error_message(
                                "No connected clients to move focus for".to_string(),
                            );
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
                },
                ScreenInstruction::MoveFocusUp(client_id, mut _completion_tx) => {
                    if screen.get_first_client_id().is_none() {
                        log::error!("No connected clients to move focus for");
                        if let Some(ref mut c) = _completion_tx {
                            c.set_exit_status(1);
                            c.set_error_message(
                                "No connected clients to move focus for".to_string(),
                            );
                        }
                    } else {
                        screen.move_focus_in_direction_or_escalate(Direction::Up, client_id)?;
                    }
                },
                ScreenInstruction::ClearScreen(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.clear_active_terminal_screen(
                            client_id,
                        ),
                        ?
                    );
                    screen.render()?;
                },
                ScreenInstruction::DumpScreen(
                    file,
                    client_id,
                    full,
                    pane_id,
                    completion_tx,
                    cli_client_id,
                    ansi,
                ) => {
                    screen.dump_screen(
                        file,
                        client_id,
                        full,
                        pane_id,
                        completion_tx,
                        cli_client_id,
                        ansi,
                    )?;
                },
                ScreenInstruction::DumpLayout(default_shell, client_id, completion_tx) => {
                    let err_context = || "Failed to dump layout".to_string();
                    let session_layout_metadata = screen.get_layout_metadata(default_shell, None);
                    screen
                        .bus
                        .senders
                        .send_to_plugin(PluginInstruction::DumpLayout(
                            session_layout_metadata,
                            client_id,
                            completion_tx,
                        ))
                        .with_context(err_context)?;
                },
                ScreenInstruction::ListClientsMetadata(default_shell, client_id, completion_tx) => {
                    let err_context = || "Failed to dump layout".to_string();
                    let session_layout_metadata = screen.get_layout_metadata(default_shell, None);
                    screen
                        .bus
                        .senders
                        .send_to_plugin(PluginInstruction::ListClientsMetadata(
                            session_layout_metadata,
                            client_id,
                            completion_tx,
                        ))
                        .with_context(err_context)?;
                },
                ScreenInstruction::ListPanes {
                    show_all,
                    response_channel,
                } => {
                    let err_context = || "Failed to list panes";
                    let pane_entries = screen
                        .collect_pane_list(show_all)
                        .with_context(err_context)?;
                    let _ = response_channel.send(pane_entries);
                },
                ScreenInstruction::ListNotifications { response_channel } => {
                    let _ = response_channel.send(
                        screen
                            .notification_history
                            .iter()
                            .map(|record| record.info.clone())
                            .collect(),
                    );
                },
                ScreenInstruction::ListTabs {
                    client_id,
                    response_channel,
                } => {
                    let err_context = || "Failed to list tabs";
                    let tab_infos = screen
                        .collect_tab_list(client_id)
                        .with_context(err_context)?;
                    let _ = response_channel.send(tab_infos);
                },
                ScreenInstruction::GetCurrentTabInfo {
                    client_id,
                    response_channel,
                } => {
                    let err_context = || "Failed to get current tab info";
                    let tab_info = screen
                        .get_current_tab_info(client_id)
                        .with_context(err_context)?;
                    let _ = response_channel.send(tab_info);
                },
                ScreenInstruction::DumpLayoutToPlugin {
                    plugin_id,
                    tab_index,
                    response_channel,
                    trace,
                } => {
                    trace.mark("screen metadata");
                    let err_context = || "Failed to dump layout".to_string();
                    let session_layout_metadata =
                        screen.get_layout_metadata(Some(screen.default_shell.clone()), tab_index);
                    trace.mark("pty queued");
                    screen
                        .bus
                        .senders
                        .send_to_pty(PtyInstruction::DumpLayoutToPlugin {
                            session_layout_metadata,
                            plugin_id,
                            response_channel,
                            trace,
                        })
                        .with_context(err_context)
                        .non_fatal();
                },
                ScreenInstruction::GetCellPixelSize {
                    client_id,
                    response_channel,
                } => {
                    response_channel
                        .send(screen.cell_pixel_size(client_id))
                        .context("reply with client cell pixel size")
                        .non_fatal();
                },
                ScreenInstruction::ReportCellPixelSize(plugin_id, client_id) => {
                    screen.report_cell_pixel_size(Some(plugin_id), client_id);
                },
                ScreenInstruction::GetFocusedPaneInfo {
                    client_id,
                    response_channel,
                } => {
                    let response = match screen.active_tab_ids.get(&client_id) {
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
                },
                ScreenInstruction::GetPaneInfo {
                    pane_id,
                    response_channel,
                } => {
                    let pane_info = screen.get_pane_info(pane_id);
                    let _ = response_channel.send(pane_info);
                },
                ScreenInstruction::GetTabInfo {
                    tab_id,
                    response_channel,
                } => {
                    let tab_info = screen.get_tab_info(tab_id, &screen.plugin_tab_names());
                    let _ = response_channel.send(tab_info);
                },
                ScreenInstruction::ListClientsToPlugin(plugin_id, client_id, generation) => {
                    let err_context = || "Failed to dump layout".to_string();
                    let session_layout_metadata =
                        screen.get_layout_metadata(Some(screen.default_shell.clone()), None);
                    screen
                        .bus
                        .senders
                        .send_to_pty(PtyInstruction::ListClientsToPlugin(
                            session_layout_metadata,
                            plugin_id,
                            client_id,
                            generation,
                        ))
                        .with_context(err_context)
                        .non_fatal();
                },
                ScreenInstruction::EditScrollback(client_id, ansi, completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| {
                            if ansi {
                                tab.edit_scrollback_raw(client_id, completion_tx)
                            } else {
                                tab.edit_scrollback(client_id, completion_tx)
                            }
                        },
                        ?
                    );
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::GetPaneScrollback {
                    pane_id,
                    client_id,
                    get_full_scrollback,
                    response_channel,
                } => {
                    let last_output_ms_ago =
                        screen.pane_output_activity.get(&pane_id).map(|last| {
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
                },
                ScreenInstruction::ScrollUp(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    let was_scrolled = screen.active_pane_is_scrolled(client_id);
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .scroll_active_terminal_up(client_id)
                    );
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render()?;
                },
                ScreenInstruction::MovePane(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.move_active_pane(client_id)
                    );
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::MovePaneBackwards(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .move_active_pane_backwards(client_id)
                    );
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::MovePaneDown(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_down(client_id)
                    );
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::MovePaneUp(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_up(client_id)
                    );
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::MovePaneRight(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_right(client_id)
                    );
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::MovePaneLeft(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_left(client_id)
                    );
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::ScrollUpAt(
                    point,
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .handle_scrollwheel_up(&point, 3, client_id), ?
                    );
                    screen.render()?;
                },
                ScreenInstruction::ScrollDown(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    let was_scrolled = screen.active_pane_is_scrolled(client_id);
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.scroll_active_terminal_down(client_id), ?
                    );
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render()?;
                },
                ScreenInstruction::ScrollDownAt(
                    point,
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .handle_scrollwheel_down(&point, 3, client_id), ?
                    );
                    screen.render()?;
                },
                ScreenInstruction::ScrollToBottom(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    let was_scrolled = screen.active_pane_is_scrolled(client_id);
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .scroll_active_terminal_to_bottom(client_id), ?
                    );
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render()?;
                },
                ScreenInstruction::ScrollToTop(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    let was_scrolled = screen.active_pane_is_scrolled(client_id);
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .scroll_active_terminal_to_top(client_id), ?
                    );
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render()?;
                },
                // The prompt jumps deliberately do not sync scroll mode: they are bound in normal
                // mode too, and jumping to a prompt from there must not switch modes.
                ScreenInstruction::ScrollToPreviousPrompt(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .scroll_active_terminal_to_previous_prompt(client_id)
                    );
                    screen.render()?;
                },
                ScreenInstruction::ScrollToNextPrompt(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .scroll_active_terminal_to_next_prompt(client_id)
                    );
                    screen.render()?;
                },
                ScreenInstruction::SelectCommandAtScrollPosition(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .select_command_at_scroll_position(client_id)
                    );
                    screen.render()?;
                },
                ScreenInstruction::CopyLastCommandOutput(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.copy_last_command_output(client_id),
                        ?
                    );
                    screen.render()?;
                },
                ScreenInstruction::MoveTextSelection(direction, client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .move_text_selection(direction, client_id)
                    );
                    screen.render()?;
                },
                ScreenInstruction::SwapTextSelectionEnds(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .swap_text_selection_ends(client_id)
                    );
                    screen.render()?;
                },
                ScreenInstruction::MoveNotificationSelection(
                    direction,
                    client_id,
                    _completion_tx,
                ) => {
                    if screen.move_notification_selection(direction, client_id) {
                        screen.render()?;
                    }
                },
                ScreenInstruction::ActivateLastNotification(client_id, _completion_tx) => {
                    screen.activate_last_notification(client_id, Instant::now());
                    screen.render()?;
                },
                ScreenInstruction::ActivateNotification(client_id, _completion_tx) => {
                    screen.activate_selected_notification(client_id, Instant::now());
                    screen.render()?;
                },
                ScreenInstruction::ToggleNotificationDetail(client_id, _completion_tx) => {
                    if screen.toggle_notification_detail(client_id) {
                        screen.render()?;
                    }
                },
                ScreenInstruction::ClearCommandOutputFlash(pane_id) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.clear_command_output_flash(pane_id);
                    }
                    screen.render()?;
                },
                ScreenInstruction::PageScrollUp(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    let was_scrolled = screen.active_pane_is_scrolled(client_id);
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .scroll_active_terminal_up_page(client_id)
                    );
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render()?;
                },
                ScreenInstruction::PageScrollDown(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    let was_scrolled = screen.active_pane_is_scrolled(client_id);
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .scroll_active_terminal_down_page(client_id), ?
                    );
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render()?;
                },
                ScreenInstruction::HalfPageScrollUp(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    let was_scrolled = screen.active_pane_is_scrolled(client_id);
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .scroll_active_terminal_up_half_page(client_id)
                    );
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render()?;
                },
                ScreenInstruction::HalfPageScrollDown(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    let was_scrolled = screen.active_pane_is_scrolled(client_id);
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .scroll_active_terminal_down_half_page(client_id), ?
                    );
                    screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                    screen.render()?;
                },
                // Writes captured by the palette must not scroll the underlying pane either.
                ScreenInstruction::ClearScroll(client_id)
                    if screen.palette_owns_input(client_id)
                        || screen.permission_modal_open(client_id) => {},
                ScreenInstruction::ClearScroll(client_id) => {
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
                },
                ScreenInstruction::CloseFocusedPane(client_id, completion_tx) => {
                    let old_pane_id = screen.close_focused_pane(client_id, completion_tx)?;
                    let new_pane_id = screen.get_active_pane_id(&client_id);
                    if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                        screen.report_key_passthrough_state(client_id, old, new);
                    }
                    screen.sync_scroll_mode_on_focus(client_id)?;
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::SetSelectable(pid, selectable) => {
                    match screen.tab_containing_pane_mut(pid) {
                        Some(tab) => tab.set_pane_selectable(pid, selectable),
                        // the pane's tab may simply not exist yet - retry once one does
                        None => pending
                            .waiting_for_tab
                            .push(ScreenInstruction::SetSelectable(pid, selectable)),
                    }
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::ShowPluginCursor(pid, client_id, cursor_position) => {
                    if !screen.show_plugin_cursor(pid, client_id, cursor_position) {
                        pending
                            .waiting_for_pane
                            .entry(PaneId::Plugin(pid))
                            .or_default()
                            .push(ScreenInstruction::ShowPluginCursor(
                                pid,
                                client_id,
                                cursor_position,
                            ));
                    }
                    screen.render()?;
                },
                ScreenInstruction::SetMouseSelectionSupport(pid, selection_support) => {
                    if screen
                        .with_pane(pid, |pane| {
                            pane.set_mouse_selection_support(selection_support)
                        })
                        .is_none()
                    {
                        pending
                            .waiting_for_tab
                            .push(ScreenInstruction::SetMouseSelectionSupport(
                                pid,
                                selection_support,
                            ));
                    }
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::RetirePicker {
                    finder,
                    pane,
                    client,
                } => {
                    if let Some(pane) = pane {
                        screen.close_pane_from_instruction(
                            PaneId::Terminal(pane),
                            Some(client),
                            None,
                            None,
                            &mut pending,
                        )?;
                    }
                    screen.render()?;
                    screen
                        .bus
                        .senders
                        .send_to_plugin(PluginInstruction::PickRetired(finder))
                        .context("acknowledge picker retirement")?;
                },
                ScreenInstruction::ClosePane(
                    id,
                    client_id,
                    completion_tx, // the action ends here, dropping this will release anything
                    // waiting for it
                    exit_status,
                ) => {
                    screen.close_pane_from_instruction(
                        id,
                        client_id,
                        completion_tx,
                        exit_status,
                        &mut pending,
                    )?;
                    screen.log_and_report_session_state().non_fatal();
                    screen.retain_only_existing_panes_in_pane_groups();
                    screen.render()?;
                },
                ScreenInstruction::RestoreHeldPane(id, held) => {
                    if !screen.restore_held_pane(id, &held) {
                        pending
                            .waiting_for_pane
                            .entry(id)
                            .or_default()
                            .push(ScreenInstruction::RestoreHeldPane(id, held));
                    }
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::HoldPane(id, exit_status, run_command) => {
                    screen.invalidate_agent_lifecycle(id, "held");
                    let is_first_run = false;
                    let mut found = false;
                    let mut pane_is_agent = false;
                    let mut queued_respawn = None;
                    {
                        for tab in screen.tabs.values_mut() {
                            if tab.get_all_pane_ids().contains(&id) {
                                queued_respawn = tab.hold_pane(
                                    id,
                                    exit_status,
                                    is_first_run,
                                    run_command.clone(),
                                );
                                found = true;
                                pane_is_agent = tab.pane_is_agent(id);
                                break;
                            }
                        }
                    }
                    if found {
                        // The observer first, then the pane-replacing hook, so the plugin thread
                        // hears of the exit before it covers the pane.
                        screen.fire_pane_exit_hook(id, exit_status, &run_command, true);
                        screen.fire_agent_exit_hook(id, exit_status, &run_command, pane_is_agent);
                        // Only after the exit hooks have seen this exit does a queued replacement
                        // start, and it starts through Screen so agent_start fires for it too.
                        if let Some((terminal_pane_id, run_command, keep_scrollback)) =
                            queued_respawn
                        {
                            screen.run_command_in_pane_with_id(
                                terminal_pane_id,
                                run_command,
                                keep_scrollback,
                            );
                        }
                    }
                    if !found {
                        pending
                            .waiting_for_pane
                            .entry(id)
                            .or_default()
                            .push(ScreenInstruction::HoldPane(id, exit_status, run_command));
                    }
                    screen.log_and_report_session_state()?;
                    screen.render()?;
                },
                ScreenInstruction::SetPaneSuspended(id, suspended) => {
                    // Unlike a hold this is not buffered for a pane that does not
                    // exist yet: a pane cannot suspend before it has been created,
                    // so a miss here means the pane is already gone and the news is
                    // stale rather than early.
                    for tab in screen.tabs.values_mut() {
                        if tab.get_all_pane_ids().contains(&id) {
                            tab.set_pane_suspended(id, suspended)?;
                            break;
                        }
                    }
                    screen.render()?;
                },
                ScreenInstruction::UpdatePaneName(
                    c,
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.update_active_pane_name(c, client_id), ?
                    );
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::UpdatePaneHost(terminal_id, host) => {
                    screen.set_pane_host(terminal_id, host)?;
                },
                ScreenInstruction::UpdatePaneForegroundCommand(terminal_id, command, program) => {
                    let pane_id = PaneId::Terminal(terminal_id);
                    if apply_pane_foreground_command(
                        &mut screen,
                        terminal_id,
                        command.clone(),
                        program.clone(),
                    ) {
                        screen.render()?;
                    } else {
                        pending.waiting_for_pane.entry(pane_id).or_default().push(
                            ScreenInstruction::UpdatePaneForegroundCommand(
                                terminal_id,
                                command,
                                program,
                            ),
                        );
                    }
                },
                ScreenInstruction::UpdatePaneCwd(terminal_id, cwd) => {
                    // Only pty says a pane moved, and it only says so on a change,
                    // so there is nothing to compare against here. Pruning cannot be
                    // done from the manifest instead: pty reports the folder a pane
                    // starts in *before* the layout that puts the pane in a tab, so
                    // an unknown id is as likely to be arriving as departing.
                    match cwd {
                        Some(cwd) => screen.terminal_cwds.insert(terminal_id, cwd),
                        None => screen.terminal_cwds.remove(&terminal_id),
                    };
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::SetPaneSubtitle(pane_id, subtitle) => {
                    if let Some(tab) = screen
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
                },
                ScreenInstruction::SetPaneTitlePrefix(pane_id, prefix) => {
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
                        pending
                            .waiting_for_pane
                            .entry(pane_id)
                            .or_default()
                            .push(ScreenInstruction::SetPaneTitlePrefix(pane_id, prefix));
                    }
                },
                ScreenInstruction::SetPaneCaption(pane_id, caption) => {
                    // `set_title`, not a rename: the caption is a default, and a default is not
                    // lent to the tab. Buffered like a hold, for a pane still being built.
                    if screen
                        .with_pane(pane_id, |pane| pane.set_title(caption.clone()))
                        .is_some()
                    {
                        screen.render()?;
                        screen.log_and_report_session_state()?;
                    } else {
                        pending
                            .waiting_for_pane
                            .entry(pane_id)
                            .or_default()
                            .push(ScreenInstruction::SetPaneCaption(pane_id, caption));
                    }
                },
                ScreenInstruction::UndoRenamePane(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.undo_active_rename_pane(client_id), ?
                    );
                    screen.render()?;
                },
                ScreenInstruction::ToggleActiveTerminalFullscreen(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .toggle_active_pane_fullscreen(client_id)
                    );
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::ToggleActiveTerminalNoUiFullscreen(
                    client_id,
                    _completion_tx,
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .toggle_active_pane_no_ui_fullscreen(client_id)
                    );
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::TogglePaneFrames(
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    screen.pane_frame_style = match screen.pane_frame_style {
                        PaneFrameStyle::Full => PaneFrameStyle::Titles,
                        PaneFrameStyle::Titles => PaneFrameStyle::None,
                        PaneFrameStyle::None => PaneFrameStyle::Full,
                    };
                    for tab in screen.tabs.values_mut() {
                        tab.set_pane_frames(screen.pane_frame_style);
                        tab.update_input_modes()?;
                    }
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::SetPaneFrameStyle(pane_frame_style, _completion_tx) => {
                    screen.pane_frame_style = pane_frame_style;
                    for tab in screen.tabs.values_mut() {
                        tab.set_pane_frames(screen.pane_frame_style);
                        tab.update_input_modes()?;
                    }
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::SwitchTabNext(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    screen.switch_tab_next(None, true, client_id)?;
                    screen.render()?;
                },
                ScreenInstruction::SwitchTabPrev(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    screen.switch_tab_prev(None, true, client_id)?;
                    screen.render()?;
                },
                ScreenInstruction::CloseTab(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    screen.close_tab(client_id)?;
                    let connected_client_ids: Vec<ClientId> =
                        screen.active_tab_ids.keys().copied().collect();
                    for connected_client_id in connected_client_ids {
                        screen.sync_scroll_mode_on_focus(connected_client_id)?;
                    }
                    screen.render()?;
                },
                ScreenInstruction::NewTab(
                    cwd,
                    default_shell,
                    layout,
                    floating_panes_layout,
                    tab_name,
                    (swap_tiled_layouts, swap_floating_layouts),
                    initial_panes,
                    block_on_first_terminal,
                    should_change_focus_to_new_tab,
                    client_id,
                    completion_tx,
                ) => {
                    screen.open_new_tab(
                        cwd,
                        default_shell,
                        layout,
                        floating_panes_layout,
                        tab_name,
                        swap_tiled_layouts,
                        swap_floating_layouts,
                        initial_panes,
                        block_on_first_terminal,
                        should_change_focus_to_new_tab,
                        client_id,
                        completion_tx,
                    )?;
                },
                ScreenInstruction::ApplyLayout(
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
                ) => {
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
                        &mut pending,
                    )?;
                    log::info!(
                        "startup: ApplyLayout tab={tab_id:?} client={client_id} complete at {:?}",
                        startup_started.elapsed()
                    );
                    screen.render()?;
                },
                ScreenInstruction::LayoutSpawnFailed(tab_id, destination) => {
                    if destination.is_some_and(|target| {
                        !screen.tabs.get(&tab_id).is_some_and(|tab| {
                            target
                                .lifetime
                                .ptr_eq(&std::sync::Arc::downgrade(&tab.agent_tab_lifetime))
                        })
                    }) {
                        break 'instruction;
                    }
                    screen.recover_pending_tab(tab_id).non_fatal();
                    screen.replay_events_waiting_for_layout(&mut pending);
                    if !screen.has_pending_tabs() {
                        screen
                            .apply_deferred_tab_switches(&mut pending.tab_switches)
                            .non_fatal();
                    }
                    screen.render()?;
                },
                ScreenInstruction::TabGroup(action, client, completion) => {
                    screen
                        .tab_group_action(action, client, completion, &mut pending.tab_switches)
                        .non_fatal();
                    screen.render()?;
                },
                ScreenInstruction::GoToTab(
                    tab_index,
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    let client_id_to_switch = match client_id {
                        None => None,
                        Some(client_id) if screen.active_tab_ids.contains_key(&client_id) => {
                            Some(client_id)
                        },
                        Some(_) => screen.active_tab_ids.keys().next().copied(),
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
                                && let Some(tab_id) =
                                    screen.get_tab_id_at_position(tab_index.position())
                            {
                                pending.tab_switches.insert((tab_id, client_id));
                            }
                        },
                    }
                },
                ScreenInstruction::GoToTabName(
                    tab_name,
                    default_shell,
                    create,
                    client_id,
                    completion_tx,
                ) => {
                    screen.go_to_or_create_tab_by_name(
                        tab_name,
                        default_shell,
                        create,
                        client_id,
                        completion_tx,
                    )?;
                },
                ScreenInstruction::UpdateTabName(c, client_id, completion_tx) => {
                    if screen.has_no_client_to_act_for(client_id) {
                        screen.report_no_client_to_act_for(
                        client_id,
                        "Cannot rename the focused tab: no client is attached to this session. Target a tab explicitly with --tab-id.".to_owned(),
                        completion_tx,
                    );
                    } else {
                        // the action ends here, dropping completion_tx will release anything
                        // waiting for it
                        screen.update_active_tab_name(c, client_id)?;
                        screen.render()?;
                    }
                },
                ScreenInstruction::UndoRenameTab(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    screen.undo_active_rename_tab(client_id)?;
                    screen.render()?;
                },
                ScreenInstruction::MoveTabLeft(client_id, completion_tx) => {
                    if !screen.has_pending_tabs() {
                        screen.move_active_tab_to_left(client_id)?;
                        screen.render()?;
                    } else {
                        // Defer execution, forward completion_tx
                        pending
                            .waiting_for_tab
                            .push(ScreenInstruction::MoveTabLeft(client_id, completion_tx));
                    }
                },
                ScreenInstruction::MoveTabRight(client_id, completion_tx) => {
                    if !screen.has_pending_tabs() {
                        screen.move_active_tab_to_right(client_id)?;
                        screen.render()?;
                    } else {
                        // Defer execution, forward completion_tx
                        pending
                            .waiting_for_tab
                            .push(ScreenInstruction::MoveTabRight(client_id, completion_tx));
                    }
                },
                ScreenInstruction::RecomputeTabSize(client_id, new_size) => {
                    let previous_surface = screen.surface_size();
                    screen
                        .set_client_size(client_id, new_size)
                        .context("failed to set client size for tab recomputation")?;
                    // the server sends this while setting the session up, before any tab exists
                    if screen.tabs.is_empty() {
                        break 'instruction;
                    }
                    // resize_to_clients reflows every tab against the new surface size, then
                    // reports and renders - which is what render_to_clients would
                    // otherwise undo, since it lays the tabs out against the surface on every frame
                    screen.resize_to_clients()?;
                    if screen.surface_size() == previous_surface {
                        // another, smaller client is pinning the shared surface size, so the layout
                        // does not change - but this client's terminal did, and it needs the
                        // unchanged layout painted over its new viewport
                        if let Ok(tab) = screen.get_active_tab_mut(client_id) {
                            tab.set_force_render();
                        }
                        screen.render()?;
                    }
                },
                ScreenInstruction::TerminalPixelDimensions(client_id, pixel_dimensions) => {
                    let character_cell_size_changed =
                        screen.update_pixel_dimensions(client_id, pixel_dimensions);
                    if character_cell_size_changed {
                        screen.resize_pty_all_panes()?;
                        screen.render()?;
                    }
                },
                ScreenInstruction::TerminalBackgroundColor(background_color_instruction) => {
                    screen.update_terminal_background_color(background_color_instruction);
                },
                ScreenInstruction::TerminalForegroundColor(background_color_instruction) => {
                    screen.update_terminal_foreground_color(background_color_instruction);
                },
                ScreenInstruction::TerminalColorRegisters(color_registers) => {
                    screen.update_terminal_color_registers(color_registers);
                },
                ScreenInstruction::SetKittyGraphicsSupport {
                    client_id,
                    supported,
                } => {
                    screen.update_kitty_graphics_support(client_id, supported);
                    screen.render()?;
                },
                ScreenInstruction::SetSixelSupport {
                    client_id,
                    supported,
                } => {
                    screen.update_sixel_support(client_id, supported);
                },
                ScreenInstruction::SetSynchronizedOutputSupport {
                    client_id,
                    supported,
                } => {
                    screen
                        .update_synchronized_output_support(client_id, supported)
                        .context("failed to update synchronized output support")?;
                },
                ScreenInstruction::ForwardHostQuery { pane_id, query } => {
                    screen.forward_host_query(pane_id, query);
                },
                ScreenInstruction::NestedSessionMessageFromPane { pane_id, message } => {
                    screen.handle_nested_session_message_from_pane(pane_id, message);
                },
                ScreenInstruction::NestedGuestPingTick { pane_id } => {
                    screen.handle_nested_guest_ping_tick(pane_id);
                },
                ScreenInstruction::NestedSessionMessageFromHost { client_id, message } => {
                    screen.handle_nested_session_message_from_host(client_id, message);
                },
                ScreenInstruction::GuestModalChoice {
                    client_id,
                    pane_id,
                    outcome,
                } => {
                    screen.handle_guest_modal_choice(client_id, pane_id, outcome);
                },
                ScreenInstruction::ForwardedReplyFromHost { token, reply_bytes } => {
                    screen.handle_forwarded_reply_from_host(token, reply_bytes)?;
                    // The handler's replay of `pending_pty_input` mutates the
                    // grid. Without a render scheduling here the clients
                    // keep displaying the pre-reply frame
                    screen.render()?;
                },
                ScreenInstruction::ResumePaneAfterForward {
                    pane_id,
                    reply_bytes,
                } => {
                    screen.resume_pane_after_forward(pane_id, reply_bytes)?;
                    // Same rationale as ForwardedReplyFromHost above — the
                    // resume's replay paints the grid, so we must schedule
                    // a render before yielding back to the screen loop.
                    screen.render()?;
                },
                ScreenInstruction::HostTerminalThemeChanged(mode) => {
                    screen.update_host_terminal_theme_mode(mode)?;
                },
                ScreenInstruction::SetDarkTheme(mut completion_tx) => {
                    screen.apply_manual_host_terminal_theme_mode(
                        HostTerminalThemeMode::Dark,
                        &mut completion_tx,
                    )?;
                },
                ScreenInstruction::SetLightTheme(mut completion_tx) => {
                    screen.apply_manual_host_terminal_theme_mode(
                        HostTerminalThemeMode::Light,
                        &mut completion_tx,
                    )?;
                },
                ScreenInstruction::ToggleTheme(mut completion_tx) => {
                    let next = match screen.host_terminal_theme_mode {
                        Some(HostTerminalThemeMode::Dark) => HostTerminalThemeMode::Light,
                        Some(HostTerminalThemeMode::Light) => HostTerminalThemeMode::Dark,
                        // No prior mode: treat as Dark and toggle to Light.
                        None => HostTerminalThemeMode::Light,
                    };
                    screen.apply_manual_host_terminal_theme_mode(next, &mut completion_tx)?;
                },
                ScreenInstruction::ChangeMode(
                    input_mode,
                    base_mode,
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    let trace_started_at = attach_traces_waiting_for_first_render
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
                },
                ScreenInstruction::ChangeModeForAllClients(
                    input_mode,
                    base_mode,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    screen.change_mode_for_all_clients(input_mode, base_mode)?;
                    screen.render()?;
                },
                ScreenInstruction::ToggleActiveSyncTab(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, _client_id: ClientId| tab.toggle_sync_panes_is_active()
                    );
                    screen.log_and_report_session_state()?;
                    screen.render()?;
                },
                ScreenInstruction::MouseEvent(
                    event,
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    screen.handle_mouse_event(event, client_id);
                },
                ScreenInstruction::Copy(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    let selecting = screen
                        .mode_info
                        .get(&client_id)
                        .is_some_and(|mode_info| mode_info.mode == InputMode::Select);
                    screen.copy_selection(client_id)?;
                    if selecting {
                        active_tab_and_connected_client_id!(
                            screen,
                            client_id,
                            |tab: &mut Tab, client_id: ClientId| tab
                                .remember_copied_text_selection(client_id)
                        );
                    }
                    screen.render()?;
                },
                ScreenInstruction::Exit => {
                    break 'instructions;
                },
                ScreenInstruction::ToggleTab(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    screen.toggle_tab(client_id)?;
                    screen.render()?;
                },
                ScreenInstruction::AddClient(
                    client_id,
                    client_size,
                    tab_number_to_focus,
                    pane_id_to_focus,
                ) => {
                    screen.attach_client(
                        client_id,
                        client_size,
                        tab_number_to_focus,
                        pane_id_to_focus,
                        &mut attach_traces_waiting_for_first_render,
                        &mut pending,
                    )?;
                },
                ScreenInstruction::RemoveClient(client_id) => {
                    screen.detach_agent_controls(client_id);
                    screen.agent_events.remove(client_id);
                    attach_traces_waiting_for_first_render.remove(&client_id);
                    // A connection that never attached — a session probe, a one-shot
                    // CLI action client, a hook's `yinz action agent-report` — has
                    // no screen state, and its departure must not cost the session a
                    // reflow and a full session-state broadcast. At several such
                    // connections per hook event, those broadcasts were most of the
                    // plugin threads' load while an agent worked.
                    let was_attached = screen.is_client_attached(client_id);
                    screen.remove_client(client_id).non_fatal();
                    keybind_intercepts.remove(&client_id);
                    if !screen.accepts_plugin_output(client_id) {
                        pending.remove_client_plugin_output(client_id);
                    }
                    if was_attached {
                        // the departing client may have been the one constraining the
                        // screen size; resize_to_clients also reports session state.
                        screen.resize_to_clients().non_fatal();
                        screen.render().non_fatal();
                    }
                },
                ScreenInstruction::UpdateSearch(
                    c,
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.update_search_term(c, client_id)
                    );
                    screen.render()?;
                },
                ScreenInstruction::SearchDown(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.search_down(client_id)
                    );
                    screen.render()?;
                },
                ScreenInstruction::SearchUp(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.search_up(client_id)
                    );
                    screen.render()?;
                },
                ScreenInstruction::SearchToggleCaseSensitivity(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .toggle_search_case_sensitivity(client_id)
                    );
                    screen.render()?;
                },
                ScreenInstruction::SearchToggleWrap(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab.toggle_search_wrap(client_id)
                    );
                    screen.render()?;
                },
                ScreenInstruction::SearchToggleWholeWord(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, client_id: ClientId| tab
                            .toggle_search_whole_words(client_id)
                    );
                    screen.render()?;
                },
                ScreenInstruction::AddRedPaneFrameColorOverride(pane_ids, error_text) => {
                    for pane_id in pane_ids {
                        if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                            tab.add_red_pane_frame_color_override(pane_id, error_text.clone());
                        }
                    }
                    screen.render()?;
                },
                ScreenInstruction::AddHighlightPaneFrameColorOverride(pane_ids, error_text) => {
                    for pane_id in pane_ids {
                        if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                            tab.add_highlight_pane_frame_color_override(
                                pane_id,
                                error_text.clone(),
                                None,
                            );
                        }
                    }
                    screen.render()?;
                },
                ScreenInstruction::FlashPaneFrameColor(pane_ids) => {
                    let now = Instant::now();
                    for pane_id in pane_ids {
                        if let Some(tab) = screen.tab_containing_pane_mut(pane_id)
                            && tab.flash_pane_frame_color_override(pane_id, now)
                        {
                            screen.fading_frame_panes.insert(pane_id);
                        }
                    }
                    screen.sync_animation_clock(now)?;
                    screen.render()?;
                },
                ScreenInstruction::ClearPaneFrameColorOverride(pane_ids) => {
                    for pane_id in pane_ids {
                        if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                            tab.clear_pane_frame_color_override(pane_id, None);
                        }
                    }
                    screen.render()?;
                },
                ScreenInstruction::HostTerminalFocusChanged(client_id, focused) => {
                    screen.host_terminal_focus_changed(client_id, focused);
                },
                ScreenInstruction::SetClientHostTerminalEnv(client_id, host_terminal_env) => {
                    screen.set_client_host_terminal_env(client_id, host_terminal_env);
                },
                ScreenInstruction::ForwardDesktopNotifications {
                    pane_id,
                    notifications,
                } => {
                    screen.forward_desktop_notifications(notifications, pane_id);
                },
                ScreenInstruction::SetTabBellFlash(tab_id, is_flashing) => {
                    if let Some(tab) = screen.tabs.values_mut().find(|t| t.id == tab_id) {
                        tab.tab_bell_flash = is_flashing;
                        tab.clear_tab_bell_ring();
                    }
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::PreviousSwapLayout(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, _client_id: ClientId| tab.previous_swap_layout(),
                        ?
                    );
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::NextSwapLayout(client_id, _completion_tx) => {
                    active_tab_and_connected_client_id!(
                        screen,
                        client_id,
                        |tab: &mut Tab, _client_id: ClientId| tab.next_swap_layout(),
                        ?
                    );
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::OverrideLayout(
                    cwd,
                    default_shell,
                    tab_layouts,
                    retain_existing_terminal_panes,
                    retain_existing_plugin_panes,
                    apply_only_to_focused_tab,
                    client_id,
                    completion_tx,
                ) => {
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
                },
                ScreenInstruction::OverrideLayoutComplete(
                    tab_results,
                    failed_tabs,
                    tabs_to_close,
                    retain_existing_terminal_panes,
                    retain_existing_plugin_panes,
                    client_id,
                    mut completion_tx,
                ) => {
                    if let Err(error) = screen.complete_override_layout(
                        tab_results,
                        failed_tabs,
                        tabs_to_close,
                        retain_existing_terminal_panes,
                        retain_existing_plugin_panes,
                        client_id,
                        &mut pending,
                    ) {
                        screen.report_layout_error(
                            format!("{error:#}"),
                            client_id,
                            &mut completion_tx,
                        );
                    }
                    screen.log_and_report_session_state()?;
                    let _ = screen.render();
                    drop(completion_tx); // action ends here, notify the action initiator
                },
                ScreenInstruction::QueryTabNames(client_id, completion_tx) => {
                    let tab_names = screen
                        .get_tabs_mut()
                        .values()
                        .map(|tab| tab.name.clone())
                        .collect::<Vec<String>>();
                    screen.bus.senders.send_to_server(ServerInstruction::Log(
                        tab_names,
                        client_id,
                        completion_tx,
                    ))?;
                },
                ScreenInstruction::NewTiledPluginPane(
                    run_plugin,
                    pane_title,
                    skip_cache,
                    cwd,
                    client_id,
                    completion_tx,
                    explicit_tab_id,
                    no_focus,
                ) => {
                    plugin_launch::new_tiled_plugin_pane(
                        &mut screen,
                        run_plugin,
                        pane_title,
                        skip_cache,
                        cwd,
                        client_id,
                        completion_tx,
                        explicit_tab_id,
                        no_focus,
                    )?;
                },
                ScreenInstruction::NewFloatingPluginPane(
                    run_plugin,
                    pane_title,
                    skip_cache,
                    cwd,
                    placement,
                    client_id,
                    completion_tx,
                    explicit_tab_id,
                    no_focus,
                ) => {
                    screen.new_floating_plugin_pane(
                        run_plugin,
                        pane_title,
                        skip_cache,
                        cwd,
                        placement,
                        client_id,
                        completion_tx,
                        explicit_tab_id,
                        no_focus,
                    )?;
                },
                ScreenInstruction::NewInPlacePluginPane(
                    run_plugin,
                    pane_title,
                    pane_id_to_replace,
                    skip_cache,
                    close_replaced_pane,
                    client_id,
                    completion_tx,
                    explicit_tab_id,
                    no_focus,
                ) => {
                    plugin_launch::new_in_place_plugin_pane(
                        &mut screen,
                        run_plugin,
                        pane_title,
                        pane_id_to_replace,
                        skip_cache,
                        close_replaced_pane,
                        client_id,
                        completion_tx,
                        explicit_tab_id,
                        no_focus,
                    )?;
                },
                ScreenInstruction::StartOrReloadPluginPane(
                    run_plugin,
                    pane_title,
                    client_id,
                    completion_tx,
                ) => {
                    plugin_launch::start_or_reload_plugin_pane(
                        &mut screen,
                        run_plugin,
                        pane_title,
                        client_id,
                        completion_tx,
                    )?;
                },
                ScreenInstruction::AddPlugin(
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
                ) => {
                    if screen.add_plugin(
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
                        &mut pending,
                    )? {
                        screen.log_and_report_session_state()?;
                    }
                },
                ScreenInstruction::PluginPaneless {
                    plugin_id,
                    run,
                    completion,
                } => {
                    screen.plugin_paneless(plugin_id, *run, completion, &mut pending)?;
                },
                ScreenInstruction::PluginResolving {
                    plugin_id,
                    run,
                    completion,
                } => {
                    screen.plugin_resolving(plugin_id, *run, completion);
                },
                ScreenInstruction::PluginFailed(plugin_id) => {
                    screen.plugin_failed(plugin_id);
                },
                ScreenInstruction::PluginRecovered(plugin_id) => {
                    screen.failed_paneless_plugins.remove(&plugin_id);
                },
                // Here rather than in `Screen`: the key intercepts are this loop's.
                ScreenInstruction::PluginUnloaded(plugin_id) => {
                    screen.plugin_unloaded(plugin_id, &mut keybind_intercepts, &mut pending)?;
                },
                ScreenInstruction::GiveUpSlot(slot) => {
                    screen.give_up_slot(slot, &mut pending)?;
                },
                ScreenInstruction::UpdatePluginLocation(pid, location) => {
                    screen.update_plugin_location(pid, location);
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::UpdatePluginLoadingStage(pid, loading_indication) => {
                    let found_plugin =
                        screen.update_plugin_loading_stage(pid, loading_indication.clone());
                    if !found_plugin {
                        pending
                            .waiting_for_pane
                            .entry(PaneId::Plugin(pid))
                            .or_default()
                            .push(ScreenInstruction::UpdatePluginLoadingStage(
                                pid,
                                loading_indication,
                            ));
                    }
                    screen.render()?;
                },
                ScreenInstruction::StartPluginLoadingIndication(pid, loading_indication) => {
                    if !screen.start_plugin_loading_indication(pid, loading_indication.clone()) {
                        pending
                            .waiting_for_pane
                            .entry(PaneId::Plugin(pid))
                            .or_default()
                            .push(ScreenInstruction::StartPluginLoadingIndication(
                                pid,
                                loading_indication,
                            ));
                    }
                    screen.render()?;
                },
                // No replay queue for a plugin whose pane does not exist yet, unlike the loading
                // *indication* above: with the offset derived from elapsed time there is nothing to
                // replay. The plugin stays registered and the next frame after its pane appears simply
                // draws the right number of dots.
                ScreenInstruction::StartPluginLoadingAnimation(pid) => {
                    // A plugin that runs headless or is gone never shows a load, and may never
                    // hear the matching stop: a reload that drops its declaration from a layout
                    // slot is unloaded without finishing, and this start, relayed through the
                    // background jobs thread, can land after the unload.
                    if screen.plugin_has_no_pane(pid) {
                        break 'instruction;
                    }
                    screen.animating_plugin_loads.insert(pid);
                    screen.sync_animation_clock(Instant::now())?;
                    screen.render()?;
                },
                ScreenInstruction::StopPluginLoadingAnimation(pid) => {
                    screen.animating_plugin_loads.remove(&pid);
                    // so the clock stops on the spot when this was the last load, rather than at the
                    // next tick a dot period later
                    screen.sync_animation_clock(Instant::now())?;
                    screen.render()?;
                },
                ScreenInstruction::RequestStateUpdateForPlugins => {
                    let all_tabs = screen.get_tabs_mut();
                    for tab in all_tabs.values_mut() {
                        tab.update_input_modes()?;
                    }
                    screen.log_and_report_session_state()?;
                    screen.render()?;
                },
                ScreenInstruction::LaunchOrFocusPlugin(
                    run_plugin,
                    should_float,
                    move_to_focused_tab,
                    should_open_in_place,
                    close_replaced_pane,
                    pane_id_to_replace,
                    skip_cache,
                    client_id,
                    completion_tx,
                    explicit_tab_id,
                ) => {
                    screen.launch_or_focus_plugin(
                        run_plugin,
                        should_float,
                        move_to_focused_tab,
                        should_open_in_place,
                        close_replaced_pane,
                        pane_id_to_replace,
                        skip_cache,
                        client_id,
                        completion_tx,
                        explicit_tab_id,
                    )?;
                },
                ScreenInstruction::LaunchPlugin(
                    run_plugin,
                    should_float,
                    should_open_in_place,
                    close_replaced_pane,
                    pane_id_to_replace,
                    skip_cache,
                    cwd,
                    client_id,
                    completion_tx,
                    explicit_tab_id,
                    no_focus,
                    overlay_placement,
                ) => {
                    screen.launch_plugin(
                        run_plugin,
                        should_float,
                        should_open_in_place,
                        close_replaced_pane,
                        pane_id_to_replace,
                        skip_cache,
                        cwd,
                        client_id,
                        completion_tx,
                        explicit_tab_id,
                        no_focus,
                        overlay_placement,
                    )?;
                },
                ScreenInstruction::SuppressPane(pane_id, client_id) => {
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
                        break 'instruction;
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
                },
                ScreenInstruction::UnsuppressPane(pane_id, should_float_if_hidden) => {
                    if let Some(tab) = screen.tab_containing_overlay_mut(pane_id) {
                        tab.show_overlay_pane(pane_id);
                        screen.render()?;
                        screen.log_and_report_session_state()?;
                        break 'instruction;
                    }
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.unsuppress_pane(pane_id, should_float_if_hidden);
                        drop(screen.render());
                    }
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::UnsuppressOrExpandPane(pane_id, should_float_if_hidden) => {
                    if let Some(tab) = screen.tab_containing_overlay_mut(pane_id) {
                        tab.show_overlay_pane(pane_id);
                        screen.render()?;
                        screen.log_and_report_session_state()?;
                        break 'instruction;
                    }
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.unsuppress_or_expand_pane(pane_id, should_float_if_hidden);
                        drop(screen.render());
                    }
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::FocusPaneWithId(
                    pane_id,
                    should_float_if_hidden,
                    should_be_in_place_if_hidden,
                    client_id,
                    completion_tx,
                ) => {
                    screen.focus_pane_by_request(
                        pane_id,
                        should_float_if_hidden,
                        should_be_in_place_if_hidden,
                        client_id,
                        completion_tx,
                        &mut pending,
                    )?;
                },
                ScreenInstruction::RenamePane(
                    pane_id,
                    new_name,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
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
                },
                ScreenInstruction::RenameActivePane(new_name, client_id, completion_tx) => {
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
                },
                ScreenInstruction::RenameTab(
                    tab_index,
                    new_name,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
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
                },
                ScreenInstruction::GoToTabWithId(tab_id, client_id, _completion_tx) => {
                    let client_id_to_switch = client_id
                        .and_then(|cid| {
                            if screen.active_tab_ids.contains_key(&cid) {
                                Some(cid)
                            } else {
                                screen.active_tab_ids.keys().next().copied()
                            }
                        })
                        .or_else(|| screen.active_tab_ids.keys().next().copied());

                    if let Some(client_id) = client_id_to_switch {
                        // Get the position from the ID
                        if let Some(tab_position) = screen.get_tab_position_by_id(tab_id) {
                            // Navigation uses a 0-based display position.
                            screen.navigate_to_tab(tab_position, None, true, client_id)?;
                            screen.render()?;

                            screen
                                .tab_history
                                .entry(client_id)
                                .or_default()
                                .push(tab_id);
                        } else {
                            log::error!("Tab with ID {} not found", tab_id);
                        }
                    }
                },
                ScreenInstruction::RenameTabWithId(tab_id, new_name, _completion_tx) => {
                    // Use get_tab_by_id_mut() helper method
                    if let Some(tab) = screen.get_tab_by_id_mut(tab_id) {
                        tab.rename_by_user(String::from_utf8_lossy(&new_name).to_string());
                        screen.log_and_report_session_state()?;
                    } else {
                        log::error!("Failed to find tab with ID: {}", tab_id);
                    }
                },
                ScreenInstruction::ArrangeProjectPanes(groups, client_id) => {
                    screen.arrange_project_panes(groups, client_id).non_fatal();
                    screen.render()?;
                },
                ScreenInstruction::SetProjectContext(projects, panes, sources) => {
                    screen.projects = projects.into_iter().collect();
                    screen.project_sources = sources;
                    screen.pane_projects = panes
                        .into_iter()
                        .filter(|(_, project)| screen.projects.contains(project))
                        .collect();
                    screen.reconcile_project_contexts();
                    screen.log_and_report_session_state()?;
                    screen.render()?;
                },
                ScreenInstruction::SetTabProjects(tab_projects) => {
                    // Whole map each time, so a tab that lost its project loses the
                    // name with it. Tabs we have never heard of are kept: a plugin can
                    // know about a tab before we have finished making it, and the next
                    // pass drops whatever is genuinely gone.
                    let tab_projects: HashMap<TabId, String> = tab_projects.into_iter().collect();
                    if tab_projects != screen.tab_projects {
                        screen.tab_projects = tab_projects;
                        if screen.update_plugin_tab_names() {
                            screen.log_and_report_session_state()?;
                        }
                    }
                },
                ScreenInstruction::CloseTabWithId(tab_id, _completion_tx) => {
                    if screen.get_tab_by_id(tab_id).is_some_and(Tab::is_pending) {
                        screen.close_tab_by_id(tab_id).non_fatal();
                        if !screen.has_pending_tabs() {
                            screen
                                .apply_deferred_tab_switches(&mut pending.tab_switches)
                                .non_fatal();
                        }
                    } else if screen.get_tab_by_id(tab_id).is_some() {
                        screen.request_close_tab(tab_id, None).non_fatal();
                    } else {
                        log::error!("Failed to find tab with ID: {}", tab_id);
                    }
                },
                ScreenInstruction::BreakPanesToTabWithId {
                    pane_ids,
                    tab_id,
                    should_change_focus_to_target_tab,
                    client_id,
                    mut completion_tx,
                } => {
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
                                let _ = screen.bus.senders.send_to_background_jobs(
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
                },
                ScreenInstruction::RequestPluginPermissions(plugin_id, plugin_permission) => {
                    screen.open_permission_modal(plugin_id, plugin_permission);
                    screen.render()?;
                },
                ScreenInstruction::CancelPluginPermission(token) => {
                    screen.cancel_permission_modal(token);
                    screen.render()?;
                },
                ScreenInstruction::WarpPane {
                    supports_companions,
                    supports_held,
                    pane_id,
                    to_session,
                    placement,
                    client_id,
                    response_channel,
                    deadline,
                } => {
                    screen.warp_pane(
                        supports_companions,
                        supports_held,
                        pane_id,
                        to_session,
                        placement,
                        client_id,
                        response_channel,
                        deadline,
                    )?;
                },
                ScreenInstruction::PaneWarpFinished => {
                    match screen.in_flight_pane_warps.checked_sub(1) {
                        Some(remaining) => screen.in_flight_pane_warps = remaining,
                        None => log::warn!("a pane warp finished that was never started"),
                    }
                },
                ScreenInstruction::AdoptedPaneFinished(finish) => {
                    screen.finish_adopted_pane(finish);
                },
                ScreenInstruction::PaneReleasedForWarp {
                    terminal_id,
                    placement: _,
                    to_session: _,
                    client_id: _,
                    released,
                } => {
                    screen.pane_released_for_warp(terminal_id, released);
                    screen.log_and_report_session_state().non_fatal();
                    screen.render()?;
                },
                ScreenInstruction::RequestPaneRedraw(terminal_id) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(PaneId::Terminal(terminal_id))
                    {
                        tab.request_pane_redraw(terminal_id)
                            .context("failed to redraw adopted pane")
                            .non_fatal();
                    }
                },
                ScreenInstruction::AdoptedPaneReady {
                    hold,
                    placed,
                    terminal_id,
                    payload,
                    placement,
                    client_id,
                    cmd,
                    buffered,
                    child_exit_status,
                } => {
                    let mut finish = Some(yinz_bus::warp::AdoptedPaneFinish {
                        terminal_id,
                        recovery: crate::warp::recovery_command(&payload),
                        held: payload.held.is_some(),
                        hold,
                        placed,
                    });
                    // The pty thread has already told the caller this session owns
                    // the terminal, so a placement failure is handled here and
                    // reported through `placed`.
                    screen
                        .place_adopted_pane(
                            terminal_id,
                            *payload,
                            placement,
                            client_id,
                            cmd,
                            buffered,
                            child_exit_status,
                            &mut finish,
                        )
                        .non_fatal();
                    // Failed before its pane was asked for, so there is none.
                    if let Some(finish) = finish {
                        screen.finish_adopted_pane(finish);
                    }
                    screen.log_and_report_session_state().non_fatal();
                    screen.render()?;
                },
                ScreenInstruction::WarpTab {
                    supports_companions,
                    supports_held,
                    tab_id,
                    client_id,
                    response_channel,
                    deadline,
                } => {
                    screen.warp_tab(
                        supports_companions,
                        supports_held,
                        tab_id,
                        client_id,
                        response_channel,
                        deadline,
                    )?;
                },
                ScreenInstruction::TabReleasedForWarp {
                    tab_index,
                    released,
                } => {
                    screen.tab_released_for_warp(tab_index, released);
                    screen.log_and_report_session_state().non_fatal();
                    screen.render()?;
                },
                ScreenInstruction::AdoptedTabReady {
                    is_rollback,
                    response_channel,
                    hold,
                    placement,
                    payload,
                    adopted,
                    client_id,
                } => {
                    let result = screen.place_adopted_tab(
                        *payload,
                        *adopted,
                        client_id,
                        is_rollback,
                        placement,
                        response_channel.clone(),
                        hold,
                    );
                    if let Err(error) = result {
                        if let Some(response_channel) = response_channel {
                            let _ = response_channel.send(Err(error));
                        } else {
                            return Err(error);
                        }
                    }
                    screen.log_and_report_session_state().non_fatal();
                    screen.render()?;
                },
                ScreenInstruction::BreakPane(default_shell, client_id, _completion_tx) => {
                    let default_layout = screen.default_layout.clone();
                    screen.break_pane(default_shell, default_layout, client_id)?;
                },
                ScreenInstruction::BreakPaneRight(client_id, _completion_tx) => {
                    screen.break_pane_to_new_tab(Direction::Right, client_id)?;
                },
                ScreenInstruction::BreakPaneLeft(client_id, _completion_tx) => {
                    screen.break_pane_to_new_tab(Direction::Left, client_id)?;
                },
                ScreenInstruction::UpdateSessionInfos(
                    new_session_infos,
                    resurrectable_sessions,
                ) => {
                    screen.update_session_infos(new_session_infos, resurrectable_sessions)?;
                },
                ScreenInstruction::UpdateAvailableLayouts {
                    generation,
                    layouts,
                    errors,
                } => {
                    screen.update_available_layouts(generation, layouts, errors);
                },
                ScreenInstruction::ReplacePane(
                    new_pane_id,
                    hold_for_command,
                    pane_title,
                    invoked_with,
                    replaced,
                    client_id_tab_index_or_pane_id,
                    mut completion_tx,
                ) => {
                    let placed = screen.replace_pane(
                        new_pane_id,
                        hold_for_command,
                        invoked_with,
                        pane_title,
                        replaced,
                        client_id_tab_index_or_pane_id,
                        &mut completion_tx,
                    );
                    let waiting = pending.waiting_for_pane.remove(&new_pane_id);
                    if placed {
                        if let Some(c) = completion_tx.as_mut() {
                            c.set_affected_pane_id(new_pane_id)
                        }
                        // A plugin can open an agent pane in place, the same as anywhere else.
                        screen.fire_agent_start_for_new_pane(new_pane_id);
                        // Its reader can produce output before the replacement is placed, just
                        // as it can before NewPane. Agent-start precedes the buffered events.
                        for event in waiting.into_iter().flatten() {
                            replay_pending_pane_event(&mut screen, event, &mut pending).non_fatal();
                        }
                    }

                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::OpenPluginPaneInPlaceOfPaneId {
                    pane_id,
                    plugin_url,
                    configuration,
                    cwd,
                    client_id,
                    completion,
                } => {
                    screen.open_plugin_pane_in_place_of_pane_id(
                        pane_id,
                        plugin_url,
                        configuration,
                        cwd,
                        client_id,
                        completion,
                    );
                },
                ScreenInstruction::SerializeLayoutForResurrection => {
                    if screen.session_serialization {
                        screen.dump_layout_to_hd()?;
                    }
                },
                ScreenInstruction::SaveSession(_client_id, completion_tx) => {
                    screen.save_session(completion_tx)?;
                },
                ScreenInstruction::Notify(text, pane_id, on_click, completion) => {
                    screen.receive_toast(text, pane_id, on_click, None, completion)?;
                },
                ScreenInstruction::PluginNotify {
                    text,
                    pane_id,
                    on_click,
                    plugin_id,
                    completion,
                } => {
                    screen.receive_toast(text, pane_id, on_click, Some(plugin_id), completion)?;
                },
                ScreenInstruction::NotificationPolicy(plugin_id, reason) => {
                    screen.set_notification_policy(plugin_id, reason)?;
                },
                ScreenInstruction::NotificationPolicyUnavailable {
                    plugin_id,
                    reason,
                    disable,
                } => {
                    screen.notification_policy_unavailable(plugin_id, reason, disable)?;
                },
                ScreenInstruction::BindNotificationRequest {
                    id,
                    plugin_id,
                    instance_id,
                } => {
                    screen.bind_notification_request(id, plugin_id, instance_id);
                },
                ScreenInstruction::ResolveNotification {
                    id,
                    decision,
                    plugin_id,
                    instance_id,
                } => {
                    screen.resolve_notification(id, decision, plugin_id, instance_id)?;
                },
                ScreenInstruction::ContextMenuTimedOut(request_id, client_id) => {
                    screen.context_menu_timed_out(request_id, client_id);
                },
                ScreenInstruction::SetContextMenu {
                    request_id,
                    items,
                    plugin_id,
                    client_id,
                } => {
                    screen.set_context_menu_items(request_id, items, plugin_id, client_id);
                },
                ScreenInstruction::ActivateCommandPaletteAction {
                    label,
                    pane_id,
                    tab_id,
                    action,
                    client_id,
                } => {
                    screen
                        .dispatch_command_palette_action(pane_id, tab_id, action, client_id, label);
                    screen.render()?;
                },
                ScreenInstruction::MoveCommandPalettePane {
                    pane_id,
                    tab_id,
                    direction,
                    client_id,
                } => {
                    if let Err(error) =
                        screen.move_command_palette_pane(pane_id, tab_id, direction, client_id)
                    {
                        screen
                            .bus
                            .senders
                            .send_to_screen(ScreenInstruction::Notify(
                                format!("Could not move pane: {error}"),
                                None,
                                None,
                                None,
                            ))
                            .non_fatal();
                    }
                    screen.render()?;
                },
                ScreenInstruction::ActivateContextMenuItem {
                    pane_id,
                    actions,
                    client_id,
                } => {
                    screen.dispatch_context_menu_actions(pane_id, actions, client_id);
                    screen.render()?;
                },
                ScreenInstruction::CloseContextMenu(client_id) => {
                    if screen.close_context_menu(client_id) {
                        screen.render()?;
                    }
                },
                ScreenInstruction::DismissNotification(id) => {
                    // the toast stays in the stack, sliding back out, until
                    // `reap_settled_notifications` sees its reveal reach zero and actually removes it
                    screen.dismiss_notification(id, Instant::now())?;
                },
                ScreenInstruction::AnimationTick => {
                    screen.on_animation_tick(Instant::now())?;
                },
                ScreenInstruction::RenameSession(
                    name,
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    screen.rename_session(name, client_id)?;
                },
                ScreenInstruction::Reconfigure {
                    client_id,
                    keybinds,
                    config,
                } => {
                    screen.reconfigure(config, keybinds, client_id).non_fatal();
                },
                ScreenInstruction::RerunCommandPane(terminal_pane_id, completion_tx) => {
                    screen.rerun_command_pane_with_id(terminal_pane_id, completion_tx);
                    screen.log_and_report_session_state()?;
                    screen.render()?;
                },
                ScreenInstruction::RestartAgentPane {
                    pane_id,
                    prompt,
                    session,
                    originating_plugin,
                    response,
                } => {
                    let result = screen.restart_agent_pane(
                        pane_id,
                        prompt.as_deref(),
                        session,
                        Some(originating_plugin),
                    );
                    let _ = response.send(result);
                    screen.log_and_report_session_state()?;
                    screen.render()?;
                },
                ScreenInstruction::RunCommandInPane(
                    terminal_pane_id,
                    run_command,
                    keep_scrollback,
                ) => {
                    screen.run_command_in_pane_with_id(
                        terminal_pane_id,
                        run_command,
                        keep_scrollback,
                    );
                    screen.log_and_report_session_state()?;
                    screen.render()?;
                },
                ScreenInstruction::ResizePaneWithId(resize, pane_id) => {
                    screen.resize_pane_with_id(resize, pane_id)?;
                },
                ScreenInstruction::EditScrollbackForPaneWithId(pane_id, completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.edit_scrollback_for_pane_with_id(pane_id, completion_tx)
                            .non_fatal();
                    }
                    screen.render()?;
                },
                ScreenInstruction::WriteToPaneId(bytes, pane_id, _completion) => {
                    screen
                        .write_to_pane_id(&None, bytes, false, pane_id, None, _completion)
                        .non_fatal();
                    screen.render()?;
                },
                ScreenInstruction::Paste(bytes, pane_id, client_id, _completion) => {
                    match pane_id {
                        None if screen
                            .command_palette_handles_input(&None, &bytes, false, client_id) => {},
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
                },
                ScreenInstruction::ResolveAgentTab {
                    placement,
                    invoked_with,
                    client_id,
                    reply,
                } => {
                    let result = screen.resolve_agent_tab(placement, invoked_with, client_id);
                    // If the receiver disappeared, retain the tab: the outcome is unknown.
                    let _ = reply.send(result);
                },
                ScreenInstruction::CleanupAgentTab(placement, reply) => {
                    let _ = reply.send(screen.cleanup_agent_tab(&placement));
                },
                ScreenInstruction::AgentSpawned(agent) => {
                    screen.agent_launches.insert(agent.pane_id, agent);
                },
                ScreenInstruction::AgentControlReply(request, reply) => {
                    screen.agent_control_with_reply(
                        request,
                        super::agent_control::Reply::Host(reply),
                    );
                },
                ScreenInstruction::AgentControl(request, client) => {
                    screen.agent_control(request, client);
                },
                ScreenInstruction::AgentReport(report, accepted_at) => {
                    if !apply_queued_agent_report(&mut screen, &report, accepted_at)? {
                        pending
                            .waiting_for_pane
                            .entry(report.pane_id)
                            .or_default()
                            .push(ScreenInstruction::AgentReport(report, accepted_at));
                    }
                },
                ScreenInstruction::RestoreWarpedAgentInput(pane_id, input) => {
                    if screen.get_pane_info(pane_id).is_none() {
                        pending
                            .waiting_for_pane
                            .entry(pane_id)
                            .or_default()
                            .push(ScreenInstruction::RestoreWarpedAgentInput(pane_id, input));
                    } else {
                        screen.restore_warped_agent_input(pane_id, input);
                    }
                },
                ScreenInstruction::AgentWorktreeResolved {
                    cwd,
                    generation,
                    root,
                } => {
                    screen.agent_worktree_resolved(cwd, generation, root)?;
                },
                ScreenInstruction::SetPaneAgent(pane_id, agent, _completion) => {
                    if !apply_pane_agent_report(&mut screen, pane_id, agent.clone())? {
                        let pending_events = pending.waiting_for_pane.entry(pane_id).or_default();
                        pending_events
                            .retain(|event| !matches!(event, ScreenInstruction::SetPaneAgent(..)));
                        pending_events.push(ScreenInstruction::SetPaneAgent(
                            pane_id,
                            agent,
                            _completion,
                        ));
                        break 'instruction;
                    }
                },
                ScreenInstruction::SetPaneAgentRun(pane_id, run_id, _completion) => {
                    // Unlike a record, this waits for nothing: a run id names a record, and a pane
                    // with no record has nothing for it to name. Dropping it is the answer, not
                    // queueing it — the plugin that sent this re-sends on its next pass anyway.
                    let updated = screen
                        .tab_containing_pane(pane_id)
                        .and_then(|tab| tab.pane_agent_info(pane_id))
                        .filter(|agent| agent.run_id != run_id)
                        .map(|mut agent| {
                            agent.run_id = run_id;
                            agent
                        });
                    if let Some(agent) = updated {
                        if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                            tab.set_pane_agent(pane_id, Some(agent));
                        }
                        screen.report_pane_state()?;
                    }
                },
                ScreenInstruction::SetQuitNotice(plugin_id, notice, guard) => {
                    screen.set_quit_notice(plugin_id, notice, guard);
                },
                ScreenInstruction::ConfirmQuit(client_id, completion) => {
                    screen.confirm_quit(client_id, false, completion)?;
                },
                ScreenInstruction::SetPaneColor(pane_id, fg, bg, _completion) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.set_pane_color(pane_id, fg, bg).non_fatal();
                    }
                    screen.render()?;
                },
                ScreenInstruction::WriteKeyToPaneId(
                    key_with_modifier,
                    bytes,
                    is_kitty,
                    pane_id,
                    _completion,
                ) => {
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
                },
                ScreenInstruction::CopyTextToClipboard(text, plugin_id) => {
                    let plugin_pane_id = PaneId::Plugin(plugin_id);
                    if let Some(tab) = screen.tab_containing_pane_mut(plugin_pane_id) {
                        tab.copy_text_to_clipboard(&text)
                            .with_context(|| {
                                format!(
                                    "failed to copy text to clipboard from plugin {}",
                                    plugin_id
                                )
                            })
                            .non_fatal();
                    }
                    screen.render()?;
                },
                ScreenInstruction::MovePaneWithPaneId(pane_id) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.move_pane(pane_id);
                    }
                    screen.render()?;
                },
                ScreenInstruction::MovePaneWithPaneIdInDirection(pane_id, direction) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        match direction {
                            Direction::Down => tab.move_pane_down(pane_id),
                            Direction::Up => tab.move_pane_up(pane_id),
                            Direction::Left => tab.move_pane_left(pane_id),
                            Direction::Right => tab.move_pane_right(pane_id),
                        }
                    }
                    screen.render()?;
                },
                ScreenInstruction::ClearScreenForPaneId(pane_id) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.clear_screen_for_pane_id(pane_id);
                    }
                    screen.render()?;
                },
                ScreenInstruction::ScrollUpInPaneId(pane_id) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        if let PaneId::Terminal(terminal_pane_id) = pane_id {
                            tab.scroll_terminal_up(terminal_pane_id)?;
                        } else {
                            // this is because to do this with plugins, we need the client_id -
                            // which we do not have (yet?) in this context...
                            log::error!(
                                "Currently only terminal panes are supported for scrolling up"
                            );
                        }
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::ScrollDownInPaneId(pane_id) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        if let PaneId::Terminal(terminal_pane_id) = pane_id {
                            tab.scroll_terminal_down(terminal_pane_id)?;
                        } else {
                            // this is because to do this with plugins, we need the client_id -
                            // which we do not have (yet?) in this context...
                            log::error!(
                                "Currently only terminal panes are supported for scrolling down"
                            );
                        }
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::ScrollToTopInPaneId(pane_id) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        if let PaneId::Terminal(terminal_pane_id) = pane_id {
                            tab.scroll_terminal_to_top(terminal_pane_id);
                        } else {
                            // this is because to do this with plugins, we need the client_id -
                            // which we do not have (yet?) in this context...
                            log::error!(
                                "Currently only terminal panes are supported for scrolling to top"
                            );
                        }
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::ScrollToBottomInPaneId(pane_id) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        if let PaneId::Terminal(terminal_pane_id) = pane_id {
                            tab.scroll_terminal_to_bottom(terminal_pane_id);
                        } else {
                            // this is because to do this with plugins, we need the client_id -
                            // which we do not have (yet?) in this context...
                            log::error!(
                            "Currently only terminal panes are supported for scrolling to bottom"
                        );
                        }
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::PageScrollUpInPaneId(pane_id) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        if let PaneId::Terminal(terminal_pane_id) = pane_id {
                            tab.scroll_terminal_page_up(terminal_pane_id);
                        } else {
                            // this is because to do this with plugins, we need the client_id -
                            // which we do not have (yet?) in this context...
                            log::error!(
                                "Currently only terminal panes are supported for scrolling"
                            );
                        }
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::PageScrollDownInPaneId(pane_id) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        if let PaneId::Terminal(terminal_pane_id) = pane_id {
                            tab.scroll_terminal_page_down(terminal_pane_id);
                        } else {
                            // this is because to do this with plugins, we need the client_id -
                            // which we do not have (yet?) in this context...
                            log::error!(
                                "Currently only terminal panes are supported for scrolling"
                            );
                        }
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::TogglePaneIdFullscreen(pane_id) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.toggle_pane_fullscreen(pane_id);
                    }
                    screen.render()?;
                },
                ScreenInstruction::TogglePaneEmbedOrEjectForPaneId(pane_id) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.toggle_pane_embed_or_floating_for_pane_id(pane_id, None)
                            .non_fatal();
                    }
                    screen.render()?;
                },
                ScreenInstruction::CloseTabWithIndex(tab_index) => {
                    // tab_index here means display position (0-based) per API convention
                    // Resolve the display position before the shared close preflight.

                    if let Some(tab_id) = screen.get_tab_id_at_position(tab_index) {
                        screen.request_close_tab(tab_id, None).non_fatal();
                    } else {
                        log::error!("Failed to find tab at position: {}", tab_index);
                    }
                },
                ScreenInstruction::BreakPanesToNewTab {
                    pane_ids,
                    default_shell,
                    should_change_focus_to_new_tab,
                    new_tab_name,
                    client_id,
                    mut completion_tx,
                } => {
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
                        let _ = screen.bus.senders.send_to_background_jobs(
                            BackgroundJob::HighlightPanesWithMessage(
                                pane_group.iter().copied().collect(),
                                "BROKEN OUT".to_owned(),
                            ),
                        );
                    }
                    screen.clear_pane_group(&client_id);
                },
                ScreenInstruction::BreakPanesToTabWithIndex {
                    pane_ids,
                    tab_index,
                    should_change_focus_to_new_tab,
                    client_id,
                    mut completion_tx,
                } => {
                    // tab_index here means display position (0-based) per API convention,
                    // and the callee is id-space, so it must be mapped to a stable ID -
                    // the same conversion CloseTabWithIndex does
                    let Some(tab_id) = screen.get_tab_id_at_position(tab_index) else {
                        log::error!("Failed to find tab at position: {}", tab_index);
                        break 'instruction; // affected_tab_id stays None, signalling failure
                    };
                    let outcome = screen.break_multiple_panes_to_tab_with_id(
                        pane_ids,
                        tab_id,
                        should_change_focus_to_new_tab,
                        client_id,
                    )?;
                    match outcome.affected_tab_id() {
                    Some(affected_tab_id) => {
                        if let Some(c) = completion_tx
                            .as_mut() { c.set_affected_tab_id(affected_tab_id) }
                        let pane_group = screen.get_client_pane_group(&client_id);
                        if !pane_group.is_empty() {
                            let _ = screen.bus.senders.send_to_background_jobs(
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
                },
                ScreenInstruction::TogglePanePinned(client_id, _completion_tx) => {
                    screen.toggle_pane_pinned(client_id);
                },
                ScreenInstruction::SetFloatingPanePinned(pane_id, should_be_pinned) => {
                    screen.set_floating_pane_pinned(pane_id, should_be_pinned);
                },
                ScreenInstruction::StackPanes(pane_ids_to_stack, client_id, _completion_tx) => {
                    if let Some(last_pane_id) = screen.stack_panes(pane_ids_to_stack) {
                        let _ = screen.focus_pane_with_id(last_pane_id, false, false, client_id);
                        let _ = screen.render();
                        let pane_group = screen.get_client_pane_group(&client_id);
                        if !pane_group.is_empty() {
                            let _ = screen.bus.senders.send_to_background_jobs(
                                BackgroundJob::HighlightPanesWithMessage(
                                    pane_group.iter().copied().collect(),
                                    "STACKED".to_owned(),
                                ),
                            );
                        }
                        screen.clear_pane_group(&client_id);
                    }
                },
                ScreenInstruction::ChangeFloatingPanesCoordinates(
                    pane_ids_and_coordinates,
                    _completion_tx,
                ) => {
                    screen.change_floating_panes_coordinates(pane_ids_and_coordinates);
                    let _ = screen.render();
                },
                ScreenInstruction::PaneTabsAction(step, client, _completion) => {
                    screen.pane_tabs_action(step, client)?;
                    screen.render()?;
                },
                ScreenInstruction::PaneTabs {
                    command,
                    client_id,
                    response,
                } => {
                    let result = screen
                        .pane_tabs_command(command, client_id)
                        .map_err(|error| error.to_string());
                    let _ = response.send(result);
                    screen.render()?;
                },
                ScreenInstruction::OverlayTabCwd {
                    id,
                    cwd,
                    caller_cwd,
                    caller_terminal_pane_id,
                } => {
                    screen.overlay_tab_cwd(id, cwd, caller_cwd, caller_terminal_pane_id)?;
                },
                ScreenInstruction::PaneTabLoaded {
                    id,
                    index,
                    generation,
                    result,
                    client_id: _,
                } => {
                    let plugin = result.as_ref().ok().map(|(plugin, _)| *plugin);
                    screen.pane_tab_loaded(id, index, generation, result)?;
                    if let Some(plugin) = plugin {
                        let events = pending
                            .waiting_for_pane
                            .remove(&PaneId::Plugin(plugin))
                            .unwrap_or_default();
                        if screen.tab_containing_pane(PaneId::Plugin(plugin)).is_some() {
                            super::replay_pending_plugin_events(&mut screen, events)?;
                        } else {
                            screen.cleanup_closed_pane(PaneId::Plugin(plugin));
                        }
                    }
                    screen.render()?;
                },
                ScreenInstruction::Multipane {
                    operation,
                    client_id,
                    response,
                } => {
                    let result = screen
                        .multipane_command(operation, client_id)
                        .map_err(|error| error.to_string());
                    let _ = response.send(result);
                    screen.log_and_report_session_state().non_fatal();
                    screen.render()?;
                },
                ScreenInstruction::ChangeOverlayCoordinates(
                    pane_ids_and_coordinates,
                    override_user_placement,
                    _completion_tx,
                ) => {
                    for (pane_id, coordinates) in screen.change_overlay_coordinates(
                        pane_ids_and_coordinates,
                        override_user_placement,
                    ) {
                        pending.waiting_for_pane.entry(pane_id).or_default().push(
                            ScreenInstruction::ChangeOverlayCoordinates(
                                vec![(pane_id, coordinates)],
                                override_user_placement,
                                None,
                            ),
                        );
                    }
                    let _ = screen.render();
                },
                ScreenInstruction::TogglePaneBorderless(pane_id, _completion_tx) => {
                    screen.toggle_pane_borderless(pane_id);
                    let _ = screen.render();
                },
                ScreenInstruction::SetPaneBorderless(pane_id, borderless, _completion_tx) => {
                    screen.set_pane_borderless(pane_id, borderless);
                    let _ = screen.render();
                },
                ScreenInstruction::GroupAndUngroupPanes(
                    pane_ids_to_group,
                    pane_ids_to_ungroup,
                    for_all_clients,
                    client_id,
                ) => {
                    screen.group_and_ungroup_panes(
                        pane_ids_to_group,
                        pane_ids_to_ungroup,
                        for_all_clients,
                        client_id,
                    );
                    let _ = screen.log_and_report_session_state();
                },
                ScreenInstruction::TogglePaneInGroup(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    screen.toggle_pane_in_group(client_id).non_fatal();
                },
                ScreenInstruction::ToggleGroupMarking(
                    client_id,
                    _completion_tx, // the action ends here, dropping this will release anything
                                    // waiting for it
                ) => {
                    screen.toggle_group_marking(client_id).non_fatal();
                },
                ScreenInstruction::HighlightAndUnhighlightPanes(
                    pane_ids_to_highlight,
                    pane_ids_to_unhighlight,
                    client_id,
                ) => {
                    {
                        for pane_id in pane_ids_to_highlight {
                            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                                tab.add_highlight_pane_frame_color_override(
                                    pane_id,
                                    None,
                                    Some(client_id),
                                );
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
                },
                ScreenInstruction::RevealPaneWithId(pane_id, label, client_id) => {
                    screen.reveal_pane_with_id(pane_id, label, client_id)?;
                },
                ScreenInstruction::FloatMultiplePanes(pane_ids_to_float, client_id) => {
                    if screen.selection_has_multipane(&pane_ids_to_float) {
                        Err::<(), _>(anyhow!(
                            "cannot detach a multipane child from its container"
                        ))
                        .non_fatal();
                        break 'instruction;
                    }
                    {
                        let mut ejected_panes_in_group = vec![];
                        for pane_id in pane_ids_to_float {
                            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                                if !tab.pane_id_is_floating(&pane_id) {
                                    ejected_panes_in_group.push(pane_id);
                                    tab.toggle_pane_embed_or_floating_for_pane_id(
                                        pane_id,
                                        Some(client_id),
                                    )
                                    .non_fatal();
                                }
                                tab.show_floating_panes();
                            }
                        }
                        screen.render()?;
                        if !ejected_panes_in_group.is_empty() {
                            let _ = screen.bus.senders.send_to_background_jobs(
                                BackgroundJob::HighlightPanesWithMessage(
                                    ejected_panes_in_group,
                                    "EJECTED".to_owned(),
                                ),
                            );
                        }
                    }
                    let _ = screen.log_and_report_session_state();
                },
                ScreenInstruction::EmbedMultiplePanes(pane_ids_to_float, client_id) => {
                    if screen.selection_has_multipane(&pane_ids_to_float) {
                        Err::<(), _>(anyhow!(
                            "cannot detach a multipane child from its container"
                        ))
                        .non_fatal();
                        break 'instruction;
                    }
                    {
                        let mut embedded_panes_in_group = vec![];
                        for pane_id in pane_ids_to_float {
                            if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                                if tab.pane_id_is_floating(&pane_id) {
                                    embedded_panes_in_group.push(pane_id);
                                    tab.toggle_pane_embed_or_floating_for_pane_id(
                                        pane_id,
                                        Some(client_id),
                                    )
                                    .non_fatal();
                                }
                                tab.hide_floating_panes();
                            }
                        }
                        screen.render()?;
                        if !embedded_panes_in_group.is_empty() {
                            let _ = screen.bus.senders.send_to_background_jobs(
                                BackgroundJob::HighlightPanesWithMessage(
                                    embedded_panes_in_group,
                                    "EMBEDDED".to_owned(),
                                ),
                            );
                        }
                    }
                    let _ = screen.log_and_report_session_state();
                },
                ScreenInstruction::InterceptKeyPresses(plugin_id, client_id) => {
                    keybind_intercepts.insert(client_id, plugin_id);
                },
                ScreenInstruction::ClearKeyPressesIntercepts(plugin_id, client_id) => {
                    if keybind_intercepts.get(&client_id) == Some(&plugin_id) {
                        keybind_intercepts.remove(&client_id);
                    }
                },
                ScreenInstruction::ReplacePaneWithExistingPane(
                    old_pane_id,
                    new_pane_id,
                    suppress_replaced_pane,
                    completion_tx,
                ) => screen
                    .replace_pane_with_existing_pane(
                        old_pane_id,
                        new_pane_id,
                        suppress_replaced_pane,
                        completion_tx,
                    )
                    .non_fatal(),
                ScreenInstruction::AddWatcherClient(client_id, size) => {
                    screen
                        .add_watcher_client(client_id)
                        .context("failed to add watcher client")?;
                    screen.set_watcher_size(client_id, size);
                    screen.render()?;
                },
                ScreenInstruction::RemoveWatcherClient(client_id) => {
                    let departed = screen.departed_follow_source();
                    screen.remove_watcher_client(client_id);
                    if screen.followed_client_id.is_none()
                        && let Some((source, _)) = departed
                    {
                        pending.remove_client_plugin_output(source);
                    }
                },
                ScreenInstruction::SetFollowedClient(client_id) => {
                    if let Some(departed) = screen.followed_client_id.filter(|id| {
                        *id != client_id && !screen.connected_clients.borrow().contains(id)
                    }) {
                        pending.remove_client_plugin_output(departed);
                    }
                    screen
                        .set_followed_client(client_id)
                        .context("failed to set followed client")?;
                },
                ScreenInstruction::WatcherTerminalResize(client_id, size) => {
                    screen.set_watcher_size(client_id, size);
                    screen.render()?;
                },
                ScreenInstruction::ClearMouseHelpText(client_id) => {
                    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
                        tab.clear_mouse_help_text(client_id);
                        screen.render()?;
                    }
                },
                ScreenInstruction::SetPluginRegexHighlights {
                    pane_id,
                    plugin_id,
                    highlights,
                } => {
                    let style = screen.style;
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.set_plugin_regex_highlights_for_pane(
                            pane_id, plugin_id, highlights, &style,
                        );
                    }
                    screen.render()?;
                },
                ScreenInstruction::ClearPluginHighlights { pane_id, plugin_id } => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.clear_plugin_highlights_for_pane(pane_id, plugin_id);
                    }
                    screen.render()?;
                },
                ScreenInstruction::ClearAllPluginHighlights(plugin_id) => {
                    let all_tabs = screen.get_tabs_mut();
                    for tab in all_tabs.values_mut() {
                        tab.clear_all_plugin_highlights(plugin_id);
                    }
                    screen.render()?;
                },
                ScreenInstruction::DesktopNotificationResponse(raw_bytes, client_id) => {
                    if screen.activate_policy_desktop(&raw_bytes, client_id) {
                        screen.render()?;
                    } else if let Some(response) = denormalize_notification_response(&raw_bytes) {
                        let pane_id = PaneId::Terminal(response.terminal_id);
                        if response.focus_pane {
                            screen.consume_pane_desktop_notification(
                                pane_id,
                                &raw_bytes,
                                Instant::now(),
                            );
                        }
                        if response.forward_to_pane
                            && let Some(tab) = screen.tab_containing_pane_mut(pane_id)
                        {
                            tab.write_to_pane_id(&None, response.bytes, false, pane_id, None, None)
                                .non_fatal();
                        }
                        if response.focus_pane {
                            screen
                                .focus_pane_with_id(pane_id, false, false, client_id)
                                .non_fatal();
                        }
                        screen.render()?;
                        screen.log_and_report_session_state()?;
                    }
                },
                ScreenInstruction::SubscribeToPaneRenders {
                    client_id,
                    pane_ids,
                    scrollback,
                    ansi,
                } => {
                    screen.subscribe_to_pane_renders(client_id, pane_ids, scrollback, ansi);
                },
                ScreenInstruction::NotifyPaneClosedToSubscribers { pane_id } => {
                    screen.cleanup_closed_pane(pane_id);
                    pending.waiting_for_pane.remove(&pane_id);
                    screen.notify_pane_closed_to_subscribers(pane_id);
                },
                ScreenInstruction::InvalidateAgentEventProjects => {
                    screen.invalidate_agent_event_projects();
                },
                ScreenInstruction::AgentEventProjects(version, result) => {
                    screen.apply_agent_event_projects(version, result);
                },
                ScreenInstruction::SubscribeAgentEvents(client, options) => {
                    screen.subscribe_agent_events(client, options);
                },
                ScreenInstruction::SetCliEventSubscribersPresent(present) => {
                    screen.cli_event_subscribers_present = present;
                    if present {
                        // A new subscriber gets the state as it stands, not the next change to it.
                        screen.log_and_report_session_state()?;
                    }
                },
                ScreenInstruction::UpdateBackgroundPluginSubscriptions(
                    plugin_id,
                    client_id,
                    subscriptions,
                ) => {
                    let previous = screen
                        .background_plugin_subscriptions
                        .get(&(plugin_id, client_id));
                    let needs_state =
                        [EventType::PaneUpdate, EventType::TabUpdate]
                            .iter()
                            .any(|event| {
                                subscriptions.contains(event)
                                    && previous.is_none_or(|previous| !previous.contains(event))
                            });
                    if subscriptions.is_empty() {
                        screen
                            .background_plugin_subscriptions
                            .remove(&(plugin_id, client_id));
                    } else {
                        screen
                            .background_plugin_subscriptions
                            .insert((plugin_id, client_id), subscriptions);
                    }
                    // The plugin may subscribe after startup's last layout report. Give it
                    // the current panes and tabs even when the session is otherwise quiet.
                    if needs_state {
                        screen.log_and_report_session_state()?;
                    }
                },
                ScreenInstruction::ClearHintTextCache => {
                    for tab in screen.tabs.values_mut() {
                        tab.clear_hint_text_cache();
                    }
                    screen.render()?;
                },
                ScreenInstruction::BroadcastModeUpdate(mode_info, target_client_id) => {
                    screen.broadcast_mode_update(mode_info, target_client_id)?;
                },
                // Pane-targeting CLI handlers
                ScreenInstruction::ScrollUpWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.scroll_up_by_pane_id(pane_id);
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::ScrollDownWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.scroll_down_by_pane_id(pane_id).non_fatal();
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::ScrollToTopWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.scroll_to_top_by_pane_id(pane_id).non_fatal();
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::ScrollToBottomWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.scroll_to_bottom_by_pane_id(pane_id).non_fatal();
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::PageScrollUpWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.page_scroll_up_by_pane_id(pane_id);
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::PageScrollDownWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.page_scroll_down_by_pane_id(pane_id).non_fatal();
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::HalfPageScrollUpWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.half_page_scroll_up_by_pane_id(pane_id);
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::HalfPageScrollDownWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.half_page_scroll_down_by_pane_id(pane_id).non_fatal();
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.sync_scroll_mode_for_pane_id(pane_id)?;
                    screen.render()?;
                },
                ScreenInstruction::ResizeWithPaneId(pane_id, strategy, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.resize_by_pane_id(pane_id, strategy);
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::MovePaneWithPaneIdCli(
                    pane_id,
                    direction,
                    mut _completion_tx,
                ) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.move_pane_by_pane_id(pane_id, direction);
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::MovePaneBackwardsWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.move_pane_backwards_by_pane_id(pane_id);
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::ClearScreenWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.clear_screen_by_pane_id(pane_id).non_fatal();
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.render()?;
                },
                ScreenInstruction::EditScrollbackWithPaneId(pane_id, ansi, mut completion_tx) => {
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
                },
                ScreenInstruction::ToggleFullscreenWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.toggle_fullscreen_by_pane_id(pane_id);
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::ToggleNoUiFullscreenWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.toggle_no_ui_fullscreen_by_pane_id(pane_id);
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::TogglePaneEmbedOrFloatingWithPaneId(
                    pane_id,
                    mut completion_tx,
                ) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        if let Err(error) =
                            tab.toggle_pane_embed_or_floating_for_pane_id(pane_id, None)
                        {
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
                },
                ScreenInstruction::CloseFocusWithPaneId(pane_id, mut completion_tx) => {
                    // A headless plugin was unloaded by `settle_headless_target`. A pane that is
                    // already gone is closed; one that never existed is an error, not a success.
                    if screen.tab_containing_pane(pane_id).is_some() {
                        screen.close_pane_explicitly(pane_id, completion_tx);
                    } else if !screen.pane_is_gone(pane_id) {
                        Screen::report_pane_not_found(pane_id, &mut completion_tx);
                    }
                    let client_ids: Vec<_> = screen.active_tab_ids.keys().copied().collect();
                    for client_id in client_ids {
                        screen.sync_scroll_mode_on_focus(client_id)?;
                    }
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::RenamePaneWithPaneId(pane_id, name, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.rename_pane_by_pane_id(pane_id, name).non_fatal();
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.render()?;
                    screen.log_and_report_session_state()?;
                },
                ScreenInstruction::UndoRenamePaneWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.undo_rename_pane_by_pane_id(pane_id);
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                    screen.render()?;
                },
                ScreenInstruction::TogglePanePinnedWithPaneId(pane_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.toggle_pane_pinned_by_pane_id(pane_id);
                    } else {
                        Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                    }
                },
                // Tab-targeting CLI handlers
                ScreenInstruction::UndoRenameTabWithTabId(tab_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tabs.get_mut(&tab_id) {
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
                },
                ScreenInstruction::ToggleActiveSyncTabWithTabId(tab_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tabs.get_mut(&tab_id) {
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
                },
                ScreenInstruction::ToggleFloatingPanesWithTabId(
                    tab_id,
                    default_shell,
                    mut completion_tx,
                ) => {
                    if let Some(tab) = screen.tabs.get_mut(&tab_id) {
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
                },
                ScreenInstruction::PreviousSwapLayoutWithTabId(tab_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tabs.get_mut(&tab_id) {
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
                },
                ScreenInstruction::NextSwapLayoutWithTabId(tab_id, mut _completion_tx) => {
                    if let Some(tab) = screen.tabs.get_mut(&tab_id) {
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
                },
                ScreenInstruction::MoveTabWithTabId(tab_id, direction, _completion_tx) => {
                    if !screen.has_pending_tabs() {
                        screen.move_tab_by_id(tab_id, direction)?;
                        screen.render()?;
                    } else {
                        pending
                            .waiting_for_tab
                            .push(ScreenInstruction::MoveTabWithTabId(
                                tab_id,
                                direction,
                                _completion_tx,
                            ));
                    }
                },
                ScreenInstruction::FocusHostSession(client_id, _completion_tx) => {
                    let payload =
                        nested_session::encode_payload(&NestedSessionMessage::FocusHost {
                            direction: None,
                        });
                    let _ = screen.bus.senders.send_to_server(
                        ServerInstruction::EmitNestedSessionFrameToClient(client_id, payload),
                    );
                },
                ScreenInstruction::FocusGuestSession(client_id, _completion_tx) => {
                    screen.focus_guest_session(client_id);
                },
                ScreenInstruction::ToggleHostFullscreen(client_id, _completion_tx) => {
                    screen.own_fullscreen_requested = !screen.own_fullscreen_requested;
                    let fullscreen = screen.own_fullscreen_requested;
                    let payload = nested_session::encode_payload(
                        &NestedSessionMessage::ToggleHostFullscreen { fullscreen },
                    );
                    let _ = screen.bus.senders.send_to_server(
                        ServerInstruction::EmitNestedSessionFrameToClient(client_id, payload),
                    );
                },
            }
        }
        // Whatever the instruction did, no tab it emptied outlives it: a tab with nothing
        // selectable is closed here - and the session with it, unless something asks it to go
        // on - rather than left to a frame that may never come (nobody attached) or was never
        // scheduled (the route removed a pane without asking for one).
        screen
            .settle_tabs()
            .context("failed to settle tabs after instruction")
            .non_fatal();
        screen.reconcile_project_contexts();
        // Teardown can finish before its queued close notification reaches this thread.
        pending
            .waiting_for_pane
            .retain(|id, _| !screen.destroyed_panes.contains(id));
        // IDs can be reused after removal; discard stale focus before another tab is created.
        pending
            .tab_switches
            .retain(|(id, _)| screen.tabs.contains_key(id));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../unit/typable_text_tests.rs"]
mod typable_text_tests;

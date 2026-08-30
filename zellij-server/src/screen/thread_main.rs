use zellij_utils::errors::prelude::*;

use super::warp_ops::{PANE_WARP_SCREEN_RESERVE, TAB_WARP_SCREEN_RESERVE};
use super::{
    active_tab, active_tab_and_connected_client_id,
    active_tab_and_connected_client_id_with_first_tab_fallback, denormalize_notification_response,
    find_already_running_panes, get_default_editor, replay_pending_plugin_events, CopyOptions,
    Notification, Screen, ScreenInstruction,
};
use crate::background_jobs::BackgroundJob;
use crate::os_input_output::ResizeCache;
use crate::panes::terminal_pane::{BRACKETED_PASTE_BEGIN, BRACKETED_PASTE_END};
use crate::panes::PaneId;
use crate::plugins::PluginInstruction;
#[cfg(not(test))]
use crate::pty::get_default_shell;
use crate::pty::{ClientTabIndexOrPaneId, PtyInstruction};
use crate::session_layout_metadata::SessionLayoutMetadata;
use crate::tab::Tab;
use crate::thread_bus::Bus;
use crate::{ClientId, ServerInstruction};
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::path::PathBuf;
use std::time::{Duration, Instant};
use zellij_utils::consts::{session_info_folder_for_session, ZELLIJ_SOCK_DIR};
use zellij_utils::data::{
    AgentInfo, Direction, Event, GetFocusedPaneInfoResponse, HostTerminalThemeMode, InputMode,
    NewPanePlacement, NotificationInfo, OverlayPlacement, PaneScrollbackResponse,
    PluginCapabilities, SessionInfo,
};
use zellij_utils::envs::set_session_name;
use zellij_utils::errors::ContextType;
use zellij_utils::input::command::{RunCommand, TerminalAction};
use zellij_utils::input::config::Config;
use zellij_utils::input::get_mode_info;
use zellij_utils::input::layout::{Layout, Run};
use zellij_utils::input::options::{
    PaneFrameStyle, DEFAULT_NOTIFICATION_TIMEOUT_SECS, DEFAULT_WORD_SEPARATORS,
};
use zellij_utils::ipc::{ClientAttributes, ServerToClientMsg};
use zellij_utils::nested_session;
use zellij_utils::nested_session::NestedSessionMessage;
use zellij_utils::pane_size::Size;

fn apply_pane_agent_report(
    screen: &mut Screen,
    pane_id: PaneId,
    agent: Option<AgentInfo>,
) -> Result<bool> {
    // Read before the write: which hooks a change fires is a question about the edge, and the
    // pane only answers whether anything changed.
    let Some(previous) = screen
        .tab_containing_pane(pane_id)
        .map(|tab| tab.pane_agent_info(pane_id))
    else {
        return Ok(false);
    };
    let changed = screen
        .tab_containing_pane_mut(pane_id)
        .map(|tab| tab.set_pane_agent(pane_id, agent.clone()))
        .unwrap_or(false);
    // A report that repeats itself costs nothing; a changed one is news to every reader of the
    // manifest.
    if changed {
        screen.fire_agent_state_hooks(pane_id, previous.as_ref(), agent.as_ref());
        // Rebuild only the pane state. `SessionInfo` embeds that manifest and is synchronized by
        // `report_pane_state`, but the tab state did not change.
        screen.report_pane_state()?;
    }
    Ok(true)
}

// The box is here in order to make the
// NewClient enum smaller
#[allow(clippy::boxed_local)]
pub(crate) fn screen_thread_main(
    bus: Bus<ScreenInstruction>,
    max_panes: Option<usize>,
    client_attributes: ClientAttributes,
    config: Config,
    debug: bool,
    default_layout: Box<Layout>,
) -> Result<()> {
    // Resolve `theme_dark` / `theme_light` to concrete `Styling` from the
    // bundled themes BEFORE `config.options` is moved out below. These
    // populate Screen's auto-switch state at startup; runtime updates
    // continue to flow through `propagate_configuration_changes`.
    let host_theme_dark_styling = config
        .options
        .theme_dark
        .as_ref()
        .and_then(|name| config.themes.get_theme(name).map(|t| t.palette));
    let host_theme_light_styling = config
        .options
        .theme_light
        .as_ref()
        .and_then(|name| config.themes.get_theme(name).map(|t| t.palette));
    if config.options.theme_dark.is_some() && host_theme_dark_styling.is_none() {
        log::warn!(
            "theme_dark='{}' not found in themes; auto-theme switch disabled for dark.",
            config.options.theme_dark.as_deref().unwrap_or("?")
        );
    }
    if config.options.theme_light.is_some() && host_theme_light_styling.is_none() {
        log::warn!(
            "theme_light='{}' not found in themes; auto-theme switch disabled for light.",
            config.options.theme_light.as_deref().unwrap_or("?")
        );
    }

    let context_menu = config.context_menu.clone();
    let explicit_theme_hue = config.options.explicit_theme_hue;
    let config_options = config.options;
    let arrow_fonts = !config_options.simplified_ui.unwrap_or_default();
    let pane_frame_style = PaneFrameStyle::from_options(&config_options);
    let auto_layout = config_options.auto_layout.unwrap_or(true);
    let session_serialization = config_options.session_serialization.unwrap_or(true);
    let serialize_pane_viewport = config_options.serialize_pane_viewport.unwrap_or(false);
    let scrollback_lines_to_serialize = config_options.scrollback_lines_to_serialize;
    let session_is_mirrored = config_options.mirror_session.unwrap_or(false);
    let notification_position = config_options.notification_position.unwrap_or_default();
    let notification_timeout_secs = config_options
        .notification_timeout
        .unwrap_or(DEFAULT_NOTIFICATION_TIMEOUT_SECS);
    let layout_dir = config_options.layout_dir;
    #[cfg(test)]
    let default_shell = config_options
        .default_shell
        .clone()
        .unwrap_or(PathBuf::from("/bin/sh"));
    #[cfg(not(test))]
    let default_shell = config_options
        .default_shell
        .clone()
        .unwrap_or_else(|| get_default_shell());
    let default_shell_action = config_options.default_shell.clone().map(|shell| {
        TerminalAction::RunCommand(RunCommand {
            command: shell,
            cwd: config_options.default_cwd.clone(),
            use_terminal_title: true,
            ..Default::default()
        })
    });
    let default_editor = config_options
        .scrollback_editor
        .clone()
        .or_else(|| get_default_editor());
    let default_layout_name = config_options
        .default_layout
        .map(|l| format!("{}", l.display()));
    let copy_options = CopyOptions::new(
        config_options.copy_command,
        config_options.copy_clipboard.unwrap_or_default(),
        config_options.copy_on_select.unwrap_or(true),
    );
    let styled_underlines = config_options.styled_underlines.unwrap_or(true);
    let osc8_hyperlinks = config_options.osc8_hyperlinks.unwrap_or(true);
    let explicitly_disable_kitty_keyboard_protocol = config_options
        .support_kitty_keyboard_protocol
        .map(|e| !e) // this is due to the config options wording, if
        // "support_kitty_keyboard_protocol" is true,
        // explicitly_disable_kitty_keyboard_protocol is false and vice versa
        .unwrap_or(false); // by default, we try to support this if the terminal supports it and
                           // the program running inside a pane requests it
    let support_kitty_graphics_protocol = config_options
        .support_kitty_graphics_protocol
        .unwrap_or(true);
    let stacked_resize = config_options.stacked_resize.unwrap_or(true);
    let stacked_pane_list = config_options.stacked_pane_list.unwrap_or(true);
    let advanced_mouse_actions = config_options.advanced_mouse_actions.unwrap_or(true);
    let osc133_command_selection = config_options.osc133_command_selection.unwrap_or(true);
    let word_separators = config_options
        .word_separators
        .clone()
        .unwrap_or_else(|| DEFAULT_WORD_SEPARATORS.to_owned());
    let mouse_scroll_resize = config_options.mouse_scroll_resize.unwrap_or(true);
    let mouse_hover_effects = config_options.mouse_hover_effects.unwrap_or(true);
    let visual_bell = config_options.visual_bell.unwrap_or(true);
    let focus_follows_mouse = config_options.focus_follows_mouse.unwrap_or(false);
    let mouse_click_through = config_options.mouse_click_through.unwrap_or(false);
    let context_menu_on_right_click = config_options.context_menu_on_right_click.unwrap_or(false);
    let nested_session_handling = config_options.nested_session_handling.unwrap_or_default();
    let dangerously_enable_paste_buffer_read = config_options
        .dangerously_enable_paste_buffer_read
        .unwrap_or(false);
    let host_notification_protocol = config_options
        .host_notification_protocol
        .unwrap_or_default();

    let thread_senders = bus.senders.clone();
    let mut screen = Screen::new(
        bus,
        &client_attributes,
        max_panes,
        get_mode_info(
            config_options.default_mode.unwrap_or_default(),
            &client_attributes,
            PluginCapabilities {
                //  ¯\_(ツ)_/¯
                arrow_fonts: !arrow_fonts,
            },
            &config.keybinds,
            config_options.default_mode,
        ),
        pane_frame_style,
        auto_layout,
        session_is_mirrored,
        copy_options,
        debug,
        default_layout,
        default_layout_name,
        default_shell,
        default_shell_action,
        session_serialization,
        serialize_pane_viewport,
        scrollback_lines_to_serialize,
        styled_underlines,
        osc8_hyperlinks,
        arrow_fonts,
        layout_dir,
        explicitly_disable_kitty_keyboard_protocol,
        support_kitty_graphics_protocol,
        stacked_resize,
        stacked_pane_list,
        default_editor,
        advanced_mouse_actions,
        osc133_command_selection,
        word_separators,
        mouse_scroll_resize,
        mouse_hover_effects,
        visual_bell,
        focus_follows_mouse,
        mouse_click_through,
        context_menu_on_right_click,
        context_menu,
        nested_session_handling,
        notification_position,
        notification_timeout_secs,
    );
    screen.host_theme_dark_styling = host_theme_dark_styling;
    screen.host_theme_light_styling = host_theme_light_styling;
    screen.paste_buffer_read_enabled = dangerously_enable_paste_buffer_read;
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
    screen.set_host_notification_protocol(host_notification_protocol);

    let mut pending_tab_ids: HashSet<usize> = HashSet::new();
    let mut pending_tab_switches: HashSet<(usize, ClientId)> = HashSet::new(); // usize is the
                                                                               // tab_index
    let mut pending_events_waiting_for_tab: Vec<ScreenInstruction> = vec![];
    let mut pending_events_waiting_for_client: Vec<ScreenInstruction> = vec![];
    let mut pending_events_waiting_for_pane: HashMap<PaneId, Vec<ScreenInstruction>> =
        HashMap::new();
    let mut attach_traces_waiting_for_first_render: HashMap<ClientId, Instant> = HashMap::new();
    let mut keybind_intercepts = HashMap::new();
    loop {
        let (event, mut err_ctx) = screen
            .bus
            .recv()
            .context("failed to receive event on channel")?;
        err_ctx.add_call(ContextType::Screen((&event).into()));
        // here we start caching resizes, so that we'll send them in bulk at the end of each event
        // when this cache is Dropped, for more information, see the comments in PtyWriter
        let _resize_cache = ResizeCache::new(thread_senders.clone());

        match event {
            ScreenInstruction::PtyBytes(pid, vte_bytes) => {
                screen
                    .pane_output_activity
                    .insert(PaneId::Terminal(pid), Instant::now());
                let mut vte_bytes = Some(vte_bytes);
                {
                    let all_tabs = screen.get_tabs_mut();
                    for tab in all_tabs.values_mut() {
                        if tab.has_terminal_pid(pid) {
                            if let Some(bytes) = vte_bytes.take() {
                                tab.handle_pty_bytes(pid, bytes)
                                    .context("failed to process pty bytes")?;
                            }
                            break;
                        }
                    }
                }
                if let Some(vte_bytes) = vte_bytes {
                    pending_events_waiting_for_pane
                        .entry(PaneId::Terminal(pid))
                        .or_default()
                        .push(ScreenInstruction::PtyBytes(pid, vte_bytes));
                }
                let _ = screen
                    .bus
                    .senders
                    .send_to_background_jobs(BackgroundJob::RenderToClients);
            },
            ScreenInstruction::PluginBytes(plugin_render_assets) => {
                for plugin_render_asset in
                    screen.handle_plugin_render_assets(plugin_render_assets, true)?
                {
                    let plugin_id = plugin_render_asset.plugin_id;
                    pending_events_waiting_for_pane
                        .entry(PaneId::Plugin(plugin_id))
                        .or_default()
                        .push(ScreenInstruction::PluginBytes(vec![plugin_render_asset]));
                }
            },
            ScreenInstruction::Render => {
                screen.render(None)?;
            },
            ScreenInstruction::RenderToClients => {
                // render_blocker.can_render() returning true means that either all pending plugins
                // (only those waiting for a new tab layout to be applied!) have been rendered or
                // that a 100ms timeout has been reached (more info in the RenderBlocker comment)
                if screen.render_blocker.can_render() {
                    let tracing_attach = !attach_traces_waiting_for_first_render.is_empty();
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
                    screen.render(None)?;
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
                mut completion_tx,
                set_blocking,
            ) => {
                completion_tx.as_mut().map(|c| c.set_affected_pane_id(pid));

                let blocking_notification = if set_blocking { completion_tx } else { None };

                match client_or_tab_index {
                    ClientTabIndexOrPaneId::ClientId(client_id)
                    | ClientTabIndexOrPaneId::ClientIdNoFocus(client_id) => {
                        let should_focus_pane =
                            matches!(client_or_tab_index, ClientTabIndexOrPaneId::ClientId(_));
                        active_tab_and_connected_client_id_with_first_tab_fallback!(screen, client_id, |tab: &mut Tab, client_id: Option<ClientId>| {
                            tab.new_pane(pid,
                               initial_pane_title,
                               invoked_with,
                               start_suppressed,
                               should_focus_pane,
                               new_pane_placement,
                               client_id,
                               blocking_notification
                           )
                        }, ?);
                        if let Some(hold_for_command) = hold_for_command {
                            let is_first_run = true;
                            active_tab_and_connected_client_id_with_first_tab_fallback!(
                                screen,
                                client_id,
                                |tab: &mut Tab, _client_id: Option<ClientId>| tab.hold_pane(
                                    pid,
                                    None,
                                    is_first_run,
                                    hold_for_command
                                )
                            )
                        }
                        if should_focus_pane {
                            screen.sync_scroll_mode_on_focus(client_id)?;
                        }
                    },
                    ClientTabIndexOrPaneId::TabIndex(tab_index)
                    | ClientTabIndexOrPaneId::TabIndexNoFocus(tab_index) => {
                        let should_focus_pane =
                            matches!(client_or_tab_index, ClientTabIndexOrPaneId::TabIndex(_));
                        // Some placements (directional split, stacked without a
                        // target pane) need a client_id to know which pane to
                        // split relative to. Only resolve one when required.
                        let needs_client_id = matches!(
                            new_pane_placement,
                            NewPanePlacement::Tiled {
                                direction: Some(_),
                                ..
                            } | NewPanePlacement::Stacked {
                                pane_id_to_stack_under: None,
                                ..
                            }
                        );
                        let client_id = if needs_client_id {
                            screen
                                .active_tab_ids
                                .iter()
                                .find(|(_, tid)| **tid == tab_index)
                                .map(|(cid, _)| *cid)
                                .or_else(|| screen.active_tab_ids.keys().next().copied())
                        } else {
                            None
                        };
                        if let Some(active_tab) = screen.tabs.get_mut(&tab_index) {
                            active_tab.new_pane(
                                pid,
                                initial_pane_title,
                                invoked_with,
                                start_suppressed,
                                should_focus_pane,
                                new_pane_placement,
                                client_id,
                                blocking_notification,
                            )?;
                            if let Some(hold_for_command) = hold_for_command {
                                let is_first_run = true;
                                let _ =
                                    active_tab.hold_pane(pid, None, is_first_run, hold_for_command);
                            }
                        } else {
                            log::error!("Tab index not found: {:?}", tab_index);
                        }
                    },
                    ClientTabIndexOrPaneId::PaneId(pane_id) => {
                        let mut found = false;
                        let all_tabs = screen.get_tabs_mut();
                        let should_focus_pane = false;
                        for tab in all_tabs.values_mut() {
                            if tab.has_pane_with_pid(&pane_id) {
                                match new_pane_placement {
                                    NewPanePlacement::Tiled {
                                        direction: Some(direction),
                                        borderless,
                                    } => {
                                        if direction == Direction::Left
                                            || direction == Direction::Right
                                        {
                                            tab.vertical_split_of_pane_id(
                                                pid,
                                                initial_pane_title,
                                                invoked_with,
                                                pane_id,
                                                blocking_notification,
                                                borderless,
                                            )?;
                                        } else {
                                            tab.horizontal_split_of_pane_id(
                                                pid,
                                                initial_pane_title,
                                                invoked_with,
                                                pane_id,
                                                blocking_notification,
                                                borderless,
                                            )?;
                                        }
                                    },
                                    _ => {
                                        tab.new_pane(
                                            pid,
                                            initial_pane_title,
                                            invoked_with,
                                            start_suppressed,
                                            should_focus_pane,
                                            new_pane_placement,
                                            None,
                                            blocking_notification,
                                        )?;
                                    },
                                }
                                if let Some(hold_for_command) = hold_for_command {
                                    let is_first_run = true;
                                    let _ =
                                        tab.hold_pane(pid, None, is_first_run, hold_for_command);
                                }
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            log::error!(
                                "Failed to find tab containing pane with id: {:?}",
                                pane_id
                            );
                        }
                    },
                };
                // Before the replay: a pane that exited before the screen created it starts,
                // and then exits, in that order.
                screen.fire_agent_start_for_new_pane(pid);
                if let Some(pending_events) = pending_events_waiting_for_pane.remove(&pid) {
                    for event in pending_events {
                        screen.bus.senders.send_to_screen(event).non_fatal();
                    }
                }
                screen.log_and_report_session_state()?;

                screen.render(None)?;
            },
            ScreenInstruction::OpenInPlaceEditor(pid, client_tab_index_or_pane_id) => {
                match client_tab_index_or_pane_id {
                    ClientTabIndexOrPaneId::ClientId(client_id)
                    | ClientTabIndexOrPaneId::ClientIdNoFocus(client_id) => {
                        active_tab!(screen, client_id, |tab: &mut Tab| tab
                            .replace_active_pane_with_editor_pane(pid, client_id), ?);
                        screen.log_and_report_session_state()?;
                    },
                    ClientTabIndexOrPaneId::TabIndex(_tab_index)
                    | ClientTabIndexOrPaneId::TabIndexNoFocus(_tab_index) => {
                        log::error!("Cannot OpenInPlaceEditor with a TabIndex");
                    },
                    ClientTabIndexOrPaneId::PaneId(pane_id_to_replace) => {
                        match screen.tab_containing_pane_mut(pane_id_to_replace) {
                            Some(tab) => tab
                                .replace_pane_with_editor_pane(pid, pane_id_to_replace)
                                .non_fatal(),
                            None => log::error!(
                                "Could not find pane with id {:?} to replace",
                                pane_id_to_replace
                            ),
                        }
                    },
                }

                if let Some(pending_events) = pending_events_waiting_for_pane.remove(&pid) {
                    for event in pending_events {
                        screen.bus.senders.send_to_screen(event).non_fatal();
                    }
                }

                screen.render(None)?;
            },
            ScreenInstruction::TogglePaneEmbedOrFloating(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| tab
                    .toggle_pane_embed_or_floating(client_id), ?);
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::ToggleFloatingPanes(client_id, default_shell, completion_tx) => {
                active_tab_and_connected_client_id!(screen, client_id, |tab: &mut Tab, client_id: ClientId| tab
                    .toggle_floating_panes(Some(client_id), default_shell, completion_tx), ?);
                screen.clear_bell_for_focused_pane(client_id);
                screen.sync_scroll_mode_on_focus(client_id)?;
                screen.log_and_report_session_state()?;

                screen.render(None)?;
            },
            ScreenInstruction::ShowFloatingPanes {
                client_id,
                tab_id,
                completion,
            } => {
                screen.show_floating_panes_in_tab(client_id, tab_id, completion)?;
                screen.sync_scroll_mode_on_focus(client_id)?;
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::HideFloatingPanes {
                client_id,
                tab_id,
                completion,
            } => {
                screen.hide_floating_panes_in_tab(client_id, tab_id, completion)?;
                screen.sync_scroll_mode_on_focus(client_id)?;
                screen.log_and_report_session_state()?;
                screen.render(None)?;
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
                client_id,
                completion,
            } => {
                let handled = screen.context_menu_handles_key(&key, client_id);
                if let Some(mut completion) = completion {
                    completion.set_stdout_message(handled.to_string());
                }
            },
            ScreenInstruction::WriteCharacter(
                key_with_modifier,
                raw_bytes,
                is_kitty_keyboard_protocol,
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                if let Some(plugin_id) = keybind_intercepts.get(&client_id) {
                    if let Some(key_with_modifier) = key_with_modifier {
                        let _ = screen
                            .bus
                            .senders
                            .send_to_plugin(PluginInstruction::Update(vec![(
                                Some(*plugin_id),
                                Some(client_id),
                                Event::InterceptedKeyPress(key_with_modifier),
                            )]));
                        continue;
                    }
                }
                // An open context menu takes the keys it uses - arrows, Enter, Esc - before any
                // pane sees them. Anything else falls through and reaches the pane as usual; the
                // focus or layout change it causes then closes the menu, which is what pressing an
                // unrelated key while a menu is up means anyway.
                if screen.context_menu_handles_key(&key_with_modifier, client_id) {
                    continue;
                }
                // A key the menu did not want is about to do something else - reach the pane, or
                // fire whatever the current mode binds it to. Either way the menu is answering a
                // question about a moment that has passed, so it gets out of the way. This is the
                // same discipline `defocus_dismissible_panes` applies to dismissible overlays.
                if screen.close_context_menu(client_id) {
                    screen.render(None)?;
                }
                let mut state_changed = false;
                let client_input_mode = screen.get_client_input_mode(client_id);
                match client_input_mode {
                    Some(InputMode::RenameTab) => {
                        if !(raw_bytes == BRACKETED_PASTE_BEGIN || raw_bytes == BRACKETED_PASTE_END)
                        {
                            screen.update_active_tab_name(raw_bytes, client_id)?;
                            state_changed = true;
                        }
                    },
                    _ => {
                        active_tab_and_connected_client_id!(
                            screen,
                            client_id,
                            |tab: &mut Tab, client_id: ClientId| {
                                match client_input_mode {
                                    Some(InputMode::EnterSearch) => {
                                        if !(raw_bytes == BRACKETED_PASTE_BEGIN
                                            || raw_bytes == BRACKETED_PASTE_END)
                                        {
                                            if let Err(e) =
                                                tab.update_search_term(raw_bytes, client_id)
                                            {
                                                log::error!("{}", e);
                                            }
                                        }
                                        state_changed = true;
                                    },
                                    Some(InputMode::RenamePane) => {
                                        if !(raw_bytes == BRACKETED_PASTE_BEGIN
                                            || raw_bytes == BRACKETED_PASTE_END)
                                        {
                                            if let Err(e) =
                                                tab.update_active_pane_name(raw_bytes, client_id)
                                            {
                                                log::error!("{}", e);
                                            }
                                            state_changed = true;
                                        }
                                    },
                                    _ => {
                                        let write_result = match tab.is_sync_panes_active() {
                                            true => tab.write_to_terminals_on_current_tab(
                                                &key_with_modifier,
                                                raw_bytes,
                                                is_kitty_keyboard_protocol,
                                                client_id,
                                            ),
                                            false => tab.write_to_active_terminal(
                                                &key_with_modifier,
                                                raw_bytes,
                                                is_kitty_keyboard_protocol,
                                                client_id,
                                            ),
                                        };
                                        if let Ok(true) = write_result {
                                            state_changed = true;
                                        }
                                    },
                                }
                            }
                        );
                    },
                };
                if state_changed {
                    screen.log_and_report_session_state()?;
                }
                screen.render(None)?;
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
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::SwitchFocus(client_id, _completion_tx) => {
                let old_pane_id = screen.get_active_pane_id(&client_id);
                screen.cycle_focus(client_id, true);
                let new_pane_id = screen.get_active_pane_id(&client_id);
                if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                    screen.report_key_passthrough_state(client_id, old, new);
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::FocusNextPane(client_id, mut _completion_tx) => {
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
                    screen.render(None)?;
                }
            },
            ScreenInstruction::FocusPreviousPane(client_id, mut _completion_tx) => {
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
                    screen.render(None)?;
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
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MoveFocusLeft(client_id, mut _completion_tx) => {
                if screen.get_first_client_id().is_none() {
                    log::error!("No connected clients to move focus for");
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message("No connected clients to move focus for".to_string());
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
                        c.set_error_message("No connected clients to move focus for".to_string());
                    }
                } else {
                    let old_pane_id = screen.get_active_pane_id(&client_id);
                    screen.move_focus_left_or_previous_tab(client_id)?;
                    let new_pane_id = screen.get_active_pane_id(&client_id);
                    if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                        if old != new {
                            screen.report_key_passthrough_state_with_direction(
                                client_id,
                                old,
                                new,
                                Some(Direction::Left),
                            );
                        }
                    }
                    screen.clear_bell_for_focused_pane(client_id);
                    screen.add_active_pane_to_group_if_marking(&client_id);
                    screen.sync_scroll_mode_on_focus(client_id)?;
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                }
            },
            ScreenInstruction::MoveFocusDown(client_id, mut _completion_tx) => {
                if screen.get_first_client_id().is_none() {
                    log::error!("No connected clients to move focus for");
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message("No connected clients to move focus for".to_string());
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
                        c.set_error_message("No connected clients to move focus for".to_string());
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
                        c.set_error_message("No connected clients to move focus for".to_string());
                    }
                } else {
                    let old_pane_id = screen.get_active_pane_id(&client_id);
                    screen.move_focus_right_or_next_tab(client_id)?;
                    let new_pane_id = screen.get_active_pane_id(&client_id);
                    if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                        if old != new {
                            screen.report_key_passthrough_state_with_direction(
                                client_id,
                                old,
                                new,
                                Some(Direction::Right),
                            );
                        }
                    }
                    screen.clear_bell_for_focused_pane(client_id);
                    screen.add_active_pane_to_group_if_marking(&client_id);
                    screen.sync_scroll_mode_on_focus(client_id)?;
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                }
            },
            ScreenInstruction::MoveFocusUp(client_id, mut _completion_tx) => {
                if screen.get_first_client_id().is_none() {
                    log::error!("No connected clients to move focus for");
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message("No connected clients to move focus for".to_string());
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
                screen.render(None)?;
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
                match file {
                    Some(file_path) => {
                        // Write dump to file (existing behavior)
                        match pane_id {
                            Some(pane_id) => {
                                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                                    if ansi {
                                        tab.dump_with_ansi_terminal_screen(
                                            Some(file_path.clone()),
                                            pane_id,
                                            full,
                                        )?;
                                    } else {
                                        tab.dump_terminal_screen(
                                            Some(file_path.clone()),
                                            pane_id,
                                            full,
                                        )?;
                                    }
                                }
                            },
                            None => {
                                if ansi {
                                    active_tab_and_connected_client_id!(
                                        screen,
                                        client_id,
                                        |tab: &mut Tab, client_id: ClientId| tab.dump_with_ansi_active_terminal_screen(
                                            Some(file_path.to_string()),
                                            client_id,
                                            full
                                        ),
                                        ?
                                    );
                                } else {
                                    active_tab_and_connected_client_id!(
                                        screen,
                                        client_id,
                                        |tab: &mut Tab, client_id: ClientId| tab.dump_active_terminal_screen(
                                            Some(file_path.to_string()),
                                            client_id,
                                            full
                                        ),
                                        ?
                                    );
                                }
                            },
                        }
                        screen.render(None)?;
                        drop(completion_tx);
                    },
                    None => {
                        // Dump to STDOUT via Log
                        let dump = match pane_id {
                            Some(pane_id) => {
                                let mut result = String::new();
                                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                                    let dump = if ansi {
                                        tab.get_dump_with_ansi_terminal_screen(pane_id, full)
                                    } else {
                                        tab.get_dump_terminal_screen(pane_id, full)
                                    };
                                    if let Some(dump) = dump {
                                        result = dump;
                                    }
                                }
                                result
                            },
                            None => {
                                let mut result = String::new();
                                if ansi {
                                    active_tab_and_connected_client_id!(
                                        screen,
                                        client_id,
                                        |tab: &mut Tab, client_id: ClientId| {
                                            result = tab.get_dump_with_ansi_active_terminal_screen(client_id, full);
                                            Ok::<(), anyhow::Error>(())
                                        },
                                        ?
                                    );
                                } else {
                                    active_tab_and_connected_client_id!(
                                        screen,
                                        client_id,
                                        |tab: &mut Tab, client_id: ClientId| {
                                            result = tab.get_dump_active_terminal_screen(client_id, full);
                                            Ok::<(), anyhow::Error>(())
                                        },
                                        ?
                                    );
                                }
                                result
                            },
                        };
                        screen.bus.senders.send_to_server(ServerInstruction::Log(
                            vec![dump],
                            cli_client_id.unwrap_or(client_id),
                            completion_tx,
                        ))?;
                    },
                }
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
                let _ = response_channel.send(screen.notification_history.clone());
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
            } => {
                let err_context = || "Failed to dump layout".to_string();
                let session_layout_metadata =
                    screen.get_layout_metadata(Some(screen.default_shell.clone()), tab_index);
                screen
                    .bus
                    .senders
                    .send_to_pty(PtyInstruction::DumpLayoutToPlugin {
                        session_layout_metadata,
                        plugin_id,
                        response_channel,
                    })
                    .with_context(err_context)
                    .non_fatal();
            },
            ScreenInstruction::GetFocusedPaneInfo {
                client_id,
                response_channel,
            } => {
                let response = match screen.active_tab_ids.get(&client_id) {
                    Some(&focused_tab_index) => match screen.get_active_pane_id(&client_id) {
                        Some(focused_pane_id) => GetFocusedPaneInfoResponse::Ok {
                            tab_index: focused_tab_index,
                            pane_id: focused_pane_id.into(),
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
                let tab_info = screen.get_tab_info(tab_id);
                let _ = response_channel.send(tab_info);
            },
            ScreenInstruction::ListClientsToPlugin(plugin_id, client_id) => {
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
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::GetPaneScrollback {
                pane_id,
                client_id,
                get_full_scrollback,
                response_channel,
            } => {
                let last_output_ms_ago = screen
                    .pane_output_activity
                    .get(&pane_id)
                    .map(|last| Instant::now().saturating_duration_since(*last).as_millis() as u64);
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
                    |tab: &mut Tab, client_id: ClientId| tab.scroll_active_terminal_up(client_id)
                );
                screen.sync_scroll_mode_if_scroll_changed(client_id, was_scrolled)?;
                screen.render(None)?;
            },
            ScreenInstruction::MovePane(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.move_active_pane(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MovePaneBackwards(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_backwards(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MovePaneDown(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_down(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MovePaneUp(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_up(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MovePaneRight(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_right(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MovePaneLeft(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.move_active_pane_left(client_id)
                );
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
            },
            ScreenInstruction::ScrollToNextPrompt(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .scroll_active_terminal_to_next_prompt(client_id)
                );
                screen.render(None)?;
            },
            ScreenInstruction::SelectCommandAtScrollPosition(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .select_command_at_scroll_position(client_id)
                );
                screen.render(None)?;
            },
            ScreenInstruction::CopyLastCommandOutput(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.copy_last_command_output(client_id),
                    ?
                );
                screen.render(None)?;
            },
            ScreenInstruction::ClearCommandOutputFlash(pane_id) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.clear_command_output_flash(pane_id);
                }
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
            },
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
                    screen.render(None)?;
                }
            },
            ScreenInstruction::CloseFocusedPane(client_id, completion_tx) => {
                let old_pane_id = screen.get_active_pane_id(&client_id);
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.close_focused_pane(client_id, completion_tx), ?
                );
                let new_pane_id = screen.get_active_pane_id(&client_id);
                if let (Some(old), Some(new)) = (old_pane_id, new_pane_id) {
                    screen.report_key_passthrough_state(client_id, old, new);
                }
                screen.sync_scroll_mode_on_focus(client_id)?;
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::SetSelectable(pid, selectable) => {
                match screen.tab_containing_pane_mut(pid) {
                    Some(tab) => tab.set_pane_selectable(pid, selectable),
                    // the pane's tab may simply not exist yet - retry once one does
                    None => pending_events_waiting_for_tab
                        .push(ScreenInstruction::SetSelectable(pid, selectable)),
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::ShowPluginCursor(pid, client_id, cursor_position) => {
                if !screen.show_plugin_cursor(pid, client_id, cursor_position) {
                    pending_events_waiting_for_pane
                        .entry(PaneId::Plugin(pid))
                        .or_default()
                        .push(ScreenInstruction::ShowPluginCursor(
                            pid,
                            client_id,
                            cursor_position,
                        ));
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::SetMouseSelectionSupport(pid, selection_support) => {
                if screen
                    .with_pane(pid, |pane| {
                        pane.set_mouse_selection_support(selection_support)
                    })
                    .is_none()
                {
                    pending_events_waiting_for_tab.push(
                        ScreenInstruction::SetMouseSelectionSupport(pid, selection_support),
                    );
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::ClosePane(
                id,
                client_id,
                completion_tx, // the action ends here, dropping this will release anything
                // waiting for it
                exit_status,
            ) => {
                // A command pane whose command exited without `hold_on_close` arrives here from
                // the pty with no client and no completion; every other close - a user's, a
                // plugin's `close_self` - carries one or the other. That is the `pane_exit`
                // hook's moment for a pane that is not held.
                if client_id.is_none() && completion_tx.is_none() {
                    if let Some(run_command) = screen
                        .tab_containing_pane(id)
                        .and_then(|tab| tab.pane_run_command(id))
                    {
                        screen.fire_pane_exit_hook(id, exit_status, &run_command, false);
                    }
                }
                // A menu opened over this pane is offering to act on something that is about to
                // stop existing.
                screen.close_context_menus_for_pane(id);
                screen.repaint_tab_of_closing_overlay(id);
                match client_id {
                    Some(client_id) => {
                        active_tab!(screen, client_id, |tab: &mut Tab| tab.close_pane(
                            id,
                            false,
                            exit_status
                        ));
                    },
                    None => {
                        let mut found = false;
                        for tab in screen.tabs.values_mut() {
                            if tab.get_all_pane_ids().contains(&id) {
                                tab.close_pane(id, false, exit_status);
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            pending_events_waiting_for_pane
                                .entry(id)
                                .or_default()
                                .push(ScreenInstruction::ClosePane(id, None, None, exit_status));
                        }
                    },
                }

                screen.clear_nested_guest(id);
                screen.pane_output_activity.remove(&id);

                // Clean up PTY-side resources (async reader task, child PID mapping,
                // terminal_id_to_raw_fd entry). This is needed because the natural
                // child exit path (quit_cb) only sends ScreenInstruction::ClosePane
                // and never sends PtyInstruction::ClosePane. The handler in Pty is
                // idempotent, so this is safe even if ClosePane was already sent.
                let _ = screen
                    .bus
                    .senders
                    .send_to_pty(PtyInstruction::ClosePane(id, None));

                let connected_client_ids: Vec<ClientId> =
                    screen.active_tab_ids.keys().copied().collect();
                for connected_client_id in connected_client_ids {
                    screen.sync_scroll_mode_on_focus(connected_client_id)?;
                }
                screen.log_and_report_session_state().non_fatal();
                screen.retain_only_existing_panes_in_pane_groups();
            },
            ScreenInstruction::HoldPane(id, exit_status, run_command) => {
                let is_first_run = false;
                let mut found = false;
                let mut pane_is_agent = false;
                let mut queued_respawn = None;
                {
                    for tab in screen.tabs.values_mut() {
                        if tab.get_all_pane_ids().contains(&id) {
                            queued_respawn =
                                tab.hold_pane(id, exit_status, is_first_run, run_command.clone());
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
                    if let Some((terminal_pane_id, run_command, keep_scrollback)) = queued_respawn {
                        screen.run_command_in_pane_with_id(
                            terminal_pane_id,
                            run_command,
                            keep_scrollback,
                        );
                    }
                }
                if !found {
                    pending_events_waiting_for_pane
                        .entry(id)
                        .or_default()
                        .push(ScreenInstruction::HoldPane(id, exit_status, run_command));
                }
                screen.log_and_report_session_state()?;
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
                screen.render(None)?;
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
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UpdatePaneHost(terminal_id, host) => {
                // Usually arrives once when a pane is created and once more when it is
                // closed - but a workflow step can respawn a pane's command on a different
                // remote host, so this can also arrive again mid-life with a changed (or
                // newly-absent) host.
                for tab in screen.tabs.values_mut() {
                    if let Some(pane) = tab.get_pane_with_id_mut(PaneId::Terminal(terminal_id)) {
                        pane.set_remote_host(host.clone());
                        break;
                    }
                }
                match host {
                    Some(host) => screen.terminal_hosts.insert(terminal_id, host),
                    None => screen.terminal_hosts.remove(&terminal_id),
                };
                screen.log_and_report_session_state()?;
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
            ScreenInstruction::SetPaneTitlePrefix(pane_id, prefix) => {
                // No `log_and_report_session_state`: the prefix is a decoration on
                // the frame and never enters `current_title`, so nothing a plugin
                // can see has changed — and the plugin that sets this is reading
                // that very manifest to decide what to set.
                if screen
                    .with_pane(pane_id, |pane| pane.set_title_prefix(prefix))
                    .is_some()
                {
                    screen.render(None)?;
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
                screen.render(None)?;
            },
            ScreenInstruction::ToggleActiveTerminalFullscreen(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .toggle_active_pane_fullscreen(client_id)
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::ToggleActiveTerminalNoUiFullscreen(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab
                        .toggle_active_pane_no_ui_fullscreen(client_id)
                );
                screen.render(None)?;
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
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::SetPaneFrameStyle(pane_frame_style, _completion_tx) => {
                screen.pane_frame_style = pane_frame_style;
                for tab in screen.tabs.values_mut() {
                    tab.set_pane_frames(screen.pane_frame_style);
                    tab.update_input_modes()?;
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::SwitchTabNext(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.switch_tab_next(None, true, client_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::SwitchTabPrev(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.switch_tab_prev(None, true, client_id)?;
                screen.render(None)?;
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
                screen.render(None)?;
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
                let tab_index = screen.get_new_tab_id();
                pending_tab_ids.insert(tab_index);
                let client_id_for_new_tab = if should_change_focus_to_new_tab {
                    Some(client_id)
                } else {
                    None
                };
                let resolved_swap_layouts = (
                    swap_tiled_layouts
                        .unwrap_or_else(|| screen.default_layout.swap_tiled_layouts.clone()),
                    swap_floating_layouts
                        .unwrap_or_else(|| screen.default_layout.swap_floating_layouts.clone()),
                );
                screen.new_tab(
                    tab_index,
                    resolved_swap_layouts,
                    tab_name.clone(),
                    client_id_for_new_tab,
                )?;
                screen
                    .bus
                    .senders
                    .send_to_plugin(PluginInstruction::NewTab(
                        cwd,
                        default_shell,
                        layout,
                        floating_panes_layout,
                        tab_index,
                        initial_panes,
                        block_on_first_terminal,
                        should_change_focus_to_new_tab,
                        client_id,
                        completion_tx,
                    ))?;
            },
            ScreenInstruction::ApplyLayout(
                layout,
                floating_panes_layout,
                new_pane_pids,
                new_floating_pane_pids,
                adopted_agents,
                new_plugin_ids,
                tab_id,
                should_change_focus_to_new_tab,
                client_id,
                mut completion_tx,
                blocking_terminal,
            ) => {
                log::info!(
                    "ScreenInstruction::ApplyLayout: applying layout for tab {}",
                    tab_id
                );
                // tab_id is a stable identifier from NewTab instruction
                if let Some(first_terminal_pane) = new_pane_pids.first() {
                    completion_tx
                        .as_mut()
                        .map(|c| c.set_affected_pane_id(PaneId::Terminal(first_terminal_pane.0)));
                } else if let Some(plugin_id) =
                    new_plugin_ids.values().next().and_then(|v| v.first())
                {
                    completion_tx
                        .as_mut()
                        .map(|c| c.set_affected_pane_id(PaneId::Plugin(*plugin_id)));
                }
                // Set the affected tab ID for plugin API return value
                completion_tx
                    .as_mut()
                    .map(|c| c.set_affected_tab_id(tab_id));
                let new_terminal_pane_ids: Vec<_> = new_pane_pids
                    .iter()
                    .chain(&new_floating_pane_pids)
                    .map(|(terminal_id, _)| PaneId::Terminal(*terminal_id))
                    .collect();
                screen.apply_layout(
                    layout,
                    floating_panes_layout,
                    new_pane_pids.clone(),
                    new_floating_pane_pids,
                    new_plugin_ids.clone(),
                    tab_id,
                    should_change_focus_to_new_tab,
                    client_id,
                    blocking_terminal,
                )?;
                for (terminal_id, agent) in adopted_agents {
                    let pane_id = PaneId::Terminal(terminal_id);
                    if !apply_pane_agent_report(&mut screen, pane_id, Some(agent))? {
                        Err::<(), _>(anyhow!(
                            "adopted agent record refers to missing terminal pane {terminal_id}"
                        ))
                        .non_fatal();
                    }
                }
                pending_tab_ids.remove(&tab_id);
                if pending_tab_ids.is_empty() {
                    for (tab_index, client_id) in pending_tab_switches.drain() {
                        screen.go_to_tab(tab_index + 1, client_id)?;
                    }
                    if should_change_focus_to_new_tab {
                        // Convert ID → position for go_to_tab (which expects 1-based position)
                        if let Some(tab_position) = screen.get_tab_position_by_id(tab_id) {
                            screen.go_to_tab(tab_position + 1, client_id)?;
                        }
                    }
                } else if should_change_focus_to_new_tab {
                    let client_id_to_switch = if screen.active_tab_ids.contains_key(&client_id) {
                        Some(client_id)
                    } else {
                        screen.active_tab_ids.keys().next().copied()
                    };
                    if let Some(client_id_to_switch) = client_id_to_switch {
                        // Convert ID → position for pending_tab_switches (which stores positions)
                        if let Some(tab_position) = screen.get_tab_position_by_id(tab_id) {
                            pending_tab_switches.insert((tab_position, client_id_to_switch));
                        }
                    }
                }

                for pane_id in new_terminal_pane_ids {
                    if let Some(pending_events) = pending_events_waiting_for_pane.remove(&pane_id) {
                        for event in pending_events {
                            screen.bus.senders.send_to_screen(event).non_fatal();
                        }
                    }
                }

                for plugin_ids in new_plugin_ids.values() {
                    for plugin_id in plugin_ids {
                        let pane_id = PaneId::Plugin(*plugin_id);
                        screen.render_blocker.register_blocking_plugin(*plugin_id);
                        let plugin_exists =
                            screen.tabs.values().any(|tab| tab.has_plugin(*plugin_id));
                        if plugin_exists {
                            if let Some(pending_events) =
                                pending_events_waiting_for_pane.remove(&pane_id)
                            {
                                replay_pending_plugin_events(&mut screen, pending_events)?;
                            }
                        }
                    }
                }

                for event in pending_events_waiting_for_client.drain(..) {
                    screen.bus.senders.send_to_screen(event).non_fatal();
                }

                for event in pending_events_waiting_for_tab.drain(..) {
                    screen.bus.senders.send_to_screen(event).non_fatal();
                }

                screen.render(None)?;
            },
            ScreenInstruction::GoToTab(
                tab_index,
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                let client_id_to_switch = if client_id.is_none() {
                    None
                } else if screen
                    .active_tab_ids
                    .contains_key(&client_id.expect("This is checked above"))
                {
                    client_id
                } else {
                    screen.active_tab_ids.keys().next().copied()
                };
                match client_id_to_switch {
                    // we must make sure pending_tab_ids is empty because otherwise we cannot be
                    // sure this instruction is applied at the right time (eg. we might have a
                    // pending tab that will become not-pending after this instruction and change
                    // the client focus, which should have happened before this instruction and not
                    // after)
                    Some(client_id) if pending_tab_ids.is_empty() => {
                        screen.go_to_tab(tab_index as usize, client_id)?;
                        screen.render(None)?;
                    },
                    _ => {
                        if let Some(client_id) = client_id {
                            pending_tab_switches.insert((tab_index as usize, client_id));
                        }
                    },
                }
            },
            ScreenInstruction::GoToTabName(
                tab_name,
                default_shell,
                create,
                client_id,
                mut completion_tx,
            ) => {
                let swap_layouts = (
                    screen.default_layout.swap_tiled_layouts.clone(),
                    screen.default_layout.swap_floating_layouts.clone(),
                );
                let client_id = if client_id.is_none() {
                    None
                } else if screen
                    .active_tab_ids
                    .contains_key(&client_id.expect("This is checked above"))
                {
                    client_id
                } else {
                    screen.active_tab_ids.keys().next().copied()
                };
                if let Some(client_id) = client_id {
                    if let Ok(tab_exists) = screen.go_to_tab_name(tab_name.clone(), client_id) {
                        screen.render(None)?;
                        if tab_exists {
                            // Tab already exists - find its ID and set in completion
                            if let Some(existing_tab) =
                                screen.tabs.values().find(|t| t.name == tab_name)
                            {
                                completion_tx
                                    .as_mut()
                                    .map(|c| c.set_affected_tab_id(existing_tab.id));
                            }
                        }
                        if create && !tab_exists {
                            let tab_index = screen.get_new_tab_id();
                            let should_change_focus_to_new_tab = true;
                            screen.new_tab(
                                tab_index,
                                swap_layouts,
                                Some(tab_name),
                                Some(client_id),
                            )?;
                            screen
                                .bus
                                .senders
                                .send_to_plugin(PluginInstruction::NewTab(
                                    None,
                                    default_shell,
                                    None,
                                    vec![],
                                    tab_index,
                                    None,  // initial_panes
                                    false, // block_on_first_terminal
                                    should_change_focus_to_new_tab,
                                    client_id,
                                    completion_tx,
                                ))?;
                            continue; // so we don't get to the completion signalling below
                        }
                    }
                }
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
                    screen.render(None)?;
                }
            },
            ScreenInstruction::UndoRenameTab(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.undo_active_rename_tab(client_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::MoveTabLeft(client_id, completion_tx) => {
                if pending_tab_ids.is_empty() {
                    screen.move_active_tab_to_left(client_id)?;
                    screen.render(None)?;
                } else {
                    // Defer execution, forward completion_tx
                    pending_events_waiting_for_tab
                        .push(ScreenInstruction::MoveTabLeft(client_id, completion_tx));
                }
            },
            ScreenInstruction::MoveTabRight(client_id, completion_tx) => {
                if pending_tab_ids.is_empty() {
                    screen.move_active_tab_to_right(client_id)?;
                    screen.render(None)?;
                } else {
                    // Defer execution, forward completion_tx
                    pending_events_waiting_for_tab
                        .push(ScreenInstruction::MoveTabRight(client_id, completion_tx));
                }
            },
            ScreenInstruction::RecomputeTabSize(client_id, new_size) => {
                let previous_surface = screen.surface_size();
                screen.set_client_size(client_id, new_size);
                // the server sends this while setting the session up, before any tab exists
                if screen.tabs.is_empty() {
                    continue;
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
                    screen.render(None)?;
                }
            },
            ScreenInstruction::TerminalPixelDimensions(client_id, pixel_dimensions) => {
                let character_cell_size_changed =
                    screen.update_pixel_dimensions(client_id, pixel_dimensions);
                if character_cell_size_changed {
                    screen.resize_pty_all_panes()?;
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
            },
            ScreenInstruction::SetSixelSupport {
                client_id,
                supported,
            } => {
                screen.update_sixel_support(client_id, supported);
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
                screen.render(None)?;
            },
            ScreenInstruction::ResumePaneAfterForward {
                pane_id,
                reply_bytes,
            } => {
                screen.resume_pane_after_forward(pane_id, reply_bytes)?;
                // Same rationale as ForwardedReplyFromHost above — the
                // resume's replay paints the grid, so we must schedule
                // a render before yielding back to the screen loop.
                screen.render(None)?;
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
                screen.render(None)?;
            },
            ScreenInstruction::ChangeModeForAllClients(
                input_mode,
                base_mode,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.change_mode_for_all_clients(input_mode, base_mode)?;
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.copy_selection(client_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::Exit => {
                break;
            },
            ScreenInstruction::ToggleTab(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                screen.toggle_tab(client_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::AddClient(
                client_id,
                client_size,
                tab_position_to_focus,
                pane_id_to_focus,
            ) => {
                let attach_started_at = Instant::now();
                attach_traces_waiting_for_first_render.insert(client_id, attach_started_at);
                log::info!(
                    "attach trace: screen client_id={client_id} AddClient begin size={client_size:?}"
                );
                screen.set_client_size(client_id, client_size);
                log::info!(
                    "attach trace: screen client_id={client_id} client size recorded after {:?}; Screen::add_client begin",
                    attach_started_at.elapsed()
                );
                // the new client can shrink the shared surface, so every tab is reflowed - but
                // only the reflow: this handler reports and renders on its own way out.
                // Reject a client whose size leaves nothing to lay out: keeping its size in the
                // shared minimum would make every later render fail for all clients.
                let attach_result = screen.add_client(client_id);
                log::info!(
                    "attach trace: screen client_id={client_id} Screen::add_client finished after {:?} ({})",
                    attach_started_at.elapsed(),
                    if attach_result.is_ok() { "ok" } else { "error" }
                );
                let attach_result = attach_result.and_then(|_| {
                    log::info!(
                        "attach trace: screen client_id={client_id} reflow_all_tabs begin"
                    );
                    let result = screen.reflow_all_tabs();
                    log::info!(
                        "attach trace: screen client_id={client_id} reflow_all_tabs finished after {:?} ({})",
                        attach_started_at.elapsed(),
                        if result.is_ok() { "ok" } else { "error" }
                    );
                    result
                });
                if let Err(error) = attach_result {
                    Err::<(), _>(error)
                        .context("failed to attach client at its reported size")
                        .non_fatal();
                    screen.remove_client(client_id).non_fatal();
                    screen.resize_to_clients().non_fatal();
                    screen
                        .bus
                        .senders
                        .send_to_server(ServerInstruction::ClientExit(client_id, None))
                        .non_fatal();
                    attach_traces_waiting_for_first_render.remove(&client_id);
                    continue;
                }
                let pane_id = pane_id_to_focus.map(|(pane_id, is_plugin)| {
                    if is_plugin {
                        PaneId::Plugin(pane_id)
                    } else {
                        PaneId::Terminal(pane_id)
                    }
                });
                if let Some(pane_id) = pane_id {
                    log::info!(
                        "attach trace: screen client_id={client_id} restoring pane focus {pane_id:?}"
                    );
                    screen.focus_pane_with_id(pane_id, true, false, client_id)?;
                } else if let Some(tab_position_to_focus) = tab_position_to_focus {
                    log::info!(
                        "attach trace: screen client_id={client_id} restoring tab focus position={tab_position_to_focus}"
                    );
                    screen.go_to_tab(tab_position_to_focus, client_id)?;
                }
                log::info!(
                    "attach trace: screen client_id={client_id} focus restoration complete after {:?}",
                    attach_started_at.elapsed()
                );
                screen.show_guest_modals_for_new_client(client_id);
                let focused_pane_id = screen.get_active_pane_id(&client_id);
                if let Some(focused) = focused_pane_id {
                    screen.report_key_passthrough_state(client_id, focused, focused);
                }
                log::info!(
                    "attach trace: screen client_id={client_id} sync_scroll_mode_on_focus begin"
                );
                screen.sync_scroll_mode_on_focus(client_id)?;
                log::info!(
                    "attach trace: screen client_id={client_id} pending events drain begin after {:?}",
                    attach_started_at.elapsed()
                );
                for event in pending_events_waiting_for_client.drain(..) {
                    screen.bus.senders.send_to_screen(event).non_fatal();
                }
                log::info!(
                    "attach trace: screen client_id={client_id} log_and_report_session_state begin"
                );
                screen.log_and_report_session_state()?;

                log::info!(
                    "attach trace: screen client_id={client_id} scheduling first render after {:?}",
                    attach_started_at.elapsed()
                );
                screen.render(None)?;
                log::info!(
                    "attach trace: screen client_id={client_id} AddClient handler complete after {:?}",
                    attach_started_at.elapsed()
                );
            },
            ScreenInstruction::RemoveClient(client_id) => {
                attach_traces_waiting_for_first_render.remove(&client_id);
                // A connection that never attached — a session probe, a one-shot
                // CLI action client, a hook's `zellij action agent-report` — has
                // no screen state, and its departure must not cost the session a
                // reflow and a full session-state broadcast. At several such
                // connections per hook event, those broadcasts were most of the
                // plugin threads' load while an agent worked.
                let was_attached = screen.is_client_attached(client_id);
                screen.remove_client(client_id).non_fatal();
                if was_attached {
                    // the departing client may have been the one constraining the
                    // screen size; resize_to_clients also reports session state.
                    screen.resize_to_clients().non_fatal();
                    screen.render(None).non_fatal();
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
                    |tab: &mut Tab, client_id: ClientId| tab.update_search_term(c, client_id), ?
                );
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
            },
            ScreenInstruction::SearchToggleWholeWord(
                client_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, client_id: ClientId| tab.toggle_search_whole_words(client_id)
                );
                screen.render(None)?;
            },
            ScreenInstruction::AddRedPaneFrameColorOverride(pane_ids, error_text) => {
                for pane_id in pane_ids {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.add_red_pane_frame_color_override(pane_id, error_text.clone());
                    }
                }
                screen.render(None)?;
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
                screen.render(None)?;
            },
            ScreenInstruction::FlashPaneFrameColor(pane_ids) => {
                let now = Instant::now();
                for pane_id in pane_ids {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        if tab.flash_pane_frame_color_override(pane_id, now) {
                            screen.fading_frame_panes.insert(pane_id);
                        }
                    }
                }
                screen.sync_animation_clock(now)?;
                screen.render(None)?;
            },
            ScreenInstruction::ClearPaneFrameColorOverride(pane_ids) => {
                for pane_id in pane_ids {
                    if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                        tab.clear_pane_frame_color_override(pane_id, None);
                    }
                }
                screen.render(None)?;
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
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::PreviousSwapLayout(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, _client_id: ClientId| tab.previous_swap_layout(),
                    ?
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::NextSwapLayout(client_id, _completion_tx) => {
                active_tab_and_connected_client_id!(
                    screen,
                    client_id,
                    |tab: &mut Tab, _client_id: ClientId| tab.next_swap_layout(),
                    ?
                );
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::OverrideLayout(
                cwd,
                default_shell,
                mut tab_layouts,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                apply_only_to_focused_tab,
                client_id,
                completion_tx,
            ) => {
                // 1. Determine which tabs to close (exist but not in layout)
                let existing_tab_indices: HashSet<usize> = screen.tabs.keys().copied().collect();
                let layout_tab_indices: HashSet<usize> =
                    tab_layouts.iter().map(|tl| tl.tab_index).collect();
                let tabs_to_close: Vec<usize> = existing_tab_indices
                    .difference(&layout_tab_indices)
                    .copied()
                    .collect();

                // 2. Process each tab layout
                let mut processed_tab_layouts = Vec::new();

                if apply_only_to_focused_tab {
                    match screen.get_active_tab_mut(client_id) {
                        Ok(active_tab) => {
                            if tab_layouts.is_empty() {
                                log::error!("No tab layouts found, cannot override.");
                                continue;
                            }
                            let mut tab_layout_info = tab_layouts.remove(0);
                            tab_layout_info.tab_index = active_tab.id;
                            // Set the tab name if provided
                            if let Some(name) = tab_layout_info.tab_name.take() {
                                active_tab.name = name;
                            }

                            // Find already-running panes for this tab
                            let (tiled_to_ignore, floating_indices) = find_already_running_panes(
                                &tab_layout_info.tiled_layout,
                                &tab_layout_info.floating_layouts,
                                active_tab,
                            );

                            // Mark run instructions to ignore (prevents re-spawning)
                            for run_instruction in tiled_to_ignore {
                                tab_layout_info
                                    .tiled_layout
                                    .ignore_run_instruction(run_instruction);
                            }

                            for idx in floating_indices {
                                tab_layout_info
                                    .floating_layouts
                                    .get_mut(idx)
                                    .map(|f| f.already_running = true);
                            }

                            processed_tab_layouts.push(tab_layout_info);
                        },
                        Err(e) => {
                            log::error!("Failed to override layout of active tab: {}", e);
                        },
                    }
                } else {
                    for mut tab_layout_info in tab_layouts {
                        // Get the tab by index
                        let tab = match screen.tabs.get_mut(&tab_layout_info.tab_index) {
                            Some(t) => t,
                            None => {
                                // no corresponding tab exists, we'll create it
                                processed_tab_layouts.push(tab_layout_info);
                                continue;
                            },
                        };

                        // Set the tab name if provided
                        if let Some(name) = tab_layout_info.tab_name.take() {
                            tab.name = name;
                        }

                        // Find already-running panes for this tab
                        let (tiled_to_ignore, floating_indices) = find_already_running_panes(
                            &tab_layout_info.tiled_layout,
                            &tab_layout_info.floating_layouts,
                            tab,
                        );

                        // Mark run instructions to ignore (prevents re-spawning)
                        for run_instruction in tiled_to_ignore {
                            tab_layout_info
                                .tiled_layout
                                .ignore_run_instruction(run_instruction);
                        }

                        for idx in floating_indices {
                            tab_layout_info
                                .floating_layouts
                                .get_mut(idx)
                                .map(|f| f.already_running = true);
                        }

                        processed_tab_layouts.push(tab_layout_info);
                    }
                }

                // 3. Send to plugin thread with all tab layouts
                screen
                    .bus
                    .senders
                    .send_to_plugin(PluginInstruction::OverrideLayout(
                        cwd,
                        default_shell,
                        processed_tab_layouts,
                        retain_existing_terminal_panes,
                        retain_existing_plugin_panes,
                        client_id,
                        completion_tx,
                    ))?;

                // 4. Close tabs that aren't in the layout
                if !apply_only_to_focused_tab {
                    for tab_index in tabs_to_close {
                        screen.close_tab_by_id(tab_index)?;
                    }
                }
            },
            ScreenInstruction::OverrideLayoutComplete(
                tab_results,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                client_id,
                completion_tx,
            ) => {
                // Process each tab result
                for tab_result in tab_results {
                    if let Some(tab) = screen.tabs.get_mut(&tab_result.tab_index) {
                        if let Err(e) = tab.override_layout(
                            tab_result.tiled_layout,
                            tab_result.floating_layouts,
                            tab_result.swap_tiled_layouts.clone(),
                            tab_result.swap_floating_layouts.clone(),
                            tab_result.new_terminal_pids,
                            tab_result.new_floating_pane_pids,
                            tab_result.plugin_ids,
                            retain_existing_terminal_panes,
                            retain_existing_plugin_panes,
                            client_id,
                            None,
                        ) {
                            log::error!(
                                "Failed to override layout for tab {}: {:?}",
                                tab_result.tab_index,
                                e
                            );
                        }
                    } else {
                        // Tab doesn't exist - create it
                        let swap_layouts = (
                            tab_result.swap_tiled_layouts.clone().unwrap_or_default(),
                            tab_result.swap_floating_layouts.clone().unwrap_or_default(),
                        );
                        if let Err(e) = screen.new_tab(
                            tab_result.tab_index,
                            swap_layouts,
                            tab_result.tab_name.clone(),
                            None,
                        ) {
                            log::error!(
                                "Failed to create new tab {} during override completion: {:?}",
                                tab_result.tab_index,
                                e
                            );
                            continue;
                        }

                        // Now override the newly created tab's layout
                        if let Some(tab) = screen.tabs.get_mut(&tab_result.tab_index) {
                            if let Err(e) = tab.override_layout(
                                tab_result.tiled_layout,
                                tab_result.floating_layouts,
                                tab_result.swap_tiled_layouts,
                                tab_result.swap_floating_layouts,
                                tab_result.new_terminal_pids,
                                tab_result.new_floating_pane_pids,
                                tab_result.plugin_ids,
                                retain_existing_terminal_panes,
                                retain_existing_plugin_panes,
                                client_id,
                                None,
                            ) {
                                log::error!(
                                    "Failed to override layout for new tab {}: {:?}",
                                    tab_result.tab_index,
                                    e
                                );
                            }
                        }
                    }
                }

                for event in pending_events_waiting_for_client.drain(..) {
                    screen.bus.senders.send_to_screen(event).non_fatal();
                }

                for event in pending_events_waiting_for_tab.drain(..) {
                    screen.bus.senders.send_to_screen(event).non_fatal();
                }

                // Single render and log after all tabs are updated (performance optimization)
                screen.log_and_report_session_state()?;
                let _ = screen.render(None);
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
                let tab_index = explicit_tab_id
                    .unwrap_or_else(|| *screen.active_tab_ids.values().next().unwrap_or(&1));
                let size = Size::default();
                let should_float = Some(false);
                let should_be_opened_in_place = false;
                let should_focus_plugin = if no_focus { Some(false) } else { None };
                screen
                    .bus
                    .senders
                    .send_to_pty(PtyInstruction::FillPluginCwd(
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
                        None,
                        completion_tx,
                        None,
                    ))?;
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
                let resolved_tab_index =
                    explicit_tab_id.or_else(|| screen.active_tab_ids.values().next().copied());
                match resolved_tab_index {
                    Some(tab_index) => {
                        let size = Size::default();
                        let should_float = Some(true);
                        let should_be_opened_in_place = false;
                        let should_focus_plugin = if no_focus { Some(false) } else { None };
                        let overlay_placement = match &placement {
                            NewPanePlacement::Overlay(overlay) => Some(overlay.clone()),
                            _ => None,
                        };
                        screen
                            .bus
                            .senders
                            .send_to_pty(PtyInstruction::FillPluginCwd(
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
                    },
                    None => {
                        log::error!(
                            "Could not find an active tab - is there at least 1 connected user?"
                        );
                    },
                }
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
                let resolved_tab_index =
                    explicit_tab_id.or_else(|| screen.active_tab_ids.values().next().copied());
                match resolved_tab_index {
                    Some(tab_index) => {
                        let size = Size::default();
                        let should_float = None;
                        let should_be_in_place = true;
                        let should_focus_plugin = if no_focus { Some(false) } else { None };
                        screen
                            .bus
                            .senders
                            .send_to_pty(PtyInstruction::FillPluginCwd(
                                should_float,
                                should_be_in_place,
                                close_replaced_pane,
                                pane_title,
                                run_plugin,
                                tab_index,
                                Some(pane_id_to_replace),
                                client_id,
                                size,
                                skip_cache,
                                None,
                                should_focus_plugin,
                                None,
                                completion_tx,
                                None,
                            ))?;
                    },
                    None => {
                        log::error!(
                            "Could not find an active tab - is there at least 1 connected user?"
                        );
                    },
                }
            },
            ScreenInstruction::StartOrReloadPluginPane(run_plugin, pane_title, completion_tx) => {
                let tab_index = screen.active_tab_ids.values().next().unwrap_or(&1);
                let size = Size::default();
                let should_float = Some(false);

                screen
                    .bus
                    .senders
                    .send_to_plugin(PluginInstruction::Reload(
                        should_float,
                        pane_title,
                        run_plugin,
                        *tab_index,
                        size,
                        completion_tx,
                    ))?;
            },
            ScreenInstruction::AddPlugin(
                should_float,
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
                mut completion_tx,
                stated_overlay_placement,
            ) => {
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
                let should_be_tiled = maybe_should_float.map(|f| !f).unwrap_or(false)
                    && floating_pane_coordinates.is_none();
                if let Some(overlay_placement) = overlay_placement {
                    new_pane_placement = NewPanePlacement::Overlay(overlay_placement);
                } else if floating_pane_coordinates.is_some() || should_float {
                    new_pane_placement = NewPanePlacement::with_floating_pane_coordinates(
                        floating_pane_coordinates.clone(),
                    );
                }
                if should_be_tiled {
                    new_pane_placement = NewPanePlacement::Tiled {
                        direction: None,
                        borderless: None,
                    };
                }
                if should_be_in_place {
                    new_pane_placement = NewPanePlacement::with_pane_id_to_replace(
                        pane_id_to_replace.map(|id| id.into()),
                        close_replaced_pane,
                    );
                }
                if screen.active_tab_ids.is_empty() && tab_index.is_none() {
                    pending_events_waiting_for_client.push(ScreenInstruction::AddPlugin(
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
                    continue;
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
                        screen.replace_pane(
                            PaneId::Plugin(plugin_id),
                            None,
                            Some(run_plugin),
                            Some(pane_title),
                            close_replaced_pane,
                            client_tab_index_or_pane_id,
                        )?;
                    } else if let Some(client_id) = client_id {
                        let client_tab_index_or_pane_id =
                            ClientTabIndexOrPaneId::ClientId(client_id);
                        screen.replace_pane(
                            PaneId::Plugin(plugin_id),
                            None,
                            Some(run_plugin),
                            Some(pane_title),
                            close_replaced_pane,
                            client_tab_index_or_pane_id,
                        )?;
                    } else {
                        log::error!("Must have pane id to replace or connected client_id if replacing a pane");
                    }
                } else if let Some(client_id) = client_id {
                    active_tab_and_connected_client_id!(screen, client_id, |active_tab: &mut Tab, _client_id: ClientId| {
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
                } else if let Some(active_tab) =
                    tab_index.and_then(|tab_index| screen.tabs.get_mut(&tab_index))
                {
                    active_tab.new_pane(
                        PaneId::Plugin(plugin_id),
                        Some(pane_title),
                        Some(run_plugin),
                        start_suppressed,
                        should_focus_plugin.unwrap_or(true),
                        new_pane_placement,
                        None,
                        None,
                    )?;
                } else {
                    log::error!("Tab index not found: {:?}", tab_index);
                }
                let pane_id = PaneId::Plugin(plugin_id);
                let plugin_exists = screen.tabs.values().any(|tab| tab.has_plugin(plugin_id));
                if plugin_exists {
                    if let Some(pending_events) = pending_events_waiting_for_pane.remove(&pane_id) {
                        replay_pending_plugin_events(&mut screen, pending_events)?;
                    }
                }
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UpdatePluginLoadingStage(pid, loading_indication) => {
                let found_plugin =
                    screen.update_plugin_loading_stage(pid, loading_indication.clone());
                if !found_plugin {
                    pending_events_waiting_for_pane
                        .entry(PaneId::Plugin(pid))
                        .or_default()
                        .push(ScreenInstruction::UpdatePluginLoadingStage(
                            pid,
                            loading_indication,
                        ));
                }
                screen.render(None)?;
            },
            ScreenInstruction::StartPluginLoadingIndication(pid, loading_indication) => {
                if !screen.start_plugin_loading_indication(pid, loading_indication.clone()) {
                    pending_events_waiting_for_pane
                        .entry(PaneId::Plugin(pid))
                        .or_default()
                        .push(ScreenInstruction::StartPluginLoadingIndication(
                            pid,
                            loading_indication,
                        ));
                }
                screen.render(None)?;
            },
            // No replay queue for a plugin whose pane does not exist yet, unlike the loading
            // *indication* above: with the offset derived from elapsed time there is nothing to
            // replay. The plugin stays registered and the next frame after its pane appears simply
            // draws the right number of dots.
            ScreenInstruction::StartPluginLoadingAnimation(pid) => {
                screen.animating_plugin_loads.insert(pid);
                screen.sync_animation_clock(Instant::now())?;
                screen.render(None)?;
            },
            ScreenInstruction::StopPluginLoadingAnimation(pid) => {
                screen.animating_plugin_loads.remove(&pid);
                // so the clock stops on the spot when this was the last load, rather than at the
                // next tick a dot period later
                screen.sync_animation_clock(Instant::now())?;
                screen.render(None)?;
            },
            ScreenInstruction::RequestStateUpdateForPlugins => {
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    tab.update_input_modes()?;
                }
                screen.log_and_report_session_state()?;
                screen.render(None)?;
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
                mut completion_tx,
                explicit_tab_id,
            ) => match pane_id_to_replace {
                Some(pane_id_to_replace) if should_open_in_place => {
                    let resolved_tab_index =
                        explicit_tab_id.or_else(|| screen.active_tab_ids.values().next().copied());
                    match resolved_tab_index {
                        Some(tab_index) => {
                            let size = Size::default();
                            screen
                                .bus
                                .senders
                                .send_to_pty(PtyInstruction::FillPluginCwd(
                                    Some(should_float),
                                    should_open_in_place,
                                    close_replaced_pane,
                                    None,
                                    run_plugin,
                                    tab_index,
                                    Some(pane_id_to_replace),
                                    client_id,
                                    size,
                                    skip_cache,
                                    None,
                                    None,
                                    None,
                                    completion_tx,
                                    None,
                                ))?;
                        },
                        None => {
                            log::error!(
                        "Could not find an active tab - is there at least 1 connected user?"
                    );
                        },
                    }
                },
                _ => {
                    let resolved_tab_and_client =
                        screen.resolve_tab_and_client(client_id, explicit_tab_id);
                    match resolved_tab_and_client {
                        Some((tab_index, client_id)) => {
                            if screen.focus_plugin_pane(
                                &run_plugin,
                                should_float,
                                move_to_focused_tab,
                                should_open_in_place,
                                client_id,
                                &mut completion_tx,
                            )? {
                                screen.render(None)?;
                                screen.log_and_report_session_state()?;
                            } else {
                                screen
                                    .bus
                                    .senders
                                    .send_to_pty(PtyInstruction::FillPluginCwd(
                                        Some(should_float),
                                        should_open_in_place,
                                        close_replaced_pane,
                                        None,
                                        run_plugin,
                                        tab_index,
                                        None,
                                        client_id,
                                        Size::default(),
                                        skip_cache,
                                        None,
                                        None,
                                        None,
                                        completion_tx,
                                        None,
                                    ))?;
                            }
                        },
                        None => {
                            log::error!("No connected clients found - cannot load or focus plugin")
                        },
                    }
                },
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
            ) => match pane_id_to_replace {
                Some(pane_id_to_replace) => {
                    let resolved_tab_index =
                        explicit_tab_id.or_else(|| screen.active_tab_ids.values().next().copied());
                    match resolved_tab_index {
                        Some(tab_index) => {
                            let size = Size::default();
                            screen
                                .bus
                                .senders
                                .send_to_pty(PtyInstruction::FillPluginCwd(
                                    Some(should_float),
                                    should_open_in_place,
                                    close_replaced_pane,
                                    None,
                                    run_plugin,
                                    tab_index,
                                    Some(pane_id_to_replace),
                                    client_id,
                                    size,
                                    skip_cache,
                                    cwd,
                                    if no_focus { Some(false) } else { None },
                                    None,
                                    completion_tx,
                                    overlay_placement,
                                ))?;
                        },
                        None => {
                            log::error!(
                            "Could not find an active tab - is there at least 1 connected user?"
                        );
                        },
                    }
                },
                None => {
                    let resolved_tab_and_client =
                        screen.resolve_tab_and_client(client_id, explicit_tab_id);
                    match resolved_tab_and_client {
                        Some((tab_index, client_id)) => {
                            screen
                                .bus
                                .senders
                                .send_to_pty(PtyInstruction::FillPluginCwd(
                                    Some(should_float),
                                    should_open_in_place,
                                    close_replaced_pane,
                                    None,
                                    run_plugin,
                                    tab_index,
                                    None,
                                    client_id,
                                    Size::default(),
                                    skip_cache,
                                    cwd,
                                    if no_focus { Some(false) } else { None },
                                    None,
                                    completion_tx,
                                    overlay_placement,
                                ))?;
                        },
                        None => {
                            log::error!("No connected clients found - cannot load or focus plugin")
                        },
                    }
                },
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
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                    continue;
                }
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    if tab.has_non_suppressed_pane_with_pid(&pane_id) {
                        tab.suppress_pane(pane_id, Some(client_id));
                        drop(screen.render(None));
                        break;
                    }
                }
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UnsuppressPane(pane_id, should_float_if_hidden) => {
                if let Some(tab) = screen.tab_containing_overlay_mut(pane_id) {
                    tab.show_overlay_pane(pane_id);
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                    continue;
                }
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.unsuppress_pane(pane_id, should_float_if_hidden);
                    drop(screen.render(None));
                }
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UnsuppressOrExpandPane(pane_id, should_float_if_hidden) => {
                if let Some(tab) = screen.tab_containing_overlay_mut(pane_id) {
                    tab.show_overlay_pane(pane_id);
                    screen.render(None)?;
                    screen.log_and_report_session_state()?;
                    continue;
                }
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.unsuppress_or_expand_pane(pane_id, should_float_if_hidden);
                    drop(screen.render(None));
                }
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::FocusPaneWithId(
                pane_id,
                should_float_if_hidden,
                should_be_in_place_if_hidden,
                client_id,
                mut completion_tx,
            ) => {
                let pane_exists = screen.tab_containing_pane(pane_id).is_some();
                if !pane_exists {
                    if let Some(c) = completion_tx.as_mut() {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Pane with id {:?} not found", pane_id));
                    }
                } else {
                    let already_focused = screen
                        .get_active_pane_id(&client_id)
                        .map(|active| active == pane_id)
                        .unwrap_or(false);
                    if already_focused {
                        if let Some(c) = completion_tx.as_mut() {
                            c.set_exit_status(1);
                            c.set_error_message(format!("Pane {:?} is already focused", pane_id));
                        }
                    } else {
                        screen.focus_pane_with_id(
                            pane_id,
                            should_float_if_hidden,
                            should_be_in_place_if_hidden,
                            client_id,
                        )?;
                        screen.clear_bell_for_pane_id(pane_id, client_id);
                        screen.sync_scroll_mode_on_focus(client_id)?;
                        screen.log_and_report_session_state()?;
                    }
                }
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
                                drop(screen.render(None));
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
                    screen.render(None)?;
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
                let tab_position = tab_index.saturating_sub(1); // Convert to 0-based

                match screen.get_tab_by_position_mut(tab_position) {
                    Some(tab) => {
                        tab.name = String::from_utf8_lossy(&new_name).to_string();
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
                        // switch_active_tab expects 0-based position
                        screen.switch_active_tab(tab_position, None, true, client_id)?;
                        screen.render(None)?;

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
                    tab.name = String::from_utf8_lossy(&new_name).to_string();
                    screen.log_and_report_session_state()?;
                } else {
                    log::error!("Failed to find tab with ID: {}", tab_id);
                }
            },
            ScreenInstruction::SetTabProjects(tab_projects) => {
                // Whole map each time, so a tab that lost its project loses the
                // name with it. Tabs we have never heard of are kept: a plugin can
                // know about a tab before we have finished making it, and the next
                // pass drops whatever is genuinely gone.
                let tab_projects: HashMap<usize, String> = tab_projects.into_iter().collect();
                if tab_projects != screen.tab_projects {
                    screen.tab_projects = tab_projects;
                    if screen.update_plugin_tab_names() {
                        screen.log_and_report_session_state()?;
                    }
                }
            },
            ScreenInstruction::CloseTabWithId(tab_id, _completion_tx) => {
                if screen.get_tab_by_id(tab_id).is_some() {
                    screen.close_tab_by_id(tab_id).non_fatal();
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
                        completion_tx
                            .as_mut()
                            .map(|c| c.set_affected_tab_id(affected_tab_id));
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
                let all_tabs = screen.get_tabs_mut();
                let found = all_tabs.values_mut().any(|tab| {
                    if tab.has_plugin(plugin_id) {
                        tab.request_plugin_permissions(plugin_id, Some(plugin_permission.clone()));
                        true
                    } else {
                        false
                    }
                });

                if !found {
                    log::error!("PluginId '{}' not found - caching request", plugin_id);
                    pending_events_waiting_for_client.push(
                        ScreenInstruction::RequestPluginPermissions(plugin_id, plugin_permission),
                    );
                }
            },
            ScreenInstruction::WarpPane {
                pane_id,
                to_session,
                placement,
                client_id,
                response_channel,
                deadline,
            } => {
                // Every check that can fail happens before the pty thread is asked
                // for anything: a warp that is going to be refused must leave the
                // pane exactly where it was.
                match screen.check_pane_can_be_warped(pane_id, client_id) {
                    Ok(terminal_id) => {
                        // Minus what this thread needs to serialize the pane
                        // once the release lands, so the answer still beats
                        // the caller's clock. `checked_sub` because `Instant -
                        // Duration` panics rather than saturating, and a panic
                        // here would take the screen thread with it; a deadline
                        // that has already passed is the honest answer for a
                        // caller with nothing left to spend. The same instant
                        // is what `PaneReleasedForWarp` checks before it starts
                        // that serialization, for the same reason.
                        let deadline = deadline
                            .checked_sub(PANE_WARP_SCREEN_RESERVE)
                            .unwrap_or_else(Instant::now);
                        screen
                            .pending_warps
                            .insert(terminal_id, (response_channel, deadline));
                        screen
                            .bus
                            .senders
                            .send_to_pty(PtyInstruction::ReleaseTerminalForWarp {
                                terminal_id,
                                to_session,
                                placement,
                                client_id,
                                deadline,
                            })?;
                    },
                    Err(e) => {
                        let _ = response_channel.send(Err(e));
                    },
                }
            },
            ScreenInstruction::PaneReleasedForWarp {
                terminal_id,
                placement: _,
                to_session: _,
                client_id: _,
                released,
            } => {
                // Asked before the pane is taken apart, exactly as
                // `TabReleasedForWarp` asks below and for the same reason:
                // `take_released_pane_for_warp` is what closes the pane, so a
                // caller found to be gone on the far side of it has already
                // cost the user the pane, with its process running unwatched
                // and a log line as the only record. A caller past its
                // deadline has stopped waiting; the pane stays where it is,
                // held and told why.
                let still_waiting = match screen.pending_warps.remove(&terminal_id) {
                    Some((_, deadline)) if Instant::now() >= deadline => None,
                    other => other,
                };
                if let Some((response_channel, _)) = still_waiting {
                    let result = screen.take_released_pane_for_warp(terminal_id, released);
                    if let Err(crossbeam::channel::SendError(Ok(payload))) =
                        response_channel.send(result)
                    {
                        // The caller timed out between asking and being
                        // answered. The pane is out of this session by now and
                        // its process is running unwatched, so the recovery
                        // line has to go somewhere: this log is the only place
                        // left that it can.
                        log::error!(
                            "pane {terminal_id} was released for a warp nobody was waiting on; \
                             it is still running and can be recovered with `{}`",
                            crate::warp::recovery_command(&payload)
                        );
                    }
                } else {
                    // Nobody is waiting, which means the supervisor has been
                    // released with no one to hand it to. The pane is still
                    // on screen and now talking to nothing, so it has to say
                    // so itself — a log line alone leaves the user with a pane
                    // that looks alive and answers nothing. A release that
                    // *failed* was already stranded by the pty thread, with
                    // the failure as its reason.
                    log::error!(
                        "pane {terminal_id} was released for a warp nobody was waiting on; it is \
                         still running and can be recovered with `zellij pty-host adopt <id>`"
                    );
                    if let Ok(released) = released {
                        screen
                            .bus
                            .senders
                            .send_to_pty(PtyInstruction::StrandPaneAfterWarp {
                                terminal_id,
                                why: "this pane was moved to another session, but nothing took it"
                                    .to_owned(),
                                recovery: Some(released.recovery_command()),
                            })
                            .non_fatal();
                    }
                }
                screen.log_and_report_session_state().non_fatal();
                screen.render(None)?;
            },
            ScreenInstruction::AdoptedPaneReady {
                terminal_id,
                payload,
                placement,
                client_id,
                cmd,
                buffered,
                child_exit_status,
            } => {
                screen.place_adopted_pane(
                    terminal_id,
                    *payload,
                    placement,
                    client_id,
                    cmd,
                    buffered,
                    child_exit_status,
                )?;
                screen.log_and_report_session_state().non_fatal();
                screen.render(None)?;
            },
            ScreenInstruction::WarpTab {
                tab_index,
                client_id,
                response_channel,
                deadline,
            } => {
                // Every check that can fail happens before the pty thread is
                // asked for anything, so a tab whose warp is going to be refused
                // stays exactly where it was.
                match screen.check_tab_can_be_warped(tab_index, client_id) {
                    Ok((tab_index, terminal_ids, prepared)) => {
                        // The caller's deadline less what this thread will
                        // need afterwards: `take_released_tab_for_warp`
                        // serializes every pane's scrollback, and a release
                        // that used the whole budget would leave no time to do
                        // it in. `checked_sub` for the reason given in
                        // `WarpPane` above. One number for both the pty thread
                        // and the check in `TabReleasedForWarp`: a release
                        // that lands inside the reserve is one this thread can
                        // no longer serialize in time, however punctual the
                        // pty thread was.
                        let deadline = deadline
                            .checked_sub(TAB_WARP_SCREEN_RESERVE)
                            .unwrap_or_else(Instant::now);
                        screen
                            .pending_tab_warps
                            .insert(tab_index, (response_channel, prepared, deadline));
                        screen.bus.senders.send_to_pty(
                            PtyInstruction::ReleaseTerminalsForWarp {
                                tab_index,
                                terminal_ids,
                                deadline,
                            },
                        )?;
                    },
                    Err(e) => {
                        let _ = response_channel.send(Err(e));
                    },
                }
            },
            ScreenInstruction::TabReleasedForWarp {
                tab_index,
                released,
            } => {
                // Asked before the tab is taken apart rather than after.
                // `take_released_tab_for_warp` is what closes the tab, so a
                // caller found to be gone on the far side of it has already
                // cost this session the whole tab, with every process in it
                // running unwatched and a log line as the only record. A caller
                // whose deadline has passed — less the reserve this thread
                // needs, which is what the stored instant already is — is a
                // caller that has stopped waiting, so the tab stays where it
                // is and its panes are held and told why — visible, and
                // recoverable — below.
                let still_waiting = match screen.pending_tab_warps.remove(&tab_index) {
                    Some((_, _, deadline)) if Instant::now() >= deadline => None,
                    other => other,
                };
                if let Some((response_channel, prepared, _)) = still_waiting {
                    let result = screen.take_released_tab_for_warp(tab_index, released, prepared);
                    if let Err(crossbeam::channel::SendError(Ok(payload))) =
                        response_channel.send(result)
                    {
                        // The caller timed out between asking and being
                        // answered. The tab is out of this session by now and
                        // every process in it is running unwatched.
                        log::error!(
                            "tab {tab_index} was released for a warp nobody was waiting on; its \
                             panes are still running and can be recovered with: {}",
                            payload
                                .tiled_panes
                                .iter()
                                .chain(payload.floating_panes.iter())
                                .map(crate::warp::recovery_command)
                                .collect::<Vec<_>>()
                                .join("; ")
                        );
                    }
                } else {
                    // Nobody is waiting — the caller gave up, or left too
                    // little time to serialize the tab in — which means the
                    // supervisors have been released with no one to hand them
                    // to. The tab is still on screen and every pane in it is
                    // now talking to nothing, so saying where the processes
                    // went in the log is not enough — each pane has to say it
                    // too, or the user is left with a tab that looks alive and
                    // answers nothing.
                    log::error!(
                        "tab {tab_index} was released for a warp nobody was waiting on any \
                         more; the tab was kept, and its panes are still running and can be \
                         recovered with `zellij pty-host adopt <id>`"
                    );
                    for (terminal_id, released) in released {
                        if let Ok(released) = released {
                            screen
                                .bus
                                .senders
                                .send_to_pty(PtyInstruction::StrandPaneAfterWarp {
                                    terminal_id,
                                    why:
                                        "this tab was moved to another session, but nothing took it"
                                            .to_owned(),
                                    recovery: Some(released.recovery_command()),
                                })
                                .non_fatal();
                        }
                    }
                }
                screen.log_and_report_session_state().non_fatal();
                screen.render(None)?;
            },
            ScreenInstruction::AdoptedTabReady {
                payload,
                adopted,
                client_id,
            } => {
                screen.place_adopted_tab(*payload, *adopted, client_id)?;
                screen.log_and_report_session_state().non_fatal();
                screen.render(None)?;
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
            ScreenInstruction::UpdateSessionInfos(new_session_infos, resurrectable_sessions) => {
                screen.update_session_infos(new_session_infos, resurrectable_sessions)?;
            },
            ScreenInstruction::UpdateAvailableLayouts(layouts, errors) => {
                screen.update_available_layouts(layouts, errors);
            },
            ScreenInstruction::ReplacePane(
                new_pane_id,
                hold_for_command,
                pane_title,
                invoked_with,
                close_replaced_pane,
                client_id_tab_index_or_pane_id,
                mut completion_tx,
            ) => {
                completion_tx
                    .as_mut()
                    .map(|c| c.set_affected_pane_id(new_pane_id));
                screen.replace_pane(
                    new_pane_id,
                    hold_for_command,
                    invoked_with,
                    pane_title,
                    close_replaced_pane,
                    client_id_tab_index_or_pane_id,
                )?;
                // A plugin can open an agent pane in place, the same as anywhere else.
                screen.fire_agent_start_for_new_pane(new_pane_id);

                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::SerializeLayoutForResurrection => {
                if screen.session_serialization {
                    screen.dump_layout_to_hd()?;
                }
            },
            ScreenInstruction::SaveSession(_client_id, completion_tx) => {
                let err_context = || "Failed to save session";

                screen.update_active_pane_ids();
                let pane_manifest = screen.generate_and_report_pane_state()?;
                let tab_infos = screen.generate_and_report_tab_state()?;

                #[cfg(not(test))]
                let (available_layouts, _layout_errors) = Layout::list_available_layouts(
                    screen.layout_dir.clone(),
                    &screen.default_layout_name,
                );
                #[cfg(test)]
                let available_layouts = vec![];

                let creation_time = {
                    let sock_path = ZELLIJ_SOCK_DIR.join(&screen.session_name);
                    std::fs::metadata(&sock_path)
                        .ok()
                        .and_then(|f| f.created().ok().or_else(|| f.modified().ok()))
                        .and_then(|d| d.elapsed().ok())
                        .map(|d| Duration::from_secs(d.as_secs()))
                        .unwrap_or_default()
                };
                let session_info = SessionInfo {
                    name: screen.session_name.clone(),
                    tabs: tab_infos,
                    panes: pane_manifest,
                    connected_clients: screen.active_tab_ids.keys().len(),
                    is_current_session: true,
                    available_layouts,
                    plugins: Default::default(),
                    tab_history: screen.tab_history.clone(),
                    pane_history: screen
                        .pane_history
                        .iter()
                        .map(|(k, v)| (*k, v.iter().map(|v| (*v).into()).collect()))
                        .collect(),
                    creation_time,
                };

                let session_layout_metadata = if screen.session_serialization {
                    screen.get_layout_metadata(Some(screen.default_shell.clone()), None)
                } else {
                    // Create empty metadata if serialization is disabled
                    SessionLayoutMetadata::new(screen.default_layout.clone())
                };

                screen
                    .bus
                    .senders
                    .send_to_pty(PtyInstruction::SaveSessionToDisk {
                        session_name: screen.session_name.clone(),
                        session_info,
                        session_layout_metadata,
                        completion_tx,
                    })
                    .with_context(err_context)?;
            },
            ScreenInstruction::Notify(
                text,
                pane_id,
                _completion_tx, // the action ends here, dropping this will release anything
                                // waiting for it
            ) => {
                let now = Instant::now();
                let id = screen.next_notification_id;
                screen.next_notification_id += 1;
                let received_at_unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                screen.notification_history.push(NotificationInfo {
                    id,
                    message: text.clone(),
                    received_at_unix_ms,
                });
                // the survivors are about to be pushed away from the corner - bracketing the
                // insert is what glides them there instead of letting them jump
                screen.with_notification_stack_change(now, |screen| {
                    screen
                        .notifications
                        .insert(0, Notification::new(id, text, pane_id, now));
                });
                screen
                    .bus
                    .senders
                    .send_to_background_jobs(BackgroundJob::DismissNotification(
                        id,
                        screen.notification_timeout_secs,
                    ))?;
                screen.sync_animation_clock(now)?;
                screen.render(None)?;
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
            ScreenInstruction::ActivateContextMenuItem {
                pane_id,
                actions,
                client_id,
            } => {
                screen.dispatch_context_menu_actions(pane_id, actions, client_id);
                screen.render(None)?;
            },
            ScreenInstruction::CloseContextMenu(client_id) => {
                if screen.close_context_menu(client_id) {
                    screen.render(None)?;
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
                if screen.peer_sessions_cache.contains_key(&name) {
                    let error_text = "A session by this name already exists.";
                    log::error!("{}", error_text);
                    if let Some(os_input) = &mut screen.bus.os_input {
                        let _ = os_input.send_to_client(
                            client_id,
                            ServerToClientMsg::LogError {
                                lines: vec![error_text.to_owned()],
                            },
                        );
                    }
                } else if screen.resurrectable_sessions_cache.contains_key(&name) {
                    let error_text =
                        "A resurrectable session by this name exists, cannot use this name.";
                    log::error!("{}", error_text);
                    if let Some(os_input) = &mut screen.bus.os_input {
                        let _ = os_input.send_to_client(
                            client_id,
                            ServerToClientMsg::LogError {
                                lines: vec![error_text.to_owned()],
                            },
                        );
                    }
                } else {
                    let err_context = || "Failed to rename session".to_string();
                    let old_session_name = screen.session_name.clone();

                    // update state
                    screen.session_name = name.clone();
                    screen.default_mode_info.session_name = Some(name.clone());
                    for (_client_id, mode_info) in screen.mode_info.iter_mut() {
                        mode_info.session_name = Some(name.clone());
                    }
                    for (_, tab) in screen.tabs.iter_mut() {
                        tab.rename_session(name.clone()).with_context(err_context)?;
                    }

                    // rename socket file
                    let old_socket_file_path = ZELLIJ_SOCK_DIR.join(&old_session_name);
                    let new_socket_file_path = ZELLIJ_SOCK_DIR.join(&name);
                    if let Err(e) = std::fs::rename(old_socket_file_path, new_socket_file_path) {
                        log::error!("Failed to rename ipc socket: {:?}", e);
                    }

                    // rename session_info folder (TODO: make this atomic, right now there is a
                    // chance background_jobs will re-create this folder before it knows the
                    // session was renamed)
                    let old_session_info_folder =
                        session_info_folder_for_session(&old_session_name);
                    let new_session_info_folder = session_info_folder_for_session(&name);
                    if let Err(e) =
                        std::fs::rename(old_session_info_folder, new_session_info_folder)
                    {
                        log::error!("Failed to rename session_info folder: {:?}", e);
                    }

                    // report
                    screen
                        .log_and_report_session_state()
                        .with_context(err_context)?;

                    // set the env variable
                    set_session_name(name.clone());
                    let connected_client_ids: Vec<ClientId> =
                        screen.active_tab_ids.keys().copied().collect();
                    for client_id in connected_client_ids {
                        if let Some(os_input) = &mut screen.bus.os_input {
                            let _ = os_input.send_to_client(
                                client_id,
                                ServerToClientMsg::RenamedSession { name: name.clone() },
                            );
                        }
                    }
                }
            },
            ScreenInstruction::Reconfigure {
                client_id,
                keybinds,
                default_mode,
                theme,
                host_theme_dark,
                host_theme_light,
                explicit_theme_hue,
                simplified_ui,
                default_shell,
                default_cwd,
                pane_frame_style,
                copy_to_clipboard,
                copy_command,
                copy_on_select,
                auto_layout,
                rounded_corners,
                hide_session_name,
                stacked_resize,
                stacked_pane_list,
                default_editor,
                advanced_mouse_actions,
                mouse_scroll_resize,
                mouse_hover_effects,
                visual_bell,
                focus_follows_mouse,
                mouse_click_through,
                context_menu_on_right_click,
                context_menu,
                osc133_command_selection,
                word_separators,
                nested_session_handling,
                dangerously_enable_paste_buffer_read,
                host_notification_protocol,
            } => {
                screen.host_theme_dark_styling = host_theme_dark;
                screen.host_theme_light_styling = host_theme_light;
                screen
                    .reconfigure(
                        keybinds,
                        default_mode,
                        theme,
                        simplified_ui,
                        default_shell,
                        default_cwd,
                        pane_frame_style,
                        copy_command,
                        copy_to_clipboard,
                        copy_on_select,
                        auto_layout,
                        rounded_corners,
                        hide_session_name,
                        stacked_resize,
                        stacked_pane_list,
                        default_editor,
                        advanced_mouse_actions,
                        mouse_scroll_resize,
                        mouse_hover_effects,
                        visual_bell,
                        focus_follows_mouse,
                        mouse_click_through,
                        context_menu_on_right_click,
                        context_menu,
                        osc133_command_selection,
                        word_separators,
                        nested_session_handling,
                        dangerously_enable_paste_buffer_read,
                        host_notification_protocol,
                        client_id,
                    )
                    .non_fatal();
                // `reconfigure` painted the static theme; put the session's
                // effective hue back on top of it, against the possibly
                // changed dark/light definitions.
                if explicit_theme_hue != screen.configured_explicit_theme_hue {
                    screen
                        .apply_configured_explicit_theme_hue(explicit_theme_hue)
                        .non_fatal();
                } else {
                    screen.reapply_effective_theme_mode().non_fatal();
                }
                screen.resolve_default_theme_mode().non_fatal();
            },
            ScreenInstruction::RerunCommandPane(terminal_pane_id, completion_tx) => {
                screen.rerun_command_pane_with_id(terminal_pane_id, completion_tx);
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::RunCommandInPane(terminal_pane_id, run_command, keep_scrollback) => {
                screen.run_command_in_pane_with_id(terminal_pane_id, run_command, keep_scrollback);
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::ResizePaneWithId(resize, pane_id) => {
                screen.resize_pane_with_id(resize, pane_id)?;
            },
            ScreenInstruction::EditScrollbackForPaneWithId(pane_id, completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.edit_scrollback_for_pane_with_id(pane_id, completion_tx)
                        .non_fatal();
                }
                screen.render(None)?;
            },
            ScreenInstruction::WriteToPaneId(bytes, pane_id, _completion) => {
                screen
                    .write_to_pane_id(&None, bytes, false, pane_id, None, _completion)
                    .non_fatal();
                screen.render(None)?;
            },
            ScreenInstruction::Paste(bytes, pane_id, client_id, _completion) => {
                match pane_id {
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
                screen.render(None)?;
            },
            ScreenInstruction::SetPaneAgent(pane_id, agent, _completion) => {
                if !apply_pane_agent_report(&mut screen, pane_id, agent.clone())? {
                    let pending_events =
                        pending_events_waiting_for_pane.entry(pane_id).or_default();
                    pending_events
                        .retain(|event| !matches!(event, ScreenInstruction::SetPaneAgent(..)));
                    pending_events.push(ScreenInstruction::SetPaneAgent(
                        pane_id,
                        agent,
                        _completion,
                    ));
                    continue;
                }
            },
            ScreenInstruction::SetPaneColor(pane_id, fg, bg, _completion) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.set_pane_color(pane_id, fg, bg).non_fatal();
                }
                screen.render(None)?;
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
                screen.render(None)?;
            },
            ScreenInstruction::CopyTextToClipboard(text, plugin_id) => {
                let plugin_pane_id = PaneId::Plugin(plugin_id);
                if let Some(tab) = screen.tab_containing_pane_mut(plugin_pane_id) {
                    tab.copy_text_to_clipboard(&text)
                        .with_context(|| {
                            format!("failed to copy text to clipboard from plugin {}", plugin_id)
                        })
                        .non_fatal();
                }
                screen.render(None)?;
            },
            ScreenInstruction::MovePaneWithPaneId(pane_id) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.move_pane(pane_id);
                }
                screen.render(None)?;
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
                screen.render(None)?;
            },
            ScreenInstruction::ClearScreenForPaneId(pane_id) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.clear_screen_for_pane_id(pane_id);
                }
                screen.render(None)?;
            },
            ScreenInstruction::ScrollUpInPaneId(pane_id) => {
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
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
            },
            ScreenInstruction::PageScrollUpInPaneId(pane_id) => {
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
                screen.render(None)?;
            },
            ScreenInstruction::PageScrollDownInPaneId(pane_id) => {
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
                screen.render(None)?;
            },
            ScreenInstruction::TogglePaneIdFullscreen(pane_id) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.toggle_pane_fullscreen(pane_id);
                }
                screen.render(None)?;
            },
            ScreenInstruction::TogglePaneEmbedOrEjectForPaneId(pane_id) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.toggle_pane_embed_or_floating_for_pane_id(pane_id, None)
                        .non_fatal();
                }
                screen.render(None)?;
            },
            ScreenInstruction::CloseTabWithIndex(tab_index) => {
                // tab_index here means display position (0-based) per API convention
                // Must map to stable ID for close_tab_by_id()

                if let Some(tab_id) = screen.get_tab_id_at_position(tab_index) {
                    screen.close_tab_by_id(tab_id).non_fatal();
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
                completion_tx
                    .as_mut()
                    .map(|c| c.set_affected_tab_id(tab_id));
                // TODO: is this a race?
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
                    continue; // affected_tab_id stays None, signalling failure
                };
                let outcome = screen.break_multiple_panes_to_tab_with_id(
                    pane_ids,
                    tab_id,
                    should_change_focus_to_new_tab,
                    client_id,
                )?;
                match outcome.affected_tab_id() {
                    Some(affected_tab_id) => {
                        completion_tx
                            .as_mut()
                            .map(|c| c.set_affected_tab_id(affected_tab_id));
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
                    let _ = screen.render(None);
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
                let _ = screen.render(None);
            },
            ScreenInstruction::ChangeOverlayCoordinates(
                pane_ids_and_coordinates,
                override_user_placement,
                _completion_tx,
            ) => {
                screen
                    .change_overlay_coordinates(pane_ids_and_coordinates, override_user_placement);
                let _ = screen.render(None);
            },
            ScreenInstruction::TogglePaneBorderless(pane_id, _completion_tx) => {
                screen.toggle_pane_borderless(pane_id);
                let _ = screen.render(None);
            },
            ScreenInstruction::SetPaneBorderless(pane_id, borderless, _completion_tx) => {
                screen.set_pane_borderless(pane_id, borderless);
                let _ = screen.render(None);
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
                    screen.render(None)?;
                }
                let _ = screen.log_and_report_session_state();
            },
            ScreenInstruction::RevealPaneWithId(pane_id, label, client_id) => {
                screen.reveal_pane_with_id(pane_id, label, client_id)?;
            },
            ScreenInstruction::FloatMultiplePanes(pane_ids_to_float, client_id) => {
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
                    screen.render(None)?;
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
                    screen.render(None)?;
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
            ScreenInstruction::ClearKeyPressesIntercepts(client_id) => {
                keybind_intercepts.remove(&client_id);
            },
            ScreenInstruction::ReplacePaneWithExistingPane(
                old_pane_id,
                new_pane_id,
                suppress_replaced_pane,
                completion_tx,
            ) => screen.replace_pane_with_existing_pane(
                old_pane_id,
                new_pane_id,
                suppress_replaced_pane,
                completion_tx,
            ),
            ScreenInstruction::AddWatcherClient(client_id, size) => {
                screen
                    .add_watcher_client(client_id)
                    .context("failed to add watcher client")?;
                screen.set_watcher_size(client_id, size);
                screen.render(None)?;
            },
            ScreenInstruction::RemoveWatcherClient(client_id) => {
                screen.remove_watcher_client(client_id);
            },
            ScreenInstruction::SetFollowedClient(client_id) => {
                screen
                    .set_followed_client(client_id)
                    .context("failed to set followed client")?;
            },
            ScreenInstruction::WatcherTerminalResize(client_id, size) => {
                screen.set_watcher_size(client_id, size);
                screen.render(None)?;
            },
            ScreenInstruction::ClearMouseHelpText(client_id) => {
                if let Ok(tab) = screen.get_active_tab_mut(client_id) {
                    tab.clear_mouse_help_text(client_id);
                    screen.render(None)?;
                }
            },
            ScreenInstruction::SetPluginRegexHighlights {
                pane_id,
                plugin_id,
                highlights,
            } => {
                let style = screen.style.clone();
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.set_plugin_regex_highlights_for_pane(
                        pane_id, plugin_id, highlights, &style,
                    );
                }
                screen.render(None)?;
            },
            ScreenInstruction::ClearPluginHighlights { pane_id, plugin_id } => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.clear_plugin_highlights_for_pane(pane_id, plugin_id);
                }
                screen.render(None)?;
            },
            ScreenInstruction::ClearAllPluginHighlights(plugin_id) => {
                let all_tabs = screen.get_tabs_mut();
                for tab in all_tabs.values_mut() {
                    tab.clear_all_plugin_highlights(plugin_id);
                }
                screen.render(None)?;
            },
            ScreenInstruction::DesktopNotificationResponse(raw_bytes, client_id) => {
                if let Some(response) = denormalize_notification_response(&raw_bytes) {
                    let pane_id = PaneId::Terminal(response.terminal_id);
                    if response.forward_to_pane {
                        if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                            tab.write_to_pane_id(&None, response.bytes, false, pane_id, None, None)
                                .non_fatal();
                        }
                    }
                    if response.focus_pane {
                        screen
                            .focus_pane_with_id(pane_id, false, false, client_id)
                            .non_fatal();
                    }
                    screen.render(None)?;
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
                screen.notify_pane_closed_to_subscribers(pane_id);
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
                if subscriptions.is_empty() {
                    screen
                        .background_plugin_subscriptions
                        .remove(&(plugin_id, client_id));
                } else {
                    screen
                        .background_plugin_subscriptions
                        .insert((plugin_id, client_id), subscriptions);
                }
            },
            ScreenInstruction::ClearHintTextCache => {
                for tab in screen.tabs.values_mut() {
                    tab.clear_hint_text_cache();
                }
                screen.render(None)?;
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
                screen.render(None)?;
            },
            ScreenInstruction::ScrollDownWithPaneId(pane_id, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.scroll_down_by_pane_id(pane_id).non_fatal();
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::ScrollToTopWithPaneId(pane_id, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.scroll_to_top_by_pane_id(pane_id).non_fatal();
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::ScrollToBottomWithPaneId(pane_id, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.scroll_to_bottom_by_pane_id(pane_id).non_fatal();
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::PageScrollUpWithPaneId(pane_id, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.page_scroll_up_by_pane_id(pane_id);
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::PageScrollDownWithPaneId(pane_id, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.page_scroll_down_by_pane_id(pane_id).non_fatal();
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::HalfPageScrollUpWithPaneId(pane_id, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.half_page_scroll_up_by_pane_id(pane_id);
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::HalfPageScrollDownWithPaneId(pane_id, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.half_page_scroll_down_by_pane_id(pane_id).non_fatal();
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.sync_scroll_mode_for_pane_id(pane_id)?;
                screen.render(None)?;
            },
            ScreenInstruction::ResizeWithPaneId(pane_id, strategy, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.resize_by_pane_id(pane_id, strategy);
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MovePaneWithPaneIdCli(pane_id, direction, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.move_pane_by_pane_id(pane_id, direction);
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MovePaneBackwardsWithPaneId(pane_id, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.move_pane_backwards_by_pane_id(pane_id);
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::ClearScreenWithPaneId(pane_id, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.clear_screen_by_pane_id(pane_id).non_fatal();
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.render(None)?;
            },
            ScreenInstruction::EditScrollbackWithPaneId(pane_id, ansi, completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    if ansi {
                        tab.edit_scrollback_raw_for_pane_with_id(pane_id, completion_tx)
                            .non_fatal();
                    } else {
                        tab.edit_scrollback_for_pane_with_id(pane_id, completion_tx)
                            .non_fatal();
                    }
                } else {
                    log::error!("Pane with id {:?} not found", pane_id);
                }
                screen.render(None)?;
            },
            ScreenInstruction::ToggleFullscreenWithPaneId(pane_id, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.toggle_fullscreen_by_pane_id(pane_id);
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::ToggleNoUiFullscreenWithPaneId(pane_id, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.toggle_no_ui_fullscreen_by_pane_id(pane_id);
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::TogglePaneEmbedOrFloatingWithPaneId(pane_id, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.toggle_pane_embed_or_floating_for_pane_id(pane_id, None)
                        .non_fatal();
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.log_and_report_session_state()?;
                screen.render(None)?;
            },
            ScreenInstruction::CloseFocusWithPaneId(pane_id, completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.close_pane_by_pane_id(pane_id, completion_tx)
                        .non_fatal();
                } else {
                    log::error!("Pane with id {:?} not found", pane_id);
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::RenamePaneWithPaneId(pane_id, name, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.rename_pane_by_pane_id(pane_id, name).non_fatal();
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::UndoRenamePaneWithPaneId(pane_id, mut _completion_tx) => {
                if let Some(tab) = screen.tab_containing_pane_mut(pane_id) {
                    tab.undo_rename_pane_by_pane_id(pane_id);
                } else {
                    Screen::report_pane_not_found(pane_id, &mut _completion_tx);
                }
                screen.render(None)?;
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
                    if tab.name != tab.prev_name {
                        tab.name = tab.prev_name.clone();
                        screen.log_and_report_session_state()?;
                    }
                } else {
                    log::error!("Tab with id {} not found", tab_id);
                    if let Some(ref mut c) = _completion_tx {
                        c.set_exit_status(1);
                        c.set_error_message(format!("Tab with id {} not found", tab_id));
                    }
                }
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
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
                screen.render(None)?;
                screen.log_and_report_session_state()?;
            },
            ScreenInstruction::MoveTabWithTabId(tab_id, direction, _completion_tx) => {
                if pending_tab_ids.is_empty() {
                    screen.move_tab_by_id(tab_id, direction)?;
                    screen.render(None)?;
                } else {
                    pending_events_waiting_for_tab.push(ScreenInstruction::MoveTabWithTabId(
                        tab_id,
                        direction,
                        _completion_tx,
                    ));
                }
            },
            ScreenInstruction::FocusHostSession(client_id, _completion_tx) => {
                let payload = nested_session::encode_payload(&NestedSessionMessage::FocusHost {
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
                let payload =
                    nested_session::encode_payload(&NestedSessionMessage::ToggleHostFullscreen {
                        fullscreen,
                    });
                let _ = screen.bus.senders.send_to_server(
                    ServerInstruction::EmitNestedSessionFrameToClient(client_id, payload),
                );
            },
        }
    }
    Ok(())
}

//! Synchronous action dispatch over the thread bus, shared by the server and plugin host.
mod completion;
mod panes;
mod queries;

pub use completion::wait_for_action_completion;
use completion::{send_error_to_client, send_output_to_client};
use panes::new_pane_routing;
use std::collections::{BTreeMap, HashSet};
use tokio::sync::oneshot;
use yinz_bus::route::{ActionCompletionResult, NotificationEnd};

use uuid::Uuid;
use yinz_bus::thread_bus::ThreadSenders;
use yinz_bus::{
    os_input_output::ClientTransport, plugins::PluginInstruction, pty::PtyInstruction,
    screen::ScreenInstruction, PaneId, ServerInstruction,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::geometry::{Direction, ResizeStrategy};
use {
    yinz_types::data::Event, yinz_types::data::InputMode, yinz_types::data::NewPanePlacement,
    yinz_types::data::SessionConnection, yinz_types::input::actions::Action,
    yinz_types::input::actions::SearchDirection, yinz_types::input::actions::SearchOption,
    yinz_types::input::command::TerminalAction,
};

use yinz_bus::ClientId;

// `route_action` must not borrow from the `session_data` read guard.
// otherwise blocking-CLI actions
// (`wait_forever=true`) park this function while still holding the guard,
// deadlocking concurrent `session_data.write()`s.
/// Whether a key's resolved actions take an open context menu with them.
///
/// A key that writes to the pane is handled in `ScreenInstruction::WriteCharacter`, which gives the
/// menu first refusal on the keys it uses and dismisses it for the rest. Anything else was resolved
/// to its own instructions here and never passes that point, so a menu would survive the tab
/// switch, focus move or layout change it is now describing wrongly.
pub fn actions_dismiss_context_menu(actions: &[Action]) -> bool {
    actions
        .iter()
        .any(|action| !matches!(action, Action::Write { .. } | Action::WriteChars { .. }))
}

/// Whether a key's resolved actions copy text, and so must not tell the plugins about the input.
///
/// The status bar shows "Text copied to..." from a `CopyToClipboard` and drops it again on the next
/// `InputReceived`, so the keystroke that *caused* the copy must not send one - the same rule the
/// mouse path keeps with `MouseEffect::leave_clipboard_message`. It has to be decided for the whole
/// batch rather than per action, because the copy is routed to completion before the rest of the
/// key's actions are: `bind "Enter" { Copy; SwitchToMode "Normal"; }` sets the message with the
/// first action and would clear it with the second.
pub fn actions_leave_clipboard_message(actions: &[Action]) -> bool {
    actions
        .iter()
        .any(|action| action.leaves_clipboard_message())
}

pub fn route_action(
    action: Action,
    client_id: ClientId,
    cli_client_id: Option<ClientId>,
    pane_id: Option<PaneId>,
    senders: ThreadSenders,
    default_shell: Option<TerminalAction>,
    mut seen_cli_pipes: Option<&mut HashSet<String>>,
    default_mode: InputMode,
    os_input: Option<Box<dyn ClientTransport>>,
    leaves_clipboard_message: bool,
    is_client_input: bool,
) -> Result<(bool, Option<ActionCompletionResult>)> {
    let mut should_break = false;
    let err_context = || format!("failed to route action for client {client_id}");
    let action_name = action.to_string();

    if !action.is_mouse_action() && !leaves_clipboard_message {
        // mouse actions should only send InputReceived to plugins
        // if they do not result in text being marked, this is handled in Tab
        // - and a key whose actions copy text is silent for the same reason, see
        // `actions_leave_clipboard_message`
        senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                Some(client_id),
                Event::InputReceived,
            )]))
            .with_context(err_context)?;
    }

    // we use this oneshot channel to wait for an action to be "logically"
    // done, meaning that it traveled through all the threads it needed to travel through and the
    // app has confirmed that it is complete. Once this happens, we get a signal through the
    // wait_for_action_completion call below (or the timeout for other actions) and release this thread,
    // allowing the client to produce another action without risking races
    let (completion_tx, completion_rx) = oneshot::channel();

    let creates_group = matches!(
        action,
        Action::NewNamedTabGroup { .. } | Action::GoToTabGroupName { create: true, .. }
    );
    let creates_pane = matches!(
        action,
        Action::NewPane { .. }
            | Action::NewTiledPane { .. }
            | Action::NewFloatingPane { .. }
            | Action::NewInPlacePane { .. }
            | Action::NewStackedPane { .. }
            | Action::EditFile { .. }
            | Action::NewTiledPluginPane { .. }
            | Action::NewFloatingPluginPane { .. }
            | Action::NewInPlacePluginPane { .. }
    );
    // Creation completes when its pane is placed, however long startup takes.
    // Blocking panes retain their separate command-exit/unblock-condition wait.
    let mut wait_forever = creates_pane || creates_group;

    match action {
        Action::ToggleTab => {
            senders
                .send_to_screen(ScreenInstruction::ToggleTab(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::Write {
            key_with_modifier,
            bytes: raw_bytes,
            is_kitty_keyboard_protocol,
        } => {
            let completion = if is_client_input {
                NotificationEnd::new_client_input(completion_tx)
            } else {
                NotificationEnd::new(completion_tx)
            };
            senders
                .send_to_screen(ScreenInstruction::ClearScroll(None, client_id))
                .with_context(err_context)?;
            senders
                .send_to_screen(ScreenInstruction::WriteCharacter(
                    key_with_modifier,
                    raw_bytes,
                    is_kitty_keyboard_protocol,
                    client_id,
                    Some(completion),
                ))
                .with_context(err_context)?;
        },
        Action::WriteChars { chars } => {
            let completion = if is_client_input {
                NotificationEnd::new_client_input(completion_tx)
            } else {
                NotificationEnd::new(completion_tx)
            };
            senders
                .send_to_screen(ScreenInstruction::ClearScroll(None, client_id))
                .with_context(err_context)?;
            let chars = chars.into_bytes();
            senders
                .send_to_screen(ScreenInstruction::WriteCharacter(
                    None,
                    chars,
                    false,
                    client_id,
                    Some(completion),
                ))
                .with_context(err_context)?;
        },
        Action::WriteToPaneId { bytes, pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::ClearScroll(Some(pane_id), client_id))
                .with_context(err_context)?;
            senders
                .send_to_screen(ScreenInstruction::WriteToPaneId(
                    bytes,
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::WriteCharsToPaneId { chars, pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::ClearScroll(Some(pane_id), client_id))
                .with_context(err_context)?;
            let bytes = chars.into_bytes();
            senders
                .send_to_screen(ScreenInstruction::WriteToPaneId(
                    bytes,
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::Paste { chars, pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::ClearScroll(pane_id, client_id))
                .with_context(err_context)?;
            let bytes = chars.into_bytes();
            senders
                .send_to_screen(ScreenInstruction::Paste(
                    bytes,
                    pane_id,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::Reconfigure {
            config,
            write_to_disk,
        } => {
            // Disk flushes and plugin startup can outlast the ordinary action timeout.
            wait_forever = true;
            senders
                .send_to_server(ServerInstruction::ReconfigureAllClients {
                    config,
                    write_config_to_disk: write_to_disk,
                    completion: Some(NotificationEnd::new(completion_tx)),
                })
                .with_context(err_context)?;
        },
        Action::SetPaneAgent { pane_id, agent } => {
            senders
                .send_to_screen(ScreenInstruction::SetPaneAgent(
                    pane_id,
                    agent,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::AgentEvent(event) => {
            // To the plugin thread as an ordinary event: subscribers get it through the same
            // permission check as every other piece of session state, and the CLI event
            // stream sees it there. Nothing waits on it - a hook has seconds to live and
            // must never stall the turn it fired in.
            senders
                .send_to_plugin(PluginInstruction::Update(vec![(
                    None,
                    None,
                    Event::AgentEvent(event),
                )]))
                .with_context(err_context)?;
            drop(completion_tx);
        },
        Action::SetPaneColor { pane_id, fg, bg } => {
            senders
                .send_to_screen(ScreenInstruction::SetPaneColor(
                    pane_id,
                    fg,
                    bg,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::SwitchToMode { input_mode } => {
            senders
                .send_to_server(ServerInstruction::ChangeMode(
                    client_id,
                    input_mode,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::Resize { resize, direction } => {
            let screen_instr = ScreenInstruction::Resize(
                client_id,
                ResizeStrategy::new(resize, direction),
                Some(NotificationEnd::new(completion_tx)),
            );
            senders
                .send_to_screen(screen_instr)
                .with_context(err_context)?;
        },
        Action::SwitchFocus => {
            senders
                .send_to_screen(ScreenInstruction::SwitchFocus(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::FocusNextPane => {
            senders
                .send_to_screen(ScreenInstruction::FocusNextPane(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::FocusPreviousPane => {
            senders
                .send_to_screen(ScreenInstruction::FocusPreviousPane(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::FocusPaneByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::FocusPaneWithId(
                    pane_id,
                    true,  // should_float_if_hidden
                    false, // should_be_in_place_if_hidden
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::FocusPaneTabs => {
            senders
                .send_to_screen(ScreenInstruction::PaneTabsAction(
                    0,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::PreviousPaneTab => {
            senders
                .send_to_screen(ScreenInstruction::PaneTabsAction(
                    -1,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::NextPaneTab => {
            senders
                .send_to_screen(ScreenInstruction::PaneTabsAction(
                    1,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::FocusLastPane => {
            senders
                .send_to_screen(ScreenInstruction::FocusLastPane(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::MoveFocus { direction } => {
            let notification_end = Some(NotificationEnd::new(completion_tx));

            let screen_instr = match direction {
                Direction::Left => ScreenInstruction::MoveFocusLeft(client_id, notification_end),
                Direction::Right => ScreenInstruction::MoveFocusRight(client_id, notification_end),
                Direction::Up => ScreenInstruction::MoveFocusUp(client_id, notification_end),
                Direction::Down => ScreenInstruction::MoveFocusDown(client_id, notification_end),
            };
            senders
                .send_to_screen(screen_instr)
                .with_context(err_context)?;
        },
        Action::MoveFocusOrTab { direction } => {
            let notification_end = Some(NotificationEnd::new(completion_tx));

            let screen_instr = match direction {
                Direction::Left => {
                    ScreenInstruction::MoveFocusLeftOrPreviousTab(client_id, notification_end)
                },
                Direction::Right => {
                    ScreenInstruction::MoveFocusRightOrNextTab(client_id, notification_end)
                },
                Direction::Up => ScreenInstruction::SwitchTabNext(client_id, notification_end),
                Direction::Down => ScreenInstruction::SwitchTabPrev(client_id, notification_end),
            };
            senders
                .send_to_screen(screen_instr)
                .with_context(err_context)?;
        },
        Action::MovePane { direction } => {
            let notification_end = Some(NotificationEnd::new(completion_tx));

            let screen_instr = match direction {
                Some(Direction::Left) => {
                    ScreenInstruction::MovePaneLeft(client_id, notification_end)
                },
                Some(Direction::Right) => {
                    ScreenInstruction::MovePaneRight(client_id, notification_end)
                },
                Some(Direction::Up) => ScreenInstruction::MovePaneUp(client_id, notification_end),
                Some(Direction::Down) => {
                    ScreenInstruction::MovePaneDown(client_id, notification_end)
                },
                None => ScreenInstruction::MovePane(client_id, notification_end),
            };
            senders
                .send_to_screen(screen_instr)
                .with_context(err_context)?;
        },
        Action::MovePaneBackwards => {
            senders
                .send_to_screen(ScreenInstruction::MovePaneBackwards(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ClearScreen => {
            senders
                .send_to_screen(ScreenInstruction::ClearScreen(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::DumpScreen {
            file_path,
            include_scrollback,
            pane_id,
            ansi,
        } => {
            senders
                .send_to_screen(ScreenInstruction::DumpScreen(
                    file_path,
                    client_id,
                    include_scrollback,
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                    cli_client_id,
                    ansi,
                ))
                .with_context(err_context)?;
        },
        Action::DumpLayout => {
            let default_shell = match default_shell {
                Some(TerminalAction::RunCommand(run_command)) => Some(run_command.command),
                _ => None,
            };
            senders
                .send_to_screen(ScreenInstruction::DumpLayout(
                    default_shell,
                    cli_client_id.unwrap_or(client_id), // we prefer the cli client here because
                    // this is a cli query and we want to print
                    // it there
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::SaveSession => {
            senders
                .send_to_screen(ScreenInstruction::SaveSession(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::EditScrollback { ansi } => {
            senders
                .send_to_screen(ScreenInstruction::EditScrollback(
                    client_id,
                    ansi,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },

        Action::ScrollUp => {
            senders
                .send_to_screen(ScreenInstruction::ScrollUp(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ScrollUpAt { position } => {
            senders
                .send_to_screen(ScreenInstruction::ScrollUpAt(
                    position,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ScrollDown => {
            senders
                .send_to_screen(ScreenInstruction::ScrollDown(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ScrollDownAt { position } => {
            senders
                .send_to_screen(ScreenInstruction::ScrollDownAt(
                    position,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ScrollToBottom => {
            senders
                .send_to_screen(ScreenInstruction::ScrollToBottom(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ScrollToTop => {
            senders
                .send_to_screen(ScreenInstruction::ScrollToTop(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ScrollToPreviousPrompt => {
            senders
                .send_to_screen(ScreenInstruction::ScrollToPreviousPrompt(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ScrollToNextPrompt => {
            senders
                .send_to_screen(ScreenInstruction::ScrollToNextPrompt(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::SelectCommandAtScrollPosition => {
            senders
                .send_to_screen(ScreenInstruction::SelectCommandAtScrollPosition(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::CopyLastCommandOutput => {
            senders
                .send_to_screen(ScreenInstruction::CopyLastCommandOutput(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::MoveTextSelection { direction } => {
            senders
                .send_to_screen(ScreenInstruction::MoveTextSelection(
                    direction,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::SwapTextSelectionEnds => {
            senders
                .send_to_screen(ScreenInstruction::SwapTextSelectionEnds(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::MoveNotificationSelection { direction } => {
            senders
                .send_to_screen(ScreenInstruction::MoveNotificationSelection(
                    direction,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ActivateNotification => {
            senders
                .send_to_screen(ScreenInstruction::ActivateNotification(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ActivateLastNotification => {
            senders
                .send_to_screen(ScreenInstruction::ActivateLastNotification(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ToggleNotificationDetail => {
            senders
                .send_to_screen(ScreenInstruction::ToggleNotificationDetail(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::PageScrollUp => {
            senders
                .send_to_screen(ScreenInstruction::PageScrollUp(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::PageScrollDown => {
            senders
                .send_to_screen(ScreenInstruction::PageScrollDown(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::HalfPageScrollUp => {
            senders
                .send_to_screen(ScreenInstruction::HalfPageScrollUp(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::HalfPageScrollDown => {
            senders
                .send_to_screen(ScreenInstruction::HalfPageScrollDown(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ToggleFocusFullscreen => {
            senders
                .send_to_screen(ScreenInstruction::ToggleActiveTerminalFullscreen(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ToggleFocusNoUiFullscreen => {
            senders
                .send_to_screen(ScreenInstruction::ToggleActiveTerminalNoUiFullscreen(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ToggleCommandPalette => {
            senders
                .send_to_screen(ScreenInstruction::ToggleCommandPalette(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::OpenAgents => {
            senders
                .send_to_screen(ScreenInstruction::OpenAgents(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::TogglePaneFrames => {
            senders
                .send_to_screen(ScreenInstruction::TogglePaneFrames(Some(
                    NotificationEnd::new(completion_tx),
                )))
                .with_context(err_context)?;
        },
        Action::SetPaneFrameStyle(pane_frame_style) => {
            senders
                .send_to_screen(ScreenInstruction::SetPaneFrameStyle(
                    pane_frame_style,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::NewPane {
            direction,
            pane_name,
            start_suppressed,
        } => {
            panes::new_pane(
                direction,
                pane_name,
                start_suppressed,
                &senders,
                &default_shell,
                client_id,
                completion_tx,
            )?;
        },
        Action::NewBlockingPane {
            placement,
            pane_name,
            command,
            unblock_condition,
            near_current_pane,
            no_focus,
            tab_id,
        } => {
            panes::new_blocking_pane(
                placement,
                pane_name,
                command,
                unblock_condition,
                near_current_pane,
                no_focus,
                tab_id,
                &senders,
                &default_shell,
                client_id,
                pane_id,
                completion_tx,
            )?;
            wait_forever = true;
        },
        Action::EditFile {
            payload: open_file_payload,
            direction: split_direction,
            floating: should_float,
            in_place: should_open_in_place,
            close_replaced_pane,
            start_suppressed,
            placement,
            near_current_pane,
            no_focus,
            tab_id,
        } => {
            panes::edit_file(
                open_file_payload,
                split_direction,
                should_float,
                should_open_in_place,
                close_replaced_pane,
                start_suppressed,
                placement,
                near_current_pane,
                no_focus,
                tab_id,
                &senders,
                client_id,
                pane_id,
                completion_tx,
            )?;
        },
        Action::SwitchModeForAllClients { input_mode } => {
            // ModeUpdate broadcast is handled by the screen thread via
            // change_mode_for_all_clients() -> change_mode() -> update_input_modes()
            senders
                .send_to_server(ServerInstruction::ChangeModeForAllClients(input_mode))
                .with_context(err_context)?;

            senders
                .send_to_screen(ScreenInstruction::ChangeModeForAllClients(
                    input_mode,
                    Some(default_mode),
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::NewFloatingPane {
            command: run_command,
            pane_name: name,
            placement,
            near_current_pane,
            no_focus,
            tab_id,
        } => {
            panes::new_floating_pane(
                run_command,
                name,
                placement,
                near_current_pane,
                no_focus,
                tab_id,
                &senders,
                &default_shell,
                client_id,
                pane_id,
                completion_tx,
            )?;
        },
        Action::NewInPlacePane {
            command: run_command,
            pane_name: name,
            near_current_pane,
            no_focus,
            pane_id_to_replace,
            close_replaced_pane,
            tab_id,
        } => {
            panes::new_in_place_pane(
                run_command,
                name,
                near_current_pane,
                no_focus,
                pane_id_to_replace,
                close_replaced_pane,
                tab_id,
                &senders,
                &default_shell,
                client_id,
                pane_id,
                completion_tx,
            )?;
        },
        Action::NewStackedPane {
            command: run_command,
            pane_name: name,
            near_current_pane,
            no_focus,
            tab_id,
        } => {
            panes::new_stacked_pane(
                run_command,
                name,
                near_current_pane,
                no_focus,
                tab_id,
                &senders,
                &default_shell,
                client_id,
                pane_id,
                completion_tx,
            )?;
        },
        Action::NewTiledPane {
            direction,
            command: run_command,
            pane_name: name,
            near_current_pane,
            no_focus,
            borderless,
            tab_id,
        } => {
            panes::new_tiled_pane(
                direction,
                run_command,
                name,
                near_current_pane,
                no_focus,
                borderless,
                tab_id,
                &senders,
                &default_shell,
                client_id,
                pane_id,
                completion_tx,
            )?;
        },
        Action::TogglePaneEmbedOrFloating => {
            senders
                .send_to_screen(ScreenInstruction::TogglePaneEmbedOrFloating(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ToggleFloatingPanes => {
            senders
                .send_to_screen(ScreenInstruction::ToggleFloatingPanes(
                    client_id,
                    default_shell.clone(),
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::PaneNameInput { input } => {
            senders
                .send_to_screen(ScreenInstruction::UpdatePaneName(
                    input,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::UndoRenamePane => {
            senders
                .send_to_screen(ScreenInstruction::UndoRenamePane(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::Run {
            command,
            near_current_pane,
            no_focus,
        } => {
            let run_cmd = Some(TerminalAction::RunCommand(command.clone().into()));
            let client_tab_index_or_paneid =
                new_pane_routing(no_focus, near_current_pane, None, pane_id, client_id);
            senders
                .send_to_pty(PtyInstruction::SpawnTerminal(
                    run_cmd,
                    None,
                    NewPanePlacement::Tiled {
                        direction: command.direction,
                        borderless: None,
                    },
                    false,
                    client_tab_index_or_paneid,
                    Some(NotificationEnd::new(completion_tx)),
                    false, // set_blocking
                ))
                .with_context(err_context)?;
        },
        Action::CloseFocus => {
            senders
                .send_to_screen(ScreenInstruction::CloseFocusedPane(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::NewTab {
            layout_source: _,
            tiled_layout: tab_layout,
            floating_layouts: floating_panes_layout,
            swap_tiled_layouts,
            swap_floating_layouts,
            tab_name,
            should_change_focus_to_new_tab,
            cwd,
            initial_panes,
            first_pane_unblock_condition,
        } => {
            let shell = default_shell.clone();
            // Construct completion_tx conditionally
            let (completion_tx, block_on_first_terminal) = if let Some(condition) =
                first_pane_unblock_condition
            {
                let notification = NotificationEnd::new_with_condition(completion_tx, condition);
                wait_forever = true;
                (notification, true)
            } else {
                (NotificationEnd::new(completion_tx), false)
            };

            senders
                .send_to_screen(ScreenInstruction::NewTab(
                    cwd,
                    shell,
                    tab_layout,
                    floating_panes_layout,
                    tab_name,
                    (swap_tiled_layouts, swap_floating_layouts),
                    initial_panes,
                    block_on_first_terminal,
                    should_change_focus_to_new_tab,
                    client_id,
                    Some(completion_tx),
                ))
                .with_context(err_context)?;
        },
        action @ (Action::NewTabGroup
        | Action::NewNamedTabGroup { .. }
        | Action::GoToTabGroupName { .. }
        | Action::RenameCurrentTabGroup { .. }
        | Action::CloseCurrentTabGroup
        | Action::ListTabGroups { .. }
        | Action::CurrentTabGroupInfo { .. }
        | Action::GoToNextTabGroup
        | Action::MovePaneToNextTabGroup
        | Action::MovePaneToPreviousTabGroup
        | Action::GoToPreviousTabGroup
        | Action::GoToTabInGroup { .. }
        | Action::SwitchTabGroup { .. }
        | Action::RenameTabGroup { .. }
        | Action::CloseTabGroup { .. }) => {
            senders
                .send_to_screen(ScreenInstruction::TabGroup(
                    action,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::GoToNextTab => {
            senders
                .send_to_screen(ScreenInstruction::SwitchTabNext(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::GoToPreviousTab => {
            senders
                .send_to_screen(ScreenInstruction::SwitchTabPrev(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ToggleActiveSyncTab => {
            senders
                .send_to_screen(ScreenInstruction::ToggleActiveSyncTab(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::CloseTab => {
            senders
                .send_to_screen(ScreenInstruction::CloseTab(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::GoToTab { index } => {
            senders
                .send_to_screen(ScreenInstruction::GoToTab(
                    index,
                    Some(client_id),
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::GoToTabName { name, create } => {
            let shell = default_shell.clone();
            senders
                .send_to_screen(ScreenInstruction::GoToTabName(
                    name,
                    shell,
                    create,
                    Some(client_id),
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::TabNameInput { input } => {
            senders
                .send_to_screen(ScreenInstruction::UpdateTabName(
                    input,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::UndoRenameTab => {
            senders
                .send_to_screen(ScreenInstruction::UndoRenameTab(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::GoToTabById { id } => {
            senders
                .send_to_screen(ScreenInstruction::GoToTabWithId(
                    id,
                    Some(client_id),
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::CloseTabById { id } => {
            senders
                .send_to_screen(ScreenInstruction::CloseTabWithId(
                    id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::RenameTabById { id, name } => {
            senders
                .send_to_screen(ScreenInstruction::RenameTabWithId(
                    id,
                    name.into_bytes(),
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::MoveTab { direction } => {
            let screen_instr = match direction {
                Direction::Left | Direction::Up => ScreenInstruction::MoveTabLeft(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ),
                Direction::Right | Direction::Down => ScreenInstruction::MoveTabRight(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ),
            };
            senders
                .send_to_screen(screen_instr)
                .with_context(err_context)?;
        },
        // A keybinding is client input with neither an originating pane nor a CLI client.
        // Plugin quits stay immediate even without a pane, including the confirmation plugin's.
        //
        // The modal itself is built on the screen thread rather than here: it carries whatever
        // the plugins asked to have said about a quit, and those notices live there.
        Action::Quit if is_client_input && pane_id.is_none() && cli_client_id.is_none() => {
            senders
                .send_to_screen(ScreenInstruction::ConfirmQuit(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::Quit => {
            senders
                .send_to_server(ServerInstruction::ClientExit(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
            should_break = true;
        },
        Action::Detach => {
            senders
                .send_to_server(ServerInstruction::DetachSession(
                    vec![client_id],
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
            should_break = true;
        },
        Action::SetDarkTheme => {
            senders
                .send_to_screen(ScreenInstruction::SetDarkTheme(Some(NotificationEnd::new(
                    completion_tx,
                ))))
                .with_context(err_context)?;
        },
        Action::SetLightTheme => {
            senders
                .send_to_screen(ScreenInstruction::SetLightTheme(Some(
                    NotificationEnd::new(completion_tx),
                )))
                .with_context(err_context)?;
        },
        Action::ToggleTheme => {
            senders
                .send_to_screen(ScreenInstruction::ToggleTheme(Some(NotificationEnd::new(
                    completion_tx,
                ))))
                .with_context(err_context)?;
        },
        Action::WarpPane {
            pane_id,
            to_session,
            placement,
        } => {
            drop(completion_tx);
            let (result, hold) = yinz_bus::warp::warp_pane_to_session(
                &senders,
                client_id,
                pane_id,
                &senders.session.current(),
                &to_session,
                placement,
                yinz_bus::warp::WARP_TIMEOUT,
            );
            match result {
                Ok(message) => {
                    send_output_to_client(cli_client_id, os_input.as_deref(), vec![message])
                },
                Err(message) => send_error_to_client(cli_client_id, os_input.as_deref(), &message),
            }
            // Only now may a source this warp emptied shut down: a session that
            // pulled the pane reads its goodbye as a failure.
            drop(hold);
        },
        Action::AdoptPane { payload, placement } => {
            drop(completion_tx);
            match yinz_bus::warp::adopt_pane_here(
                &senders,
                client_id,
                payload,
                placement,
                None,
                yinz_bus::warp::WARP_TIMEOUT,
            ) {
                Ok(message) => {
                    send_output_to_client(cli_client_id, os_input.as_deref(), vec![message])
                },
                Err(message) => send_error_to_client(cli_client_id, os_input.as_deref(), &message),
            }
        },
        Action::WarpTab { tab_id, to_session } => {
            drop(completion_tx);
            match yinz_bus::warp::warp_tab_to_session(
                &senders,
                client_id,
                tab_id,
                &senders.session.current(),
                &to_session,
                yinz_bus::warp::WARP_TIMEOUT,
            ) {
                Ok(message) => {
                    send_output_to_client(cli_client_id, os_input.as_deref(), vec![message])
                },
                Err(message) => send_error_to_client(cli_client_id, os_input.as_deref(), &message),
            }
        },
        Action::AdoptTab { payload } => {
            drop(completion_tx);
            match yinz_bus::warp::adopt_tab_here(
                &senders,
                client_id,
                payload,
                false,
                yinz_bus::warp::WARP_TIMEOUT,
            ) {
                Ok(message) => {
                    send_output_to_client(cli_client_id, os_input.as_deref(), vec![message])
                },
                Err(message) => send_error_to_client(cli_client_id, os_input.as_deref(), &message),
            }
        },
        Action::WarpTabFrom {
            from_session,
            tab_id,
        } => {
            drop(completion_tx);
            // Handed straight on to the source session, which does all of it —
            // including connecting back into this server to deliver the tab,
            // which our own route and pty threads then handle.
            match yinz_bus::warp::warp_tab_from_session(
                &senders.session.current(),
                &from_session,
                tab_id,
                yinz_bus::warp::WARP_PULL_TIMEOUT,
            ) {
                Ok(message) => {
                    send_output_to_client(cli_client_id, os_input.as_deref(), vec![message])
                },
                Err(message) => send_error_to_client(cli_client_id, os_input.as_deref(), &message),
            }
        },
        Action::SwitchSession {
            name,
            tab_position,
            pane_id,
            layout,
            cwd,
        } => {
            let current_session_name = senders.session.current();
            if name != current_session_name {
                let connect_to_session = SessionConnection {
                    name: Some(name.clone()),
                    tab_position,
                    pane_id,
                    layout: layout.clone(),
                    cwd: cwd.clone(),
                };
                senders
                    .send_to_server(ServerInstruction::SwitchSession(
                        connect_to_session,
                        client_id,
                        Some(NotificationEnd::new(completion_tx)),
                    ))
                    .with_context(err_context)?;
                should_break = true;
            } else {
                drop(completion_tx); // no need to wait, this is a no-op
            }
        },
        Action::MouseEvent { event } => {
            senders
                .send_to_screen(ScreenInstruction::MouseEvent(
                    event,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::Copy => {
            senders
                .send_to_screen(ScreenInstruction::Copy(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::Confirm => {
            // no-op, these are deprecated and should be removed when we upgrade the server/client
            // contract
            drop(completion_tx);
        },
        Action::Deny => {
            // no-op, these are deprecated and should be removed when we upgrade the server/client
            // contract
            drop(completion_tx);
        },
        Action::SkipConfirm { action } => {
            drop(completion_tx);
            if *action == Action::Quit {
                senders
                    .send_to_server(ServerInstruction::ClientExit(client_id, None))
                    .with_context(err_context)?;
                should_break = true;
            }
        },
        Action::NoOp => {
            drop(completion_tx);
        },
        Action::SearchInput { input } => {
            senders
                .send_to_screen(ScreenInstruction::UpdateSearch(
                    input,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::Search { direction } => {
            let notification_end = Some(NotificationEnd::new(completion_tx));

            let instruction = match direction {
                SearchDirection::Down => ScreenInstruction::SearchDown(client_id, notification_end),
                SearchDirection::Up => ScreenInstruction::SearchUp(client_id, notification_end),
            };
            senders
                .send_to_screen(instruction)
                .with_context(err_context)?;
        },
        Action::SearchToggleOption { option } => {
            let notification_end = Some(NotificationEnd::new(completion_tx));

            let instruction = match option {
                SearchOption::CaseSensitivity => {
                    ScreenInstruction::SearchToggleCaseSensitivity(client_id, notification_end)
                },
                SearchOption::WholeWord => {
                    ScreenInstruction::SearchToggleWholeWord(client_id, notification_end)
                },
                SearchOption::Wrap => {
                    ScreenInstruction::SearchToggleWrap(client_id, notification_end)
                },
            };
            senders
                .send_to_screen(instruction)
                .with_context(err_context)?;
        },
        Action::ToggleMouseMode => drop(completion_tx), // Handled client side
        Action::PreviousSwapLayout => {
            senders
                .send_to_screen(ScreenInstruction::PreviousSwapLayout(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::NextSwapLayout => {
            senders
                .send_to_screen(ScreenInstruction::NextSwapLayout(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::OverrideLayout {
            tabs,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            apply_only_to_active_tab,
        } => {
            let cwd = None;
            let shell = default_shell.clone();

            senders
                .send_to_screen(ScreenInstruction::OverrideLayout(
                    cwd,
                    shell,
                    tabs,
                    retain_existing_terminal_panes,
                    retain_existing_plugin_panes,
                    apply_only_to_active_tab,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::QueryTabNames => {
            senders
                .send_to_screen(ScreenInstruction::QueryTabNames(
                    cli_client_id.unwrap_or(client_id),
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::NewTiledPluginPane {
            plugin: run_plugin,
            pane_name: name,
            skip_cache,
            cwd,
            no_focus,
            tab_id,
        } => {
            senders
                .send_to_screen(ScreenInstruction::NewTiledPluginPane(
                    run_plugin,
                    name,
                    skip_cache,
                    cwd,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                    tab_id,
                    no_focus,
                ))
                .with_context(err_context)?;
        },
        Action::NewFloatingPluginPane {
            plugin: run_plugin,
            pane_name: name,
            skip_cache,
            cwd,
            placement,
            no_focus,
            tab_id,
        } => {
            senders
                .send_to_screen(ScreenInstruction::NewFloatingPluginPane(
                    run_plugin,
                    name,
                    skip_cache,
                    cwd,
                    placement,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                    tab_id,
                    no_focus,
                ))
                .with_context(err_context)?;
        },
        Action::NewInPlacePluginPane {
            plugin: run_plugin,
            pane_name: name,
            skip_cache,
            close_replaced_pane,
            no_focus,
            tab_id,
        } => {
            if let Some(pane_id) = pane_id {
                senders
                    .send_to_screen(ScreenInstruction::NewInPlacePluginPane(
                        run_plugin,
                        name,
                        pane_id,
                        skip_cache,
                        close_replaced_pane,
                        client_id,
                        Some(NotificationEnd::new(completion_tx)),
                        tab_id,
                        no_focus,
                    ))
                    .with_context(err_context)?;
            } else {
                log::error!("Must have pane_id in order to open in place pane");
                drop(completion_tx);
            }
        },
        Action::StartOrReloadPlugin { plugin: run_plugin } => {
            senders
                .send_to_screen(ScreenInstruction::StartOrReloadPluginPane(
                    run_plugin,
                    None,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::LaunchOrFocusPlugin {
            plugin: run_plugin,
            should_float,
            move_to_focused_tab,
            should_open_in_place,
            close_replaced_pane,
            skip_cache,
            tab_id,
        } => {
            senders
                .send_to_screen(ScreenInstruction::LaunchOrFocusPlugin(
                    run_plugin,
                    should_float,
                    move_to_focused_tab,
                    should_open_in_place,
                    close_replaced_pane,
                    pane_id,
                    skip_cache,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                    tab_id,
                ))
                .with_context(err_context)?;
        },
        Action::LaunchPlugin {
            plugin: run_plugin,
            should_float,
            should_open_in_place,
            close_replaced_pane,
            skip_cache,
            cwd,
            no_focus,
            tab_id,
        } => {
            senders
                .send_to_screen(ScreenInstruction::LaunchPlugin(
                    run_plugin,
                    should_float,
                    should_open_in_place,
                    close_replaced_pane,
                    pane_id,
                    skip_cache,
                    cwd,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                    tab_id,
                    no_focus,
                    None,
                ))
                .with_context(err_context)?;
        },
        Action::CloseTerminalPane {
            pane_id: terminal_pane_id,
        } => {
            senders
                .send_to_screen(ScreenInstruction::ClosePane(
                    PaneId::Terminal(terminal_pane_id),
                    None, // we send None here so that the terminal pane would be closed anywhere
                    // in the app, not just in the client's tab
                    Some(NotificationEnd::new(completion_tx)),
                    None,
                ))
                .with_context(err_context)?;
        },
        Action::ClosePluginPane {
            pane_id: plugin_pane_id,
        } => {
            senders
                .send_to_screen(ScreenInstruction::ClosePane(
                    PaneId::Plugin(plugin_pane_id),
                    None, // we send None here so that the terminal pane would be closed anywhere
                    // in the app, not just in the client's tab
                    Some(NotificationEnd::new(completion_tx)),
                    None,
                ))
                .with_context(err_context)?;
        },
        Action::FocusTerminalPaneWithId {
            pane_id,
            should_float_if_hidden,
            should_be_in_place_if_hidden,
        } => {
            senders
                .send_to_screen(ScreenInstruction::FocusPaneWithId(
                    PaneId::Terminal(pane_id),
                    should_float_if_hidden,
                    should_be_in_place_if_hidden,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::FocusPluginPaneWithId {
            pane_id,
            should_float_if_hidden,
            should_be_in_place_if_hidden,
        } => {
            senders
                .send_to_screen(ScreenInstruction::FocusPaneWithId(
                    PaneId::Plugin(pane_id),
                    should_float_if_hidden,
                    should_be_in_place_if_hidden,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::RenameTerminalPane {
            pane_id,
            name: name_bytes,
        } => {
            senders
                .send_to_screen(ScreenInstruction::RenamePane(
                    PaneId::Terminal(pane_id),
                    name_bytes,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::RenamePluginPane {
            pane_id,
            name: name_bytes,
        } => {
            senders
                .send_to_screen(ScreenInstruction::RenamePane(
                    PaneId::Plugin(pane_id),
                    name_bytes,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::RenameTab {
            tab_index: tab_position,
            name: name_bytes,
        } => {
            senders
                .send_to_screen(ScreenInstruction::RenameTab(
                    tab_position,
                    name_bytes,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ArrangePanesByProject => {
            senders
                .send_to_plugin(PluginInstruction::ArrangePanesByProject)
                .with_context(err_context)?;
            drop(completion_tx);
        },
        Action::BreakPane => {
            senders
                .send_to_screen(ScreenInstruction::BreakPane(
                    default_shell.clone(),
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::BreakPaneRight => {
            senders
                .send_to_screen(ScreenInstruction::BreakPaneRight(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::BreakPaneLeft => {
            senders
                .send_to_screen(ScreenInstruction::BreakPaneLeft(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::FocusHostSession => {
            senders
                .send_to_screen(ScreenInstruction::FocusHostSession(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::FocusGuestSession => {
            senders
                .send_to_screen(ScreenInstruction::FocusGuestSession(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ToggleHostFullscreen => {
            senders
                .send_to_screen(ScreenInstruction::ToggleHostFullscreen(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::RenameSession { name } => {
            senders
                .send_to_screen(ScreenInstruction::RenameSession(
                    name,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::Notify(text) => {
            let completion = Some(NotificationEnd::new(completion_tx));
            let instruction = match pane_id.filter(|_| !is_client_input) {
                Some(PaneId::Plugin(plugin_id)) => ScreenInstruction::PluginNotify {
                    text,
                    pane_id: None,
                    on_click: None,
                    plugin_id,
                    completion,
                },
                _ => ScreenInstruction::Notify(text, pane_id, None, completion),
            };
            senders
                .send_to_screen(instruction)
                .with_context(err_context)?;
        },
        Action::CliPipe {
            focus_plugin,
            pipe_id,
            mut name,
            payload,
            plugin,
            args,
            configuration,
            floating,
            in_place,
            skip_cache,
            cwd,
            pane_title,
            ..
        } => {
            drop(completion_tx); // releasing pipes is handled by the plugins, so we don't want
                                 // this to block additionallu
            let is_eof = if let Some(seen_cli_pipes) = seen_cli_pipes.as_mut() {
                let is_new = seen_cli_pipes.insert(pipe_id.clone());
                if is_new {
                    senders
                        .send_to_server(ServerInstruction::AssociatePipeWithClient {
                            pipe_id: pipe_id.clone(),
                            client_id: cli_client_id.unwrap_or(client_id),
                        })
                        .with_context(err_context)?;
                }
                !is_new && payload.is_none()
            } else {
                false
            };
            if let Some(name) = name.take() {
                let should_open_in_place = in_place.unwrap_or(false);
                if should_open_in_place && pane_id.is_none() {
                    log::error!("Was asked to open a new plugin in-place, but cannot identify the pane id... is the YINZ_PANE_ID variable set?");
                }
                let pane_id_to_replace = if should_open_in_place { pane_id } else { None };
                senders
                    .send_to_plugin(PluginInstruction::CliPipe {
                        focus_plugin,
                        pipe_id,
                        name,
                        payload,
                        is_eof,
                        plugin,
                        args,
                        configuration,
                        floating,
                        pane_id_to_replace,
                        cwd,
                        pane_title,
                        skip_cache,
                        cli_client_id: cli_client_id.unwrap_or(client_id),
                        source_pane_id: pane_id,
                    })
                    .with_context(err_context)?;
            } else {
                log::error!("Message must have a name");
            }
        },
        Action::KeybindPipe {
            mut name,
            payload,
            plugin,
            args,
            mut configuration,
            floating,
            in_place,
            skip_cache,
            cwd,
            pane_title,
            launch_new,
            plugin_id,
            ..
        } => {
            if let Some(name) = name.take() {
                let should_open_in_place = in_place.unwrap_or(false);
                let pane_id_to_replace = if should_open_in_place { pane_id } else { None };
                if launch_new && plugin_id.is_none() {
                    // we do this to make sure the plugin is unique (has a unique configuration parameter)
                    configuration
                        .get_or_insert_with(BTreeMap::new)
                        .insert("_yinz_id".to_owned(), Uuid::new_v4().to_string());
                }
                senders
                    .send_to_plugin(PluginInstruction::KeybindPipe {
                        name,
                        payload,
                        plugin,
                        args,
                        configuration,
                        floating,
                        should_open_in_place,
                        pane_id_to_replace,
                        cwd,
                        pane_title,
                        skip_cache,
                        cli_client_id: client_id,
                        plugin_and_client_id: plugin_id.map(|plugin_id| (plugin_id, client_id)),
                        notification_end: Some(NotificationEnd::new(completion_tx)),
                    })
                    .with_context(err_context)?;
            } else {
                log::error!("Message must have a name");
            }
        },
        Action::ListClients => {
            let default_shell = match default_shell {
                Some(TerminalAction::RunCommand(run_command)) => Some(run_command.command),
                _ => None,
            };
            senders
                .send_to_screen(ScreenInstruction::ListClientsMetadata(
                    default_shell,
                    cli_client_id.unwrap_or(client_id), // we prefer the cli client here because
                    // this is a cli query and we want to print
                    // it there
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ListPanes {
            show_tab,
            show_command,
            show_state,
            show_geometry,
            show_all,
            output_json,
        } => {
            queries::list_panes(
                show_tab,
                show_command,
                show_state,
                show_geometry,
                show_all,
                output_json,
                &senders,
                cli_client_id,
                os_input.as_deref(),
                client_id,
            )?;
            drop(NotificationEnd::new(completion_tx));
        },
        Action::ListTabs {
            show_state,
            show_dimensions,
            show_panes,
            show_layout,
            show_all,
            output_json,
        } => {
            queries::list_tabs(
                show_state,
                show_dimensions,
                show_panes,
                show_layout,
                show_all,
                output_json,
                &senders,
                client_id,
                cli_client_id,
                os_input.as_deref(),
            )?;
            drop(NotificationEnd::new(completion_tx));
        },
        Action::CurrentTabInfo { output_json } => {
            queries::current_tab_info(
                output_json,
                &senders,
                client_id,
                cli_client_id,
                os_input.as_deref(),
            )?;
            drop(NotificationEnd::new(completion_tx));
        },
        Action::TogglePanePinned => {
            senders
                .send_to_screen(ScreenInstruction::TogglePanePinned(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::StackPanes {
            pane_ids: pane_ids_to_stack,
        } => {
            senders
                .send_to_screen(ScreenInstruction::StackPanes(
                    pane_ids_to_stack,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ChangeFloatingPaneCoordinates {
            pane_id,
            coordinates,
        } => {
            senders
                .send_to_screen(ScreenInstruction::ChangeFloatingPanesCoordinates(
                    vec![(pane_id, coordinates)],
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::TogglePaneBorderless { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::TogglePaneBorderless(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::SetPaneBorderless {
            pane_id,
            borderless,
        } => {
            senders
                .send_to_screen(ScreenInstruction::SetPaneBorderless(
                    pane_id,
                    borderless,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::TogglePaneInGroup => {
            senders
                .send_to_screen(ScreenInstruction::TogglePaneInGroup(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ToggleGroupMarking => {
            senders
                .send_to_screen(ScreenInstruction::ToggleGroupMarking(
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ShowFloatingPanes { tab_id } => {
            senders
                .send_to_screen(ScreenInstruction::ShowFloatingPanes {
                    client_id,
                    tab_id,
                    completion: Some(NotificationEnd::new(completion_tx)),
                })
                .with_context(err_context)?;
        },
        Action::HideFloatingPanes { tab_id } => {
            senders
                .send_to_screen(ScreenInstruction::HideFloatingPanes {
                    client_id,
                    tab_id,
                    completion: Some(NotificationEnd::new(completion_tx)),
                })
                .with_context(err_context)?;
        },
        Action::AreFloatingPanesVisible { tab_id } => {
            senders
                .send_to_screen(ScreenInstruction::AreFloatingPanesVisible {
                    client_id,
                    tab_id,
                    completion: Some(NotificationEnd::new(completion_tx)),
                })
                .with_context(err_context)?;
        },
        // Pane-targeting CLI-only variants
        Action::ScrollUpByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::ScrollUpWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ScrollDownByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::ScrollDownWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ScrollToTopByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::ScrollToTopWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ScrollToBottomByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::ScrollToBottomWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::PageScrollUpByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::PageScrollUpWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::PageScrollDownByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::PageScrollDownWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::HalfPageScrollUpByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::HalfPageScrollUpWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::HalfPageScrollDownByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::HalfPageScrollDownWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ResizeByPaneId {
            pane_id,
            resize,
            direction,
        } => {
            let resize_strategy = ResizeStrategy::new(resize, direction);
            senders
                .send_to_screen(ScreenInstruction::ResizeWithPaneId(
                    pane_id,
                    resize_strategy,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::MovePaneByPaneId { pane_id, direction } => {
            senders
                .send_to_screen(ScreenInstruction::MovePaneWithPaneIdCli(
                    pane_id,
                    direction,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::MovePaneBackwardsByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::MovePaneBackwardsWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ClearScreenByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::ClearScreenWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::EditScrollbackByPaneId { pane_id, ansi } => {
            senders
                .send_to_screen(ScreenInstruction::EditScrollbackWithPaneId(
                    pane_id,
                    ansi,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ToggleFocusFullscreenByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::ToggleFullscreenWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ToggleFocusNoUiFullscreenByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::ToggleNoUiFullscreenWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::TogglePaneEmbedOrFloatingByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::TogglePaneEmbedOrFloatingWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::CloseFocusByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::CloseFocusWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::RenamePaneByPaneId { pane_id, name } => {
            let instruction = match pane_id {
                Some(pane_id) => ScreenInstruction::RenamePaneWithPaneId(
                    pane_id,
                    name,
                    Some(NotificationEnd::new(completion_tx)),
                ),
                None => ScreenInstruction::RenameActivePane(
                    name,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ),
            };
            senders
                .send_to_screen(instruction)
                .with_context(err_context)?;
        },
        Action::UndoRenamePaneByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::UndoRenamePaneWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::TogglePanePinnedByPaneId { pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::TogglePanePinnedWithPaneId(
                    pane_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        // Tab-targeting CLI-only variants
        Action::UndoRenameTabByTabId { id } => {
            senders
                .send_to_screen(ScreenInstruction::UndoRenameTabWithTabId(
                    id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ToggleActiveSyncTabByTabId { id } => {
            senders
                .send_to_screen(ScreenInstruction::ToggleActiveSyncTabWithTabId(
                    id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::ToggleFloatingPanesByTabId { id } => {
            senders
                .send_to_screen(ScreenInstruction::ToggleFloatingPanesWithTabId(
                    id,
                    default_shell.clone(),
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::PreviousSwapLayoutByTabId { id } => {
            senders
                .send_to_screen(ScreenInstruction::PreviousSwapLayoutWithTabId(
                    id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::NextSwapLayoutByTabId { id } => {
            senders
                .send_to_screen(ScreenInstruction::NextSwapLayoutWithTabId(
                    id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::MoveTabByTabId { id, direction } => {
            senders
                .send_to_screen(ScreenInstruction::MoveTabWithTabId(
                    id,
                    direction,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
    }
    let mut result = wait_for_action_completion(completion_rx, &action_name, wait_forever);
    completion::report_action_completion(
        &mut result,
        &action_name,
        creates_pane,
        creates_group,
        cli_client_id,
        os_input.as_deref(),
    );
    Ok((should_break, Some(result)))
}

#[cfg(test)]
#[path = "unit/pane_completion_tests.rs"]
mod pane_completion_tests;

#[cfg(test)]
#[path = "unit/configuration_completion_tests.rs"]
mod configuration_completion_tests;

#[cfg(test)]
#[path = "unit/input_barrier_tests.rs"]
mod input_barrier_tests;

#[cfg(test)]
#[path = "unit/notification_tests.rs"]
mod notification_tests;

#[cfg(test)]
#[path = "unit/routing_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "unit/quit_tests.rs"]
mod quit_tests;

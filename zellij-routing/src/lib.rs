#![allow(clippy::unnecessary_unwrap)]
#![allow(clippy::borrowed_box)]
//! Synchronous action dispatch over the thread bus, shared by the server and plugin host.
use std::collections::{BTreeMap, HashSet};
use tokio::sync::oneshot;
use zellij_bus::route::{ActionCompletionResult, NotificationEnd};
use zellij_types::internal_ids::TabId;

use std::time::Duration;
use uuid::Uuid;
use zellij_bus::global_async_runtime::get_tokio_runtime;
use zellij_bus::thread_bus::ThreadSenders;
use zellij_bus::{
    os_input_output::ServerOsApi,
    plugins::PluginInstruction,
    pty::{ClientTabIndexOrPaneId, PtyInstruction},
    screen::ScreenInstruction,
    PaneId, ServerInstruction,
};
use zellij_runtime::{envs, errors::prelude::*};
use zellij_types::geometry::{Direction, ResizeStrategy};
use {
    zellij_ipc::ipc::ExitReason, zellij_ipc::ipc::ServerToClientMsg, zellij_types::data::Event,
    zellij_types::data::InputMode, zellij_types::data::ListPanesResponse,
    zellij_types::data::ListTabsResponse, zellij_types::data::NewPanePlacement,
    zellij_types::data::PaneListEntry, zellij_types::data::SessionConnection,
    zellij_types::data::TabInfo, zellij_types::input::actions::Action,
    zellij_types::input::actions::SearchDirection, zellij_types::input::actions::SearchOption,
    zellij_types::input::command::TerminalAction,
};

use zellij_bus::ClientId;

const ACTION_COMPLETION_TIMEOUT: Duration = Duration::from_secs(1);

pub fn wait_for_action_completion(
    receiver: oneshot::Receiver<ActionCompletionResult>,
    action_name: &str,
    wait_forever: bool,
) -> ActionCompletionResult {
    let runtime = get_tokio_runtime();
    if wait_forever {
        runtime.block_on(async {
            match receiver.await {
                Ok(result) => result,
                Err(e) => {
                    log::error!("Failed to wait for action {}: {}", action_name, e);
                    ActionCompletionResult {
                        exit_status: None,
                        affected_pane_id: None,
                        affected_tab_id: None,
                        error_message: None,
                        stdout_message: None,
                    }
                },
            }
        })
    } else {
        match runtime
            .block_on(async { tokio::time::timeout(ACTION_COMPLETION_TIMEOUT, receiver).await })
        {
            Ok(Ok(result)) => result,
            // The sender was dropped rather than signalled. That is not a
            // failure and emphatically not a timeout: an action that does its
            // own waiting — every warp does, on a budget of its own — drops the
            // completion channel on purpose, because the thing worth waiting
            // for is not the one this channel reports. Logging it as a timeout
            // put an `Action WarpTab did not complete within 1s timeout` on top
            // of every warp, successful ones included, directly over the signal
            // anyone debugging a warp is looking for.
            Ok(Err(_)) => ActionCompletionResult {
                exit_status: None,
                affected_pane_id: None,
                affected_tab_id: None,
                error_message: None,
                stdout_message: None,
            },
            Err(_) => {
                log::error!(
                    "Action {} did not complete within {:?} timeout",
                    action_name,
                    ACTION_COMPLETION_TIMEOUT
                );
                ActionCompletionResult {
                    exit_status: None,
                    affected_pane_id: None,
                    affected_tab_id: None,
                    error_message: None,
                    stdout_message: None,
                }
            },
        }
    }
}

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

fn new_pane_routing(
    no_focus: bool,
    near_current_pane: bool,
    tab_id: Option<TabId>,
    pane_id: Option<PaneId>,
    client_id: ClientId,
) -> ClientTabIndexOrPaneId {
    if let Some(tab_id) = tab_id {
        if no_focus {
            ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id)
        } else {
            ClientTabIndexOrPaneId::TabIndex(tab_id)
        }
    } else if (no_focus || near_current_pane) && pane_id.is_some() {
        ClientTabIndexOrPaneId::PaneId(pane_id.unwrap())
    } else if no_focus {
        ClientTabIndexOrPaneId::ClientIdNoFocus(client_id)
    } else {
        ClientTabIndexOrPaneId::ClientId(client_id)
    }
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
    os_input: Option<Box<dyn ServerOsApi>>,
    leaves_clipboard_message: bool,
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
    // wait_for_action_completion call below (or timeout after 1 second) and release this thread,
    // allowing the client to produce another action without risking races
    let (completion_tx, completion_rx) = oneshot::channel();

    let mut wait_forever = false;

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
            senders
                .send_to_screen(ScreenInstruction::ClearScroll(client_id))
                .with_context(err_context)?;
            senders
                .send_to_screen(ScreenInstruction::WriteCharacter(
                    key_with_modifier,
                    raw_bytes,
                    is_kitty_keyboard_protocol,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::WriteChars { chars } => {
            senders
                .send_to_screen(ScreenInstruction::ClearScroll(client_id))
                .with_context(err_context)?;
            let chars = chars.into_bytes();
            senders
                .send_to_screen(ScreenInstruction::WriteCharacter(
                    None,
                    chars,
                    false,
                    client_id,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::WriteToPaneId { bytes, pane_id } => {
            senders
                .send_to_screen(ScreenInstruction::ClearScroll(client_id))
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
                .send_to_screen(ScreenInstruction::ClearScroll(client_id))
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
                .send_to_screen(ScreenInstruction::ClearScroll(client_id))
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
            let shell = default_shell.clone();
            let new_pane_placement = match direction {
                Some(direction) => NewPanePlacement::Tiled {
                    direction: Some(direction),
                    borderless: None,
                },
                None => NewPanePlacement::NoPreference { borderless: None },
            };
            senders
                .send_to_pty(PtyInstruction::SpawnTerminal(
                    shell,
                    pane_name,
                    new_pane_placement,
                    start_suppressed,
                    ClientTabIndexOrPaneId::ClientId(client_id),
                    Some(NotificationEnd::new(completion_tx)),
                    false, // set_blocking
                ))
                .with_context(err_context)?;
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
            let command = command
                .map(|cmd| TerminalAction::RunCommand(cmd.into()))
                .or_else(|| default_shell.clone());
            let set_pane_blocking = true;

            let notification_end = if let Some(condition) = unblock_condition {
                Some(NotificationEnd::new_with_condition(
                    completion_tx,
                    condition,
                ))
            } else {
                Some(NotificationEnd::new(completion_tx))
            };

            // we prefer the pane id provided by the action explicitly over the one that originated
            // it (this might be a bit misleading with "near_current_pane", but it's still the
            // right behavior - in the latter case, if the originator does not wish for this
            // behavior, they should not provide pane
            // inside the placement, but rather have the current pane id be picked up instead)
            let pane_id = match placement {
                NewPanePlacement::Stacked {
                    pane_id_to_stack_under,
                    ..
                } => pane_id_to_stack_under.or(pane_id),
                NewPanePlacement::InPlace {
                    pane_id_to_replace, ..
                } => pane_id_to_replace.or(pane_id),
                _ => pane_id,
            };

            let client_tab_index_or_paneid =
                new_pane_routing(no_focus, near_current_pane, tab_id, pane_id, client_id);
            senders
                .send_to_pty(PtyInstruction::SpawnTerminal(
                    command,
                    pane_name,
                    placement,
                    false,
                    client_tab_index_or_paneid,
                    notification_end,
                    set_pane_blocking,
                ))
                .with_context(err_context)?;
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
            let title = format!("Editing: {}", open_file_payload.path.display());
            let open_file = TerminalAction::OpenFile(open_file_payload);
            let pty_instr = if should_open_in_place {
                let client_tab_index_or_paneid =
                    new_pane_routing(no_focus, near_current_pane, tab_id, pane_id, client_id);
                PtyInstruction::SpawnInPlaceTerminal(
                    Some(open_file),
                    Some(title),
                    close_replaced_pane,
                    client_tab_index_or_paneid,
                    Some(NotificationEnd::new(completion_tx)),
                )
            } else {
                let client_tab_index_or_paneid =
                    new_pane_routing(no_focus, near_current_pane, tab_id, pane_id, client_id);
                PtyInstruction::SpawnTerminal(
                    Some(open_file),
                    Some(title),
                    if should_float {
                        placement
                    } else {
                        NewPanePlacement::Tiled {
                            direction: split_direction,
                            borderless: None,
                        }
                    },
                    start_suppressed,
                    client_tab_index_or_paneid,
                    Some(NotificationEnd::new(completion_tx)),
                    false, // set_blocking
                )
            };
            senders.send_to_pty(pty_instr).with_context(err_context)?;
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
            let run_cmd = run_command
                .map(|cmd| TerminalAction::RunCommand(cmd.into()))
                .or_else(|| default_shell.clone());
            let client_tab_index_or_paneid =
                new_pane_routing(no_focus, near_current_pane, tab_id, pane_id, client_id);
            senders
                .send_to_pty(PtyInstruction::SpawnTerminal(
                    run_cmd,
                    name,
                    placement,
                    false,
                    client_tab_index_or_paneid,
                    Some(NotificationEnd::new(completion_tx)),
                    false, // set_blocking
                ))
                .with_context(err_context)?;
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
            let run_cmd = run_command
                .map(|cmd| TerminalAction::RunCommand(cmd.into()))
                .or_else(|| default_shell.clone());
            let explicit_pane_id_to_replace: Option<PaneId> = pane_id_to_replace;
            let pane_id = explicit_pane_id_to_replace.or(pane_id);
            let client_tab_index_or_paneid = if let Some(tab_id) = tab_id {
                if no_focus {
                    ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id)
                } else {
                    ClientTabIndexOrPaneId::TabIndex(tab_id)
                }
            } else if let Some(pane_id) = pane_id
                .filter(|_| explicit_pane_id_to_replace.is_some() || near_current_pane || no_focus)
            {
                ClientTabIndexOrPaneId::PaneId(pane_id)
            } else if no_focus {
                ClientTabIndexOrPaneId::ClientIdNoFocus(client_id)
            } else {
                ClientTabIndexOrPaneId::ClientId(client_id)
            };
            senders
                .send_to_pty(PtyInstruction::SpawnInPlaceTerminal(
                    run_cmd,
                    name,
                    close_replaced_pane,
                    client_tab_index_or_paneid,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::NewStackedPane {
            command: run_command,
            pane_name: name,
            near_current_pane,
            no_focus,
            tab_id,
        } => {
            let run_cmd = run_command
                .map(|cmd| TerminalAction::RunCommand(cmd.into()))
                .or_else(|| default_shell.clone());

            let (pane_placement, client_tab_index_or_paneid) = if let Some(tab_id) = tab_id {
                (
                    NewPanePlacement::Stacked {
                        pane_id_to_stack_under: None,
                        borderless: None,
                    },
                    if no_focus {
                        ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id)
                    } else {
                        ClientTabIndexOrPaneId::TabIndex(tab_id)
                    },
                )
            } else if (no_focus || near_current_pane) && pane_id.is_some() {
                let pane_id = pane_id.unwrap();
                (
                    NewPanePlacement::Stacked {
                        pane_id_to_stack_under: Some(pane_id),
                        borderless: None,
                    },
                    ClientTabIndexOrPaneId::PaneId(pane_id),
                )
            } else {
                (
                    NewPanePlacement::Stacked {
                        pane_id_to_stack_under: None,
                        borderless: None,
                    },
                    if no_focus {
                        ClientTabIndexOrPaneId::ClientIdNoFocus(client_id)
                    } else {
                        ClientTabIndexOrPaneId::ClientId(client_id)
                    },
                )
            };
            senders
                .send_to_pty(PtyInstruction::SpawnTerminal(
                    run_cmd,
                    name,
                    pane_placement,
                    false,
                    client_tab_index_or_paneid,
                    Some(NotificationEnd::new(completion_tx)),
                    false, // set_blocking
                ))
                .with_context(err_context)?;
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
            let run_cmd = run_command
                .map(|cmd| TerminalAction::RunCommand(cmd.into()))
                .or_else(|| default_shell.clone());
            let client_tab_index_or_paneid =
                new_pane_routing(no_focus, near_current_pane, tab_id, pane_id, client_id);
            senders
                .send_to_pty(PtyInstruction::SpawnTerminal(
                    run_cmd,
                    name,
                    NewPanePlacement::Tiled {
                        direction,
                        borderless,
                    },
                    false,
                    client_tab_index_or_paneid,
                    Some(NotificationEnd::new(completion_tx)),
                    false, // set_blocking
                ))
                .with_context(err_context)?;
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
        // A keybinding has neither an originating pane nor a CLI client. Direct CLI and plugin
        // quits stay immediate, and the confirmation plugin's own quit cannot reopen itself.
        //
        // The modal itself is built on the screen thread rather than here: it carries whatever
        // the plugins asked to have said about a quit, and those notices live there.
        Action::Quit if pane_id.is_none() && cli_client_id.is_none() => {
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
            match zellij_bus::warp::warp_pane_to_session(
                &senders,
                client_id,
                pane_id,
                &zellij_bus::warp::current_session_name(),
                &to_session,
                placement,
                zellij_bus::warp::WARP_TIMEOUT,
            ) {
                Ok(message) => {
                    send_output_to_client(cli_client_id, os_input.as_ref(), vec![message])
                },
                Err(message) => send_error_to_client(cli_client_id, os_input.as_ref(), &message),
            }
        },
        Action::AdoptPane { payload, placement } => {
            drop(completion_tx);
            match zellij_bus::warp::adopt_pane_here(
                &senders,
                client_id,
                payload,
                placement,
                zellij_bus::warp::WARP_TIMEOUT,
            ) {
                Ok(message) => {
                    send_output_to_client(cli_client_id, os_input.as_ref(), vec![message])
                },
                Err(message) => send_error_to_client(cli_client_id, os_input.as_ref(), &message),
            }
        },
        Action::WarpTab { tab_id, to_session } => {
            drop(completion_tx);
            match zellij_bus::warp::warp_tab_to_session(
                &senders,
                client_id,
                tab_id,
                &zellij_bus::warp::current_session_name(),
                &to_session,
                zellij_bus::warp::WARP_TIMEOUT,
            ) {
                Ok(message) => {
                    send_output_to_client(cli_client_id, os_input.as_ref(), vec![message])
                },
                Err(message) => send_error_to_client(cli_client_id, os_input.as_ref(), &message),
            }
        },
        Action::AdoptTab { payload } => {
            drop(completion_tx);
            match zellij_bus::warp::adopt_tab_here(
                &senders,
                client_id,
                payload,
                zellij_bus::warp::WARP_TIMEOUT,
            ) {
                Ok(message) => {
                    send_output_to_client(cli_client_id, os_input.as_ref(), vec![message])
                },
                Err(message) => send_error_to_client(cli_client_id, os_input.as_ref(), &message),
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
            match zellij_bus::warp::warp_tab_from_session(
                &zellij_bus::warp::current_session_name(),
                &from_session,
                tab_id,
                zellij_bus::warp::WARP_PULL_TIMEOUT,
            ) {
                Ok(message) => {
                    send_output_to_client(cli_client_id, os_input.as_ref(), vec![message])
                },
                Err(message) => send_error_to_client(cli_client_id, os_input.as_ref(), &message),
            }
        },
        Action::SwitchSession {
            name,
            tab_position,
            pane_id,
            layout,
            cwd,
        } => {
            let current_session_name =
                envs::get_session_name().unwrap_or_else(|_| String::new().into());
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
        },
        Action::Deny => {
            // no-op, these are deprecated and should be removed when we upgrade the server/client
            // contract
        },
        #[allow(clippy::single_match)]
        Action::SkipConfirm { action } => match *action {
            Action::Quit => {
                drop(completion_tx);
                senders
                    .send_to_server(ServerInstruction::ClientExit(client_id, None))
                    .with_context(err_context)?;
                should_break = true;
            },
            _ => {},
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
        Action::ToggleMouseMode => {}, // Handled client side
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
            }
        },
        Action::StartOrReloadPlugin { plugin: run_plugin } => {
            senders
                .send_to_screen(ScreenInstruction::StartOrReloadPluginPane(
                    run_plugin,
                    None,
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
            senders
                .send_to_screen(ScreenInstruction::Notify(
                    text,
                    None,
                    None,
                    Some(NotificationEnd::new(completion_tx)),
                ))
                .with_context(err_context)?;
        },
        Action::CliPipe {
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
            if let Some(seen_cli_pipes) = seen_cli_pipes.as_mut() {
                if !seen_cli_pipes.contains(&pipe_id) {
                    seen_cli_pipes.insert(pipe_id.clone());
                    senders
                        .send_to_server(ServerInstruction::AssociatePipeWithClient {
                            pipe_id: pipe_id.clone(),
                            client_id: cli_client_id.unwrap_or(client_id),
                        })
                        .with_context(err_context)?;
                }
            }
            if let Some(name) = name.take() {
                let should_open_in_place = in_place.unwrap_or(false);
                if should_open_in_place && pane_id.is_none() {
                    log::error!("Was asked to open a new plugin in-place, but cannot identify the pane id... is the ZELLIJ_PANE_ID variable set?");
                }
                let pane_id_to_replace = if should_open_in_place { pane_id } else { None };
                senders
                    .send_to_plugin(PluginInstruction::CliPipe {
                        pipe_id,
                        name,
                        payload,
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
                        .insert("_zellij_id".to_owned(), Uuid::new_v4().to_string());
                }
                senders
                    .send_to_plugin(PluginInstruction::KeybindPipe {
                        name,
                        payload,
                        plugin,
                        args,
                        configuration,
                        floating,
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
            let maybe_panes =
                request_panes_from_screen(&senders, show_all).with_context(err_context)?;

            if let Some(mut pane_entries) = maybe_panes {
                if show_command || show_all || output_json {
                    enrich_panes_with_pty_data(&mut pane_entries, &senders)
                        .with_context(err_context)?;
                }

                let output_lines = if output_json {
                    format_panes_as_json(&pane_entries)
                } else {
                    format_panes_table(
                        &pane_entries,
                        show_tab || show_all,
                        show_command || show_all,
                        show_state || show_all,
                        show_geometry || show_all,
                    )
                };

                send_output_to_client(cli_client_id, os_input.as_ref(), output_lines);
            } else {
                send_error_to_client(cli_client_id, os_input.as_ref(), "Timeout listing panes");
            }
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
            let maybe_tabs =
                request_tabs_from_screen(&senders, client_id).with_context(err_context)?;

            if let Some(tab_infos) = maybe_tabs {
                let output_lines = if output_json {
                    format_tabs_as_json(&tab_infos)
                } else {
                    format_tabs_table(
                        &tab_infos,
                        show_state || show_all,
                        show_dimensions || show_all,
                        show_panes || show_all,
                        show_layout || show_all,
                    )
                };

                send_output_to_client(cli_client_id, os_input.as_ref(), output_lines);
            } else {
                send_error_to_client(cli_client_id, os_input.as_ref(), "Timeout listing tabs");
            }
            drop(NotificationEnd::new(completion_tx));
        },
        Action::CurrentTabInfo { output_json } => {
            let maybe_tab_info = request_current_tab_info_from_screen(&senders, client_id)
                .with_context(err_context)?;

            match maybe_tab_info {
                Some(tab_info) => {
                    let output_lines = if output_json {
                        format_current_tab_info_as_json(&tab_info)
                    } else {
                        format_current_tab_info_plain(&tab_info)
                    };
                    send_output_to_client(cli_client_id, os_input.as_ref(), output_lines);
                },
                None => {
                    send_error_to_client(
                        cli_client_id,
                        os_input.as_ref(),
                        "No active tab found for current client",
                    );
                },
            }
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
    let result = wait_for_action_completion(completion_rx, &action_name, wait_forever);
    if let Some(error_message) = &result.error_message {
        if let Some(cli_client_id) = cli_client_id {
            if let Some(ref os_input) = os_input {
                let _ = os_input.send_to_client(
                    cli_client_id,
                    ServerToClientMsg::LogError {
                        lines: vec![error_message.clone()],
                    },
                );
            }
        }
    } else if let Some(stdout_message) = &result.stdout_message {
        if let Some(cli_client_id) = cli_client_id {
            if let Some(ref os_input) = os_input {
                let _ = os_input.send_to_client(
                    cli_client_id,
                    ServerToClientMsg::Log {
                        lines: vec![stdout_message.clone()],
                    },
                );
            }
        }
    }
    // Return tab ID to CLI clients as plain text
    if let Some(tab_id) = result.affected_tab_id {
        if let Some(cli_client_id) = cli_client_id {
            if let Some(ref os_input) = os_input {
                let _ = os_input.send_to_client(
                    cli_client_id,
                    ServerToClientMsg::Log {
                        lines: vec![tab_id.to_string()],
                    },
                );
            }
        }
    }
    // Return pane ID to CLI clients as plain text
    if let Some(pane_id) = result.affected_pane_id {
        if let Some(cli_client_id) = cli_client_id {
            if let Some(ref os_input) = os_input {
                let _ = os_input.send_to_client(
                    cli_client_id,
                    ServerToClientMsg::Log {
                        lines: vec![pane_id.to_string()],
                    },
                );
            }
        }
    }
    // A blocking pane's exit status, last: the client stops reading at `Exit`, so the ids above
    // have to be on the wire before it. Not sent alongside an error or a stdout message, which
    // already ended the client's wait.
    if result.error_message.is_none() && result.stdout_message.is_none() {
        if let Some(exit_status) = result.exit_status {
            if let Some(cli_client_id) = cli_client_id {
                if let Some(ref os_input) = os_input {
                    let _ = os_input.send_to_client(
                        cli_client_id,
                        ServerToClientMsg::Exit {
                            exit_reason: ExitReason::CustomExitStatus(exit_status),
                        },
                    );
                }
            }
        }
    }
    Ok((should_break, Some(result)))
}

// this should only be used for one-off startup instructions

fn request_panes_from_screen(
    senders: &ThreadSenders,
    show_all: bool,
) -> Result<Option<ListPanesResponse>> {
    use crossbeam::channel::{unbounded, RecvTimeoutError};
    use std::time::Duration;

    let (response_sender, response_receiver) = unbounded();
    senders.send_to_screen(ScreenInstruction::ListPanes {
        show_all,
        response_channel: response_sender,
    })?;

    match response_receiver.recv_timeout(Duration::from_secs(1)) {
        Ok(entries) => Ok(Some(entries)),
        Err(RecvTimeoutError::Timeout) => {
            log::error!("ListPanes timed out waiting for Screen response");
            Ok(None)
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!("ListPanes channel disconnected");
            Ok(None)
        },
    }
}

fn request_tabs_from_screen(
    senders: &ThreadSenders,
    client_id: ClientId,
) -> Result<Option<ListTabsResponse>> {
    use crossbeam::channel::{unbounded, RecvTimeoutError};
    use std::time::Duration;

    let (response_sender, response_receiver) = unbounded();
    senders.send_to_screen(ScreenInstruction::ListTabs {
        client_id,
        response_channel: response_sender,
    })?;

    match response_receiver.recv_timeout(Duration::from_secs(1)) {
        Ok(entries) => Ok(Some(entries)),
        Err(RecvTimeoutError::Timeout) => {
            log::error!("ListTabs timed out waiting for Screen response");
            Ok(None)
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!("ListTabs channel disconnected");
            Ok(None)
        },
    }
}

fn request_current_tab_info_from_screen(
    senders: &ThreadSenders,
    client_id: ClientId,
) -> Result<Option<TabInfo>> {
    use crossbeam::channel::{unbounded, RecvTimeoutError};
    use std::time::Duration;

    let (response_sender, response_receiver) = unbounded();
    senders.send_to_screen(ScreenInstruction::GetCurrentTabInfo {
        client_id,
        response_channel: response_sender,
    })?;

    match response_receiver.recv_timeout(Duration::from_secs(1)) {
        Ok(tab_info_opt) => Ok(tab_info_opt),
        Err(RecvTimeoutError::Timeout) => {
            log::error!("GetCurrentTabInfo timed out waiting for Screen response");
            Ok(None)
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!("GetCurrentTabInfo channel disconnected");
            Ok(None)
        },
    }
}

fn enrich_panes_with_pty_data(
    pane_entries: &mut [PaneListEntry],
    senders: &ThreadSenders,
) -> Result<()> {
    for entry in pane_entries.iter_mut() {
        if !entry.pane_info.is_plugin() {
            let pane_id = entry.pane_info.id;
            enrich_pane_with_running_command(entry, pane_id, senders)?;
            enrich_pane_with_cwd(entry, pane_id, senders)?;
        }
    }
    Ok(())
}

fn enrich_pane_with_running_command(
    entry: &mut PaneListEntry,
    pane_id: PaneId,
    senders: &ThreadSenders,
) -> Result<()> {
    use crossbeam::channel::unbounded;
    use std::time::Duration;
    use zellij_types::data::GetPaneRunningCommandResponse;

    let (cmd_sender, cmd_receiver) = unbounded();
    senders.send_to_pty(PtyInstruction::GetPaneRunningCommand {
        pane_id,
        response_channel: cmd_sender,
    })?;

    if let Ok(GetPaneRunningCommandResponse::Ok(command_vec)) =
        cmd_receiver.recv_timeout(Duration::from_millis(100))
    {
        entry.pane_command = Some(command_vec.join(" "));
    }

    Ok(())
}

fn enrich_pane_with_cwd(
    entry: &mut PaneListEntry,
    pane_id: PaneId,
    senders: &ThreadSenders,
) -> Result<()> {
    use crossbeam::channel::unbounded;
    use std::time::Duration;
    use zellij_types::data::GetPaneCwdResponse;

    let (cwd_sender, cwd_receiver) = unbounded();
    senders.send_to_pty(PtyInstruction::GetPaneCwd {
        pane_id,
        response_channel: cwd_sender,
    })?;

    if let Ok(GetPaneCwdResponse::Ok(cwd)) = cwd_receiver.recv_timeout(Duration::from_millis(100)) {
        entry.pane_cwd = Some(cwd.to_string_lossy().to_string());
    }

    Ok(())
}

fn format_panes_as_json(pane_entries: &[PaneListEntry]) -> Vec<String> {
    vec![serde_json::to_string_pretty(pane_entries).unwrap_or_else(|_| "[]".to_string())]
}

fn format_panes_table(
    entries: &[PaneListEntry],
    show_tab: bool,
    show_command: bool,
    show_state: bool,
    show_geometry: bool,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(build_table_header(
        show_tab,
        show_command,
        show_state,
        show_geometry,
    ));

    for entry in entries {
        lines.push(build_table_row(
            entry,
            show_tab,
            show_command,
            show_state,
            show_geometry,
        ));
    }

    lines
}

fn build_table_header(
    show_tab: bool,
    show_command: bool,
    show_state: bool,
    show_geometry: bool,
) -> String {
    let mut header = Vec::new();

    if show_tab {
        header.push("TAB_ID");
        header.push("TAB_POS");
        header.push("TAB_NAME");
    }

    header.push("PANE_ID");
    header.push("TYPE");
    header.push("TITLE");

    if show_command {
        header.push("COMMAND");
        header.push("CWD");
    }

    if show_state {
        header.push("FOCUSED");
        header.push("FLOATING");
        header.push("EXITED");
    }

    if show_geometry {
        header.push("X");
        header.push("Y");
        header.push("ROWS");
        header.push("COLS");
    }

    header.join("  ")
}

fn build_table_row(
    entry: &PaneListEntry,
    show_tab: bool,
    show_command: bool,
    show_state: bool,
    show_geometry: bool,
) -> String {
    let mut row = Vec::new();

    if show_tab {
        row.push(entry.tab_id.to_string());
        row.push(entry.tab_position.to_string());
        row.push(entry.tab_name.clone());
    }

    row.push(format_pane_id(&entry.pane_info));
    row.push(format_pane_type(&entry.pane_info));
    row.push(entry.pane_info.title.clone());

    if show_command {
        row.push(extract_command(entry));
        row.push(extract_cwd(entry));
    }

    if show_state {
        row.push(entry.pane_info.is_focused.to_string());
        row.push(entry.pane_info.is_floating.to_string());
        row.push(entry.pane_info.exited.to_string());
    }

    if show_geometry {
        row.push(entry.pane_info.pane_x.to_string());
        row.push(entry.pane_info.pane_y.to_string());
        row.push(entry.pane_info.pane_rows.to_string());
        row.push(entry.pane_info.pane_columns.to_string());
    }

    row.join("  ")
}

fn format_pane_id(pane_info: &zellij_types::data::PaneInfo) -> String {
    pane_info.id.to_string()
}

fn format_pane_type(pane_info: &zellij_types::data::PaneInfo) -> String {
    if pane_info.is_plugin() {
        "plugin".to_string()
    } else {
        "terminal".to_string()
    }
}

fn extract_command(entry: &PaneListEntry) -> String {
    entry
        .pane_command
        .as_ref()
        .or(entry.pane_info.terminal_command.as_ref())
        .or(entry.pane_info.plugin_url.as_ref())
        .map(|s| s.as_str())
        .unwrap_or("-")
        .to_string()
}

fn extract_cwd(entry: &PaneListEntry) -> String {
    entry.pane_cwd.as_deref().unwrap_or("-").to_string()
}

fn format_tabs_as_json(tab_infos: &[TabInfo]) -> Vec<String> {
    vec![serde_json::to_string_pretty(tab_infos).unwrap_or_else(|_| "[]".to_string())]
}

fn format_tabs_table(
    tabs: &[TabInfo],
    show_state: bool,
    show_dimensions: bool,
    show_panes: bool,
    show_layout: bool,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(build_tabs_table_header(
        show_state,
        show_dimensions,
        show_panes,
        show_layout,
    ));

    for tab_info in tabs {
        lines.push(build_tabs_table_row(
            tab_info,
            show_state,
            show_dimensions,
            show_panes,
            show_layout,
        ));
    }

    lines
}

fn build_tabs_table_header(
    show_state: bool,
    show_dimensions: bool,
    show_panes: bool,
    show_layout: bool,
) -> String {
    let mut header = Vec::new();

    // Core fields (always shown)
    header.push("TAB_ID");
    header.push("POSITION");
    header.push("NAME");

    if show_state {
        header.push("ACTIVE");
        header.push("FULLSCREEN");
        header.push("SYNC_PANES");
        header.push("FLOATING_VIS");
    }

    if show_dimensions {
        header.push("VP_ROWS");
        header.push("VP_COLS");
        header.push("DA_ROWS");
        header.push("DA_COLS");
    }

    if show_panes {
        header.push("TILED_PANES");
        header.push("FLOAT_PANES");
        header.push("HIDDEN_PANES");
    }

    if show_layout {
        header.push("SWAP_LAYOUT");
        header.push("LAYOUT_DIRTY");
    }

    header.join("  ")
}

fn build_tabs_table_row(
    tab_info: &TabInfo,
    show_state: bool,
    show_dimensions: bool,
    show_panes: bool,
    show_layout: bool,
) -> String {
    let mut row = Vec::new();

    // Core fields
    row.push(tab_info.tab_id.to_string());
    row.push(tab_info.position.to_string());
    row.push(tab_info.name.clone());

    if show_state {
        row.push(tab_info.active.to_string());
        row.push(tab_info.is_fullscreen_active.to_string());
        row.push(tab_info.is_sync_panes_active.to_string());
        row.push(tab_info.are_floating_panes_visible.to_string());
    }

    if show_dimensions {
        row.push(tab_info.viewport_rows.to_string());
        row.push(tab_info.viewport_columns.to_string());
        row.push(tab_info.display_area_rows.to_string());
        row.push(tab_info.display_area_columns.to_string());
    }

    if show_panes {
        row.push(tab_info.selectable_tiled_panes_count.to_string());
        row.push(tab_info.selectable_floating_panes_count.to_string());
        row.push(tab_info.panes_to_hide.to_string());
    }

    if show_layout {
        row.push(
            tab_info
                .active_swap_layout_name
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        );
        row.push(tab_info.is_swap_layout_dirty.to_string());
    }

    row.join("  ")
}

fn format_current_tab_info_as_json(tab_info: &TabInfo) -> Vec<String> {
    vec![serde_json::to_string_pretty(tab_info).unwrap_or_else(|_| "{}".to_string())]
}

fn format_current_tab_info_plain(tab_info: &TabInfo) -> Vec<String> {
    vec![
        format!("name: {}", tab_info.name),
        format!("id: {}", tab_info.tab_id),
        format!("position: {}", tab_info.position),
    ]
}

fn send_error_to_client(
    cli_client_id: Option<ClientId>,
    os_input: Option<&Box<dyn ServerOsApi>>,
    error_message: &str,
) {
    if let Some(cli_client_id) = cli_client_id {
        if let Some(os_input) = os_input {
            let _ = os_input.send_to_client(
                cli_client_id,
                ServerToClientMsg::LogError {
                    lines: vec![error_message.to_string()],
                },
            );
        }
    }
}

fn send_output_to_client(
    cli_client_id: Option<ClientId>,
    os_input: Option<&Box<dyn ServerOsApi>>,
    output_lines: Vec<String>,
) {
    if let Some(cli_client_id) = cli_client_id {
        if let Some(os_input) = os_input {
            let _ = os_input.send_to_client(
                cli_client_id,
                ServerToClientMsg::Log {
                    lines: output_lines,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zellij_runtime::channels::SenderWithContext;

    /// The bug this guards: bound keys become their own screen instructions and never reach
    /// `WriteCharacter`, so without a dismissal here a tab switch leaves a stale menu on screen,
    /// still offering to act on the pane it was opened over.
    #[test]
    fn only_keys_bound_to_something_other_than_writing_dismiss_a_context_menu() {
        let write = Action::Write {
            key_with_modifier: None,
            bytes: vec![b'a'],
            is_kitty_keyboard_protocol: false,
        };
        assert!(!actions_dismiss_context_menu(std::slice::from_ref(&write)));
        assert!(!actions_dismiss_context_menu(&[Action::WriteChars {
            chars: "a".to_owned()
        }]));
        assert!(actions_dismiss_context_menu(&[Action::GoToNextTab]));
        assert!(
            actions_dismiss_context_menu(&[write, Action::GoToNextTab]),
            "one non-writing action in the sequence is enough"
        );
    }

    /// The bug this guards: `bind "Enter" { Copy; SwitchToMode "Normal"; }` set the status bar's
    /// "Text copied to..." message with the first action and cleared it with the second's
    /// `InputReceived`, so a keyboard selection copied silently while a mouse one did not.
    #[test]
    fn a_key_whose_batch_copies_leaves_the_clipboard_message_alone() {
        assert!(actions_leave_clipboard_message(&[Action::Copy]));
        assert!(
            actions_leave_clipboard_message(&[
                Action::Copy,
                Action::SwitchToMode {
                    input_mode: InputMode::Normal
                },
            ]),
            "the trailing mode switch is part of the keystroke that copied"
        );
        assert!(actions_leave_clipboard_message(&[
            Action::CopyLastCommandOutput,
            Action::SwitchToMode {
                input_mode: InputMode::Locked
            },
        ]));
        assert!(!actions_leave_clipboard_message(&[
            Action::SwitchToMode {
                input_mode: InputMode::Normal
            },
            Action::GoToNextTab,
        ]));
    }

    /// The other half of the same bug: the batch decision only helps if `route_action` honours
    /// it, and the `InputReceived` it would otherwise send is what clears the message.
    #[test]
    fn a_suppressed_action_tells_the_plugins_nothing_about_the_input() {
        let route = |leaves_clipboard_message| {
            let (tx, rx) = zellij_runtime::channels::unbounded();
            let senders = ThreadSenders {
                to_plugin: Some(SenderWithContext::new(tx)),
                should_silently_fail: true,
                ..Default::default()
            };
            let _ = route_action(
                Action::SwitchToMode {
                    input_mode: InputMode::Normal,
                },
                ClientId::new(1),
                None,
                None,
                senders,
                None,
                None,
                InputMode::Normal,
                None,
                leaves_clipboard_message,
            );
            let mut events = Vec::new();
            while let Ok((instruction, _ctx)) = rx.try_recv() {
                if let PluginInstruction::Update(updates) = instruction {
                    events.extend(updates.into_iter().map(|(_, _, event)| event));
                }
            }
            events
        };
        assert!(
            route(false)
                .iter()
                .any(|event| matches!(event, Event::InputReceived)),
            "an ordinary action still reports the input"
        );
        assert!(
            !route(true)
                .iter()
                .any(|event| matches!(event, Event::InputReceived)),
            "an action belonging to a keystroke that copied stays silent"
        );
    }
}

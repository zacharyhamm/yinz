//! Pane and tab controls routed through the existing action bus.

use super::pane_creation::in_place_pane_id;
use super::write_response;
use crate::plugin_map::PluginEnv;
use crate::plugin_panes::Surface;
use crate::{PaneId, PluginInstruction};
use prost::Message;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::sync::oneshot;
use yinz_bus::pty::PtyInstruction;
use yinz_bus::route::NotificationEnd;
use yinz_bus::screen::{NotificationClick, ScreenInstruction};
use yinz_plugin_api::plugin_command::{
    hide_floating_panes_response, show_floating_panes_response, ProtobufBreakPanesToNewTabResponse,
    ProtobufBreakPanesToTabWithIdResponse, ProtobufBreakPanesToTabWithIndexResponse,
    ProtobufFocusOrCreateTabResponse, ProtobufHideFloatingPanesResponse,
    ProtobufShowFloatingPanesResponse,
};
use yinz_routing::wait_for_action_completion;
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::AgentInfo;
use yinz_types::data::{
    BreakPanesToNewTabResponse, BreakPanesToTabWithIdResponse, BreakPanesToTabWithIndexResponse,
    FocusOrCreateTabResponse, InputMode, NotifyClick, PaneFrameStyle, RegexHighlight,
};
use yinz_types::geometry::{Direction, FloatingPaneCoordinates, Resize, ResizeStrategy};
use yinz_types::input::actions::Action;
use yinz_types::input::command::{RunCommand, TerminalAction};
use yinz_types::internal_ids::{PluginId, TabId, TabNumber, TabPosition, TerminalId};

pub(super) fn switch_tab_to(env: &PluginEnv, tab_idx: TabNumber) {
    env.senders
        .send_to_screen(ScreenInstruction::GoToTab(
            tab_idx,
            Some(env.client_id),
            None,
        ))
        .with_context(|| {
            format!(
                "failed to switch to tab {tab_idx} from plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

pub(super) fn hide_self(env: &PluginEnv) -> Result<()> {
    env.senders
        .send_to_screen(ScreenInstruction::SuppressPane(
            PaneId::Plugin(env.plugin_id),
            env.client_id,
        ))
        .with_context(|| "failed to hide self".to_string())
}

pub(super) fn hide_pane_with_id(env: &PluginEnv, pane_id: PaneId) -> Result<()> {
    env.senders
        .send_to_screen(ScreenInstruction::SuppressPane(pane_id, env.client_id))
        .with_context(|| "failed to hide self".to_string())
}

pub(super) fn show_self(env: &PluginEnv, should_float_if_hidden: bool) {
    let action = Action::FocusPluginPaneWithId {
        pane_id: env.plugin_id,
        should_float_if_hidden,
        should_be_in_place_if_hidden: false,
    };
    let error_msg = || "Failed to show self for plugin".to_string();
    apply_action!(action, error_msg, env);
}

pub(super) fn show_pane_with_id(
    env: &PluginEnv,
    pane_id: PaneId,
    should_float_if_hidden: bool,
    should_focus_pane: bool,
) {
    if should_focus_pane {
        let _ = env
            .senders
            .send_to_screen(ScreenInstruction::FocusPaneWithId(
                pane_id,
                should_float_if_hidden,
                false,
                env.client_id,
                None,
            ));
    } else {
        let _ = env
            .senders
            .send_to_screen(ScreenInstruction::UnsuppressOrExpandPane(
                pane_id,
                should_float_if_hidden,
            ));
    }
}

pub(super) fn close_self(env: &PluginEnv) {
    // A headless plugin has no pane to close: closing itself is unloading.
    if env.surface == Surface::Pane {
        env.senders
            .send_to_screen(ScreenInstruction::CloseSelf(env.plugin_id, env.client_id))
            .with_context(|| "failed to close self".to_string())
            .non_fatal();
    }
    env.senders
        .send_to_plugin(PluginInstruction::Unload(env.plugin_id))
        .with_context(|| "failed to close self".to_string())
        .non_fatal();
}

pub(super) fn switch_to_mode(env: &PluginEnv, input_mode: InputMode) {
    let action = Action::SwitchToMode { input_mode };
    let error_msg = || format!("failed to switch to mode in plugin {}", env.name());
    apply_action!(action, error_msg, env);
}

pub(super) fn go_to_next_tab(env: &PluginEnv) {
    let action = Action::GoToNextTab;
    let error_msg = || "Failed to go to next tab".to_string();
    apply_action!(action, error_msg, env);
}

pub(super) fn go_to_previous_tab(env: &PluginEnv) {
    let action = Action::GoToPreviousTab;
    let error_msg = || "Failed to go to previous tab".to_string();
    apply_action!(action, error_msg, env);
}

pub(super) fn resize(env: &PluginEnv, resize: Resize) {
    let error_msg = || format!("failed to resize in plugin {}", env.name());
    let action = Action::Resize {
        resize,
        direction: None,
    };
    apply_action!(action, error_msg, env);
}

pub(super) fn resize_with_direction(env: &PluginEnv, resize: ResizeStrategy) {
    let error_msg = || format!("failed to resize in plugin {}", env.name());
    let action = Action::Resize {
        resize: resize.resize,
        direction: resize.direction,
    };
    apply_action!(action, error_msg, env);
}

pub(super) fn focus_next_pane(env: &PluginEnv) {
    let action = Action::FocusNextPane;
    let error_msg = || "Failed to focus next pane".to_string();
    apply_action!(action, error_msg, env);
}

pub(super) fn focus_previous_pane(env: &PluginEnv) {
    let action = Action::FocusPreviousPane;
    let error_msg = || "Failed to focus previous pane".to_string();
    apply_action!(action, error_msg, env);
}

pub(super) fn focus_last_pane(env: &PluginEnv) {
    let action = Action::FocusLastPane;
    let error_msg = || "Failed to focus last pane".to_string();
    apply_action!(action, error_msg, env);
}

pub(super) fn move_focus(env: &PluginEnv, direction: Direction) {
    let error_msg = || format!("failed to move focus in plugin {}", env.name());
    let action = Action::MoveFocus { direction };
    apply_action!(action, error_msg, env);
}

pub(super) fn move_focus_or_tab(env: &PluginEnv, direction: Direction) {
    let error_msg = || format!("failed to move focus in plugin {}", env.name());
    let action = Action::MoveFocusOrTab { direction };
    apply_action!(action, error_msg, env);
}

pub(super) fn edit_scrollback(env: &PluginEnv) {
    let action = Action::EditScrollback { ansi: false };
    let error_msg = || "Failed to edit scrollback".to_string();
    apply_action!(action, error_msg, env);
}

pub(super) fn write(env: &PluginEnv, bytes: Vec<u8>) {
    let error_msg = || format!("failed to write in plugin {}", env.name());
    let action = Action::Write {
        key_with_modifier: None,
        bytes,
        is_kitty_keyboard_protocol: false,
    };
    apply_action!(action, error_msg, env);
}

pub(super) fn write_chars(env: &PluginEnv, chars_to_write: String) {
    let error_msg = || format!("failed to write in plugin {}", env.name());
    let action = Action::WriteChars {
        chars: chars_to_write,
    };
    apply_action!(action, error_msg, env);
}

pub(super) fn copy_to_clipboard(env: &PluginEnv, text: String) -> Result<()> {
    env.senders
        .send_to_screen(ScreenInstruction::CopyTextToClipboard(text, env.plugin_id))
        .with_context(|| format!("failed to copy to clipboard in plugin {}", env.name()))
}

pub(super) fn toggle_tab(env: &PluginEnv) {
    let error_msg = || "Failed to toggle tab".to_string();
    let action = Action::ToggleTab;
    apply_action!(action, error_msg, env);
}

pub(super) fn move_pane(env: &PluginEnv) {
    let error_msg = || format!("failed to move pane in plugin {}", env.name());
    let action = Action::MovePane { direction: None };
    apply_action!(action, error_msg, env);
}

pub(super) fn move_pane_with_direction(env: &PluginEnv, direction: Direction) {
    let error_msg = || format!("failed to move pane in plugin {}", env.name());
    let action = Action::MovePane {
        direction: Some(direction),
    };
    apply_action!(action, error_msg, env);
}

pub(super) fn clear_screen(env: &PluginEnv) {
    let error_msg = || format!("failed to clear screen in plugin {}", env.name());
    let action = Action::ClearScreen;
    apply_action!(action, error_msg, env);
}

pub(super) fn scroll_up(env: &PluginEnv) {
    let error_msg = || format!("failed to scroll up in plugin {}", env.name());
    let action = Action::ScrollUp;
    apply_action!(action, error_msg, env);
}

pub(super) fn scroll_down(env: &PluginEnv) {
    let error_msg = || format!("failed to scroll down in plugin {}", env.name());
    let action = Action::ScrollDown;
    apply_action!(action, error_msg, env);
}

pub(super) fn scroll_to_top(env: &PluginEnv) {
    let error_msg = || format!("failed to scroll in plugin {}", env.name());
    let action = Action::ScrollToTop;
    apply_action!(action, error_msg, env);
}

pub(super) fn scroll_to_bottom(env: &PluginEnv) {
    let error_msg = || format!("failed to scroll in plugin {}", env.name());
    let action = Action::ScrollToBottom;
    apply_action!(action, error_msg, env);
}

pub(super) fn page_scroll_up(env: &PluginEnv) {
    let error_msg = || format!("failed to scroll in plugin {}", env.name());
    let action = Action::PageScrollUp;
    apply_action!(action, error_msg, env);
}

pub(super) fn page_scroll_down(env: &PluginEnv) {
    let error_msg = || format!("failed to scroll in plugin {}", env.name());
    let action = Action::PageScrollDown;
    apply_action!(action, error_msg, env);
}

pub(super) fn toggle_focus_fullscreen(env: &PluginEnv) {
    let error_msg = || format!("failed to toggle full screen in plugin {}", env.name());
    let action = Action::ToggleFocusFullscreen;
    apply_action!(action, error_msg, env);
}

pub(super) fn toggle_focus_no_ui_fullscreen(env: &PluginEnv) {
    let error_msg = || {
        format!(
            "failed to toggle no-ui full screen in plugin {}",
            env.name()
        )
    };
    let action = Action::ToggleFocusNoUiFullscreen;
    apply_action!(action, error_msg, env);
}

pub(super) fn focus_host_session(env: &PluginEnv) {
    env.senders
        .send_to_screen(ScreenInstruction::FocusHostSession(env.client_id, None))
        .with_context(|| format!("failed to focus host session from plugin {}", env.name()))
        .non_fatal();
}

pub(super) fn toggle_pane_frames(env: &PluginEnv) {
    let error_msg = || format!("failed to toggle full screen in plugin {}", env.name());
    let action = Action::TogglePaneFrames;
    apply_action!(action, error_msg, env);
}

pub(super) fn set_pane_frame_style(env: &PluginEnv, pane_frame_style: PaneFrameStyle) {
    let error_msg = || format!("failed to set pane frame style in plugin {}", env.name());
    let action = Action::SetPaneFrameStyle(pane_frame_style);
    apply_action!(action, error_msg, env);
}

pub(super) fn toggle_pane_embed_or_eject(env: &PluginEnv) {
    let error_msg = || {
        format!(
            "failed to toggle pane embed or eject in plugin {}",
            env.name()
        )
    };
    let action = Action::TogglePaneEmbedOrFloating;
    apply_action!(action, error_msg, env);
}

pub(super) fn toggle_floating_panes(env: &PluginEnv, tab_id: Option<TabId>) {
    let error_msg = || format!("failed to toggle floating panes in plugin {}", env.name());
    let action = match tab_id {
        Some(id) => Action::ToggleFloatingPanesByTabId { id },
        None => Action::ToggleFloatingPanes,
    };
    apply_action!(action, error_msg, env);
}

pub(super) fn new_pane(env: &PluginEnv) {
    let error_msg = || format!("failed to open new pane in plugin {}", env.name());
    let action = Action::NewPane {
        direction: None,
        pane_name: None,
        start_suppressed: false,
    };
    apply_action!(action, error_msg, env);
}

pub(super) fn undo_rename_pane(env: &PluginEnv) {
    let error_msg = || format!("failed to undo rename pane in plugin {}", env.name());
    let action = Action::UndoRenamePane;
    apply_action!(action, error_msg, env);
}

pub(super) fn close_focus(env: &PluginEnv) {
    let error_msg = || format!("failed to close focused pane in plugin {}", env.name());
    let action = Action::CloseFocus;
    apply_action!(action, error_msg, env);
}

pub(super) fn toggle_active_tab_sync(env: &PluginEnv) {
    let error_msg = || format!("failed to toggle active tab sync in plugin {}", env.name());
    let action = Action::ToggleActiveSyncTab;
    apply_action!(action, error_msg, env);
}

pub(super) fn close_focused_tab(env: &PluginEnv) {
    let error_msg = || format!("failed to close active tab in plugin {}", env.name());
    let action = Action::CloseTab;
    apply_action!(action, error_msg, env);
}

pub(super) fn undo_rename_tab(env: &PluginEnv) {
    let error_msg = || format!("failed to undo rename tab in plugin {}", env.name());
    let action = Action::UndoRenameTab;
    apply_action!(action, error_msg, env);
}

pub(super) fn previous_swap_layout(env: &PluginEnv) {
    let error_msg = || format!("failed to switch swap layout in plugin {}", env.name());
    let action = Action::PreviousSwapLayout;
    apply_action!(action, error_msg, env);
}

pub(super) fn next_swap_layout(env: &PluginEnv) {
    let error_msg = || format!("failed to switch swap layout in plugin {}", env.name());
    let action = Action::NextSwapLayout;
    apply_action!(action, error_msg, env);
}

pub(super) fn go_to_tab_name(env: &PluginEnv, tab_name: String) {
    let error_msg = || format!("failed to change tab in plugin {}", env.name());
    let create = false;
    let action = Action::GoToTabName {
        name: tab_name,
        create,
    };
    apply_action!(action, error_msg, env);
}

pub(super) fn focus_or_create_tab(env: &mut PluginEnv, tab_name: String) {
    let error_msg = || format!("failed to change or create tab in plugin {}", env.name());
    let create = true;
    let action = Action::GoToTabName {
        name: tab_name,
        create,
    };
    let result = apply_action!(action, error_msg, env);

    // Return Some(tab_id) if tab was created or focused, None on error
    let tab_id: FocusOrCreateTabResponse = result.and_then(|r| r.affected_tab_id);

    let response = ProtobufFocusOrCreateTabResponse::from(tab_id);
    write_response(env, &response.encode_to_vec()).non_fatal();
}

pub(super) fn go_to_tab(env: &PluginEnv, tab_index: TabNumber) {
    let error_msg = || format!("failed to change tab focus in plugin {}", env.name());
    let action = Action::GoToTab { index: tab_index };
    apply_action!(action, error_msg, env);
}

pub(super) fn close_terminal_pane(env: &PluginEnv, terminal_pane_id: TerminalId) {
    let error_msg = || format!("failed to change tab focus in plugin {}", env.name());
    let action = Action::CloseTerminalPane {
        pane_id: terminal_pane_id,
    };
    apply_action!(action, error_msg, env);
    env.senders
        .send_to_pty(PtyInstruction::ClosePane(
            PaneId::Terminal(terminal_pane_id),
            None,
        ))
        .non_fatal();
}

pub(super) fn close_plugin_pane(env: &PluginEnv, plugin_pane_id: PluginId) {
    let error_msg = || format!("failed to change tab focus in plugin {}", env.name());
    let action = Action::ClosePluginPane {
        pane_id: plugin_pane_id,
    };
    apply_action!(action, error_msg, env);
    env.senders
        .send_to_plugin(PluginInstruction::Unload(plugin_pane_id))
        .non_fatal();
}

pub(super) fn focus_terminal_pane(
    env: &PluginEnv,
    terminal_pane_id: TerminalId,
    should_float_if_hidden: bool,
    should_be_in_place_if_hidden: bool,
) {
    let action = Action::FocusTerminalPaneWithId {
        pane_id: terminal_pane_id,
        should_float_if_hidden,
        should_be_in_place_if_hidden,
    };
    let error_msg = || "Failed to focus terminal pane".to_string();
    apply_action!(action, error_msg, env);
}

pub(super) fn focus_plugin_pane(
    env: &PluginEnv,
    plugin_pane_id: PluginId,
    should_float_if_hidden: bool,
    should_be_in_place_if_hidden: bool,
) {
    let action = Action::FocusPluginPaneWithId {
        pane_id: plugin_pane_id,
        should_float_if_hidden,
        should_be_in_place_if_hidden,
    };
    let error_msg = || "Failed to focus plugin pane".to_string();
    apply_action!(action, error_msg, env);
}

pub(super) fn rename_terminal_pane(env: &PluginEnv, terminal_pane_id: TerminalId, new_name: &str) {
    let error_msg = || "Failed to rename terminal pane".to_string();
    let rename_pane_action = Action::RenameTerminalPane {
        pane_id: terminal_pane_id,
        name: new_name.as_bytes().to_vec(),
    };
    apply_action!(rename_pane_action, error_msg, env);
}

pub(super) fn rename_plugin_pane(env: &PluginEnv, plugin_pane_id: PluginId, new_name: &str) {
    let error_msg = || "Failed to rename plugin pane".to_string();
    let rename_pane_action = Action::RenamePluginPane {
        pane_id: plugin_pane_id,
        name: new_name.as_bytes().to_vec(),
    };
    apply_action!(rename_pane_action, error_msg, env);
}

pub(super) fn rename_tab(env: &PluginEnv, tab_position: TabNumber, new_name: &str) {
    let error_msg = || "Failed to rename tab".to_string();
    let rename_tab_action = Action::RenameTab {
        // The command dispatcher converts the plugin position to a tab number.
        tab_index: tab_position,
        name: new_name.as_bytes().to_vec(),
    };
    apply_action!(rename_tab_action, error_msg, env);
}

pub(super) fn notify(
    env: &PluginEnv,
    text: String,
    pane_id: Option<PaneId>,
    click: Option<NotifyClick>,
) {
    let error_msg = || format!("failed to show notification from plugin {}", env.name());
    // Stamped here rather than carried up from the plugin: a toast may only ever call back the
    // plugin that raised it, so the id is the host's to supply. `owner_client_id` and not
    // `client_id`, because this names an *instance* rather than a turn - it is the key the plugin
    // map holds this one under, which is what the delivery matches on. Same distinction
    // `post_message_to` makes for a worker.
    let on_click = click.map(|click| NotificationClick {
        plugin_id: env.plugin_id,
        client_id: env.owner_client_id,
        message: click.message,
        payload: click.payload,
    });
    env.senders
        .send_to_screen(ScreenInstruction::PluginNotify {
            text,
            pane_id,
            on_click,
            plugin_id: env.plugin_id,
            completion: None,
        })
        .with_context(error_msg)
        .non_fatal();
}

pub(super) fn show_floating_panes(env: &mut PluginEnv, tab_id: Option<TabId>) {
    use show_floating_panes_response::Result as ShowResult;
    let (completion_tx, completion_rx) = oneshot::channel();
    let send_result = env
        .senders
        .send_to_screen(ScreenInstruction::ShowFloatingPanes {
            client_id: env.client_id,
            tab_id,
            completion: Some(NotificationEnd::new(completion_tx)),
        });
    let response = if let Err(e) = send_result {
        ProtobufShowFloatingPanesResponse {
            result: Some(ShowResult::Error(format!("{}", e))),
        }
    } else {
        let result = wait_for_action_completion(completion_rx, "show_floating_panes", false);
        match result.exit_status {
            Some(0) => ProtobufShowFloatingPanesResponse {
                result: Some(ShowResult::Success(true)),
            },
            Some(2) => ProtobufShowFloatingPanesResponse {
                result: Some(ShowResult::Success(false)),
            },
            _ => ProtobufShowFloatingPanesResponse {
                result: Some(ShowResult::Error("Tab not found".to_string())),
            },
        }
    };
    let _ = write_response(env, &response.encode_to_vec());
}

pub(super) fn hide_floating_panes(env: &mut PluginEnv, tab_id: Option<TabId>) {
    use hide_floating_panes_response::Result as HideResult;
    let (completion_tx, completion_rx) = oneshot::channel();
    let send_result = env
        .senders
        .send_to_screen(ScreenInstruction::HideFloatingPanes {
            client_id: env.client_id,
            tab_id,
            completion: Some(NotificationEnd::new(completion_tx)),
        });
    let response = if let Err(e) = send_result {
        ProtobufHideFloatingPanesResponse {
            result: Some(HideResult::Error(format!("{}", e))),
        }
    } else {
        let result = wait_for_action_completion(completion_rx, "hide_floating_panes", false);
        match result.exit_status {
            Some(0) => ProtobufHideFloatingPanesResponse {
                result: Some(HideResult::Success(true)),
            },
            Some(2) => ProtobufHideFloatingPanesResponse {
                result: Some(HideResult::Success(false)),
            },
            _ => ProtobufHideFloatingPanesResponse {
                result: Some(HideResult::Error("Tab not found".to_string())),
            },
        }
    };
    let _ = write_response(env, &response.encode_to_vec());
}

pub(super) fn set_pane_regex_highlights(
    env: &PluginEnv,
    pane_id: yinz_types::data::PaneId,
    highlights: Vec<RegexHighlight>,
) {
    let err_context = || "failed to set pane regex highlights".to_string();
    let pane_id: PaneId = pane_id;
    env.senders
        .send_to_screen(ScreenInstruction::SetPluginRegexHighlights {
            pane_id,
            plugin_id: env.plugin_id,
            highlights,
        })
        .with_context(err_context)
        .non_fatal();
}

pub(super) fn clear_pane_highlights(env: &PluginEnv, pane_id: yinz_types::data::PaneId) {
    let err_context = || "failed to clear pane highlights".to_string();
    let pane_id: PaneId = pane_id;
    env.senders
        .send_to_screen(ScreenInstruction::ClearPluginHighlights {
            pane_id,
            plugin_id: env.plugin_id,
        })
        .with_context(err_context)
        .non_fatal();
}

pub(super) fn change_host_folder(env: &PluginEnv, new_host_folder: PathBuf) {
    let _ = env.senders.to_plugin.as_ref().map(|sender| {
        sender.send(PluginInstruction::ChangePluginHostDir(
            new_host_folder,
            env.plugin_id,
            env.client_id,
            env.generation,
        ))
    });
}

pub(super) fn set_floating_pane_pinned(env: &PluginEnv, pane_id: PaneId, should_be_pinned: bool) {
    let _ = env.senders.to_screen.as_ref().map(|sender| {
        sender.send(ScreenInstruction::SetFloatingPanePinned(
            pane_id,
            should_be_pinned,
        ))
    });
}

pub(super) fn stack_panes(env: &PluginEnv, pane_ids: Vec<PaneId>) {
    let _ =
        env.senders
            .send_to_screen(ScreenInstruction::StackPanes(pane_ids, env.client_id, None));
}

/// Move or resize overlays, within the overlay collection.
///
/// The sibling of `change_floating_panes_coordinates`, and deliberately not the same instruction:
/// an overlay is not on the tab's floating layer, and a pane that is on it does not become one by
/// being resized.
pub(super) fn change_overlay_coordinates(
    env: &PluginEnv,
    pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
    override_user_placement: bool,
) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ChangeOverlayCoordinates(
            pane_ids_and_coordinates,
            override_user_placement,
            None,
        ));
}

pub(super) fn change_floating_panes_coordinates(
    env: &PluginEnv,
    pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ChangeFloatingPanesCoordinates(
            pane_ids_and_coordinates,
            None,
        ));
}

pub(super) fn toggle_pane_borderless(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::TogglePaneBorderless(pane_id, None));
}

pub(super) fn set_pane_borderless(env: &PluginEnv, pane_id: PaneId, borderless: bool) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::SetPaneBorderless(
            pane_id, borderless, None,
        ));
}

pub(super) fn set_pane_agent(
    env: &PluginEnv,
    pane_id: yinz_types::data::PaneId,
    agent: Option<AgentInfo>,
) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::SetPaneAgent(pane_id, agent, None));
}

pub(super) fn set_pane_agent_run(
    env: &PluginEnv,
    pane_id: yinz_types::data::PaneId,
    run_id: Option<String>,
) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::SetPaneAgentRun(pane_id, run_id, None));
}

pub(super) fn set_pane_color(
    env: &PluginEnv,
    pane_id: PaneId,
    fg: Option<String>,
    bg: Option<String>,
) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::SetPaneColor(pane_id, fg, bg, None));
}

pub(super) fn resize_pane_with_id(env: &PluginEnv, resize: ResizeStrategy, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ResizePaneWithId(resize, pane_id));
}

pub(super) fn edit_scrollback_for_pane_with_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::EditScrollbackForPaneWithId(
            pane_id, None,
        ));
}

pub(super) fn write_to_pane_id(env: &PluginEnv, bytes: Vec<u8>, pane_id: PaneId) {
    let (completion_tx, completion_rx) = oneshot::channel();
    let send_result = env.senders.send_to_screen(ScreenInstruction::WriteToPaneId(
        bytes,
        pane_id,
        Some(NotificationEnd::new(completion_tx)),
    ));
    if send_result.is_ok() {
        let wait_forever = false;
        let _ = wait_for_action_completion(completion_rx, "write_to_pane_id", wait_forever);
    }
}

pub(super) fn write_chars_to_pane_id(env: &PluginEnv, chars: String, pane_id: PaneId) {
    let bytes = chars.into_bytes();
    let (completion_tx, completion_rx) = oneshot::channel();
    let send_result = env.senders.send_to_screen(ScreenInstruction::WriteToPaneId(
        bytes,
        pane_id,
        Some(NotificationEnd::new(completion_tx)),
    ));
    if send_result.is_ok() {
        let wait_forever = false;
        let _ = wait_for_action_completion(completion_rx, "write_chars_to_pane_id", wait_forever);
    }
}

pub(super) fn send_sigint_to_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let err_context = || {
        format!(
            "failed to send SIGINT to pane {:?} from plugin {}",
            pane_id,
            env.name()
        )
    };

    env.senders
        .send_to_pty(PtyInstruction::SendSigintToPaneId(pane_id))
        .with_context(err_context)
        .non_fatal();
}

pub(super) fn send_sigkill_to_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let err_context = || {
        format!(
            "failed to send SIGKILL to pane {:?} from plugin {}",
            pane_id,
            env.name()
        )
    };

    env.senders
        .send_to_pty(PtyInstruction::SendSigkillToPaneId(pane_id))
        .with_context(err_context)
        .non_fatal();
}

pub(super) fn move_pane_with_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::MovePaneWithPaneId(pane_id));
}

pub(super) fn move_pane_with_pane_id_in_direction(
    env: &PluginEnv,
    pane_id: PaneId,
    direction: Direction,
) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::MovePaneWithPaneIdInDirection(
            pane_id, direction,
        ));
}

pub(super) fn clear_screen_for_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ClearScreenForPaneId(pane_id));
}

pub(super) fn scroll_up_in_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ScrollUpInPaneId(pane_id));
}

pub(super) fn scroll_down_in_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ScrollDownInPaneId(pane_id));
}

pub(super) fn scroll_to_top_in_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ScrollToTopInPaneId(pane_id));
}

pub(super) fn scroll_to_bottom_in_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ScrollToBottomInPaneId(pane_id));
}

pub(super) fn page_scroll_up_in_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::PageScrollUpInPaneId(pane_id));
}

pub(super) fn page_scroll_down_in_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::PageScrollDownInPaneId(pane_id));
}

pub(super) fn toggle_pane_id_fullscreen(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::TogglePaneIdFullscreen(pane_id));
}

pub(super) fn toggle_pane_embed_or_eject_for_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::TogglePaneEmbedOrEjectForPaneId(pane_id));
}

pub(super) fn close_tab_with_index(env: &PluginEnv, tab_index: TabPosition) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::CloseTabWithIndex(tab_index));
}

pub(super) fn break_panes_to_new_tab(
    env: &mut PluginEnv,
    pane_ids: Vec<PaneId>,
    new_tab_name: Option<String>,
    should_change_focus_to_new_tab: bool,
) {
    let default_shell = env.default_shell.clone().or_else(|| {
        Some(TerminalAction::RunCommand(RunCommand {
            command: env.path_to_default_shell.clone(),
            use_terminal_title: true,
            ..Default::default()
        }))
    });

    // Create completion channel to receive tab ID
    let (tx, rx) = tokio::sync::oneshot::channel();
    let completion_tx = Some(NotificationEnd::new(tx));

    let result = env
        .senders
        .send_to_screen(ScreenInstruction::BreakPanesToNewTab {
            pane_ids,
            default_shell,
            new_tab_name,
            should_change_focus_to_new_tab,
            client_id: env.client_id,
            completion_tx,
        });

    let tab_id: BreakPanesToNewTabResponse = if result.is_ok() {
        wait_for_action_completion(rx, "break_panes_to_new_tab", false).affected_tab_id
    } else {
        None
    };

    let response = ProtobufBreakPanesToNewTabResponse::from(tab_id);
    write_response(env, &response.encode_to_vec()).non_fatal();
}

pub(super) fn break_panes_to_tab_with_index(
    env: &mut PluginEnv,
    pane_ids: Vec<PaneId>,
    should_change_focus_to_new_tab: bool,
    tab_index: TabPosition,
) {
    // Create completion channel to receive tab ID
    let (tx, rx) = tokio::sync::oneshot::channel();
    let completion_tx = Some(NotificationEnd::new(tx));

    let result = env
        .senders
        .send_to_screen(ScreenInstruction::BreakPanesToTabWithIndex {
            pane_ids,
            tab_index,
            client_id: env.client_id,
            should_change_focus_to_new_tab,
            completion_tx,
        });

    let tab_id: BreakPanesToTabWithIndexResponse = if result.is_ok() {
        wait_for_action_completion(rx, "break_panes_to_tab_with_index", false).affected_tab_id
    } else {
        None
    };

    let response = ProtobufBreakPanesToTabWithIndexResponse::from(tab_id);
    write_response(env, &response.encode_to_vec()).non_fatal();
}

pub(super) fn switch_tab_to_id(env: &PluginEnv, tab_id: TabId) {
    env.senders
        .send_to_screen(ScreenInstruction::GoToTabWithId(
            tab_id,
            Some(env.client_id),
            None,
        ))
        .with_context(|| {
            format!(
                "failed to switch to tab {} from plugin {}",
                tab_id,
                env.name()
            )
        })
        .non_fatal();
}

pub(super) fn go_to_tab_with_id(env: &PluginEnv, tab_id: TabId) {
    env.senders
        .send_to_screen(ScreenInstruction::GoToTabWithId(
            tab_id,
            Some(env.client_id),
            None,
        ))
        .with_context(|| format!("failed to go to tab {} from plugin {}", tab_id, env.name()))
        .non_fatal();
}

pub(super) fn close_tab_with_id(env: &PluginEnv, tab_id: TabId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::CloseTabWithId(tab_id, None));
}

pub(super) fn set_tab_projects(env: &PluginEnv, tab_projects: BTreeMap<TabId, String>) {
    env.senders
        .send_to_screen(ScreenInstruction::SetTabProjects(tab_projects))
        .context("Failed to set tab projects")
        .non_fatal();
}

pub(super) fn rename_tab_with_id(env: &PluginEnv, tab_id: TabId, new_name: &str) {
    let rename_tab_action =
        ScreenInstruction::RenameTabWithId(tab_id, new_name.as_bytes().to_vec(), None);
    env.senders
        .send_to_screen(rename_tab_action)
        .with_context(|| format!("Failed to rename tab {}", tab_id))
        .non_fatal();
}

pub(super) fn break_panes_to_tab_with_id(
    env: &mut PluginEnv,
    pane_ids: Vec<PaneId>,
    should_change_focus_to_target_tab: bool,
    tab_id: TabId,
) {
    // Create completion channel to receive tab ID
    let (tx, rx) = tokio::sync::oneshot::channel();
    let completion_tx = Some(NotificationEnd::new(tx));

    let result = env
        .senders
        .send_to_screen(ScreenInstruction::BreakPanesToTabWithId {
            pane_ids,
            tab_id,
            should_change_focus_to_target_tab,
            client_id: env.client_id,
            completion_tx,
        });

    let result_tab_id: BreakPanesToTabWithIdResponse = if result.is_ok() {
        wait_for_action_completion(rx, "break_panes_to_tab_with_id", false).affected_tab_id
    } else {
        None
    };

    let response = ProtobufBreakPanesToTabWithIdResponse::from(result_tab_id);
    write_response(env, &response.encode_to_vec()).non_fatal();
}

pub(super) fn group_and_ungroup_panes(
    env: &PluginEnv,
    panes_to_group: Vec<PaneId>,
    panes_to_ungroup: Vec<PaneId>,
    for_all_clients: bool,
) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::GroupAndUngroupPanes(
            panes_to_group,
            panes_to_ungroup,
            for_all_clients,
            env.client_id,
        ));
}

pub(super) fn highlight_and_unhighlight_panes(
    env: &PluginEnv,
    panes_to_highlight: Vec<PaneId>,
    panes_to_unhighlight: Vec<PaneId>,
) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::HighlightAndUnhighlightPanes(
            panes_to_highlight,
            panes_to_unhighlight,
            env.client_id,
        ));
}

pub(super) fn reveal_pane_with_id(env: &PluginEnv, pane_id: PaneId, label: Option<String>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::RevealPaneWithId(
            pane_id,
            label,
            env.client_id,
        ));
}

pub(super) fn set_pane_title_prefix(env: &PluginEnv, pane_id: PaneId, prefix: Option<String>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::SetPaneTitlePrefix(pane_id, prefix));
}

pub(super) fn set_pane_subtitle(env: &PluginEnv, pane_id: PaneId, subtitle: Option<String>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::SetPaneSubtitle(pane_id, subtitle));
}

pub(super) fn close_multiple_panes(env: &PluginEnv, pane_ids: Vec<PaneId>) {
    for pane_id in pane_ids {
        match pane_id {
            PaneId::Terminal(terminal_pane_id) => {
                close_terminal_pane(env, terminal_pane_id);
            },
            PaneId::Plugin(plugin_pane_id) => {
                close_plugin_pane(env, plugin_pane_id);
            },
        }
    }
}

pub(super) fn float_multiple_panes(env: &PluginEnv, pane_ids: Vec<PaneId>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::FloatMultiplePanes(
            pane_ids,
            env.client_id,
        ));
}

pub(super) fn embed_multiple_panes(env: &PluginEnv, pane_ids: Vec<PaneId>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::EmbedMultiplePanes(
            pane_ids,
            env.client_id,
        ));
}

pub(super) fn set_self_mouse_selection_support(env: &PluginEnv, selection_support: bool) {
    env.senders
        .send_to_screen(ScreenInstruction::SetMouseSelectionSupport(
            PaneId::Plugin(env.plugin_id),
            selection_support,
        ))
        .with_context(|| {
            format!(
                "failed to set plugin {} selectable from plugin {}",
                selection_support,
                env.name()
            )
        })
        .non_fatal();
}

pub(super) fn replace_pane_with_existing_pane(
    env: &mut PluginEnv,
    pane_to_replace: PaneId,
    existing_pane: PaneId,
    suppress_replaced_pane: bool,
) -> Result<()> {
    let (completion_tx, completion_rx) = oneshot::channel();
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ReplacePaneWithExistingPane(
            pane_to_replace,
            existing_pane,
            suppress_replaced_pane,
            Some(NotificationEnd::new(completion_tx)),
        ));
    let result =
        wait_for_action_completion(completion_rx, "replace_pane_with_existing_pane", false);
    in_place_pane_id(result).map(|_| ())
}

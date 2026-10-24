//! Translate CLI options into validated application actions.

mod agents;
mod keys;
mod layouts;
mod panes;

use crate::args::CliAction;
use std::path::PathBuf;
use std::str::FromStr;
use uuid::Uuid;
use yinz_config::config::Config;
use yinz_types::data::{NewPanePlacement, PaneId};
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::input::actions::{warp_placement, Action};
pub use yinz_types::input::command::{OpenFilePayload, RunCommandAction};
use yinz_types::input::layout::PluginUserConfiguration;
use yinz_types::internal_ids::TabId;

pub fn actions_from_cli(
    cli_action: CliAction,
    get_current_dir: Box<dyn Fn() -> PathBuf>,
    config: Option<Config>,
) -> Result<Vec<Action>, String> {
    match cli_action {
        CliAction::Write { bytes, pane_id } => match pane_id {
            Some(pane_id_str) => {
                let parsed_pane_id = PaneId::from_str(&pane_id_str);
                match parsed_pane_id {
                            Ok(parsed_pane_id) => {
                                Ok(vec![Action::WriteToPaneId {
                                    bytes,
                                    pane_id: parsed_pane_id,
                                }])
                            },
                            Err(_e) => {
                                Err(format!(
                                    "Malformed pane id: {}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                                    pane_id_str
                                ))
                            }
                        }
            },
            None => Ok(vec![Action::Write {
                key_with_modifier: None,
                bytes,
                is_kitty_keyboard_protocol: false,
            }]),
        },
        CliAction::WriteChars { chars, pane_id } => match pane_id {
            Some(pane_id_str) => {
                let parsed_pane_id = PaneId::from_str(&pane_id_str);
                match parsed_pane_id {
                            Ok(parsed_pane_id) => {
                                Ok(vec![Action::WriteCharsToPaneId {
                                    chars,
                                    pane_id: parsed_pane_id,
                                }])
                            },
                            Err(_e) => {
                                Err(format!(
                                    "Malformed pane id: {}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                                    pane_id_str
                                ))
                            }
                        }
            },
            None => Ok(vec![Action::WriteChars { chars }]),
        },
        CliAction::Paste { chars, pane_id } => match pane_id {
            Some(pane_id_str) => {
                let parsed_pane_id = PaneId::from_str(&pane_id_str);
                match parsed_pane_id {
                        Ok(parsed_pane_id) => {
                            Ok(vec![Action::Paste {
                                chars,
                                pane_id: Some(parsed_pane_id),
                            }])
                        },
                        Err(_e) => {
                            Err(format!(
                                "Malformed pane id: {}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                                pane_id_str
                            ))
                        }
                    }
            },
            None => Ok(vec![Action::Paste {
                chars,
                pane_id: None,
            }]),
        },
        CliAction::SendKeys { keys, pane_id } => keys::send_keys(keys, pane_id),
        CliAction::Resize {
            resize,
            direction,
            pane_id,
        } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::ResizeByPaneId {
                    pane_id,
                    resize,
                    direction,
                }])
            },
            None => Ok(vec![Action::Resize { resize, direction }]),
        },
        CliAction::FocusNextPane => Ok(vec![Action::FocusNextPane]),
        CliAction::FocusPreviousPane => Ok(vec![Action::FocusPreviousPane]),
        CliAction::FocusPaneId { pane_id } => {
            let pane_id = PaneId::from_str(&pane_id)
                    .map_err(|_| format!(
                        "Malformed pane id: {pane_id}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                    ))?;
            Ok(vec![Action::FocusPaneByPaneId { pane_id }])
        },
        CliAction::WarpPane {
            to_session,
            pane_id,
            focused_tab,
            floating,
        } => {
            let pane_id = pane_id
                    .map(|pane_id| {
                        PaneId::from_str(&pane_id).map_err(|_| format!(
                            "Malformed pane id: {pane_id}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))
                    })
                    .transpose()?;
            if to_session.trim().is_empty() {
                return Err("--to-session cannot be empty".to_owned());
            }
            Ok(vec![Action::WarpPane {
                pane_id,
                to_session: to_session.into(),
                placement: warp_placement(focused_tab, floating),
            }])
        },
        CliAction::WarpTab {
            to_session,
            from_session,
            tab_id,
        } => match (to_session, from_session) {
            (Some(to_session), None) => {
                if to_session.trim().is_empty() {
                    return Err("--to-session cannot be empty".to_owned());
                }
                Ok(vec![Action::WarpTab {
                    tab_id,
                    to_session: to_session.into(),
                }])
            },
            (None, Some(from_session)) => {
                if from_session.trim().is_empty() {
                    return Err("--from-session cannot be empty".to_owned());
                }
                // Unlike the push, there is no focused tab to fall back on:
                // the tab being named lives in a session that is not ours.
                let tab_id =
                    tab_id.ok_or_else(|| "--from-session also needs --tab-id".to_owned())?;
                Ok(vec![Action::WarpTabFrom {
                    from_session: from_session.into(),
                    tab_id,
                }])
            },
            _ => Err("warp-tab needs exactly one of --to-session or --from-session".to_owned()),
        },
        CliAction::FocusPaneTabs => Ok(vec![Action::FocusPaneTabs]),
        CliAction::PreviousPaneTab => Ok(vec![Action::PreviousPaneTab]),
        CliAction::NextPaneTab => Ok(vec![Action::NextPaneTab]),
        CliAction::FocusLastPane => Ok(vec![Action::FocusLastPane]),
        CliAction::MoveFocus { direction } => Ok(vec![Action::MoveFocus { direction }]),
        CliAction::MoveFocusOrTab { direction } => Ok(vec![Action::MoveFocusOrTab { direction }]),
        CliAction::MovePane { direction, pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::MovePaneByPaneId { pane_id, direction }])
            },
            None => Ok(vec![Action::MovePane { direction }]),
        },
        CliAction::MovePaneBackwards { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::MovePaneBackwardsByPaneId { pane_id }])
            },
            None => Ok(vec![Action::MovePaneBackwards]),
        },
        CliAction::MoveTab { direction, tab_id } => match tab_id {
            Some(id) => Ok(vec![Action::MoveTabByTabId { id, direction }]),
            None => Ok(vec![Action::MoveTab { direction }]),
        },
        CliAction::Clear { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::ClearScreenByPaneId { pane_id }])
            },
            None => Ok(vec![Action::ClearScreen]),
        },
        CliAction::DumpScreen {
            path,
            full,
            pane_id,
            ansi,
        } => match pane_id {
            Some(pane_id_str) => {
                let parsed_pane_id = PaneId::from_str(&pane_id_str);
                match parsed_pane_id {
                        Ok(parsed_pane_id) => {
                            Ok(vec![Action::DumpScreen {
                                file_path: path.map(|p| p.as_os_str().to_string_lossy().into()),
                                include_scrollback: full,
                                pane_id: Some(parsed_pane_id),
                                ansi,
                            }])
                        },
                        Err(_e) => {
                            Err(format!(
                                "Malformed pane id: {}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                                pane_id_str
                            ))
                        }
                    }
            },
            None => Ok(vec![Action::DumpScreen {
                file_path: path.map(|p| p.as_os_str().to_string_lossy().into()),
                include_scrollback: full,
                pane_id: None,
                ansi,
            }]),
        },
        CliAction::DumpLayout => Ok(vec![Action::DumpLayout]),
        CliAction::SaveSession => Ok(vec![Action::SaveSession]),
        CliAction::EditScrollback { pane_id, ansi } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::EditScrollbackByPaneId { pane_id, ansi }])
            },
            None => Ok(vec![Action::EditScrollback { ansi }]),
        },
        CliAction::ScrollUp { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::ScrollUpByPaneId { pane_id }])
            },
            None => Ok(vec![Action::ScrollUp]),
        },
        CliAction::ScrollDown { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::ScrollDownByPaneId { pane_id }])
            },
            None => Ok(vec![Action::ScrollDown]),
        },
        CliAction::ScrollToBottom { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::ScrollToBottomByPaneId { pane_id }])
            },
            None => Ok(vec![Action::ScrollToBottom]),
        },
        CliAction::ScrollToTop { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::ScrollToTopByPaneId { pane_id }])
            },
            None => Ok(vec![Action::ScrollToTop]),
        },
        CliAction::PageScrollUp { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::PageScrollUpByPaneId { pane_id }])
            },
            None => Ok(vec![Action::PageScrollUp]),
        },
        CliAction::PageScrollDown { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::PageScrollDownByPaneId { pane_id }])
            },
            None => Ok(vec![Action::PageScrollDown]),
        },
        CliAction::HalfPageScrollUp { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::HalfPageScrollUpByPaneId { pane_id }])
            },
            None => Ok(vec![Action::HalfPageScrollUp]),
        },
        CliAction::HalfPageScrollDown { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::HalfPageScrollDownByPaneId { pane_id }])
            },
            None => Ok(vec![Action::HalfPageScrollDown]),
        },
        CliAction::ToggleFullscreen { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::ToggleFocusFullscreenByPaneId { pane_id }])
            },
            None => Ok(vec![Action::ToggleFocusFullscreen]),
        },
        CliAction::ToggleNoUiFullscreen { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::ToggleFocusNoUiFullscreenByPaneId { pane_id }])
            },
            None => Ok(vec![Action::ToggleFocusNoUiFullscreen]),
        },
        CliAction::TogglePaneFrames => Ok(vec![Action::TogglePaneFrames]),
        CliAction::SetPaneFrameStyle { style } => Ok(vec![Action::SetPaneFrameStyle(style)]),
        CliAction::ToggleActiveSyncTab { tab_id } => match tab_id {
            Some(id) => Ok(vec![Action::ToggleActiveSyncTabByTabId { id }]),
            None => Ok(vec![Action::ToggleActiveSyncTab]),
        },
        CliAction::NewPane {
            direction,
            command,
            remote_host,

            plugin,
            cwd,
            floating,
            in_place,
            close_replaced_pane,
            pane_id,
            name,
            close_on_exit,
            start_suspended,
            configuration,
            configuration_json,
            skip_plugin_cache,
            x,
            y,
            width,
            height,
            pinned,
            stacked,
            blocking,
            block_until_exit_success,
            block_until_exit_failure,
            block_until_exit,
            unblock_condition,
            near_current_pane,
            no_focus,
            borderless,
            overlay,
            modal,
            tab_id,
        } => panes::new_pane(
            direction,
            command,
            remote_host,
            plugin,
            cwd,
            floating,
            in_place,
            close_replaced_pane,
            pane_id,
            name,
            close_on_exit,
            start_suspended,
            configuration,
            configuration_json,
            skip_plugin_cache,
            x,
            y,
            width,
            height,
            pinned,
            stacked,
            blocking,
            block_until_exit_success,
            block_until_exit_failure,
            block_until_exit,
            unblock_condition,
            near_current_pane,
            no_focus,
            borderless,
            overlay,
            modal,
            tab_id,
            get_current_dir.as_ref(),
        ),
        CliAction::Edit {
            direction,
            file,
            line_number,
            floating,
            in_place,
            close_replaced_pane,
            cwd,
            x,
            y,
            width,
            height,
            pinned,
            near_current_pane,
            no_focus,
            borderless,
            overlay,
            modal,
            tab_id,
        } => {
            let mut file = file;
            let current_dir = get_current_dir();
            let cwd = cwd.map(|cwd| current_dir.join(cwd)).or(Some(current_dir));
            if file.is_relative()
                && let Some(cwd) = cwd.as_ref()
            {
                file = cwd.join(file);
            }
            let start_suppressed = false;
            Ok(vec![Action::EditFile {
                payload: OpenFilePayload::new(file, line_number, cwd),
                direction,
                floating,
                in_place,
                close_replaced_pane,
                start_suppressed,
                placement: NewPanePlacement::floating_or_overlay(
                    x, y, width, height, pinned, borderless, overlay, modal,
                ),
                near_current_pane,
                no_focus,
                tab_id,
            }])
        },
        CliAction::SwitchMode { input_mode } => Ok(vec![Action::SwitchToMode { input_mode }]),
        CliAction::TogglePaneEmbedOrFloating { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::TogglePaneEmbedOrFloatingByPaneId { pane_id }])
            },
            None => Ok(vec![Action::TogglePaneEmbedOrFloating]),
        },
        CliAction::ToggleFloatingPanes { tab_id } => match tab_id {
            Some(id) => Ok(vec![Action::ToggleFloatingPanesByTabId { id }]),
            None => Ok(vec![Action::ToggleFloatingPanes]),
        },
        CliAction::ShowFloatingPanes { tab_id } => Ok(vec![Action::ShowFloatingPanes { tab_id }]),
        CliAction::HideFloatingPanes { tab_id } => Ok(vec![Action::HideFloatingPanes { tab_id }]),
        CliAction::AreFloatingPanesVisible { tab_id } => {
            Ok(vec![Action::AreFloatingPanesVisible { tab_id }])
        },
        CliAction::ClosePane { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::CloseFocusByPaneId { pane_id }])
            },
            None => Ok(vec![Action::CloseFocus]),
        },
        CliAction::RenamePane { name, pane_id } => {
            let pane_id = match pane_id {
                    Some(pane_id_str) => Some(
                        PaneId::from_str(&pane_id_str).map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?,
                    ),
                    None => None,
                };
            Ok(vec![Action::RenamePaneByPaneId {
                pane_id,
                name: name.as_bytes().to_vec(),
            }])
        },
        CliAction::UndoRenamePane { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::UndoRenamePaneByPaneId { pane_id }])
            },
            None => Ok(vec![Action::UndoRenamePane]),
        },
        CliAction::NewTabGroup { name } => Ok(vec![Action::NewNamedTabGroup { name }]),
        CliAction::ListTabGroups { json } => Ok(vec![Action::ListTabGroups { output_json: json }]),
        CliAction::CurrentTabGroupInfo { json } => {
            Ok(vec![Action::CurrentTabGroupInfo { output_json: json }])
        },
        CliAction::GoToTabGroupById { id } => Ok(vec![Action::SwitchTabGroup { id }]),
        CliAction::GoToTabGroupName { name, create } => {
            Ok(vec![Action::GoToTabGroupName { name, create }])
        },
        CliAction::RenameTabGroup { name, group_id } => Ok(vec![match group_id {
            Some(id) => Action::RenameTabGroup { id, name },
            None => Action::RenameCurrentTabGroup { name },
        }]),
        CliAction::CloseTabGroup { group_id } => Ok(vec![match group_id {
            Some(id) => Action::CloseTabGroup { id },
            None => Action::CloseCurrentTabGroup,
        }]),
        CliAction::RenameTabGroupById { id, name } => Ok(vec![Action::RenameTabGroup { id, name }]),
        CliAction::CloseTabGroupById { id } => Ok(vec![Action::CloseTabGroup { id }]),
        CliAction::GoToNextTabGroup => Ok(vec![Action::GoToNextTabGroup]),
        CliAction::GoToPreviousTabGroup => Ok(vec![Action::GoToPreviousTabGroup]),
        CliAction::GoToTabInGroup { index } => Ok(vec![Action::GoToTabInGroup { index }]),
        CliAction::MovePaneToNextTabGroup => Ok(vec![Action::MovePaneToNextTabGroup]),
        CliAction::MovePaneToPreviousTabGroup => Ok(vec![Action::MovePaneToPreviousTabGroup]),
        CliAction::GoToNextTab => Ok(vec![Action::GoToNextTab]),
        CliAction::GoToPreviousTab => Ok(vec![Action::GoToPreviousTab]),
        CliAction::CloseTab { tab_id } => match tab_id {
            Some(id) => Ok(vec![Action::CloseTabById { id }]),
            None => Ok(vec![Action::CloseTab]),
        },
        CliAction::GoToTab { index } => Ok(vec![Action::GoToTab { index }]),
        CliAction::GoToTabName { name, create } => Ok(vec![Action::GoToTabName { name, create }]),
        CliAction::RenameTab { name, tab_id } => match tab_id {
            Some(id) => Ok(vec![Action::RenameTabById { id, name }]),
            None => Ok(vec![
                Action::TabNameInput { input: vec![0] },
                Action::TabNameInput {
                    input: name.as_bytes().to_vec(),
                },
            ]),
        },
        CliAction::UndoRenameTab { tab_id } => match tab_id {
            Some(id) => Ok(vec![Action::UndoRenameTabByTabId { id }]),
            None => Ok(vec![Action::UndoRenameTab]),
        },
        CliAction::GoToTabById { id } => Ok(vec![Action::GoToTabById {
            id: TabId::try_from(id).map_err(|_| "Tab id is out of range")?,
        }]),
        CliAction::CloseTabById { id } => Ok(vec![Action::CloseTabById {
            id: TabId::try_from(id).map_err(|_| "Tab id is out of range")?,
        }]),
        CliAction::RenameTabById { id, name } => Ok(vec![Action::RenameTabById {
            id: TabId::try_from(id).map_err(|_| "Tab id is out of range")?,
            name,
        }]),
        CliAction::NewTab {
            name,
            layout,
            layout_string,
            layout_dir,
            cwd,
            initial_command,
            initial_plugin,
            close_on_exit,
            start_suspended,
            block_until_exit_success,
            block_until_exit_failure,
            block_until_exit,
            no_focus,
        } => layouts::new_tab(
            name,
            layout,
            layout_string,
            layout_dir,
            cwd,
            initial_command,
            initial_plugin,
            close_on_exit,
            start_suspended,
            block_until_exit_success,
            block_until_exit_failure,
            block_until_exit,
            no_focus,
            get_current_dir.as_ref(),
            config,
        ),
        CliAction::PreviousSwapLayout { tab_id } => match tab_id {
            Some(id) => Ok(vec![Action::PreviousSwapLayoutByTabId { id }]),
            None => Ok(vec![Action::PreviousSwapLayout]),
        },
        CliAction::NextSwapLayout { tab_id } => match tab_id {
            Some(id) => Ok(vec![Action::NextSwapLayoutByTabId { id }]),
            None => Ok(vec![Action::NextSwapLayout]),
        },
        CliAction::OverrideLayout {
            layout,
            layout_string,
            layout_dir,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            apply_only_to_active_tab,
        } => layouts::override_layout(
            layout,
            layout_string,
            layout_dir,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            apply_only_to_active_tab,
            config,
        ),
        CliAction::QueryTabNames => Ok(vec![Action::QueryTabNames]),
        CliAction::StartOrReloadPlugin {
            url,
            configuration,
            configuration_json,
        } => {
            let configuration = select_configuration(configuration, configuration_json)?;
            let current_dir = get_current_dir();
            let run_plugin_or_alias = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                &url,
                &configuration.map(PluginUserConfiguration::into_inner),
                None,
                Some(current_dir),
            )?;
            Ok(vec![Action::StartOrReloadPlugin {
                plugin: run_plugin_or_alias,
            }])
        },
        CliAction::LaunchOrFocusPlugin {
            url,
            floating,
            in_place,
            close_replaced_pane,
            move_to_focused_tab,
            configuration,
            configuration_json,
            skip_plugin_cache,
            tab_id,
        } => {
            let configuration = select_configuration(configuration, configuration_json)?;
            let current_dir = get_current_dir();
            let run_plugin_or_alias = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                url.as_str(),
                &configuration.map(PluginUserConfiguration::into_inner),
                None,
                Some(current_dir),
            )?;
            Ok(vec![Action::LaunchOrFocusPlugin {
                plugin: run_plugin_or_alias,
                should_float: floating,
                move_to_focused_tab,
                should_open_in_place: in_place,
                close_replaced_pane,
                skip_cache: skip_plugin_cache,
                tab_id,
            }])
        },
        CliAction::LaunchPlugin {
            url,
            floating,
            in_place,
            close_replaced_pane,
            configuration,
            configuration_json,
            skip_plugin_cache,
            no_focus,
            tab_id,
        } => {
            let configuration = select_configuration(configuration, configuration_json)?;
            let current_dir = get_current_dir();
            let run_plugin_or_alias = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                url.as_str(),
                &configuration.map(PluginUserConfiguration::into_inner),
                None,
                Some(current_dir.clone()),
            )?;
            Ok(vec![Action::LaunchPlugin {
                plugin: run_plugin_or_alias,
                should_float: floating,
                should_open_in_place: in_place,
                close_replaced_pane,
                skip_cache: skip_plugin_cache,
                cwd: Some(current_dir),
                no_focus,
                tab_id,
            }])
        },
        CliAction::RenameSession { name } => Ok(vec![Action::RenameSession { name: name.into() }]),
        CliAction::Reconfigure { kdl, file, write } => {
            let fragment = match (kdl, file) {
                (Some(_), Some(_)) => {
                    return Err("give the config as --kdl or --file, not both".to_string())
                },
                (Some(kdl), None) => kdl,
                (None, Some(file)) => std::fs::read_to_string(&file)
                    .map_err(|e| format!("failed to read {}: {e}", file.display()))?,
                (None, None) => {
                    use std::io::Read;
                    let mut kdl = String::new();
                    std::io::stdin().read_to_string(&mut kdl).map_err(|e| {
                        format!("failed to read the config fragment from stdin: {e}")
                    })?;
                    kdl
                },
            };
            Ok(vec![reconfigure_action(fragment, write, config)?])
        },
        CliAction::SetHook { point, url, write } => {
            use std::str::FromStr;
            let point = yinz_types::input::hooks::HookPoint::from_str(&point)?;
            let fragment = format!("hooks {{\n    {} {}\n}}\n", point, kdl_string(&url));
            Ok(vec![reconfigure_action(fragment, write, config)?])
        },
        CliAction::Wait(_) => Err("waits require a subscribing CLI client".into()),
        CliAction::Operations(command) => Ok(vec![command.action(&mut std::io::stdin())?]),
        CliAction::Projects(command) => Ok(vec![command.action()?]),
        CliAction::Notify { text } => Ok(vec![Action::Notify(text)]),
        CliAction::Pipe {
            focus_plugin,
            name,
            payload,
            args,
            plugin,
            plugin_configuration,
            configuration_json,
            force_launch_plugin,
            skip_plugin_cache,
            floating_plugin,
            in_place_plugin,
            plugin_cwd,
            plugin_title,
        } => {
            let plugin_configuration =
                select_configuration(plugin_configuration, configuration_json)?;
            let current_dir = get_current_dir();
            let cwd = plugin_cwd
                .map(|cwd| current_dir.join(cwd))
                .or(Some(current_dir));
            let skip_cache = skip_plugin_cache;
            let pipe_id = Uuid::new_v4().to_string();
            Ok(vec![Action::CliPipe {
                focus_plugin,
                pipe_id,
                name,
                payload,
                args: args.map(PluginUserConfiguration::into_inner),
                plugin,
                configuration: plugin_configuration.map(PluginUserConfiguration::into_inner),
                launch_new: force_launch_plugin,
                floating: floating_plugin,
                in_place: in_place_plugin,
                cwd,
                pane_title: plugin_title,
                skip_cache,
            }])
        },
        CliAction::ListClients => Ok(vec![Action::ListClients]),
        CliAction::ListPanes {
            tab,
            command,
            state,
            geometry,
            all,
            json,
        } => Ok(vec![Action::ListPanes {
            show_tab: tab,
            show_command: command,
            show_state: state,
            show_geometry: geometry,
            show_all: all,
            output_json: json,
        }]),
        CliAction::ListTabs {
            state,
            dimensions,
            panes,
            layout,
            all,
            json,
        } => Ok(vec![Action::ListTabs {
            show_state: state,
            show_dimensions: dimensions,
            show_panes: panes,
            show_layout: layout,
            show_all: all,
            output_json: json,
        }]),
        CliAction::CurrentTabInfo { json } => {
            Ok(vec![Action::CurrentTabInfo { output_json: json }])
        },
        CliAction::ArrangePanesByProject => Ok(vec![Action::ArrangePanesByProject]),
        CliAction::TogglePanePinned { pane_id } => match pane_id {
            Some(pane_id_str) => {
                let pane_id = PaneId::from_str(&pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {pane_id_str}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)"
                        ))?;
                Ok(vec![Action::TogglePanePinnedByPaneId { pane_id }])
            },
            None => Ok(vec![Action::TogglePanePinned]),
        },
        CliAction::StackPanes { pane_ids } => {
            let mut malformed_ids = vec![];
            let pane_ids = pane_ids
                .iter()
                .filter_map(
                    |stringified_pane_id| match PaneId::from_str(stringified_pane_id) {
                        Ok(pane_id) => Some(pane_id),
                        Err(_e) => {
                            malformed_ids.push(stringified_pane_id.to_owned());
                            None
                        },
                    },
                )
                .collect();
            if !malformed_ids.is_empty() {
                Err(
                        format!(
                            "Malformed pane ids: {}, expecting a space separated list of either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                            malformed_ids.join(", ")
                        )
                    )
            } else {
                Ok(vec![Action::StackPanes { pane_ids }])
            }
        },
        CliAction::ChangeFloatingPaneCoordinates {
            pane_id,
            x,
            y,
            width,
            height,
            pinned,
            borderless,
        } => {
            let Some(coordinates) =
                FloatingPaneCoordinates::new(x, y, width, height, pinned, borderless)
            else {
                return Err("Failed to parse floating pane coordinates".to_string());
            };
            let parsed_pane_id = PaneId::from_str(&pane_id);
            match parsed_pane_id {
                    Ok(parsed_pane_id) => {
                        Ok(vec![Action::ChangeFloatingPaneCoordinates {
                            pane_id: parsed_pane_id,
                            coordinates,
                        }])
                    },
                    Err(_e) => {
                        Err(format!(
                            "Malformed pane id: {}, expecting a space separated list of either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                            pane_id
                        ))
                    }
                }
        },
        CliAction::TogglePaneBorderless { pane_id } => {
            let parsed_pane_id = PaneId::from_str(&pane_id);
            match parsed_pane_id {
                    Ok(parsed_pane_id) => {
                        Ok(vec![Action::TogglePaneBorderless {
                            pane_id: parsed_pane_id,
                        }])
                    },
                    Err(_e) => {
                        Err(format!(
                            "Malformed pane id: {}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                            pane_id
                        ))
                    }
                }
        },
        CliAction::SetPaneBorderless {
            pane_id,
            borderless,
        } => {
            let parsed_pane_id = PaneId::from_str(&pane_id);
            match parsed_pane_id {
                    Ok(parsed_pane_id) => {
                        Ok(vec![Action::SetPaneBorderless {
                            pane_id: parsed_pane_id,
                            borderless,
                        }])
                    },
                    Err(_e) => {
                        Err(format!(
                            "Malformed pane id: {}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                            pane_id
                        ))
                    }
                }
        },
        CliAction::AgentReport {
            pane_id,
            kind,
            state,
            detail,
            cwd,
            hook_json,
            clear,
            run_id,
        } => agents::agent_report(pane_id, kind, state, detail, cwd, hook_json, clear, run_id),
        CliAction::SetPaneColor {
            pane_id,
            fg,
            bg,
            reset,
        } => {
            let pane_id_str = match pane_id {
                Some(id) => id,
                None => return Err("No --pane-id provided and no resolved caller pane".to_string()),
            };
            let parsed_pane_id = PaneId::from_str(&pane_id_str);
            match parsed_pane_id {
                    Ok(parsed_pane_id) => {
                        let (fg, bg) = if reset {
                            (None, None)
                        } else {
                            (fg, bg)
                        };
                        Ok(vec![Action::SetPaneColor {
                            pane_id: parsed_pane_id,
                            fg,
                            bg,
                        }])
                    },
                    Err(_e) => Err(format!(
                        "Malformed pane id: {}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                        pane_id_str
                    )),
                }
        },
        CliAction::Detach => Ok(vec![Action::Detach]),
        CliAction::SetDarkTheme => Ok(vec![Action::SetDarkTheme]),
        CliAction::SetLightTheme => Ok(vec![Action::SetLightTheme]),
        CliAction::ToggleTheme => Ok(vec![Action::ToggleTheme]),
        CliAction::SwitchSession {
            name,
            tab_position,
            pane_id,
            layout,
            layout_string,
            layout_dir,
            cwd,
        } => layouts::switch_session(
            name,
            tab_position,
            pane_id,
            layout,
            layout_string,
            layout_dir,
            cwd,
            get_current_dir.as_ref(),
            config,
        ),
    }
}
fn reconfigure_action(
    fragment: String,
    write_to_disk: bool,
    config: Option<Config>,
) -> Result<Action, String> {
    let base = config.unwrap_or_default();
    Config::from_kdl(&fragment, Some(base))
        .map_err(|e| format!("the config fragment does not parse: {e}"))?;
    Ok(Action::Reconfigure {
        config: fragment,
        write_to_disk,
    })
}
fn kdl_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn select_configuration(
    legacy: Option<PluginUserConfiguration>,
    json: Option<PluginUserConfiguration>,
) -> Result<Option<PluginUserConfiguration>, String> {
    match (legacy, json) {
        (Some(_), Some(_)) => {
            Err("legacy configuration and --configuration-json are mutually exclusive".into())
        },
        (legacy, json) => Ok(legacy.or(json)),
    }
}

#[cfg(test)]
#[path = "unit/actions.rs"]
mod tests;

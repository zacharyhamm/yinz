//! Definition of the actions that can be bound to keys.

use crate::args::CliAction;
use miette::{NamedSource, Report};
use uuid::Uuid;
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabPosition;
use yinz_types::keys::KeyWithModifier;
use {
    yinz_config::config::Config, yinz_config::config::ConfigError, yinz_config::config::KdlError,
};
use {yinz_runtime::home::find_default_config_dir, yinz_runtime::home::get_layout_dir};
use {
    yinz_types::data::CommandOrPlugin, yinz_types::data::LayoutInfo,
    yinz_types::data::NewPanePlacement, yinz_types::data::PaneId,
    yinz_types::data::UnblockCondition,
};
pub use {
    yinz_types::input::command::OpenFilePayload, yinz_types::input::command::RunCommandAction,
};
use {
    yinz_types::input::layout::PluginAlias, yinz_types::input::layout::PluginUserConfiguration,
    yinz_types::input::layout::RunPlugin, yinz_types::input::layout::RunPluginOrAlias,
    yinz_types::input::layout::TabLayoutInfo,
};

use std::path::PathBuf;
use std::str::FromStr;

use yinz_types::input::actions::*;
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
        CliAction::SendKeys { keys, pane_id } => {
            let mut actions = Vec::new();

            for (index, key_str) in keys.iter().enumerate() {
                let key = KeyWithModifier::from_str(key_str).map_err(|e| {
                    let suggestion = suggest_key_fix(key_str);
                    format!(
                        "Invalid key at position {}: \"{}\"\n  Error: {}\n{}",
                        index + 1,
                        key_str,
                        e,
                        suggestion
                    )
                })?;

                #[cfg(not(target_family = "wasm"))]
                let bytes = yinz_key_codec::keys::serialize_kitty(&key)
                    .map(|s| s.into_bytes())
                    .unwrap_or_default();

                #[cfg(target_family = "wasm")]
                let bytes = vec![];

                match &pane_id {
                    Some(pane_id_str) => {
                        let parsed_pane_id = PaneId::from_str(pane_id_str)
                                .map_err(|_| format!(
                                    "Malformed pane id: {}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                                    pane_id_str
                                ))?;
                        actions.push(Action::WriteToPaneId {
                            bytes,
                            pane_id: parsed_pane_id,
                        });
                    },
                    None => {
                        actions.push(Action::Write {
                            key_with_modifier: Some(key),
                            bytes,
                            is_kitty_keyboard_protocol: true,
                        });
                    },
                }
            }

            Ok(actions)
        },
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
        } => {
            let configuration = select_configuration(configuration, configuration_json)?;
            let target = remote_host
                .as_deref()
                .map(yinz_types::command::PaneTarget::ssh)
                .transpose()?
                .unwrap_or_default();
            let pane_id_to_replace = match pane_id {
                    Some(pane_id_str) => match PaneId::from_str(&pane_id_str) {
                        Ok(parsed_pane_id) => Some(parsed_pane_id),
                        Err(_e) => {
                            return Err(format!(
                                "Malformed pane id: {}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                                pane_id_str
                            ))
                        },
                    },
                    None => None,
                };
            let current_dir = get_current_dir();
            // cwd should only be specified in a plugin alias if it was explicitly given to us,
            // otherwise the current_dir might override a cwd defined in the alias itself
            let alias_cwd = cwd.clone().map(|cwd| current_dir.join(cwd));
            // A remote pane's cwd names a directory on the *other* machine,
            // so this machine's working directory is neither a sensible
            // default for it nor a sensible base to resolve a relative path
            // against. Left exactly as given: `None` then means "wherever
            // that machine logs you in", which is what ssh does.
            let command_cwd = if target.is_remote() {
                cwd.clone()
            } else {
                cwd.clone()
                    .map(|cwd| current_dir.join(cwd))
                    .or_else(|| Some(current_dir.clone()))
            };
            let cwd = cwd
                .map(|cwd| current_dir.join(cwd))
                .or_else(|| Some(current_dir.clone()));
            // One construction for both the blocking and the ordinary pane,
            // so that "no command, but a host" means the same thing in each.
            let terminal_run_command = if !command.is_empty() {
                let mut command = command.clone();
                let (command, args) = (PathBuf::from(command.remove(0)), command);
                Some(RunCommandAction {
                    command,
                    args,
                    cwd: command_cwd,
                    direction,
                    hold_on_close: !close_on_exit,
                    hold_on_start: start_suspended,
                    target: target.clone(),
                    ..Default::default()
                })
            } else if target.is_remote() {
                // No command and a host: that machine's login shell. Not
                // held on close — `ssh host` ends when you log out, and so
                // does a local default-shell pane; holding it would leave a
                // "Waiting to run" banner every time.
                Some(RunCommandAction {
                    command: PathBuf::new(),
                    args: vec![],
                    cwd: command_cwd,
                    direction,
                    hold_on_close: false,
                    hold_on_start: false,
                    target: target.clone(),
                    remote_default_shell: true,
                    ..Default::default()
                })
            } else {
                None
            };
            let unblock_condition = unblock_condition.or({
                if block_until_exit_success {
                    Some(UnblockCondition::OnExitSuccess)
                } else if block_until_exit_failure {
                    Some(UnblockCondition::OnExitFailure)
                } else if block_until_exit {
                    Some(UnblockCondition::OnAnyExit)
                } else {
                    None
                }
            });
            if blocking || unblock_condition.is_some() {
                // For blocking panes, we don't support plugins
                if plugin.is_some() {
                    return Err("Blocking panes do not support plugin variants".to_string());
                }

                let command = terminal_run_command;

                let placement = if floating {
                    NewPanePlacement::floating_or_overlay(
                        x, y, width, height, pinned, borderless, overlay, modal,
                    )
                } else if in_place {
                    NewPanePlacement::InPlace {
                        pane_id_to_replace,
                        close_replaced_pane,
                        borderless,
                    }
                } else if stacked {
                    NewPanePlacement::Stacked {
                        pane_id_to_stack_under: None,
                        borderless,
                    }
                } else {
                    NewPanePlacement::Tiled {
                        direction,
                        borderless,
                    }
                };

                Ok(vec![Action::NewBlockingPane {
                    placement,
                    pane_name: name,
                    command,
                    unblock_condition,
                    near_current_pane,
                    no_focus,
                    tab_id,
                }])
            } else if let Some(plugin) = plugin {
                let plugin = match yinz_runtime::plugin_location::run_plugin_from_url_with_cwd(
                    &plugin,
                    cwd.clone(),
                ) {
                    Ok(run_plugin) => {
                        let user_configuration = configuration.unwrap_or_default();
                        RunPluginOrAlias::RunPlugin(RunPlugin {
                            configuration: user_configuration,
                            initial_cwd: cwd.clone(),
                            ..run_plugin
                        })
                    },
                    Err(_) => {
                        let mut plugin_alias = PluginAlias::new(
                            &plugin,
                            &configuration.map(PluginUserConfiguration::into_inner),
                            alias_cwd,
                        );
                        plugin_alias.set_caller_cwd_if_not_set(Some(current_dir));
                        RunPluginOrAlias::Alias(plugin_alias)
                    },
                };
                if floating {
                    Ok(vec![Action::NewFloatingPluginPane {
                        plugin,
                        pane_name: name,
                        skip_cache: skip_plugin_cache,
                        cwd,
                        placement: NewPanePlacement::floating_or_overlay(
                            x, y, width, height, pinned, borderless, overlay, modal,
                        ),
                        no_focus,
                        tab_id,
                    }])
                } else if in_place {
                    Ok(vec![Action::NewInPlacePluginPane {
                        plugin,
                        pane_name: name,
                        skip_cache: skip_plugin_cache,
                        close_replaced_pane,
                        no_focus,
                        tab_id,
                    }])
                } else {
                    // it is intentional that a new tiled plugin pane cannot include a
                    // direction
                    // this is because the cli client opening a tiled plugin pane is a
                    // different client than the one opening the pane, and this can potentially
                    // create very confusing races if the client changes focus while the plugin
                    // is being loaded
                    // this is not the case with terminal panes for historical reasons of
                    // backwards compatibility to a time before we had auto layouts
                    Ok(vec![Action::NewTiledPluginPane {
                        plugin,
                        pane_name: name,
                        skip_cache: skip_plugin_cache,
                        cwd,
                        no_focus,
                        tab_id,
                    }])
                }
            } else if let Some(run_command_action) = terminal_run_command {
                if floating {
                    Ok(vec![Action::NewFloatingPane {
                        command: Some(run_command_action),
                        pane_name: name,
                        placement: NewPanePlacement::floating_or_overlay(
                            x, y, width, height, pinned, borderless, overlay, modal,
                        ),
                        near_current_pane,
                        no_focus,
                        tab_id,
                    }])
                } else if in_place {
                    Ok(vec![Action::NewInPlacePane {
                        command: Some(run_command_action),
                        pane_name: name,
                        near_current_pane,
                        no_focus,
                        pane_id_to_replace,
                        close_replaced_pane,
                        tab_id,
                    }])
                } else if stacked {
                    Ok(vec![Action::NewStackedPane {
                        command: Some(run_command_action),
                        pane_name: name,
                        near_current_pane,
                        no_focus,
                        tab_id,
                    }])
                } else {
                    Ok(vec![Action::NewTiledPane {
                        direction,
                        command: Some(run_command_action),
                        pane_name: name,
                        near_current_pane,
                        no_focus,
                        borderless,
                        tab_id,
                    }])
                }
            } else {
                if floating {
                    Ok(vec![Action::NewFloatingPane {
                        command: None,
                        pane_name: name,
                        placement: NewPanePlacement::floating_or_overlay(
                            x, y, width, height, pinned, borderless, overlay, modal,
                        ),
                        near_current_pane,
                        no_focus,
                        tab_id,
                    }])
                } else if in_place {
                    Ok(vec![Action::NewInPlacePane {
                        command: None,
                        pane_name: name,
                        near_current_pane,
                        no_focus,
                        pane_id_to_replace,
                        close_replaced_pane,
                        tab_id,
                    }])
                } else if stacked {
                    Ok(vec![Action::NewStackedPane {
                        command: None,
                        pane_name: name,
                        near_current_pane,
                        no_focus,
                        tab_id,
                    }])
                } else {
                    Ok(vec![Action::NewTiledPane {
                        direction,
                        command: None,
                        pane_name: name,
                        near_current_pane,
                        no_focus,
                        borderless,
                        tab_id,
                    }])
                }
            }
        },
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
            if file.is_relative() {
                if let Some(cwd) = cwd.as_ref() {
                    file = cwd.join(file);
                }
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
        } => {
            let current_dir = get_current_dir();
            let cwd = cwd
                .map(|cwd| current_dir.join(cwd))
                .or_else(|| Some(current_dir.clone()));

            // Map CLI flags to UnblockCondition
            let first_pane_unblock_condition = if block_until_exit_success {
                Some(UnblockCondition::OnExitSuccess)
            } else if block_until_exit_failure {
                Some(UnblockCondition::OnExitFailure)
            } else if block_until_exit {
                Some(UnblockCondition::OnAnyExit)
            } else {
                None
            };

            // Parse initial_panes from initial_command or initial_plugin
            let initial_panes = if let Some(plugin_url) = initial_plugin {
                let plugin = match yinz_runtime::plugin_location::run_plugin_from_url_with_cwd(
                    &plugin_url,
                    cwd.clone(),
                ) {
                    Ok(run_plugin) => RunPluginOrAlias::RunPlugin(RunPlugin {
                        initial_cwd: cwd.clone(),
                        ..run_plugin
                    }),
                    Err(_) => {
                        let mut plugin_alias = PluginAlias::new(&plugin_url, &None, cwd.clone());
                        plugin_alias.set_caller_cwd_if_not_set(Some(current_dir.clone()));
                        RunPluginOrAlias::Alias(plugin_alias)
                    },
                };
                Some(vec![CommandOrPlugin::Plugin(plugin)])
            } else if !initial_command.is_empty() {
                let mut command: Vec<String> = initial_command.clone();
                let (command, args) = (
                    PathBuf::from(command.remove(0)),
                    command.into_iter().collect(),
                );
                let hold_on_close = !close_on_exit;
                let hold_on_start = start_suspended;
                let run_command_action = RunCommandAction {
                    command,
                    args,
                    cwd: cwd.clone(),
                    direction: None,
                    hold_on_close,
                    hold_on_start,
                    ..Default::default()
                };
                Some(vec![CommandOrPlugin::Command(run_command_action)])
            } else {
                None
            };
            if let Some(raw_layout) = layout_string {
                let layout_source_name = "layout-string".to_owned();
                let path_to_raw_layout = layout_source_name.clone();
                let swap_layouts: Option<(String, String)> = None;
                let raw_layout_for_error = raw_layout.clone();
                let layout = yinz_config::layout::from_str(&raw_layout, path_to_raw_layout, swap_layouts.as_ref().map(|(f, p)| (f.as_str(), p.as_str())), cwd).map_err(|e| {
                        let stringified_error = match e {
                            ConfigError::KdlError(kdl_error) => {
                                let error = kdl_error.add_src(layout_source_name.clone(), raw_layout_for_error);
                                let report: Report = error.into();
                                format!("{:?}", report)
                            }
                            ConfigError::KdlDeserializationError(kdl_error) => {
                                let error_message = match kdl_error.kind {
                                    kdl::KdlErrorKind::Context("valid node terminator") => {
                                        format!("Failed to deserialize KDL node. \nPossible reasons:\n{}\n{}\n{}\n{}",
                                        "- Missing `;` after a node name, eg. { node; another_node; }",
                                        "- Missing quotations (\") around an argument node eg. { first_node \"argument_node\"; }",
                                        "- Missing an equal sign (=) between node arguments on a title line. eg. argument=\"value\"",
                                        "- Found an extraneous equal sign (=) between node child arguments and their values. eg. { argument=\"value\" }")
                                    },
                                    _ => String::from(kdl_error.help.unwrap_or("Kdl Deserialization Error")),
                                };
                                let kdl_error = KdlError {
                                    error_message,
                                    src: Some(NamedSource::new(layout_source_name.clone(), raw_layout_for_error)),
                                    offset: Some(kdl_error.span.offset()),
                                    len: Some(kdl_error.span.len()),
                                    help_message: None,
                                };
                                let report: Report = kdl_error.into();
                                format!("{:?}", report)
                            },
                            e => format!("{}", e)
                        };
                        stringified_error
                    })?;
                let mut tabs = layout.tabs();
                if !tabs.is_empty() {
                    let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
                    let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());
                    let mut new_tab_actions = vec![];
                    let mut has_focused_tab = tabs
                        .iter()
                        .any(|(_, layout, _)| layout.focus.unwrap_or(false));
                    for (tab_name, mut layout, floating_panes_layout) in tabs.drain(..) {
                        // A `--name` filling in for a layout that gave none is the user's.
                        layout.user_named |= tab_name.is_none() && name.is_some();
                        let name = tab_name.or_else(|| name.clone());
                        let should_change_focus_to_new_tab = !no_focus
                            && layout.focus.unwrap_or_else(|| {
                                if !has_focused_tab {
                                    has_focused_tab = true;
                                    true
                                } else {
                                    false
                                }
                            });
                        new_tab_actions.push(Action::NewTab {
                            layout_source: None,
                            tiled_layout: Some(layout),
                            floating_layouts: floating_panes_layout,
                            swap_tiled_layouts: swap_tiled_layouts.clone(),
                            swap_floating_layouts: swap_floating_layouts.clone(),
                            tab_name: name,
                            should_change_focus_to_new_tab,
                            cwd: None,
                            initial_panes: initial_panes.clone(),
                            first_pane_unblock_condition,
                        });
                    }
                    Ok(new_tab_actions)
                } else {
                    let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
                    let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());
                    let (mut layout, floating_panes_layout) = layout.new_tab();
                    layout.user_named |= name.is_some();
                    let should_change_focus_to_new_tab = !no_focus;
                    Ok(vec![Action::NewTab {
                        layout_source: None,
                        tiled_layout: Some(layout),
                        floating_layouts: floating_panes_layout,
                        swap_tiled_layouts,
                        swap_floating_layouts,
                        tab_name: name,
                        should_change_focus_to_new_tab,
                        cwd: None,
                        initial_panes,
                        first_pane_unblock_condition,
                    }])
                }
            } else if let Some(layout_path) = layout {
                let layout_dir = layout_dir
                    .or_else(|| config.and_then(|c| c.options.layout_dir))
                    .or_else(|| get_layout_dir(find_default_config_dir()));

                let layout_source_name = layout_path
                    .as_path()
                    .as_os_str()
                    .to_string_lossy()
                    .to_string();
                let (path_to_raw_layout, raw_layout, swap_layouts) =
                    yinz_config::layout::stringified_from_path_or_default(
                        Some(&layout_path),
                        layout_dir,
                    )
                    .map_err(|e| format!("Failed to load layout: {}", e))?;
                let layout = yinz_config::layout::from_str(&raw_layout, path_to_raw_layout, swap_layouts.as_ref().map(|(f, p)| (f.as_str(), p.as_str())), cwd).map_err(|e| {
                        let stringified_error = match e {
                            ConfigError::KdlError(kdl_error) => {
                                let error = kdl_error.add_src(layout_source_name.clone(), raw_layout);
                                let report: Report = error.into();
                                format!("{:?}", report)
                            }
                            ConfigError::KdlDeserializationError(kdl_error) => {
                                let error_message = match kdl_error.kind {
                                    kdl::KdlErrorKind::Context("valid node terminator") => {
                                        format!("Failed to deserialize KDL node. \nPossible reasons:\n{}\n{}\n{}\n{}",
                                        "- Missing `;` after a node name, eg. { node; another_node; }",
                                        "- Missing quotations (\") around an argument node eg. { first_node \"argument_node\"; }",
                                        "- Missing an equal sign (=) between node arguments on a title line. eg. argument=\"value\"",
                                        "- Found an extraneous equal sign (=) between node child arguments and their values. eg. { argument=\"value\" }")
                                    },
                                    _ => String::from(kdl_error.help.unwrap_or("Kdl Deserialization Error")),
                                };
                                let kdl_error = KdlError {
                                    error_message,
                                    src: Some(NamedSource::new(layout_source_name.clone(), raw_layout)),
                                    offset: Some(kdl_error.span.offset()),
                                    len: Some(kdl_error.span.len()),
                                    help_message: None,
                                };
                                let report: Report = kdl_error.into();
                                format!("{:?}", report)
                            },
                            e => format!("{}", e)
                        };
                        stringified_error
                    })?;
                let mut tabs = layout.tabs();
                if !tabs.is_empty() {
                    let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
                    let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());
                    let mut new_tab_actions = vec![];
                    let mut has_focused_tab = tabs
                        .iter()
                        .any(|(_, layout, _)| layout.focus.unwrap_or(false));
                    for (tab_name, mut layout, floating_panes_layout) in tabs.drain(..) {
                        // A `--name` filling in for a layout that gave none is the user's.
                        layout.user_named |= tab_name.is_none() && name.is_some();
                        let name = tab_name.or_else(|| name.clone());
                        let should_change_focus_to_new_tab = !no_focus
                            && layout.focus.unwrap_or_else(|| {
                                if !has_focused_tab {
                                    has_focused_tab = true;
                                    true
                                } else {
                                    false
                                }
                            });
                        new_tab_actions.push(Action::NewTab {
                            layout_source: None,
                            tiled_layout: Some(layout),
                            floating_layouts: floating_panes_layout,
                            swap_tiled_layouts: swap_tiled_layouts.clone(),
                            swap_floating_layouts: swap_floating_layouts.clone(),
                            tab_name: name,
                            should_change_focus_to_new_tab,
                            cwd: None, // the cwd is done through the layout
                            initial_panes: initial_panes.clone(),
                            first_pane_unblock_condition,
                        });
                    }
                    Ok(new_tab_actions)
                } else {
                    let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
                    let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());
                    let (mut layout, floating_panes_layout) = layout.new_tab();
                    layout.user_named |= name.is_some();
                    let should_change_focus_to_new_tab = !no_focus;
                    Ok(vec![Action::NewTab {
                        layout_source: None,
                        tiled_layout: Some(layout),
                        floating_layouts: floating_panes_layout,
                        swap_tiled_layouts,
                        swap_floating_layouts,
                        tab_name: name,
                        should_change_focus_to_new_tab,
                        cwd: None, // the cwd is done through the layout
                        initial_panes,
                        first_pane_unblock_condition,
                    }])
                }
            } else {
                let should_change_focus_to_new_tab = !no_focus;
                Ok(vec![Action::NewTab {
                    layout_source: None,
                    tiled_layout: None,
                    floating_layouts: vec![],
                    swap_tiled_layouts: None,
                    swap_floating_layouts: None,
                    tab_name: name,
                    should_change_focus_to_new_tab,
                    cwd,
                    initial_panes,
                    first_pane_unblock_condition,
                }])
            }
        },
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
        } => {
            // Determine layout_dir: CLI arg > config > default
            let layout_dir = layout_dir
                .or_else(|| config.and_then(|c| c.options.layout_dir))
                .or_else(|| get_layout_dir(find_default_config_dir()));

            // Load layout from string or file path
            let layout_source_name;
            let (path_to_raw_layout, raw_layout, swap_layouts) = if let Some(raw) = layout_string {
                layout_source_name = "layout-string".to_owned();
                (layout_source_name.clone(), raw, None)
            } else if let Some(layout_path) = &layout {
                layout_source_name = layout_path
                    .as_path()
                    .as_os_str()
                    .to_string_lossy()
                    .to_string();
                yinz_config::layout::stringified_from_path_or_default(Some(layout_path), layout_dir)
                    .map_err(|e| format!("Failed to load layout: {}", e))?
            } else {
                return Err("Either layout or layout-string must be provided".to_string());
            };

            // Parse KDL layout
            let layout = yinz_config::layout::from_str(
                &raw_layout,
                path_to_raw_layout,
                swap_layouts.as_ref().map(|(f, p)| (f.as_str(), p.as_str())),
                None, // cwd
            )
            .map_err(|e| {
                let stringified_error = match e {
                    ConfigError::KdlError(kdl_error) => {
                        let error = kdl_error.add_src(layout_source_name.clone(), raw_layout);
                        let report: Report = error.into();
                        format!("{:?}", report)
                    },
                    ConfigError::KdlDeserializationError(kdl_error) => {
                        let error_message = kdl_error.to_string();
                        format!("Failed to deserialize KDL layout: {}", error_message)
                    },
                    e => format!("{}", e),
                };
                stringified_error
            })?;

            // Convert all tabs to Vec<TabLayoutInfo>
            let tabs: Vec<TabLayoutInfo> = layout
                .tabs
                .iter()
                .enumerate()
                .map(|(index, (tab_name, tiled, floating))| TabLayoutInfo {
                    tab_index: TabPosition::new(index),
                    tab_name: tab_name.clone(),
                    tiled_layout: tiled.clone(),
                    floating_layouts: floating.clone(),
                    swap_tiled_layouts: Some(layout.swap_tiled_layouts.clone()),
                    swap_floating_layouts: Some(layout.swap_floating_layouts.clone()),
                })
                .collect();

            // If no tabs, create default tab
            let tabs = if tabs.is_empty() {
                let (tiled, floating) = layout.new_tab();
                vec![TabLayoutInfo {
                    tab_index: TabPosition::new(0),
                    tab_name: None,
                    tiled_layout: tiled,
                    floating_layouts: floating,
                    swap_tiled_layouts: Some(layout.swap_tiled_layouts),
                    swap_floating_layouts: Some(layout.swap_floating_layouts),
                }]
            } else {
                tabs
            };

            Ok(vec![Action::OverrideLayout {
                tabs,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                apply_only_to_active_tab,
            }])
        },
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
        CliAction::NotesTodos(command) => Ok(vec![command.action(&mut std::io::stdin())?]),
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
        } => {
            let pane_id_str = match pane_id {
                Some(id) => id,
                None => return Err("No --pane-id provided and no resolved caller pane".to_string()),
            };
            let pane_id = PaneId::from_str(&pane_id_str).map_err(|_| {
                format!(
                    "Malformed pane id: {}, expecting either a bare integer (eg. 1), a \
                         terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                    pane_id_str
                )
            })?;
            let kind = kind
                .map(|kind| kind.trim().to_lowercase())
                .filter(|kind| !kind.is_empty())
                .unwrap_or_else(|| "claude".to_owned());
            // A hook event does two things, from the one process the hook may spend: it
            // sets the pane's record, and it is broadcast whole to the plugins, because
            // what a record holds is deliberately less than what an event carries. The
            // agent's own last message, the user's prompt and the permission mode are
            // read off the broadcast by whoever wants them - see `docs/AGENT_HOOKS.md`.
            if hook_json {
                use std::io::Read;
                let mut json = String::new();
                std::io::stdin()
                    .take(yinz_types::agent::NATIVE_HOOK_MAX_BYTES as u64 + 1)
                    .read_to_string(&mut json)
                    .map_err(|e| format!("failed to read the hook event from stdin: {e}"))?;
                if json.len() > yinz_types::agent::NATIVE_HOOK_MAX_BYTES {
                    return Err("hook input exceeds 2097152 bytes".into());
                }
                let (event, json) = yinz_runtime::agent::hooks::parse_hook_event(&kind, json)
                    .map_err(|e| format!("the hook event is not valid agent hook JSON: {e}"))?;
                return Ok(hook_report_actions(&kind, pane_id, &event, json));
            }
            let agent = if clear {
                None
            } else {
                let state = state.ok_or_else(|| {
                    "one of --state, --hook-json or --clear is required".to_string()
                })?;
                let state = AgentState::parse_lenient(&state).ok_or_else(|| {
                    format!("Unknown agent state '{state}': busy, waiting, idle or exited")
                })?;
                Some(AgentInfo {
                    kind,
                    state,
                    source: AgentSource::Reported,
                    detail: detail.filter(|detail| !detail.trim().is_empty()),
                    // `--detail` is the tool hint; a script that wants to say what it is
                    // blocked on says it there, and the hook road fills this in itself.
                    waiting_on: None,
                    // A script that troubles itself to say `waiting` is saying a person
                    // is wanted; there is no permission mode here to answer for it.
                    wait_needs_a_person: state == AgentState::Waiting,
                    cwd: cwd.map(|cwd| cwd.display().to_string()),
                    session_id: None,
                    // `None` is not "no run": a record that does not name one inherits
                    // the pane's, so a script that sets it once need not repeat it on
                    // every report. See `AgentInfo::run_id`.
                    run_id: run_id.filter(|run_id| !run_id.trim().is_empty()),
                })
            };
            Ok(vec![Action::SetPaneAgent { pane_id, agent }])
        },
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
        } => {
            let pane_id = match pane_id {
                Some(stringified_pane_id) => match PaneId::from_str(&stringified_pane_id) {
                    Ok(pane_id) => Some(pane_id),
                    Err(_e) => {
                        return Err(format!(
                                "Malformed pane id: {}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                                stringified_pane_id
                            ));
                    },
                },
                None => None,
            };

            let cwd = cwd.map(|cwd| {
                let current_dir = get_current_dir();
                current_dir.join(cwd)
            });

            let layout_dir = layout_dir.map(|layout_dir| {
                let current_dir = get_current_dir();
                current_dir.join(layout_dir)
            });

            let layout_info = if let Some(layout_string) = layout_string {
                // validate the layout string before sending it to the target session
                let layout_source_name = "layout-string".to_owned();
                let raw_layout_for_error = layout_string.clone();
                yinz_config::layout::from_str(&layout_string, layout_source_name.clone(), None, None)
                        .map_err(|e| {
                            match e {
                                ConfigError::KdlError(kdl_error) => {
                                    let error = kdl_error.add_src(layout_source_name, raw_layout_for_error);
                                    let report: Report = error.into();
                                    format!("{:?}", report)
                                },
                                ConfigError::KdlDeserializationError(kdl_error) => {
                                    let error_message = match kdl_error.kind {
                                        kdl::KdlErrorKind::Context("valid node terminator") => {
                                            format!("Failed to deserialize KDL node. \nPossible reasons:\n{}\n{}\n{}\n{}",
                                            "- Missing `;` after a node name, eg. {{ node; another_node; }}",
                                            "- Missing quotations (\") around an argument node eg. {{ first_node \"argument_node\"; }}",
                                            "- Missing an equal sign (=) between node arguments on a title line. eg. argument=\"value\"",
                                            "- Found an extraneous equal sign (=) between node child arguments and their values. eg. {{ argument=\"value\" }}")
                                        },
                                        _ => String::from(kdl_error.help.unwrap_or("Kdl Deserialization Error")),
                                    };
                                    let kdl_error = KdlError {
                                        error_message,
                                        src: Some(NamedSource::new(layout_source_name, raw_layout_for_error)),
                                        offset: Some(kdl_error.span.offset()),
                                        len: Some(kdl_error.span.len()),
                                        help_message: None,
                                    };
                                    let report: Report = kdl_error.into();
                                    format!("{:?}", report)
                                },
                                e => format!("{}", e),
                            }
                        })?;
                Some(LayoutInfo::Stringified(layout_string))
            } else if let Some(layout_path) = layout {
                let layout_dir = layout_dir
                    .or_else(|| config.and_then(|c| c.options.layout_dir.clone()))
                    .or_else(|| get_layout_dir(find_default_config_dir()));
                // validate the layout file before sending it to the target session
                let layout_source_name = layout_path.display().to_string();
                yinz_config::layout::from_path_or_default_without_config(
                        Some(&layout_path),
                        layout_dir.clone(),
                    )
                    .map_err(|e| {
                        match e {
                            ConfigError::KdlError(kdl_error) => {
                                let report: Report = kdl_error.into();
                                format!("{:?}", report)
                            },
                            ConfigError::KdlDeserializationError(kdl_error) => {
                                let error_message = match kdl_error.kind {
                                    kdl::KdlErrorKind::Context("valid node terminator") => {
                                        format!("Failed to deserialize KDL node. \nPossible reasons:\n{}\n{}\n{}\n{}",
                                        "- Missing `;` after a node name, eg. {{ node; another_node; }}",
                                        "- Missing quotations (\") around an argument node eg. {{ first_node \"argument_node\"; }}",
                                        "- Missing an equal sign (=) between node arguments on a title line. eg. argument=\"value\"",
                                        "- Found an extraneous equal sign (=) between node child arguments and their values. eg. {{ argument=\"value\" }}")
                                    },
                                    _ => String::from(kdl_error.help.unwrap_or("Kdl Deserialization Error")),
                                };
                                let kdl_error = KdlError {
                                    error_message,
                                    src: Some(NamedSource::new(layout_source_name, String::new())),
                                    offset: Some(kdl_error.span.offset()),
                                    len: Some(kdl_error.span.len()),
                                    help_message: None,
                                };
                                let report: Report = kdl_error.into();
                                format!("{:?}", report)
                            },
                            e => format!("{}", e),
                        }
                    })?;
                yinz_config::layout::layout_info_from_config(&layout_dir, &Some(layout_path))
            } else {
                None
            };

            Ok(vec![Action::SwitchSession {
                name: name.clone().into(),
                tab_position,
                pane_id,
                layout: layout_info,
                cwd,
            }])
        },
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
fn suggest_key_fix(key_str: &str) -> String {
    if key_str.contains('-') {
        return "  Hint: Use spaces instead of hyphens (e.g., \"Ctrl a\" not \"Ctrl-a\")"
            .to_string();
    }

    if key_str.trim().is_empty() {
        return "  Hint: Key string cannot be empty".to_string();
    }

    let parts: Vec<&str> = key_str.split_whitespace().collect();
    if parts.len() > 1 {
        for part in &parts[..parts.len() - 1] {
            let lower = part.to_ascii_lowercase();
            if lower.starts_with("ctr") && lower != "ctrl" {
                return format!("  Hint: Did you mean \"Ctrl\" instead of \"{}\"?", part);
            }
            if !matches!(lower.as_str(), "ctrl" | "alt" | "shift" | "super") {
                return "  Hint: Valid modifiers are: Ctrl, Alt, Shift, Super".to_string();
            }
        }
    }

    "  Hint: Use format like \"Ctrl a\", \"Alt Shift F1\", or \"Enter\"".to_string()
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
mod tests {
    use super::*;
    use std::path::PathBuf;
    use yinz_types::data::KeyModifier;
    use yinz_types::data::{BareKey, InputMode};
    use yinz_types::geometry::{Direction, Resize};

    #[test]
    fn actions_that_leave_a_clipboard_message() {
        assert!(Action::Copy.leaves_clipboard_message());
        assert!(Action::CopyLastCommandOutput.leaves_clipboard_message());
        // wrapping one does not change what it does to the clipboard
        assert!(Action::SkipConfirm {
            action: Box::new(Action::Copy),
        }
        .leaves_clipboard_message());
        assert!(!Action::SwitchToMode {
            input_mode: InputMode::Normal,
        }
        .leaves_clipboard_message());
        assert!(!Action::SkipConfirm {
            action: Box::new(Action::Quit),
        }
        .leaves_clipboard_message());
    }

    #[test]
    fn test_send_keys_single_key() {
        let cli_action = CliAction::SendKeys {
            keys: vec!["Enter".to_string()],
            pane_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Write {
                key_with_modifier,
                bytes,
                is_kitty_keyboard_protocol,
            } => {
                assert!(key_with_modifier.is_some());
                let key = key_with_modifier.as_ref().unwrap();
                assert_eq!(key.bare_key, BareKey::Enter);
                assert!(key.key_modifiers.is_empty());
                assert!(!bytes.is_empty());
                assert!(*is_kitty_keyboard_protocol);
            },
            _ => panic!("Expected Write action"),
        }
    }

    #[test]
    fn test_send_keys_with_modifier() {
        let cli_action = CliAction::SendKeys {
            keys: vec!["Ctrl a".to_string()],
            pane_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Write {
                key_with_modifier,
                is_kitty_keyboard_protocol,
                ..
            } => {
                assert!(key_with_modifier.is_some());
                let key = key_with_modifier.as_ref().unwrap();
                assert_eq!(key.bare_key, BareKey::Char('a'));
                assert!(key.key_modifiers.contains(&KeyModifier::Ctrl));
                assert!(*is_kitty_keyboard_protocol);
            },
            _ => panic!("Expected Write action"),
        }
    }

    #[test]
    fn test_send_keys_multiple_keys() {
        let cli_action = CliAction::SendKeys {
            keys: vec!["Ctrl a".to_string(), "F1".to_string(), "Enter".to_string()],
            pane_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 3);
        for action in &actions {
            match action {
                Action::Write {
                    is_kitty_keyboard_protocol,
                    ..
                } => {
                    assert!(*is_kitty_keyboard_protocol);
                },
                _ => panic!("Expected Write action"),
            }
        }
    }

    #[test]
    fn test_send_keys_error_hyphen_syntax() {
        let cli_action = CliAction::SendKeys {
            keys: vec!["Ctrl-a".to_string()],
            pane_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Use spaces instead of hyphens"));
    }

    #[test]
    fn test_send_keys_error_typo() {
        let cli_action = CliAction::SendKeys {
            keys: vec!["Ctrll a".to_string()],
            pane_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Ctrl") || err.contains("modifier"));
    }

    #[test]
    fn test_send_keys_with_pane_id() {
        let cli_action = CliAction::SendKeys {
            keys: vec!["a".to_string()],
            pane_id: Some("terminal_1".to_string()),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::WriteToPaneId { pane_id, bytes } => {
                assert_eq!(
                    *pane_id,
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1))
                );
                assert!(!bytes.is_empty());
            },
            _ => panic!("Expected WriteToPaneId action"),
        }
    }

    #[test]
    fn test_send_keys_error_invalid_pane_id() {
        let cli_action = CliAction::SendKeys {
            keys: vec!["a".to_string()],
            pane_id: Some("invalid_id".to_string()),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Malformed pane id"));
    }

    // =============================================
    // Category 1: Pane-targeting tests
    // =============================================

    // 1. ScrollUp
    #[test]
    fn test_resize_with_pane_id() {
        let cli_action = CliAction::Resize {
            resize: Resize::Increase,
            direction: Some(Direction::Left),
            pane_id: Some("terminal_3".to_string()),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::ResizeByPaneId {
                pane_id,
                resize,
                direction,
            } => {
                assert_eq!(
                    *pane_id,
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(3))
                );
                assert!(matches!(resize, Resize::Increase));
                assert!(matches!(direction, Some(Direction::Left)));
            },
            _ => panic!("Expected ResizeByPaneId action"),
        }
    }

    #[test]
    fn test_resize_without_pane_id() {
        let cli_action = CliAction::Resize {
            resize: Resize::Increase,
            direction: Some(Direction::Left),
            pane_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Resize { resize, direction } => {
                assert!(matches!(resize, Resize::Increase));
                assert!(matches!(direction, Some(Direction::Left)));
            },
            _ => panic!("Expected Resize action"),
        }
    }

    // 10. MovePane
    #[test]
    fn test_move_pane_with_pane_id() {
        let cli_action = CliAction::MovePane {
            direction: Some(Direction::Right),
            pane_id: Some("terminal_9".to_string()),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::MovePaneByPaneId { pane_id, direction } => {
                assert_eq!(
                    *pane_id,
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(9))
                );
                assert!(matches!(direction, Some(Direction::Right)));
            },
            _ => panic!("Expected MovePaneByPaneId action"),
        }
    }

    #[test]
    fn test_move_pane_without_pane_id() {
        let cli_action = CliAction::MovePane {
            direction: Some(Direction::Right),
            pane_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::MovePane { direction } => {
                assert!(matches!(direction, Some(Direction::Right)));
            },
            _ => panic!("Expected MovePane action"),
        }
    }

    // 11. MovePaneBackwards
    #[test]
    fn test_edit_scrollback_with_pane_id() {
        let cli_action = CliAction::EditScrollback {
            pane_id: Some("terminal_15".to_string()),
            ansi: false,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::EditScrollbackByPaneId { pane_id, ansi } => {
                assert_eq!(
                    *pane_id,
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(15))
                );
                assert!(!ansi);
            },
            _ => panic!("Expected EditScrollbackByPaneId action"),
        }
    }

    #[test]
    fn test_rename_pane_with_pane_id() {
        let cli_action = CliAction::RenamePane {
            name: "my-pane".to_string(),
            pane_id: Some("terminal_19".to_string()),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::RenamePaneByPaneId { pane_id, name } => {
                assert_eq!(
                    *pane_id,
                    Some(PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
                        19
                    )))
                );
                assert_eq!(name, &"my-pane".as_bytes().to_vec());
            },
            _ => panic!("Expected RenamePaneByPaneId action"),
        }
    }

    #[test]
    fn test_rename_pane_without_pane_id() {
        let cli_action = CliAction::RenamePane {
            name: "my-pane".to_string(),
            pane_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::RenamePaneByPaneId { pane_id, name } => {
                assert!(pane_id.is_none());
                assert_eq!(name, &"my-pane".as_bytes().to_vec());
            },
            _ => panic!("Expected RenamePaneByPaneId action"),
        }
    }

    // 18. UndoRenamePane
    #[test]
    fn test_scroll_up_with_invalid_pane_id() {
        let cli_action = CliAction::ScrollUp {
            pane_id: Some("invalid_id".to_string()),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Malformed pane id"));
    }

    // =============================================
    // Category 1: Tab-targeting tests
    // =============================================

    // 20. CloseTab
    #[test]
    fn test_close_tab_with_tab_id() {
        let cli_action = CliAction::CloseTab {
            tab_id: Some(TabId::new(5)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::CloseTabById { id } => {
                assert_eq!(*id, TabId::new(5));
            },
            _ => panic!("Expected CloseTabById action"),
        }
    }

    #[test]
    fn test_close_tab_without_tab_id() {
        let cli_action = CliAction::CloseTab { tab_id: None };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::CloseTab));
    }

    #[test]
    fn test_set_dark_theme_cli_to_action() {
        let result = crate::actions::actions_from_cli(
            CliAction::SetDarkTheme,
            Box::new(|| PathBuf::from("/tmp")),
            None,
        );
        let actions = result.expect("SetDarkTheme conversion should succeed");
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::SetDarkTheme));
    }

    #[test]
    fn test_set_light_theme_cli_to_action() {
        let result = crate::actions::actions_from_cli(
            CliAction::SetLightTheme,
            Box::new(|| PathBuf::from("/tmp")),
            None,
        );
        let actions = result.expect("SetLightTheme conversion should succeed");
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::SetLightTheme));
    }

    #[test]
    fn test_toggle_theme_cli_to_action() {
        let result = crate::actions::actions_from_cli(
            CliAction::ToggleTheme,
            Box::new(|| PathBuf::from("/tmp")),
            None,
        );
        let actions = result.expect("ToggleTheme conversion should succeed");
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::ToggleTheme));
    }

    // 21. RenameTab
    #[test]
    fn test_rename_tab_with_tab_id() {
        let cli_action = CliAction::RenameTab {
            name: "my-tab".to_string(),
            tab_id: Some(TabId::new(3)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::RenameTabById { id, name } => {
                assert_eq!(*id, TabId::new(3));
                assert_eq!(name, "my-tab");
            },
            _ => panic!("Expected RenameTabById action"),
        }
    }

    #[test]
    fn test_rename_tab_without_tab_id() {
        let cli_action = CliAction::RenameTab {
            name: "my-tab".to_string(),
            tab_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 2);
        assert!(matches!(actions[0], Action::TabNameInput { .. }));
        assert!(matches!(actions[1], Action::TabNameInput { .. }));
    }

    // 22. UndoRenameTab
    #[test]
    fn test_undo_rename_tab_with_tab_id() {
        let cli_action = CliAction::UndoRenameTab {
            tab_id: Some(TabId::new(7)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::UndoRenameTabByTabId { id } => {
                assert_eq!(*id, TabId::new(7));
            },
            _ => panic!("Expected UndoRenameTabByTabId action"),
        }
    }

    #[test]
    fn test_undo_rename_tab_without_tab_id() {
        let cli_action = CliAction::UndoRenameTab { tab_id: None };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::UndoRenameTab));
    }

    // 23. ToggleActiveSyncTab
    #[test]
    fn test_toggle_active_sync_tab_with_tab_id() {
        let cli_action = CliAction::ToggleActiveSyncTab {
            tab_id: Some(TabId::new(2)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::ToggleActiveSyncTabByTabId { id } => {
                assert_eq!(*id, TabId::new(2));
            },
            _ => panic!("Expected ToggleActiveSyncTabByTabId action"),
        }
    }

    #[test]
    fn test_toggle_active_sync_tab_without_tab_id() {
        let cli_action = CliAction::ToggleActiveSyncTab { tab_id: None };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::ToggleActiveSyncTab));
    }

    // 24. ToggleFloatingPanes
    #[test]
    fn test_toggle_floating_panes_with_tab_id() {
        let cli_action = CliAction::ToggleFloatingPanes {
            tab_id: Some(TabId::new(4)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::ToggleFloatingPanesByTabId { id } => {
                assert_eq!(*id, TabId::new(4));
            },
            _ => panic!("Expected ToggleFloatingPanesByTabId action"),
        }
    }

    #[test]
    fn test_toggle_floating_panes_without_tab_id() {
        let cli_action = CliAction::ToggleFloatingPanes { tab_id: None };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::ToggleFloatingPanes));
    }

    // 25. PreviousSwapLayout
    #[test]
    fn test_previous_swap_layout_with_tab_id() {
        let cli_action = CliAction::PreviousSwapLayout {
            tab_id: Some(TabId::new(6)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::PreviousSwapLayoutByTabId { id } => {
                assert_eq!(*id, TabId::new(6));
            },
            _ => panic!("Expected PreviousSwapLayoutByTabId action"),
        }
    }

    #[test]
    fn test_previous_swap_layout_without_tab_id() {
        let cli_action = CliAction::PreviousSwapLayout { tab_id: None };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::PreviousSwapLayout));
    }

    // 26. NextSwapLayout
    #[test]
    fn test_next_swap_layout_with_tab_id() {
        let cli_action = CliAction::NextSwapLayout {
            tab_id: Some(TabId::new(8)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::NextSwapLayoutByTabId { id } => {
                assert_eq!(*id, TabId::new(8));
            },
            _ => panic!("Expected NextSwapLayoutByTabId action"),
        }
    }

    #[test]
    fn test_next_swap_layout_without_tab_id() {
        let cli_action = CliAction::NextSwapLayout { tab_id: None };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::NextSwapLayout));
    }

    // 27. MoveTab
    #[test]
    fn test_move_tab_with_tab_id() {
        let cli_action = CliAction::MoveTab {
            direction: Direction::Right,
            tab_id: Some(TabId::new(10)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::MoveTabByTabId { id, direction } => {
                assert_eq!(*id, TabId::new(10));
                assert!(matches!(direction, Direction::Right));
            },
            _ => panic!("Expected MoveTabByTabId action"),
        }
    }

    #[test]
    fn test_move_tab_without_tab_id() {
        let cli_action = CliAction::MoveTab {
            direction: Direction::Right,
            tab_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::MoveTab { direction } => {
                assert!(matches!(direction, Direction::Right));
            },
            _ => panic!("Expected MoveTab action"),
        }
    }

    // 28. ANSI flag tests

    #[test]
    fn test_edit_scrollback_with_ansi_flag() {
        let cli_action = CliAction::EditScrollback {
            pane_id: None,
            ansi: true,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::EditScrollback { ansi: true }));
    }

    #[test]
    fn test_edit_scrollback_with_pane_id_and_ansi() {
        let cli_action = CliAction::EditScrollback {
            pane_id: Some("terminal_15".to_string()),
            ansi: true,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::EditScrollbackByPaneId { pane_id, ansi } => {
                assert_eq!(
                    *pane_id,
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(15))
                );
                assert!(*ansi);
            },
            _ => panic!("Expected EditScrollbackByPaneId action"),
        }
    }

    #[test]
    fn test_dump_screen_with_ansi_flag() {
        let cli_action = CliAction::DumpScreen {
            path: Some(PathBuf::from("/tmp/test")),
            full: true,
            pane_id: None,
            ansi: true,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::DumpScreen {
                ansi,
                include_scrollback,
                ..
            } => {
                assert!(*ansi);
                assert!(*include_scrollback);
            },
            _ => panic!("Expected DumpScreen action"),
        }
    }

    #[test]
    fn test_dump_screen_with_pane_id_and_ansi() {
        let cli_action = CliAction::DumpScreen {
            path: None,
            full: false,
            pane_id: Some("terminal_5".to_string()),
            ansi: true,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::DumpScreen { pane_id, ansi, .. } => {
                assert_eq!(
                    *pane_id,
                    Some(PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
                        5
                    )))
                );
                assert!(*ansi);
            },
            _ => panic!("Expected DumpScreen action"),
        }
    }

    #[test]
    fn test_focus_pane_id() {
        let cli_action = CliAction::FocusPaneId {
            pane_id: "terminal_7".to_string(),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::FocusPaneByPaneId { pane_id } => {
                assert_eq!(
                    *pane_id,
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7))
                );
            },
            _ => panic!("Expected FocusPaneByPaneId action"),
        }
    }

    #[test]
    fn test_focus_pane_id_bare_int() {
        let cli_action = CliAction::FocusPaneId {
            pane_id: "3".to_string(),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::FocusPaneByPaneId { pane_id } => {
                assert_eq!(
                    *pane_id,
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(3))
                );
            },
            _ => panic!("Expected FocusPaneByPaneId action"),
        }
    }

    #[test]
    fn test_focus_pane_id_plugin() {
        let cli_action = CliAction::FocusPaneId {
            pane_id: "plugin_2".to_string(),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::FocusPaneByPaneId { pane_id } => {
                assert_eq!(
                    *pane_id,
                    PaneId::Plugin(yinz_types::internal_ids::PluginId::new(2))
                );
            },
            _ => panic!("Expected FocusPaneByPaneId action"),
        }
    }

    #[test]
    fn test_focus_pane_id_malformed() {
        let cli_action = CliAction::FocusPaneId {
            pane_id: "invalid_id".to_string(),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_tab_with_layout_string() {
        let cli_action = CliAction::NewTab {
            name: None,
            layout: None,
            layout_string: Some("layout {\n    pane\n    pane\n}\n".into()),
            layout_dir: None,
            cwd: None,
            initial_command: vec![],
            initial_plugin: None,
            close_on_exit: Default::default(),
            start_suspended: Default::default(),
            block_until_exit: false,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            no_focus: false,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::NewTab {
                layout_source: None,
                tiled_layout,
                floating_layouts,
                ..
            } => {
                assert!(tiled_layout.is_some());
                let layout = tiled_layout.as_ref().unwrap();
                // layout { pane; pane } produces a layout with 2 children
                assert_eq!(layout.children.len(), 2);
                assert!(floating_layouts.is_empty());
            },
            _ => panic!("Expected NewTab action"),
        }
    }

    #[test]
    fn test_new_tab_with_invalid_layout_string() {
        let cli_action = CliAction::NewTab {
            name: None,
            layout: None,
            layout_string: Some("invalid { kdl".into()),
            layout_dir: None,
            cwd: None,
            initial_command: vec![],
            initial_plugin: None,
            close_on_exit: Default::default(),
            start_suspended: Default::default(),
            block_until_exit: false,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            no_focus: false,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_override_layout_with_layout_string() {
        let cli_action = CliAction::OverrideLayout {
            layout: None,
            layout_string: Some("layout {\n    pane\n    pane\n}\n".into()),
            layout_dir: None,
            retain_existing_terminal_panes: false,
            retain_existing_plugin_panes: false,
            apply_only_to_active_tab: false,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::OverrideLayout { tabs, .. } => {
                assert!(!tabs.is_empty());
            },
            _ => panic!("Expected OverrideLayout action"),
        }
    }

    #[test]
    fn test_switch_session_with_layout_string() {
        let cli_action = CliAction::SwitchSession {
            name: "test-session".into(),
            tab_position: None,
            pane_id: None,
            layout: None,
            layout_string: Some("layout {\n    pane\n}\n".into()),
            layout_dir: None,
            cwd: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::SwitchSession { layout, .. } => {
                assert!(matches!(
                    layout,
                    Some(yinz_types::data::LayoutInfo::Stringified(_))
                ));
            },
            _ => panic!("Expected SwitchSession action"),
        }
    }

    #[test]
    fn test_switch_session_with_invalid_layout_string() {
        let cli_action = CliAction::SwitchSession {
            name: "test-session".into(),
            tab_position: None,
            pane_id: None,
            layout: None,
            layout_string: Some("invalid { kdl".into()),
            layout_dir: None,
            cwd: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_err());
    }

    /// A remote pane asked for with no command is the far machine's login
    /// shell. It has to become a `RunCommandAction` all the same — a `None`
    /// command would drop the host on the floor and open a pane here.
    fn remote_new_pane(command: Vec<String>, cwd: Option<PathBuf>) -> CliAction {
        CliAction::NewPane {
            remote_host: Some("build-box".to_owned()),

            direction: None,
            command,
            plugin: None,
            cwd,
            floating: false,
            in_place: false,
            close_replaced_pane: false,
            pane_id: None,
            name: None,
            close_on_exit: false,
            start_suspended: false,
            configuration: None,
            configuration_json: None,
            skip_plugin_cache: false,
            x: None,
            y: None,
            width: None,
            height: None,
            pinned: None,
            overlay: None,
            modal: None,
            stacked: false,
            blocking: false,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            block_until_exit: false,
            unblock_condition: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: None,
        }
    }

    fn tiled_run_command(cli_action: CliAction) -> RunCommandAction {
        let actions =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None)
                .expect("the cli action should be accepted");
        match &actions[0] {
            Action::NewTiledPane {
                command: Some(command),
                ..
            } => command.clone(),
            other => panic!("expected a tiled pane with a command, got {other:?}"),
        }
    }

    #[test]
    fn a_remote_pane_with_no_command_asks_for_that_machines_login_shell() {
        let command = tiled_run_command(remote_new_pane(vec![], None));
        assert!(command.remote_default_shell);
        assert_eq!(command.target.ssh_host(), Some("build-box"));
        assert_eq!(
            command.command,
            PathBuf::new(),
            "naming a command here would name a path on the wrong machine"
        );
        assert_eq!(
            command.cwd, None,
            "the local working directory is not a directory on the far machine; \
             leaving it unset is what lands the pane in the remote home"
        );
        assert!(
            !command.hold_on_close,
            "logging out of an ssh ends the session; holding it would leave a banner"
        );
    }

    /// An explicit `--cwd` names a path on the *far* machine, so it must survive
    /// unjoined — joining it with this machine's cwd would mangle a relative one
    /// and is meaningless for an absolute one.
    #[test]
    fn an_explicit_cwd_reaches_a_remote_pane_untouched() {
        let command = tiled_run_command(remote_new_pane(vec![], Some(PathBuf::from("srv/www"))));
        assert_eq!(command.cwd, Some(PathBuf::from("srv/www")));

        let with_command = tiled_run_command(remote_new_pane(
            vec!["htop".to_owned()],
            Some(PathBuf::from("/srv/www")),
        ));
        assert_eq!(with_command.cwd, Some(PathBuf::from("/srv/www")));
        assert!(!with_command.remote_default_shell);
        assert_eq!(with_command.command, PathBuf::from("htop"));
    }

    /// Without a host, no command still means no command — the local pane keeps
    /// getting its shell from the server, not from an empty `RunCommandAction`.
    #[test]
    fn a_local_pane_with_no_command_is_still_commandless() {
        let mut cli_action = remote_new_pane(vec![], None);
        if let CliAction::NewPane { remote_host, .. } = &mut cli_action {
            *remote_host = None;
        }
        let actions =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None)
                .expect("the cli action should be accepted");
        match &actions[0] {
            Action::NewTiledPane { command, .. } => assert!(command.is_none()),
            other => panic!("expected a tiled pane, got {other:?}"),
        }
    }

    // Tab-targeting for pane creation commands

    #[test]
    fn test_new_pane_tiled_with_tab_id() {
        let cli_action = CliAction::NewPane {
            remote_host: None,

            direction: Some(Direction::Right),
            command: vec![],
            plugin: None,
            cwd: None,
            floating: false,
            in_place: false,
            close_replaced_pane: false,
            pane_id: None,
            name: None,
            close_on_exit: false,
            start_suspended: false,
            configuration: None,
            configuration_json: None,
            skip_plugin_cache: false,
            x: None,
            y: None,
            width: None,
            height: None,
            pinned: None,
            overlay: None,
            modal: None,
            stacked: false,
            blocking: false,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            block_until_exit: false,
            unblock_condition: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: Some(TabId::new(3)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::NewTiledPane { tab_id, .. } => {
                assert_eq!(*tab_id, Some(TabId::new(3)));
            },
            _ => panic!("Expected NewTiledPane action"),
        }
    }

    #[test]
    fn test_new_pane_tiled_without_tab_id() {
        let cli_action = CliAction::NewPane {
            remote_host: None,

            direction: None,
            command: vec![],
            plugin: None,
            cwd: None,
            floating: false,
            in_place: false,
            close_replaced_pane: false,
            pane_id: None,
            name: None,
            close_on_exit: false,
            start_suspended: false,
            configuration: None,
            configuration_json: None,
            skip_plugin_cache: false,
            x: None,
            y: None,
            width: None,
            height: None,
            pinned: None,
            overlay: None,
            modal: None,
            stacked: false,
            blocking: false,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            block_until_exit: false,
            unblock_condition: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::NewTiledPane { tab_id, .. } => {
                assert_eq!(*tab_id, None);
            },
            _ => panic!("Expected NewTiledPane action"),
        }
    }

    #[test]
    fn test_new_in_place_pane_with_pane_id_to_replace() {
        let cli_action = CliAction::NewPane {
            remote_host: None,

            direction: None,
            command: vec![],
            plugin: None,
            cwd: None,
            floating: false,
            in_place: true,
            close_replaced_pane: true,
            pane_id: Some("terminal_4".to_string()),
            name: None,
            close_on_exit: false,
            start_suspended: false,
            configuration: None,
            configuration_json: None,
            skip_plugin_cache: false,
            x: None,
            y: None,
            width: None,
            height: None,
            pinned: None,
            overlay: None,
            modal: None,
            stacked: false,
            blocking: false,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            block_until_exit: false,
            unblock_condition: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::NewInPlacePane {
                pane_id_to_replace, ..
            } => {
                assert_eq!(
                    *pane_id_to_replace,
                    Some(PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
                        4
                    )))
                );
            },
            _ => panic!("Expected NewInPlacePane action"),
        }
    }

    #[test]
    fn test_new_in_place_pane_with_malformed_pane_id() {
        let cli_action = CliAction::NewPane {
            remote_host: None,

            direction: None,
            command: vec![],
            plugin: None,
            cwd: None,
            floating: false,
            in_place: true,
            close_replaced_pane: false,
            pane_id: Some("not_a_pane".to_string()),
            name: None,
            close_on_exit: false,
            start_suspended: false,
            configuration: None,
            configuration_json: None,
            skip_plugin_cache: false,
            x: None,
            y: None,
            width: None,
            height: None,
            pinned: None,
            overlay: None,
            modal: None,
            stacked: false,
            blocking: false,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            block_until_exit: false,
            unblock_condition: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Malformed pane id"));
    }

    #[test]
    fn test_new_pane_floating_with_tab_id() {
        let cli_action = CliAction::NewPane {
            remote_host: None,

            direction: None,
            command: vec![],
            plugin: None,
            cwd: None,
            floating: true,
            in_place: false,
            close_replaced_pane: false,
            pane_id: None,
            name: None,
            close_on_exit: false,
            start_suspended: false,
            configuration: None,
            configuration_json: None,
            skip_plugin_cache: false,
            x: None,
            y: None,
            width: None,
            height: None,
            pinned: None,
            overlay: None,
            modal: None,
            stacked: false,
            blocking: false,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            block_until_exit: false,
            unblock_condition: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: Some(TabId::new(5)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::NewFloatingPane { tab_id, .. } => {
                assert_eq!(*tab_id, Some(TabId::new(5)));
            },
            _ => panic!("Expected NewFloatingPane action"),
        }
    }

    #[test]
    fn test_new_pane_stacked_with_tab_id() {
        let cli_action = CliAction::NewPane {
            remote_host: None,

            direction: None,
            command: vec!["ls".into()],
            plugin: None,
            cwd: None,
            floating: false,
            in_place: false,
            close_replaced_pane: false,
            pane_id: None,
            name: None,
            close_on_exit: false,
            start_suspended: false,
            configuration: None,
            configuration_json: None,
            skip_plugin_cache: false,
            x: None,
            y: None,
            width: None,
            height: None,
            pinned: None,
            overlay: None,
            modal: None,
            stacked: true,
            blocking: false,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            block_until_exit: false,
            unblock_condition: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: Some(TabId::new(1)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::NewStackedPane { tab_id, .. } => {
                assert_eq!(*tab_id, Some(TabId::new(1)));
            },
            _ => panic!("Expected NewStackedPane action"),
        }
    }

    /// A fragment is checked against the caller's config before it leaves the machine, so a
    /// typo is an error naming the problem rather than a server log line.
    #[test]
    fn reconfigure_takes_a_fragment_and_refuses_one_that_does_not_parse() {
        let reconfigure = |kdl: &str, write: bool| {
            crate::actions::actions_from_cli(
                CliAction::Reconfigure {
                    kdl: Some(kdl.to_owned()),
                    file: None,
                    write,
                },
                Box::new(|| PathBuf::from("/tmp")),
                None,
            )
        };
        let fragment = "keybinds {\n    normal {\n        bind \"Ctrl y\" { NewPane; }\n    }\n}\n";
        assert_eq!(
            reconfigure(fragment, true).unwrap(),
            vec![Action::Reconfigure {
                config: fragment.to_owned(),
                write_to_disk: true,
            }]
        );
        let refused = reconfigure("options { nonsense", false).unwrap_err();
        assert!(refused.contains("does not parse"), "{refused}");
    }

    /// `set-hook` is the one-block fragment spelled for you; an empty url unbinds, and a point
    /// nobody has heard of is refused with the ones that exist.
    #[test]
    fn set_hook_is_a_hooks_fragment() {
        let set_hook = |point: &str, url: &str| {
            crate::actions::actions_from_cli(
                CliAction::SetHook {
                    point: point.to_owned(),
                    url: url.to_owned(),
                    write: false,
                },
                Box::new(|| PathBuf::from("/tmp")),
                None,
            )
        };
        assert_eq!(
            set_hook("agent_exit", "file:my-hook.ts").unwrap(),
            vec![Action::Reconfigure {
                config: "hooks {\n    agent_exit \"file:my-hook.ts\"\n}\n".to_owned(),
                write_to_disk: false,
            }]
        );
        assert_eq!(
            set_hook("context-menu", "").unwrap(),
            vec![Action::Reconfigure {
                config: "hooks {\n    context_menu \"\"\n}\n".to_owned(),
                write_to_disk: false,
            }]
        );
        assert!(set_hook("agent_exit", "file:quote\"d.ts").unwrap()[0]
            .to_string()
            .contains("Reconfigure"));
        let refused = set_hook("on_coffee", "file:x.ts").unwrap_err();
        assert!(refused.contains("agent_exit"), "{refused}");
        // Every point the config knows, the CLI accepts - and the near miss is refused.
        for point in yinz_types::input::hooks::HookPoint::ALL {
            let actions = set_hook(point.as_str(), "file:x.ts").unwrap();
            assert_eq!(
                actions,
                vec![Action::Reconfigure {
                    config: format!("hooks {{\n    {point} \"file:x.ts\"\n}}\n"),
                    write_to_disk: false,
                }]
            );
        }
        let refused = set_hook("agent_started", "file:x.ts").unwrap_err();
        assert!(refused.contains("agent_start,"), "{refused}");
    }

    /// A hook event does two things from the one process a hook may spend: it sets the pane's
    /// record, and it goes out whole as an `AgentEvent` for whoever wants more than a record
    /// holds. An event that says nothing about the state still goes out — the summary hangs
    /// off text a state says nothing about.
    #[test]
    fn a_hook_event_sets_the_record_and_broadcasts_itself() {
        let pane_id = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7));
        let hook = |json: &str| {
            let event: yinz_types::agent::HookEvent = serde_json::from_str(json).expect("parsed");
            hook_report_actions("claude", pane_id, &event, json.to_owned())
        };
        let broadcast = |actions: &[Action], json: &str| match actions.last() {
            Some(Action::AgentEvent(event)) => {
                assert_eq!(event.raw, json, "the event goes out whole");
                assert_eq!(
                    event.pane_id,
                    yinz_types::internal_ids::TerminalId::new(7),
                    "the bare number, which is what a reader keys by"
                );
                // The payload is the agent's own JSON and never says which agent wrote it, so
                // a reader that writes a record of its own is told here.
                assert_eq!(event.kind, "claude");
            },
            other => panic!("{other:?}"),
        };

        let json = r#"{"hook_event_name":"PermissionRequest","session_id":"s1",
                       "tool_name":"Bash","tool_input":{"command":"rm -rf target"}}"#;
        let actions = hook(json);
        assert_eq!(actions.len(), 2);
        match &actions[0] {
            Action::SetPaneAgent {
                pane_id,
                agent: Some(agent),
            } => {
                assert_eq!(
                    *pane_id,
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7))
                );
                assert_eq!(agent.state, AgentState::Busy);
                assert_eq!(agent.source, AgentSource::Reported);
                assert_eq!(agent.detail.as_deref(), Some("Bash rm"));
                assert_eq!(agent.waiting_on, None);
                assert_eq!(agent.session_id.as_deref(), Some("s1"));
            },
            other => panic!("{other:?}"),
        }
        broadcast(&actions, json);
        // The event is read once, the way the record is: the same state, the same tool.
        match &actions[1] {
            Action::AgentEvent(event) => {
                assert_eq!(event.hook, "PermissionRequest");
                assert_eq!(event.state, Some(AgentState::Busy));
                assert_eq!(event.session_id.as_deref(), Some("s1"));
                assert_eq!(event.tool_name.as_deref(), Some("Bash"));
            },
            other => panic!("{other:?}"),
        }

        // `SessionEnd` ends the record rather than moving it.
        let json = r#"{"hook_event_name":"SessionEnd","session_id":"s1"}"#;
        let actions = hook(json);
        assert_eq!(actions.len(), 2);
        assert!(matches!(
            &actions[0],
            Action::SetPaneAgent { agent: None, .. }
        ));
        broadcast(&actions, json);

        // A notification that means neither waiting nor idle leaves the record alone — and is
        // still broadcast, because `last_assistant_message` and friends live on events a state
        // reading has no opinion about.
        let json = r#"{"hook_event_name":"Notification","notification_type":"auth_success"}"#;
        let actions = hook(json);
        assert_eq!(actions.len(), 1, "nothing to report is nothing to set");
        broadcast(&actions, json);

        // Another agent's question, read the same way and named on the broadcast, so the
        // plugin that hears it does not have to guess that a codex is a claude.
        let json = r#"{"hook_event_name":"PreToolUse","session_id":"s1",
                       "tool_name":"request_user_input"}"#;
        let event: yinz_types::agent::HookEvent = serde_json::from_str(json).expect("parsed");
        let actions = hook_report_actions("codex", pane_id, &event, json.to_owned());
        match &actions[0] {
            Action::SetPaneAgent {
                agent: Some(agent), ..
            } => {
                assert_eq!(agent.kind, "codex");
                assert_eq!(agent.state, AgentState::Waiting);
            },
            other => panic!("{other:?}"),
        }
        match actions.last() {
            Some(Action::AgentEvent(event)) => assert_eq!(event.kind, "codex"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn automatic_permission_records_and_broadcasts_are_busy() {
        for kind in ["claude", "codex"] {
            for mode in ["auto", "bypassPermissions", "dontAsk", "acceptEdits"] {
                let tool = if mode == "acceptEdits" {
                    "Edit"
                } else {
                    "Bash"
                };
                let json = serde_json::json!({
                    "hook_event_name": "PermissionRequest", "permission_mode": mode,
                    "tool_name": tool, "tool_input": {"command": "git status", "file_path": "/work/main.rs"}
                }).to_string();
                let event = serde_json::from_str(&json).unwrap();
                let actions = hook_report_actions(
                    kind,
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
                    &event,
                    json,
                );
                match &actions[..] {
                    [Action::SetPaneAgent {
                        agent: Some(agent), ..
                    }, Action::AgentEvent(event)] => {
                        assert_eq!(agent.kind, kind);
                        assert_eq!(agent.state, AgentState::Busy);
                        assert_eq!(agent.waiting_on, None);
                        assert!(!agent.wait_needs_a_person);
                        assert_eq!(
                            agent.detail.as_deref(),
                            Some(if tool == "Bash" {
                                "Bash git"
                            } else {
                                "Edit main.rs"
                            })
                        );
                        assert_eq!(event.kind, kind);
                        assert_eq!(event.state, Some(AgentState::Busy));
                    },
                    other => panic!("{other:?}"),
                }
                for reminder in ["permission_prompt", "worker_permission_prompt"] {
                    let json = serde_json::json!({"hook_event_name": "Notification", "notification_type": reminder, "permission_mode": mode}).to_string();
                    let event = serde_json::from_str(&json).unwrap();
                    let actions = hook_report_actions(
                        kind,
                        PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
                        &event,
                        json,
                    );
                    if kind == "claude" && reminder == "permission_prompt" {
                        assert!(matches!(&actions[..], [Action::SetPaneAgent {
                            agent: Some(agent), ..
                        }, Action::AgentEvent(event)] if agent.state == AgentState::Waiting
                            && agent.wait_needs_a_person
                            && event.state == Some(AgentState::Waiting)));
                    } else {
                        assert!(
                            matches!(&actions[..], [Action::AgentEvent(event)] if event.state.is_none())
                        );
                    }
                }
            }
        }
    }

    /// …but not at any size. This payload is copied into every running plugin's memory,
    /// twice per tool call, inside a hook with five seconds to live, and `tool_input` and
    /// `tool_response` have no bound at all in Claude Code's schema. Over the cap the event
    /// goes out pruned — still an event, and still carrying everything a reader takes.
    #[test]
    fn a_hook_event_too_big_to_hand_round_is_cut_rather_than_dropped() {
        let json = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "session_id": "s1",
            "tool_name": "Write",
            "tool_input": {
                "file_path": "/work/src/main.rs",
                "content": "x".repeat(2 * 1024 * 1024),
            },
            // Not on `HookEvent` at all, so no reader here has ever seen it.
            "tool_response": "y".repeat(2 * 1024 * 1024),
        })
        .to_string();
        assert!(json.len() > yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);

        let event: yinz_types::agent::HookEvent = serde_json::from_str(&json).expect("parsed");
        let actions = hook_report_actions(
            "claude",
            PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
            &event,
            json,
        );
        let Some(Action::AgentEvent(yinz_types::agent::AgentEvent { raw: payload, .. })) =
            actions.last()
        else {
            panic!("{actions:?}");
        };
        assert!(
            payload.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES,
            "the broadcast is bounded, got {}",
            payload.len()
        );
        let arrived: yinz_types::agent::HookEvent =
            serde_json::from_str(payload).expect("a pruned event is still an event");
        assert_eq!(
            yinz_types::agent::hook_detail(&arrived).as_deref(),
            Some("Write main.rs"),
            "and still says what the row draws"
        );
    }

    /// `agent-report` is what an agent's hooks call: a state names the record, `--clear` ends
    /// it, and the pane is the one the caller is in unless it says otherwise.
    #[test]
    fn agent_report_sets_or_clears_the_panes_agent_record() {
        let report = |state: Option<&str>, clear: bool| {
            crate::actions::actions_from_cli(
                CliAction::AgentReport {
                    pane_id: Some("terminal_4".to_owned()),
                    kind: Some(" Codex ".to_owned()),
                    state: state.map(str::to_owned),
                    detail: Some("rebasing".to_owned()),
                    cwd: Some(PathBuf::from("/work/.worktrees/topic")),
                    hook_json: false,
                    clear,
                    run_id: None,
                },
                Box::new(|| PathBuf::from("/tmp")),
                None,
            )
        };
        assert_eq!(
            report(Some("waiting"), false).unwrap(),
            vec![Action::SetPaneAgent {
                pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(4)),
                agent: Some(AgentInfo {
                    kind: "codex".to_owned(),
                    state: AgentState::Waiting,
                    source: AgentSource::Reported,
                    detail: Some("rebasing".to_owned()),
                    waiting_on: None,
                    wait_needs_a_person: true,
                    cwd: Some("/work/.worktrees/topic".to_owned()),
                    session_id: None,
                    run_id: None,
                }),
            }]
        );
        assert_eq!(
            report(None, true).unwrap(),
            vec![Action::SetPaneAgent {
                pane_id: PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(4)),
                agent: None,
            }]
        );
        assert!(
            report(None, false).is_err(),
            "a report has to say something"
        );
        assert!(report(Some("asleep"), false).is_err());
    }

    /// A script keeping its own history names the run it is keeping. Agents that report a
    /// session id of their own do not need this — their run id is derived from it — but a
    /// pane whose run id is only ever a local invention has nothing else to be found by
    /// after a warp.
    #[test]
    fn agent_report_carries_the_run_id_it_was_given() {
        let report = |run_id: Option<&str>| {
            crate::actions::actions_from_cli(
                CliAction::AgentReport {
                    pane_id: Some("terminal_4".to_owned()),
                    kind: None,
                    state: Some("busy".to_owned()),
                    detail: None,
                    cwd: None,
                    hook_json: false,
                    clear: false,
                    run_id: run_id.map(str::to_owned),
                },
                Box::new(|| PathBuf::from("/tmp")),
                None,
            )
            .unwrap()
        };
        let run_id_of = |actions: Vec<Action>| match actions.into_iter().next() {
            Some(Action::SetPaneAgent { agent, .. }) => agent.and_then(|agent| agent.run_id),
            other => panic!("expected a SetPaneAgent, got {other:?}"),
        };
        assert_eq!(run_id_of(report(Some("mine:7"))), Some("mine:7".to_owned()));
        // Unset, not cleared: the pane keeps whatever it already carries, which is what lets a
        // script name its run once rather than on every report.
        assert_eq!(run_id_of(report(None)), None);
        assert_eq!(run_id_of(report(Some("  "))), None, "blank is not an id");
    }

    /// Each `--block-until-exit*` flag names the condition that ends the wait, and a condition
    /// handed in directly (`unblock_condition`, the escape hatch `yinz run` itself uses) wins
    /// over the flags.
    #[test]
    fn blocking_flags_map_to_their_unblock_condition() {
        let build = |success: bool, failure: bool, any: bool, direct: Option<UnblockCondition>| {
            let cli_action = CliAction::NewPane {
                remote_host: None,

                direction: None,
                command: vec!["ls".into()],
                plugin: None,
                cwd: None,
                floating: false,
                in_place: false,
                close_replaced_pane: false,
                pane_id: None,
                name: None,
                close_on_exit: false,
                start_suspended: false,
                configuration: None,
                configuration_json: None,
                skip_plugin_cache: false,
                x: None,
                y: None,
                width: None,
                height: None,
                pinned: None,
                overlay: None,
                modal: None,
                stacked: false,
                blocking: false,
                block_until_exit_success: success,
                block_until_exit_failure: failure,
                block_until_exit: any,
                unblock_condition: direct,
                near_current_pane: false,
                no_focus: false,
                borderless: None,
                tab_id: None,
            };
            let actions = crate::actions::actions_from_cli(
                cli_action,
                Box::new(|| PathBuf::from("/tmp")),
                None,
            )
            .unwrap();
            match actions.as_slice() {
                [Action::NewBlockingPane {
                    unblock_condition, ..
                }] => *unblock_condition,
                other => panic!("expected one NewBlockingPane, got {other:?}"),
            }
        };
        assert_eq!(
            build(true, false, false, None),
            Some(UnblockCondition::OnExitSuccess)
        );
        assert_eq!(
            build(false, true, false, None),
            Some(UnblockCondition::OnExitFailure)
        );
        assert_eq!(
            build(false, false, true, None),
            Some(UnblockCondition::OnAnyExit)
        );
        assert_eq!(
            build(true, false, false, Some(UnblockCondition::OnAnyExit)),
            Some(UnblockCondition::OnAnyExit),
            "a condition given directly wins over the flags"
        );
    }

    #[test]
    fn test_new_pane_blocking_with_tab_id() {
        let cli_action = CliAction::NewPane {
            remote_host: None,

            direction: None,
            command: vec!["ls".into()],
            plugin: None,
            cwd: None,
            floating: false,
            in_place: false,
            close_replaced_pane: false,
            pane_id: None,
            name: None,
            close_on_exit: false,
            start_suspended: false,
            configuration: None,
            configuration_json: None,
            skip_plugin_cache: false,
            x: None,
            y: None,
            width: None,
            height: None,
            pinned: None,
            overlay: None,
            modal: None,
            stacked: false,
            blocking: true,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            block_until_exit: false,
            unblock_condition: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: Some(TabId::new(2)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::NewBlockingPane { tab_id, .. } => {
                assert_eq!(*tab_id, Some(TabId::new(2)));
            },
            _ => panic!("Expected NewBlockingPane action"),
        }
    }

    #[test]
    fn test_edit_with_tab_id() {
        let cli_action = CliAction::Edit {
            file: PathBuf::from("/tmp/test.rs"),
            direction: None,
            line_number: None,
            floating: false,
            in_place: false,
            close_replaced_pane: false,
            cwd: None,
            x: None,
            y: None,
            width: None,
            height: None,
            pinned: None,
            overlay: None,
            modal: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: Some(TabId::new(4)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::EditFile { tab_id, .. } => {
                assert_eq!(*tab_id, Some(TabId::new(4)));
            },
            _ => panic!("Expected EditFile action"),
        }
    }

    #[test]
    fn test_edit_without_tab_id() {
        let cli_action = CliAction::Edit {
            file: PathBuf::from("/tmp/test.rs"),
            direction: None,
            line_number: None,
            floating: false,
            in_place: false,
            close_replaced_pane: false,
            cwd: None,
            x: None,
            y: None,
            width: None,
            height: None,
            pinned: None,
            overlay: None,
            modal: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: None,
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::EditFile { tab_id, .. } => {
                assert_eq!(*tab_id, None);
            },
            _ => panic!("Expected EditFile action"),
        }
    }

    #[test]
    fn test_new_pane_plugin_tiled_with_tab_id() {
        let cli_action = CliAction::NewPane {
            remote_host: None,

            direction: None,
            command: vec![],
            plugin: Some("yinz:strider".into()),
            cwd: None,
            floating: false,
            in_place: false,
            close_replaced_pane: false,
            pane_id: None,
            name: None,
            close_on_exit: false,
            start_suspended: false,
            configuration: None,
            configuration_json: None,
            skip_plugin_cache: false,
            x: None,
            y: None,
            width: None,
            height: None,
            pinned: None,
            overlay: None,
            modal: None,
            stacked: false,
            blocking: false,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            block_until_exit: false,
            unblock_condition: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: Some(TabId::new(2)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::NewTiledPluginPane { tab_id, .. } => {
                assert_eq!(*tab_id, Some(TabId::new(2)));
            },
            _ => panic!("Expected NewTiledPluginPane action"),
        }
    }

    #[test]
    fn test_new_pane_plugin_floating_with_tab_id() {
        let cli_action = CliAction::NewPane {
            remote_host: None,

            direction: None,
            command: vec![],
            plugin: Some("yinz:strider".into()),
            cwd: None,
            floating: true,
            in_place: false,
            close_replaced_pane: false,
            pane_id: None,
            name: None,
            close_on_exit: false,
            start_suspended: false,
            configuration: None,
            configuration_json: None,
            skip_plugin_cache: false,
            x: None,
            y: None,
            width: None,
            height: None,
            pinned: None,
            overlay: None,
            modal: None,
            stacked: false,
            blocking: false,
            block_until_exit_success: false,
            block_until_exit_failure: false,
            block_until_exit: false,
            unblock_condition: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: Some(TabId::new(1)),
        };
        let result =
            crate::actions::actions_from_cli(cli_action, Box::new(|| PathBuf::from("/tmp")), None);
        assert!(result.is_ok());
        let actions = result.unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::NewFloatingPluginPane { tab_id, .. } => {
                assert_eq!(*tab_id, Some(TabId::new(1)));
            },
            _ => panic!("Expected NewFloatingPluginPane action"),
        }
    }

    // 35 with_pane_id/without_pane_id example tests were removed here. Each
    // asserted only which Action variant came out and which pane id it
    // carried - both now checked for all 26 swept variants on every run by
    // props::each_cli_action_maps_to_its_expected_action_variant.

    mod props {
        use super::*;
        use hegel::generators;
        use yinz_types::geometry::{Direction, Resize};

        fn cwd() -> Box<dyn Fn() -> PathBuf> {
            Box::new(|| PathBuf::from("/tmp"))
        }

        /// Every `CliAction` carrying an optional `--pane-id`. All the other
        /// fields are pinned, so the pane id is the only thing that varies
        /// between two calls with different `which` held constant.
        ///
        /// Six of the `CliAction`s that mention a pane id are deliberately
        /// absent, and keep their own example tests:
        ///
        /// * `FocusPaneId`, `ChangeFloatingPaneCoordinates`,
        ///   `TogglePaneBorderless` and `SetPaneBorderless` take a required
        ///   `String`, so they have no "without pane id" form to compare to.
        /// * `NewPane`'s `pane_id` is the pane to *replace* and is only legal
        ///   alongside `in_place`, so it is not the same parameter at all.
        /// * `SwitchSession`'s names a pane in the session being switched to.
        const PANE_ID_ACTIONS: usize = 26;

        /// The prefix of `cli_action_with_pane_id` whose no-`--pane-id` form is
        /// actually pane-id-free. `SetPaneColor` - the last index, and the only
        /// one outside this prefix - requires the caller pane to be resolved
        /// before parsing instead of acting on the focused pane. Everything that
        /// builds `pane_id: None` stops here; the with-a-pane-id sweeps still
        /// run to `PANE_ID_ACTIONS`.
        const PANE_ID_OPTIONAL_ACTIONS: usize = PANE_ID_ACTIONS - 1;

        fn cli_action_with_pane_id(which: usize, pane_id: Option<String>) -> CliAction {
            match which {
                0 => CliAction::Write {
                    bytes: vec![10],
                    pane_id,
                },
                1 => CliAction::WriteChars {
                    chars: "foo".to_owned(),
                    pane_id,
                },
                2 => CliAction::Paste {
                    chars: "foo".to_owned(),
                    pane_id,
                },
                3 => CliAction::SendKeys {
                    keys: vec!["Enter".to_owned()],
                    pane_id,
                },
                4 => CliAction::Resize {
                    resize: Resize::Increase,
                    direction: Some(Direction::Left),
                    pane_id,
                },
                5 => CliAction::MovePane {
                    direction: Some(Direction::Right),
                    pane_id,
                },
                6 => CliAction::MovePaneBackwards { pane_id },
                7 => CliAction::Clear { pane_id },
                8 => CliAction::DumpScreen {
                    path: Some(PathBuf::from("/tmp/dump")),
                    full: false,
                    pane_id,
                    ansi: false,
                },
                9 => CliAction::EditScrollback {
                    pane_id,
                    ansi: false,
                },
                10 => CliAction::ScrollUp { pane_id },
                11 => CliAction::ScrollDown { pane_id },
                12 => CliAction::ScrollToBottom { pane_id },
                13 => CliAction::ScrollToTop { pane_id },
                14 => CliAction::PageScrollUp { pane_id },
                15 => CliAction::PageScrollDown { pane_id },
                16 => CliAction::HalfPageScrollUp { pane_id },
                17 => CliAction::HalfPageScrollDown { pane_id },
                18 => CliAction::ToggleFullscreen { pane_id },
                19 => CliAction::ToggleNoUiFullscreen { pane_id },
                20 => CliAction::TogglePaneEmbedOrFloating { pane_id },
                21 => CliAction::ClosePane { pane_id },
                22 => CliAction::RenamePane {
                    name: "new-name".to_owned(),
                    pane_id,
                },
                23 => CliAction::UndoRenamePane { pane_id },
                24 => CliAction::TogglePanePinned { pane_id },
                25 => CliAction::SetPaneColor {
                    pane_id,
                    fg: Some("red".to_owned()),
                    bg: Some("blue".to_owned()),
                    reset: false,
                },
                other => unreachable!("no CliAction at index {}", other),
            }
        }

        /// The `Action` each indexed `CliAction` must produce, as
        /// `(with a pane id, without one)`. The second element is unused past
        /// `PANE_ID_OPTIONAL_ACTIONS` - `SetPaneColor` has no pane-id-free
        /// form - and is left empty there.
        ///
        /// The relational properties below pin that the pane id reaches the
        /// action; this pins *which* action, which is the other half of what
        /// the per-variant example tests asserted. Without it a variant could
        /// route to the wrong ById action and every other property here would
        /// still pass.
        const EXPECTED_ACTIONS: [(&str, &str); PANE_ID_ACTIONS] = [
            ("WriteToPaneId", "Write"),
            ("WriteCharsToPaneId", "WriteChars"),
            ("Paste", "Paste"),
            ("WriteToPaneId", "Write"),
            ("ResizeByPaneId", "Resize"),
            ("MovePaneByPaneId", "MovePane"),
            ("MovePaneBackwardsByPaneId", "MovePaneBackwards"),
            ("ClearScreenByPaneId", "ClearScreen"),
            ("DumpScreen", "DumpScreen"),
            ("EditScrollbackByPaneId", "EditScrollback"),
            ("ScrollUpByPaneId", "ScrollUp"),
            ("ScrollDownByPaneId", "ScrollDown"),
            ("ScrollToBottomByPaneId", "ScrollToBottom"),
            ("ScrollToTopByPaneId", "ScrollToTop"),
            ("PageScrollUpByPaneId", "PageScrollUp"),
            ("PageScrollDownByPaneId", "PageScrollDown"),
            ("HalfPageScrollUpByPaneId", "HalfPageScrollUp"),
            ("HalfPageScrollDownByPaneId", "HalfPageScrollDown"),
            ("ToggleFocusFullscreenByPaneId", "ToggleFocusFullscreen"),
            (
                "ToggleFocusNoUiFullscreenByPaneId",
                "ToggleFocusNoUiFullscreen",
            ),
            (
                "TogglePaneEmbedOrFloatingByPaneId",
                "TogglePaneEmbedOrFloating",
            ),
            ("CloseFocusByPaneId", "CloseFocus"),
            ("RenamePaneByPaneId", "RenamePaneByPaneId"),
            ("UndoRenamePaneByPaneId", "UndoRenamePane"),
            ("TogglePanePinnedByPaneId", "TogglePanePinned"),
            ("SetPaneColor", ""),
        ];

        /// The variant name at the head of an `Action`'s `Debug` rendering.
        fn variant_name(action: &Action) -> String {
            format!("{:?}", action)
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect()
        }

        #[hegel::test]
        fn each_cli_action_maps_to_its_expected_action_variant(tc: hegel::TestCase) {
            let (spelling, parsed) = tc.draw(pane_id_spelling());
            for (which, (expected_with, expected_without)) in EXPECTED_ACTIONS.iter().enumerate() {
                let with = crate::actions::actions_from_cli(
                    cli_action_with_pane_id(which, Some(spelling.clone())),
                    cwd(),
                    None,
                )
                .unwrap();
                assert_eq!(
                    variant_name(&with[0]),
                    *expected_with,
                    "action {} with pane id {:?} produced {:?}",
                    which,
                    spelling,
                    with[0]
                );
                if which < PANE_ID_OPTIONAL_ACTIONS {
                    let without = crate::actions::actions_from_cli(
                        cli_action_with_pane_id(which, None),
                        cwd(),
                        None,
                    )
                    .unwrap();
                    assert_eq!(
                        variant_name(&without[0]),
                        *expected_without,
                        "action {} without a pane id produced {:?}",
                        which,
                        without[0]
                    );
                }
                // ...carrying that exact pane id, not merely some pane id that
                // varies with the input. `the_pane_id_reaches_the_action_it_
                // produces` only shows distinct ids stay distinct, so an
                // off-by-one in the parse would slip past it.
                let rendered = format!("{:?}", with[0]);
                let expected_id = format!("{:?}", parsed);
                assert!(
                    rendered.contains(&expected_id),
                    "action {} built from {:?} does not carry {}: {}",
                    which,
                    spelling,
                    expected_id,
                    rendered
                );
            }
        }

        /// The three spellings `PaneId::from_str` accepts, as
        /// `(rendered id, parsed id)`.
        #[hegel::composite]
        fn pane_id_spelling(tc: &hegel::TestCase) -> (String, PaneId) {
            let n = tc.draw(generators::integers::<u32>());
            if tc.draw(generators::booleans()) {
                (
                    format!("plugin_{}", n),
                    PaneId::Plugin(yinz_types::internal_ids::PluginId::new(n)),
                )
            } else if tc.draw(generators::booleans()) {
                (
                    format!("terminal_{}", n),
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(n)),
                )
            } else {
                // a bare integer is a terminal pane id
                (
                    format!("{}", n),
                    PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(n)),
                )
            }
        }

        // Each property sweeps all of `0..PANE_ID_ACTIONS` per test case rather
        // than drawing one index. Drawing it looks equivalent but is not: with
        // 24 variants the higher indices come up only a handful of times in a
        // default run, so a single variant dropping its pane id passed
        // unnoticed. These properties stand in for one example test per
        // variant, so every variant has to be exercised on every case.

        #[hegel::test]
        fn a_pane_id_bearing_cli_action_always_yields_exactly_one_action(tc: hegel::TestCase) {
            let (spelling, _) = tc.draw(pane_id_spelling());
            for which in 0..PANE_ID_ACTIONS {
                let with = crate::actions::actions_from_cli(
                    cli_action_with_pane_id(which, Some(spelling.clone())),
                    cwd(),
                    None,
                );
                let mut cases = vec![("with", with)];
                if which < PANE_ID_OPTIONAL_ACTIONS {
                    cases.push((
                        "without",
                        crate::actions::actions_from_cli(
                            cli_action_with_pane_id(which, None),
                            cwd(),
                            None,
                        ),
                    ));
                }
                for (label, result) in &cases {
                    let actions = result.as_ref().unwrap_or_else(|e| {
                        panic!("action {} {} pane id errored: {}", which, label, e)
                    });
                    assert_eq!(
                        actions.len(),
                        1,
                        "action {} {} pane id {:?} produced {:?}",
                        which,
                        label,
                        spelling,
                        actions
                    );
                }
            }
        }

        /// `"5"`, `"terminal_5"` both parse to `PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(5))`, so an
        /// action built from either spelling has to come out identical. This is
        /// what pins the id to `PaneId::from_str` rather than to some
        /// variant-local parse.
        #[hegel::test]
        fn equivalent_pane_id_spellings_produce_the_same_action(tc: hegel::TestCase) {
            let n = tc.draw(generators::integers::<u32>());
            for which in 0..PANE_ID_ACTIONS {
                let bare = crate::actions::actions_from_cli(
                    cli_action_with_pane_id(which, Some(format!("{}", n))),
                    cwd(),
                    None,
                );
                let prefixed = crate::actions::actions_from_cli(
                    cli_action_with_pane_id(which, Some(format!("terminal_{}", n))),
                    cwd(),
                    None,
                );
                assert_eq!(
                    bare, prefixed,
                    "action {} read {:?} and terminal_{} differently",
                    which, n, n
                );
            }
        }

        /// The pane id has to actually reach the produced action. If a variant
        /// dropped it on the floor, two different ids would collapse to the
        /// same action - and so would the no-pane-id form.
        #[hegel::test]
        fn the_pane_id_reaches_the_action_it_produces(tc: hegel::TestCase) {
            let (first, first_id) = tc.draw(pane_id_spelling());
            let (second, second_id) = tc.draw(pane_id_spelling());
            tc.assume(first_id != second_id);

            for which in 0..PANE_ID_ACTIONS {
                let a = crate::actions::actions_from_cli(
                    cli_action_with_pane_id(which, Some(first.clone())),
                    cwd(),
                    None,
                );
                let b = crate::actions::actions_from_cli(
                    cli_action_with_pane_id(which, Some(second.clone())),
                    cwd(),
                    None,
                );

                assert_ne!(
                    a, b,
                    "action {} produced the same thing for pane ids {:?} and {:?}",
                    which, first, second
                );
                if which < PANE_ID_OPTIONAL_ACTIONS {
                    let none = crate::actions::actions_from_cli(
                        cli_action_with_pane_id(which, None),
                        cwd(),
                        None,
                    );
                    assert_ne!(
                        a, none,
                        "action {} with pane id {:?} is indistinguishable from without one",
                        which, first
                    );
                }
            }
        }

        /// Anything `PaneId::from_str` rejects must come back as an `Err` from
        /// every one of these variants - never a panic, and never a silently
        /// accepted action. The "Malformed pane id" branch is written out ~15
        /// times in `actions_from_cli`; this is what checks they all fire.
        #[hegel::test]
        fn a_malformed_pane_id_is_always_an_error(tc: hegel::TestCase) {
            let spelling = tc.draw(generators::text());
            tc.assume(PaneId::from_str(&spelling).is_err());

            for which in 0..PANE_ID_ACTIONS {
                let result = crate::actions::actions_from_cli(
                    cli_action_with_pane_id(which, Some(spelling.clone())),
                    cwd(),
                    None,
                );
                assert!(
                    result.is_err(),
                    "action {} accepted malformed pane id {:?} as {:?}",
                    which,
                    spelling,
                    result
                );
            }
        }
    }
}

mod kdl_layout_parser;
use crate::data::{ClientId, ContextMenuItem};
use crate::data::{
    InputMode, LayoutInfo, LayoutMetadata, NewPanePlacement, PaletteColor, PaneId, PaneInfo,
    PaneManifest, PermissionType, SessionInfo, TabInfo,
};
use crate::home::{find_default_config_dir, get_layout_dir};
use crate::input::config::{Config, ConfigError, KdlError};
use crate::input::context_menu::ContextMenu;
use crate::input::hooks::{HookPoint, Hooks};
use crate::input::keybinds::Keybinds;
use crate::input::layout::{
    Layout, PluginUserConfiguration, RunPlugin, RunPluginOrAlias, TabLayoutInfo,
};
use crate::input::options::{
    Clipboard, NotificationPosition, OnForceClose, Options, PaneFrameStyle,
    DEFAULT_AGENT_PANE_MIN_COLUMNS, DEFAULT_AGENT_PANE_MIN_ROWS, DEFAULT_NOTIFICATION_TIMEOUT_SECS,
    DEFAULT_WORD_SEPARATORS,
};
use crate::input::permission::{GrantedPermission, PermissionCache};
use crate::input::plugins::PluginAliases;
use crate::input::theme::{FrameConfig, Theme, Themes, UiConfig};
use kdl_layout_parser::KdlLayoutParser;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use strum::IntoEnumIterator;
use uuid::Uuid;
use zellij_types::envs::EnvironmentVariables;
use zellij_types::geometry::PercentOrFixed;
use zellij_types::geometry::{Direction, FloatingPaneCoordinates, Resize};
use zellij_types::keys::KeyWithModifier;
use zellij_types::style::{
    MultiplayerColors, Palette, StyleDeclaration, Styling, ThemeHue, DEFAULT_STYLES,
};

use miette::NamedSource;

use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};

use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use crate::input::actions::{Action, SearchDirection, SearchOption};
use crate::input::command::RunCommandAction;
use zellij_types::command::PaneTarget;

/// The `firecracker_*` children of a `Run` action, the counterpart of the
/// `firecracker_vm`/`firecracker_image`/`firecracker_worktree` pane properties.
fn push_firecracker_nodes(run_command_action: &RunCommandAction, children: &mut KdlDocument) {
    let PaneTarget::Firecracker {
        vm,
        image,
        worktree,
    } = &run_command_action.target
    else {
        return;
    };
    let mut vm_node = KdlNode::new("firecracker_vm");
    vm_node.push(KdlValue::String(vm.clone()));
    children.nodes_mut().push(vm_node);
    if let Some(image) = image {
        let mut image_node = KdlNode::new("firecracker_image");
        image_node.push(KdlValue::String(image.clone()));
        children.nodes_mut().push(image_node);
    }
    if let Some(worktree) = worktree {
        let mut worktree_node = KdlNode::new("firecracker_worktree");
        worktree_node.push(KdlValue::String(worktree.display().to_string()));
        children.nodes_mut().push(worktree_node);
    }
}

#[macro_export]
macro_rules! parse_kdl_action_arguments {
    ( $action_name:expr, $action_arguments:expr, $action_node:expr ) => {{
        if !$action_arguments.is_empty() {
            Err(ConfigError::new_kdl_error(
                format!("Action '{}' must have arguments", $action_name),
                $action_node.span().offset(),
                $action_node.span().len(),
            ))
        } else {
            match $action_name {
                "Quit" => Ok(Action::Quit),
                "FocusNextPane" => Ok(Action::FocusNextPane),
                "FocusPreviousPane" => Ok(Action::FocusPreviousPane),
                "FocusLastPane" => Ok(Action::FocusLastPane),
                "FocusHostSession" => Ok(Action::FocusHostSession),
                "FocusGuestSession" => Ok(Action::FocusGuestSession),
                "ToggleHostFullscreen" => Ok(Action::ToggleHostFullscreen),
                "SwitchFocus" => Ok(Action::SwitchFocus),
                "EditScrollback" => Ok(Action::EditScrollback { ansi: false }),
                "ScrollUp" => Ok(Action::ScrollUp),
                "SwapTextSelectionEnds" => Ok(Action::SwapTextSelectionEnds),
                "ScrollDown" => Ok(Action::ScrollDown),
                "ScrollToBottom" => Ok(Action::ScrollToBottom),
                "ScrollToTop" => Ok(Action::ScrollToTop),
                "ScrollToPreviousPrompt" => Ok(Action::ScrollToPreviousPrompt),
                "ScrollToNextPrompt" => Ok(Action::ScrollToNextPrompt),
                "SelectCommandAtScrollPosition" => Ok(Action::SelectCommandAtScrollPosition),
                "CopyLastCommandOutput" => Ok(Action::CopyLastCommandOutput),
                "PageScrollUp" => Ok(Action::PageScrollUp),
                "PageScrollDown" => Ok(Action::PageScrollDown),
                "HalfPageScrollUp" => Ok(Action::HalfPageScrollUp),
                "HalfPageScrollDown" => Ok(Action::HalfPageScrollDown),
                "ToggleFocusFullscreen" => Ok(Action::ToggleFocusFullscreen),
                "ToggleFocusNoUiFullscreen" => Ok(Action::ToggleFocusNoUiFullscreen),
                "TogglePaneFrames" => Ok(Action::TogglePaneFrames),
                "ToggleActiveSyncTab" => Ok(Action::ToggleActiveSyncTab),
                "TogglePaneEmbedOrFloating" => Ok(Action::TogglePaneEmbedOrFloating),
                "ToggleFloatingPanes" => Ok(Action::ToggleFloatingPanes),
                "ShowFloatingPanes" => Ok(Action::ShowFloatingPanes { tab_id: None }),
                "HideFloatingPanes" => Ok(Action::HideFloatingPanes { tab_id: None }),
                "CloseFocus" => Ok(Action::CloseFocus),
                "UndoRenamePane" => Ok(Action::UndoRenamePane),
                "NoOp" => Ok(Action::NoOp),
                "GoToNextTab" => Ok(Action::GoToNextTab),
                "GoToPreviousTab" => Ok(Action::GoToPreviousTab),
                "CloseTab" => Ok(Action::CloseTab),
                "ToggleTab" => Ok(Action::ToggleTab),
                "UndoRenameTab" => Ok(Action::UndoRenameTab),
                "Detach" => Ok(Action::Detach),
                "SetDarkTheme" => Ok(Action::SetDarkTheme),
                "SetLightTheme" => Ok(Action::SetLightTheme),
                "ToggleTheme" => Ok(Action::ToggleTheme),
                "Copy" => Ok(Action::Copy),
                "Confirm" => Ok(Action::Confirm),
                "Deny" => Ok(Action::Deny),
                "ToggleMouseMode" => Ok(Action::ToggleMouseMode),
                "PreviousSwapLayout" => Ok(Action::PreviousSwapLayout),
                "NextSwapLayout" => Ok(Action::NextSwapLayout),
                "Clear" => Ok(Action::ClearScreen),
                _ => Err(ConfigError::new_kdl_error(
                    format!("Unsupported action: {:?}", $action_name),
                    $action_node.span().offset(),
                    $action_node.span().len(),
                )),
            }
        }
    }};
}

#[macro_export]
macro_rules! parse_kdl_action_u8_arguments {
    ( $action_name:expr, $action_arguments:expr, $action_node:expr ) => {{
        let mut bytes = vec![];
        for kdl_entry in $action_arguments.iter() {
            match kdl_entry.value().as_i64() {
                Some(int_value) => bytes.push(int_value as u8),
                None => {
                    return Err(ConfigError::new_kdl_error(
                        format!("Arguments for '{}' must be integers", $action_name),
                        kdl_entry.span().offset(),
                        kdl_entry.span().len(),
                    ));
                },
            }
        }
        Action::new_from_bytes($action_name, bytes, $action_node)
    }};
}

#[macro_export]
macro_rules! kdl_parsing_error {
    ( $message:expr, $entry:expr ) => {
        ConfigError::new_kdl_error($message, $entry.span().offset(), $entry.span().len())
    };
}

#[macro_export]
macro_rules! kdl_entries_as_i64 {
    ( $node:expr ) => {
        $node
            .entries()
            .iter()
            .map(|kdl_node| kdl_node.value().as_i64())
    };
}

#[macro_export]
macro_rules! kdl_first_entry_as_string {
    ( $node:expr ) => {
        $node
            .entries()
            .iter()
            .next()
            .and_then(|s| s.value().as_string())
    };
}

#[macro_export]
macro_rules! kdl_first_entry_as_i64 {
    ( $node:expr ) => {
        $node
            .entries()
            .iter()
            .next()
            .and_then(|i| i.value().as_i64())
    };
}

#[macro_export]
macro_rules! kdl_first_entry_as_bool {
    ( $node:expr ) => {
        $node
            .entries()
            .iter()
            .next()
            .and_then(|i| i.value().as_bool())
    };
}

#[macro_export]
macro_rules! entry_count {
    ( $node:expr ) => {{
        $node.entries().iter().len()
    }};
}

#[macro_export]
macro_rules! parse_kdl_action_char_or_string_arguments {
    ( $action_name:expr, $action_arguments:expr, $action_node:expr ) => {{
        let mut chars_to_write = String::new();
        for kdl_entry in $action_arguments.iter() {
            match kdl_entry.value().as_string() {
                Some(string_value) => chars_to_write.push_str(string_value),
                None => {
                    return Err(ConfigError::new_kdl_error(
                        format!("All entries for action '{}' must be strings", $action_name),
                        kdl_entry.span().offset(),
                        kdl_entry.span().len(),
                    ))
                },
            }
        }
        Action::new_from_string($action_name, chars_to_write, $action_node)
    }};
}

#[macro_export]
macro_rules! kdl_arg_is_truthy {
    ( $kdl_node:expr, $arg_name:expr ) => {
        match $kdl_node.get($arg_name) {
            Some(arg) => match arg.value().as_bool() {
                Some(value) => value,
                None => {
                    return Err(ConfigError::new_kdl_error(
                        format!("Argument must be true or false, found: {}", arg.value()),
                        arg.span().offset(),
                        arg.span().len(),
                    ))
                },
            },
            None => false,
        }
    };
}

#[macro_export]
macro_rules! kdl_children_nodes_or_error {
    ( $kdl_node:expr, $error:expr ) => {
        $kdl_node
            .children()
            .ok_or(ConfigError::new_kdl_error(
                $error.into(),
                $kdl_node.span().offset(),
                $kdl_node.span().len(),
            ))?
            .nodes()
    };
}

#[macro_export]
macro_rules! kdl_children_nodes {
    ( $kdl_node:expr ) => {
        $kdl_node.children().map(|c| c.nodes())
    };
}

#[macro_export]
macro_rules! kdl_property_nodes {
    ( $kdl_node:expr ) => {{
        $kdl_node
            .entries()
            .iter()
            .filter_map(|e| e.name())
            .map(|e| e.value())
    }};
}

#[macro_export]
macro_rules! kdl_children_or_error {
    ( $kdl_node:expr, $error:expr ) => {
        $kdl_node.children().ok_or(ConfigError::new_kdl_error(
            $error.into(),
            $kdl_node.span().offset(),
            $kdl_node.span().len(),
        ))?
    };
}

#[macro_export]
macro_rules! kdl_children {
    ( $kdl_node:expr ) => {
        $kdl_node.children().iter().copied().collect()
    };
}

#[macro_export]
macro_rules! kdl_get_string_property_or_child_value {
    ( $kdl_node:expr, $name:expr ) => {
        $kdl_node
            .get($name)
            .and_then(|e| e.value().as_string())
            .or_else(|| {
                $kdl_node
                    .children()
                    .and_then(|c| c.get($name))
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.value().as_string())
            })
    };
}

#[macro_export]
macro_rules! kdl_string_arguments {
    ( $kdl_node:expr ) => {{
        let res: Result<Vec<_>, _> = $kdl_node
            .entries()
            .iter()
            .map(|e| {
                e.value().as_string().ok_or(ConfigError::new_kdl_error(
                    "Not a string".into(),
                    e.span().offset(),
                    e.span().len(),
                ))
            })
            .collect();
        res?
    }};
}

#[macro_export]
macro_rules! kdl_property_names {
    ( $kdl_node:expr ) => {{
        $kdl_node
            .entries()
            .iter()
            .filter_map(|e| e.name())
            .map(|e| e.value())
    }};
}

#[macro_export]
macro_rules! kdl_argument_values {
    ( $kdl_node:expr ) => {
        $kdl_node.entries().iter().collect()
    };
}

#[macro_export]
macro_rules! kdl_name {
    ( $kdl_node:expr ) => {
        $kdl_node.name().value()
    };
}

#[macro_export]
macro_rules! kdl_document_name {
    ( $kdl_node:expr ) => {
        $kdl_node.node().name().value()
    };
}

#[macro_export]
macro_rules! keys_from_kdl {
    ( $kdl_node:expr ) => {
        kdl_string_arguments!($kdl_node)
            .iter()
            .map(|k| {
                KeyWithModifier::from_str(k).map_err(|_| {
                    ConfigError::new_kdl_error(
                        format!("Invalid key: '{}'", k),
                        $kdl_node.span().offset(),
                        $kdl_node.span().len(),
                    )
                })
            })
            .collect::<Result<_, _>>()?
    };
}

#[macro_export]
macro_rules! actions_from_kdl {
    ( $kdl_node:expr, $config_options:expr ) => {
        kdl_children_nodes_or_error!($kdl_node, "no actions found for key_block")
            .iter()
            .map(|kdl_action| Action::try_from((kdl_action, $config_options)))
            .collect::<Result<_, _>>()?
    };
}

#[allow(clippy::result_large_err)]
pub fn kdl_arguments_that_are_strings<'a>(
    arguments: impl Iterator<Item = &'a KdlEntry>,
) -> Result<Vec<String>, ConfigError> {
    let mut args: Vec<String> = vec![];
    for kdl_entry in arguments {
        match kdl_entry.value().as_string() {
            Some(string_value) => args.push(string_value.to_string()),
            None => {
                return Err(ConfigError::new_kdl_error(
                    "Argument must be a string".to_string(),
                    kdl_entry.span().offset(),
                    kdl_entry.span().len(),
                ));
            },
        }
    }
    Ok(args)
}

#[allow(clippy::result_large_err)]
pub fn kdl_arguments_that_are_digits<'a>(
    arguments: impl Iterator<Item = &'a KdlEntry>,
) -> Result<Vec<i64>, ConfigError> {
    let mut args: Vec<i64> = vec![];
    for kdl_entry in arguments {
        match kdl_entry.value().as_i64() {
            Some(digit_value) => {
                args.push(digit_value);
            },
            None => {
                return Err(ConfigError::new_kdl_error(
                    "Argument must be a digit".to_string(),
                    kdl_entry.span().offset(),
                    kdl_entry.span().len(),
                ));
            },
        }
    }
    Ok(args)
}

pub fn kdl_child_string_value_for_entry<'a>(
    command_metadata: &'a KdlDocument,
    entry_name: &'a str,
) -> Option<&'a str> {
    command_metadata
        .get(entry_name)
        .and_then(|cwd| cwd.entries().iter().next())
        .and_then(|cwd_value| cwd_value.value().as_string())
}

pub fn kdl_child_bool_value_for_entry<'a>(
    command_metadata: &'a KdlDocument,
    entry_name: &'a str,
) -> Option<bool> {
    command_metadata
        .get(entry_name)
        .and_then(|cwd| cwd.entries().iter().next())
        .and_then(|cwd_value| cwd_value.value().as_bool())
}

impl Action {
    #[allow(clippy::result_large_err)]
    pub fn new_from_bytes(
        action_name: &str,
        bytes: Vec<u8>,
        action_node: &KdlNode,
    ) -> Result<Self, ConfigError> {
        match action_name {
            "Write" => Ok(Action::Write {
                key_with_modifier: None,
                bytes,
                is_kitty_keyboard_protocol: false,
            }),
            "PaneNameInput" => Ok(Action::PaneNameInput { input: bytes }),
            "TabNameInput" => Ok(Action::TabNameInput { input: bytes }),
            "SearchInput" => Ok(Action::SearchInput { input: bytes }),
            "GoToTab" => {
                let tab_index = *bytes.first().ok_or_else(|| {
                    ConfigError::new_kdl_error(
                        "Missing tab index".to_string(),
                        action_node.span().offset(),
                        action_node.span().len(),
                    )
                })? as u32;
                Ok(Action::GoToTab { index: tab_index })
            },
            _ => Err(ConfigError::new_kdl_error(
                "Failed to parse action".into(),
                action_node.span().offset(),
                action_node.span().len(),
            )),
        }
    }
    #[allow(clippy::result_large_err)]
    pub fn new_from_string(
        action_name: &str,
        string: String,
        action_node: &KdlNode,
    ) -> Result<Self, ConfigError> {
        match action_name {
            "WriteChars" => Ok(Action::WriteChars { chars: string }),
            "SetPaneFrameStyle" => {
                let style = PaneFrameStyle::from_str(string.as_str()).map_err(|e| {
                    ConfigError::new_kdl_error(
                        format!("{}", e),
                        action_node.span().offset(),
                        action_node.span().len(),
                    )
                })?;
                Ok(Action::SetPaneFrameStyle(style))
            },
            "SwitchToMode" => match InputMode::from_str(string.as_str()) {
                Ok(input_mode) => Ok(Action::SwitchToMode { input_mode }),
                Err(_e) => Err(ConfigError::new_kdl_error(
                    format!("Unknown InputMode '{}'", string),
                    action_node.span().offset(),
                    action_node.span().len(),
                )),
            },
            "Resize" => {
                let mut resize: Option<Resize> = None;
                let mut direction: Option<Direction> = None;
                for word in string.to_ascii_lowercase().split_whitespace() {
                    match Resize::from_str(word) {
                        Ok(value) => resize = Some(value),
                        Err(_) => match Direction::from_str(word) {
                            Ok(value) => direction = Some(value),
                            Err(_) => {
                                return Err(ConfigError::new_kdl_error(
                                    format!(
                                    "failed to read either of resize type or direction from '{}'",
                                    word
                                ),
                                    action_node.span().offset(),
                                    action_node.span().len(),
                                ))
                            },
                        },
                    }
                }
                let resize = resize.unwrap_or(Resize::Increase);
                Ok(Action::Resize { resize, direction })
            },
            "MoveFocus" => {
                let direction = Direction::from_str(string.as_str()).map_err(|_| {
                    ConfigError::new_kdl_error(
                        format!("Invalid direction: '{}'", string),
                        action_node.span().offset(),
                        action_node.span().len(),
                    )
                })?;
                Ok(Action::MoveFocus { direction })
            },
            "MoveTextSelection" => {
                let direction = Direction::from_str(string.as_str()).map_err(|_| {
                    ConfigError::new_kdl_error(
                        format!("Invalid direction: '{}'", string),
                        action_node.span().offset(),
                        action_node.span().len(),
                    )
                })?;
                Ok(Action::MoveTextSelection { direction })
            },
            "MoveFocusOrTab" => {
                let direction = Direction::from_str(string.as_str()).map_err(|_| {
                    ConfigError::new_kdl_error(
                        format!("Invalid direction: '{}'", string),
                        action_node.span().offset(),
                        action_node.span().len(),
                    )
                })?;
                Ok(Action::MoveFocusOrTab { direction })
            },
            "MoveTab" => {
                let direction = Direction::from_str(string.as_str()).map_err(|_| {
                    ConfigError::new_kdl_error(
                        format!("Invalid direction: '{}'", string),
                        action_node.span().offset(),
                        action_node.span().len(),
                    )
                })?;
                if direction.is_vertical() {
                    Err(ConfigError::new_kdl_error(
                        format!("Invalid horizontal direction: '{}'", string),
                        action_node.span().offset(),
                        action_node.span().len(),
                    ))
                } else {
                    Ok(Action::MoveTab { direction })
                }
            },
            "MovePane" => {
                if string.is_empty() {
                    Ok(Action::MovePane { direction: None })
                } else {
                    let direction = Direction::from_str(string.as_str()).map_err(|_| {
                        ConfigError::new_kdl_error(
                            format!("Invalid direction: '{}'", string),
                            action_node.span().offset(),
                            action_node.span().len(),
                        )
                    })?;
                    Ok(Action::MovePane {
                        direction: Some(direction),
                    })
                }
            },
            "MovePaneBackwards" => Ok(Action::MovePaneBackwards),
            "DumpScreen" => Ok(Action::DumpScreen {
                file_path: Some(string),
                include_scrollback: false,
                pane_id: None,
                ansi: false,
            }),
            "DumpLayout" => Ok(Action::DumpLayout),
            "NewPane" => {
                if string.is_empty() {
                    Ok(Action::NewPane {
                        direction: None,
                        pane_name: None,
                        start_suppressed: false,
                    })
                } else if string == "stacked" {
                    Ok(Action::NewStackedPane {
                        command: None,
                        pane_name: None,
                        near_current_pane: false,
                        no_focus: false,
                        tab_id: None,
                    })
                } else {
                    let direction = Direction::from_str(string.as_str()).map_err(|_| {
                        ConfigError::new_kdl_error(
                            format!("Invalid direction: '{}'", string),
                            action_node.span().offset(),
                            action_node.span().len(),
                        )
                    })?;
                    Ok(Action::NewPane {
                        direction: Some(direction),
                        pane_name: None,
                        start_suppressed: false,
                    })
                }
            },
            "SearchToggleOption" => {
                let toggle_option = SearchOption::from_str(string.as_str()).map_err(|_| {
                    ConfigError::new_kdl_error(
                        format!("Invalid direction: '{}'", string),
                        action_node.span().offset(),
                        action_node.span().len(),
                    )
                })?;
                Ok(Action::SearchToggleOption {
                    option: toggle_option,
                })
            },
            "Search" => {
                let search_direction =
                    SearchDirection::from_str(string.as_str()).map_err(|_| {
                        ConfigError::new_kdl_error(
                            format!("Invalid direction: '{}'", string),
                            action_node.span().offset(),
                            action_node.span().len(),
                        )
                    })?;
                Ok(Action::Search {
                    direction: search_direction,
                })
            },
            "RenameSession" => Ok(Action::RenameSession { name: string }),
            "Notify" => Ok(Action::Notify(string)),
            _ => Err(ConfigError::new_kdl_error(
                format!("Unsupported action: {}", action_name),
                action_node.span().offset(),
                action_node.span().len(),
            )),
        }
    }
    pub fn to_kdl(&self) -> Option<KdlNode> {
        match self {
            Action::Quit => Some(KdlNode::new("Quit")),
            Action::Write {
                key_with_modifier: _key,
                bytes,
                is_kitty_keyboard_protocol: _is_kitty,
            } => {
                let mut node = KdlNode::new("Write");
                for byte in bytes {
                    node.push(KdlValue::Base10(*byte as i64));
                }
                Some(node)
            },
            Action::WriteChars { chars: string } => {
                let mut node = KdlNode::new("WriteChars");
                node.push(string.clone());
                Some(node)
            },
            Action::Notify(text) => {
                let mut node = KdlNode::new("Notify");
                node.push(text.clone());
                Some(node)
            },
            Action::SwitchToMode { input_mode } => {
                let mut node = KdlNode::new("SwitchToMode");
                node.push(format!("{:?}", input_mode).to_lowercase());
                Some(node)
            },
            Action::Resize {
                resize,
                direction: resize_direction,
            } => {
                let mut node = KdlNode::new("Resize");
                let resize = match resize {
                    Resize::Increase => "Increase",
                    Resize::Decrease => "Decrease",
                };
                if let Some(resize_direction) = resize_direction {
                    let resize_direction = match resize_direction {
                        Direction::Left => "left",
                        Direction::Right => "right",
                        Direction::Up => "up",
                        Direction::Down => "down",
                    };
                    node.push(format!("{} {}", resize, resize_direction));
                } else {
                    node.push(resize.to_string());
                }
                Some(node)
            },
            Action::FocusNextPane => Some(KdlNode::new("FocusNextPane")),
            Action::FocusPreviousPane => Some(KdlNode::new("FocusPreviousPane")),
            Action::FocusLastPane => Some(KdlNode::new("FocusLastPane")),
            Action::SwitchFocus => Some(KdlNode::new("SwitchFocus")),
            Action::MoveFocus { direction } => {
                let mut node = KdlNode::new("MoveFocus");
                let direction = match direction {
                    Direction::Left => "left",
                    Direction::Right => "right",
                    Direction::Up => "up",
                    Direction::Down => "down",
                };
                node.push(direction);
                Some(node)
            },
            Action::MoveFocusOrTab { direction } => {
                let mut node = KdlNode::new("MoveFocusOrTab");
                let direction = match direction {
                    Direction::Left => "left",
                    Direction::Right => "right",
                    Direction::Up => "up",
                    Direction::Down => "down",
                };
                node.push(direction);
                Some(node)
            },
            Action::MovePane { direction } => {
                let mut node = KdlNode::new("MovePane");
                if let Some(direction) = direction {
                    let direction = match direction {
                        Direction::Left => "left",
                        Direction::Right => "right",
                        Direction::Up => "up",
                        Direction::Down => "down",
                    };
                    node.push(direction);
                }
                Some(node)
            },
            Action::MovePaneBackwards => Some(KdlNode::new("MovePaneBackwards")),
            Action::DumpScreen {
                file_path: Some(file),
                include_scrollback: _,
                pane_id: _,
                ansi: _,
            } => {
                let mut node = KdlNode::new("DumpScreen");
                node.push(file.clone());
                Some(node)
            },
            Action::DumpScreen {
                file_path: None, ..
            } => None,
            Action::DumpLayout => Some(KdlNode::new("DumpLayout")),
            Action::EditScrollback { ansi } => {
                let mut node = KdlNode::new("EditScrollback");
                if *ansi {
                    let mut children = KdlDocument::new();
                    let mut ansi_node = KdlNode::new("ansi");
                    ansi_node.push(KdlValue::Bool(true));
                    children.nodes_mut().push(ansi_node);
                    node.set_children(children);
                }
                Some(node)
            },
            Action::ScrollUp => Some(KdlNode::new("ScrollUp")),
            Action::ScrollDown => Some(KdlNode::new("ScrollDown")),
            Action::MoveTextSelection { direction } => {
                let mut node = KdlNode::new("MoveTextSelection");
                node.push(match direction {
                    Direction::Left => "left",
                    Direction::Right => "right",
                    Direction::Up => "up",
                    Direction::Down => "down",
                });
                Some(node)
            },
            Action::SwapTextSelectionEnds => Some(KdlNode::new("SwapTextSelectionEnds")),
            Action::ScrollToBottom => Some(KdlNode::new("ScrollToBottom")),
            Action::ScrollToTop => Some(KdlNode::new("ScrollToTop")),
            Action::ScrollToPreviousPrompt => Some(KdlNode::new("ScrollToPreviousPrompt")),
            Action::ScrollToNextPrompt => Some(KdlNode::new("ScrollToNextPrompt")),
            Action::SelectCommandAtScrollPosition => {
                Some(KdlNode::new("SelectCommandAtScrollPosition"))
            },
            Action::CopyLastCommandOutput => Some(KdlNode::new("CopyLastCommandOutput")),
            Action::PageScrollUp => Some(KdlNode::new("PageScrollUp")),
            Action::PageScrollDown => Some(KdlNode::new("PageScrollDown")),
            Action::HalfPageScrollUp => Some(KdlNode::new("HalfPageScrollUp")),
            Action::HalfPageScrollDown => Some(KdlNode::new("HalfPageScrollDown")),
            Action::ToggleFocusFullscreen => Some(KdlNode::new("ToggleFocusFullscreen")),
            Action::ToggleFocusNoUiFullscreen => Some(KdlNode::new("ToggleFocusNoUiFullscreen")),
            Action::TogglePaneFrames => Some(KdlNode::new("TogglePaneFrames")),
            Action::SetPaneFrameStyle(style) => {
                let mut node = KdlNode::new("SetPaneFrameStyle");
                let style = match style {
                    PaneFrameStyle::Full => "full",
                    PaneFrameStyle::Titles => "titles",
                    PaneFrameStyle::None => "none",
                };
                node.push(style);
                Some(node)
            },
            Action::ToggleActiveSyncTab => Some(KdlNode::new("ToggleActiveSyncTab")),
            Action::NewPane {
                direction,
                pane_name: _,
                start_suppressed: _,
            } => {
                let mut node = KdlNode::new("NewPane");
                if let Some(direction) = direction {
                    let direction = match direction {
                        Direction::Left => "left",
                        Direction::Right => "right",
                        Direction::Up => "up",
                        Direction::Down => "down",
                    };
                    node.push(direction);
                }
                let children = KdlDocument::new();
                if !children.nodes().is_empty() {
                    node.set_children(children);
                }
                Some(node)
            },
            Action::TogglePaneEmbedOrFloating => Some(KdlNode::new("TogglePaneEmbedOrFloating")),
            Action::ToggleFloatingPanes => Some(KdlNode::new("ToggleFloatingPanes")),
            Action::ShowFloatingPanes { tab_id } => {
                let mut node = KdlNode::new("ShowFloatingPanes");
                if let Some(id) = tab_id {
                    node.push(KdlValue::Base10(*id as i64));
                }
                Some(node)
            },
            Action::HideFloatingPanes { tab_id } => {
                let mut node = KdlNode::new("HideFloatingPanes");
                if let Some(id) = tab_id {
                    node.push(KdlValue::Base10(*id as i64));
                }
                Some(node)
            },
            Action::CloseFocus => Some(KdlNode::new("CloseFocus")),
            Action::PaneNameInput { input: bytes } => {
                let mut node = KdlNode::new("PaneNameInput");
                for byte in bytes {
                    node.push(KdlValue::Base10(*byte as i64));
                }
                Some(node)
            },
            Action::UndoRenamePane => Some(KdlNode::new("UndoRenamePane")),
            Action::NewTab {
                layout_source,
                tiled_layout,
                floating_layouts: _,
                swap_tiled_layouts: _,
                swap_floating_layouts: _,
                tab_name: name,
                should_change_focus_to_new_tab,
                cwd,
                initial_panes: _,
                first_pane_unblock_condition: _,
            } => {
                let mut node = KdlNode::new("NewTab");
                let mut children = KdlDocument::new();
                if let Some(layout_source) = layout_source {
                    let mut layout_node = KdlNode::new("layout");
                    layout_node.push(layout_source.display().to_string());
                    children.nodes_mut().push(layout_node);
                }
                if !should_change_focus_to_new_tab
                    || tiled_layout.as_ref().and_then(|layout| layout.focus) == Some(false)
                {
                    let mut focus_node = KdlNode::new("should_change_focus_to_new_tab");
                    focus_node.push(KdlValue::Bool(*should_change_focus_to_new_tab));
                    children.nodes_mut().push(focus_node);
                }
                if let Some(name) = name {
                    let mut name_node = KdlNode::new("name");
                    name_node.push(name.clone());
                    children.nodes_mut().push(name_node);
                }
                if let Some(cwd) = cwd {
                    let mut cwd_node = KdlNode::new("cwd");
                    cwd_node.push(cwd.display().to_string());
                    children.nodes_mut().push(cwd_node);
                }
                // An empty metadata block still loads the configured default layout;
                // a bare NewTab defers layout selection until execution.
                if !children.nodes().is_empty() || tiled_layout.is_some() {
                    node.set_children(children);
                }
                Some(node)
            },
            Action::GoToNextTab => Some(KdlNode::new("GoToNextTab")),
            Action::GoToPreviousTab => Some(KdlNode::new("GoToPreviousTab")),
            Action::CloseTab => Some(KdlNode::new("CloseTab")),
            Action::GoToTab { index } => {
                let mut node = KdlNode::new("GoToTab");
                node.push(KdlValue::Base10(*index as i64));
                Some(node)
            },
            Action::ToggleTab => Some(KdlNode::new("ToggleTab")),
            Action::TabNameInput { input: bytes } => {
                let mut node = KdlNode::new("TabNameInput");
                for byte in bytes {
                    node.push(KdlValue::Base10(*byte as i64));
                }
                Some(node)
            },
            Action::UndoRenameTab => Some(KdlNode::new("UndoRenameTab")),
            Action::MoveTab { direction } => {
                let mut node = KdlNode::new("MoveTab");
                let direction = match direction {
                    Direction::Left => "left",
                    Direction::Right => "right",
                    Direction::Up => "up",
                    Direction::Down => "down",
                };
                node.push(direction);
                Some(node)
            },
            Action::NewTiledPane {
                direction,
                command: run_command_action,
                pane_name: name,
                near_current_pane: false,
                ..
            } => {
                let mut node = KdlNode::new("Run");
                let mut node_children = KdlDocument::new();
                if let Some(run_command_action) = run_command_action {
                    node.push(run_command_action.command.display().to_string());
                    for arg in &run_command_action.args {
                        node.push(arg.clone());
                    }
                    if let Some(cwd) = &run_command_action.cwd {
                        let mut cwd_node = KdlNode::new("cwd");
                        cwd_node.push(cwd.display().to_string());
                        node_children.nodes_mut().push(cwd_node);
                    }
                    push_firecracker_nodes(run_command_action, &mut node_children);
                    if run_command_action.hold_on_start {
                        let mut hos_node = KdlNode::new("hold_on_start");
                        hos_node.push(KdlValue::Bool(true));
                        node_children.nodes_mut().push(hos_node);
                    }
                    if !run_command_action.hold_on_close {
                        let mut hoc_node = KdlNode::new("hold_on_close");
                        hoc_node.push(KdlValue::Bool(false));
                        node_children.nodes_mut().push(hoc_node);
                    }
                }
                if let Some(name) = name {
                    let mut name_node = KdlNode::new("name");
                    name_node.push(name.clone());
                    node_children.nodes_mut().push(name_node);
                }
                if let Some(direction) = direction {
                    let mut direction_node = KdlNode::new("direction");
                    let direction = match direction {
                        Direction::Left => "left",
                        Direction::Right => "right",
                        Direction::Up => "up",
                        Direction::Down => "down",
                    };
                    direction_node.push(direction);
                    node_children.nodes_mut().push(direction_node);
                }
                if !node_children.nodes().is_empty() {
                    node.set_children(node_children);
                }
                Some(node)
            },
            Action::NewFloatingPane {
                command: run_command_action,
                pane_name: name,
                placement,
                near_current_pane: false,
                ..
            } => {
                let floating_pane_coordinates = placement.floating_pane_coordinates();
                let mut node = KdlNode::new("Run");
                let mut node_children = KdlDocument::new();
                let mut floating_pane = KdlNode::new("floating");
                floating_pane.push(KdlValue::Bool(true));
                node_children.nodes_mut().push(floating_pane);
                if let Some(run_command_action) = run_command_action {
                    node.push(run_command_action.command.display().to_string());
                    for arg in &run_command_action.args {
                        node.push(arg.clone());
                    }
                    if let Some(cwd) = &run_command_action.cwd {
                        let mut cwd_node = KdlNode::new("cwd");
                        cwd_node.push(cwd.display().to_string());
                        node_children.nodes_mut().push(cwd_node);
                    }
                    push_firecracker_nodes(run_command_action, &mut node_children);
                    if run_command_action.hold_on_start {
                        let mut hos_node = KdlNode::new("hold_on_start");
                        hos_node.push(KdlValue::Bool(true));
                        node_children.nodes_mut().push(hos_node);
                    }
                    if !run_command_action.hold_on_close {
                        let mut hoc_node = KdlNode::new("hold_on_close");
                        hoc_node.push(KdlValue::Bool(false));
                        node_children.nodes_mut().push(hoc_node);
                    }
                }
                if let Some(floating_pane_coordinates) = floating_pane_coordinates {
                    if let Some(x) = floating_pane_coordinates.x {
                        let mut x_node = KdlNode::new("x");
                        match x {
                            PercentOrFixed::Percent(x) => {
                                x_node.push(format!("{}%", x));
                            },
                            PercentOrFixed::Fixed(x) => {
                                x_node.push(KdlValue::Base10(x as i64));
                            },
                        };
                        node_children.nodes_mut().push(x_node);
                    }
                    if let Some(y) = floating_pane_coordinates.y {
                        let mut y_node = KdlNode::new("y");
                        match y {
                            PercentOrFixed::Percent(y) => {
                                y_node.push(format!("{}%", y));
                            },
                            PercentOrFixed::Fixed(y) => {
                                y_node.push(KdlValue::Base10(y as i64));
                            },
                        };
                        node_children.nodes_mut().push(y_node);
                    }
                    if let Some(width) = floating_pane_coordinates.width {
                        let mut width_node = KdlNode::new("width");
                        match width {
                            PercentOrFixed::Percent(width) => {
                                width_node.push(format!("{}%", width));
                            },
                            PercentOrFixed::Fixed(width) => {
                                width_node.push(KdlValue::Base10(width as i64));
                            },
                        };
                        node_children.nodes_mut().push(width_node);
                    }
                    if let Some(height) = floating_pane_coordinates.height {
                        let mut height_node = KdlNode::new("height");
                        match height {
                            PercentOrFixed::Percent(height) => {
                                height_node.push(format!("{}%", height));
                            },
                            PercentOrFixed::Fixed(height) => {
                                height_node.push(KdlValue::Base10(height as i64));
                            },
                        };
                        node_children.nodes_mut().push(height_node);
                    }
                }
                if let Some(name) = name {
                    let mut name_node = KdlNode::new("name");
                    name_node.push(name.clone());
                    node_children.nodes_mut().push(name_node);
                }
                if !node_children.nodes().is_empty() {
                    node.set_children(node_children);
                }
                Some(node)
            },
            Action::NewInPlacePane {
                command: run_command_action,
                pane_name: name,
                near_current_pane: false,
                pane_id_to_replace: None,
                close_replaced_pane,
                ..
            } => {
                let mut node = KdlNode::new("Run");
                let mut node_children = KdlDocument::new();
                if let Some(run_command_action) = run_command_action {
                    node.push(run_command_action.command.display().to_string());
                    for arg in &run_command_action.args {
                        node.push(arg.clone());
                    }
                    let mut in_place_node = KdlNode::new("in_place");
                    in_place_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(in_place_node);
                    if let Some(cwd) = &run_command_action.cwd {
                        let mut cwd_node = KdlNode::new("cwd");
                        cwd_node.push(cwd.display().to_string());
                        node_children.nodes_mut().push(cwd_node);
                    }
                    push_firecracker_nodes(run_command_action, &mut node_children);
                    if run_command_action.hold_on_start {
                        let mut hos_node = KdlNode::new("hold_on_start");
                        hos_node.push(KdlValue::Bool(true));
                        node_children.nodes_mut().push(hos_node);
                    }
                    if !run_command_action.hold_on_close {
                        let mut hoc_node = KdlNode::new("hold_on_close");
                        hoc_node.push(KdlValue::Bool(false));
                        node_children.nodes_mut().push(hoc_node);
                    }
                }
                if *close_replaced_pane {
                    let mut crp_node = KdlNode::new("close_replaced_pane");
                    crp_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(crp_node);
                }
                if let Some(name) = name {
                    let mut name_node = KdlNode::new("name");
                    name_node.push(name.clone());
                    node_children.nodes_mut().push(name_node);
                }
                if !node_children.nodes().is_empty() {
                    node.set_children(node_children);
                }
                Some(node)
            },
            Action::NewStackedPane {
                command: run_command_action,
                pane_name: name,
                ..
            } => match run_command_action {
                Some(run_command_action) => {
                    let mut node = KdlNode::new("Run");
                    let mut node_children = KdlDocument::new();
                    node.push(run_command_action.command.display().to_string());
                    for arg in &run_command_action.args {
                        node.push(arg.clone());
                    }
                    let mut stacked_node = KdlNode::new("stacked");
                    stacked_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(stacked_node);
                    if let Some(cwd) = &run_command_action.cwd {
                        let mut cwd_node = KdlNode::new("cwd");
                        cwd_node.push(cwd.display().to_string());
                        node_children.nodes_mut().push(cwd_node);
                    }
                    push_firecracker_nodes(run_command_action, &mut node_children);
                    if run_command_action.hold_on_start {
                        let mut hos_node = KdlNode::new("hold_on_start");
                        hos_node.push(KdlValue::Bool(true));
                        node_children.nodes_mut().push(hos_node);
                    }
                    if !run_command_action.hold_on_close {
                        let mut hoc_node = KdlNode::new("hold_on_close");
                        hoc_node.push(KdlValue::Bool(false));
                        node_children.nodes_mut().push(hoc_node);
                    }
                    if let Some(name) = name {
                        let mut name_node = KdlNode::new("name");
                        name_node.push(name.clone());
                        node_children.nodes_mut().push(name_node);
                    }
                    if !node_children.nodes().is_empty() {
                        node.set_children(node_children);
                    }
                    Some(node)
                },
                None => {
                    let mut node = KdlNode::new("NewPane");
                    node.push("stacked");
                    Some(node)
                },
            },
            Action::Detach => Some(KdlNode::new("Detach")),
            Action::SwitchSession {
                name,
                tab_position,
                pane_id,
                layout,
                cwd,
            } => {
                let mut node = KdlNode::new("SwitchSession");
                node.push(KdlEntry::new_prop("name", name.clone()));
                if let Some(pos) = tab_position {
                    node.push(KdlEntry::new_prop("tab_position", *pos as i64));
                }
                if let Some((id, is_plugin)) = pane_id {
                    node.push(KdlEntry::new_prop("pane_id", *id as i64));
                    if *is_plugin {
                        node.push(KdlEntry::new_prop("is_plugin", true));
                    }
                }
                if let Some(layout_info) = layout {
                    node.push(KdlEntry::new_prop("layout", layout_info.name()));
                }
                if let Some(cwd_path) = cwd {
                    node.push(KdlEntry::new_prop(
                        "cwd",
                        cwd_path.to_string_lossy().to_string(),
                    ));
                }
                Some(node)
            },
            Action::LaunchOrFocusPlugin {
                plugin: run_plugin_or_alias,
                should_float,
                move_to_focused_tab,
                should_open_in_place,
                close_replaced_pane,
                skip_cache: skip_plugin_cache,
                ..
            } => {
                let mut node = KdlNode::new("LaunchOrFocusPlugin");
                let mut node_children = KdlDocument::new();
                let location = run_plugin_or_alias.location_string();
                node.push(location);
                if *should_float {
                    let mut should_float_node = KdlNode::new("floating");
                    should_float_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(should_float_node);
                }
                if *move_to_focused_tab {
                    let mut move_to_focused_tab_node = KdlNode::new("move_to_focused_tab");
                    move_to_focused_tab_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(move_to_focused_tab_node);
                }
                if *should_open_in_place {
                    let mut should_open_in_place_node = KdlNode::new("in_place");
                    should_open_in_place_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(should_open_in_place_node);
                }
                if *close_replaced_pane {
                    let mut crp_node = KdlNode::new("close_replaced_pane");
                    crp_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(crp_node);
                }
                if *skip_plugin_cache {
                    let mut skip_plugin_cache_node = KdlNode::new("skip_plugin_cache");
                    skip_plugin_cache_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(skip_plugin_cache_node);
                }
                let configuration = match run_plugin_or_alias {
                    RunPluginOrAlias::RunPlugin(plugin) => Some(&plugin.configuration),
                    RunPluginOrAlias::Alias(alias) => alias.configuration.as_ref(),
                };
                if let Some(configuration) = configuration {
                    for (config_key, config_value) in configuration.inner().iter() {
                        let mut node = KdlNode::new(config_key.clone());
                        if matches!(
                            config_key.as_str(),
                            "overlay" | "modal" | "awaits_placement"
                        ) && config_value == "true"
                        {
                            node.push(KdlValue::Bool(true));
                        } else {
                            node.push(config_value.clone());
                        }
                        node_children.nodes_mut().push(node);
                    }
                }
                if !node_children.nodes().is_empty() {
                    node.set_children(node_children);
                }
                Some(node)
            },
            Action::LaunchPlugin {
                plugin: run_plugin_or_alias,
                should_float,
                should_open_in_place,
                close_replaced_pane,
                skip_cache: skip_plugin_cache,
                cwd,
                ..
            } => {
                let mut node = KdlNode::new("LaunchPlugin");
                let mut node_children = KdlDocument::new();
                let location = run_plugin_or_alias.location_string();
                node.push(location);
                if *should_float {
                    let mut should_float_node = KdlNode::new("floating");
                    should_float_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(should_float_node);
                }
                if *should_open_in_place {
                    let mut should_open_in_place_node = KdlNode::new("in_place");
                    should_open_in_place_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(should_open_in_place_node);
                }
                if *close_replaced_pane {
                    let mut crp_node = KdlNode::new("close_replaced_pane");
                    crp_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(crp_node);
                }
                if *skip_plugin_cache {
                    let mut skip_plugin_cache_node = KdlNode::new("skip_plugin_cache");
                    skip_plugin_cache_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(skip_plugin_cache_node);
                }
                if let Some(cwd) = &cwd {
                    let mut cwd_node = KdlNode::new("cwd");
                    cwd_node.push(cwd.display().to_string());
                    node_children.nodes_mut().push(cwd_node);
                } else if let Some(cwd) = run_plugin_or_alias.get_initial_cwd() {
                    let mut cwd_node = KdlNode::new("cwd");
                    cwd_node.push(cwd.display().to_string());
                    node_children.nodes_mut().push(cwd_node);
                }
                if let Some(configuration) = run_plugin_or_alias.get_configuration() {
                    for (config_key, config_value) in configuration.inner().iter() {
                        let mut node = KdlNode::new(config_key.clone());
                        node.push(config_value.clone());
                        node_children.nodes_mut().push(node);
                    }
                }
                if !node_children.nodes().is_empty() {
                    node.set_children(node_children);
                }
                Some(node)
            },
            Action::Copy => Some(KdlNode::new("Copy")),
            Action::SearchInput { input: bytes } => {
                let mut node = KdlNode::new("SearchInput");
                for byte in bytes {
                    node.push(KdlValue::Base10(*byte as i64));
                }
                Some(node)
            },
            Action::Search {
                direction: search_direction,
            } => {
                let mut node = KdlNode::new("Search");
                let direction = match search_direction {
                    SearchDirection::Down => "down",
                    SearchDirection::Up => "up",
                };
                node.push(direction);
                Some(node)
            },
            Action::SearchToggleOption {
                option: search_toggle_option,
            } => {
                let mut node = KdlNode::new("SearchToggleOption");
                node.push(format!("{:?}", search_toggle_option));
                Some(node)
            },
            Action::ToggleMouseMode => Some(KdlNode::new("ToggleMouseMode")),
            Action::PreviousSwapLayout => Some(KdlNode::new("PreviousSwapLayout")),
            Action::NextSwapLayout => Some(KdlNode::new("NextSwapLayout")),
            Action::BreakPane => Some(KdlNode::new("BreakPane")),
            Action::BreakPaneRight => Some(KdlNode::new("BreakPaneRight")),
            Action::BreakPaneLeft => Some(KdlNode::new("BreakPaneLeft")),
            Action::KeybindPipe {
                name,
                payload,
                args: _, // currently unsupported
                plugin,
                configuration,
                launch_new,
                skip_cache,
                floating,
                in_place: _, // currently unsupported
                cwd,
                pane_title,
                plugin_id,
            } => {
                if plugin_id.is_some() {
                    log::warn!("Not serializing temporary keybinding MessagePluginId");
                    return None;
                }
                let mut node = KdlNode::new("MessagePlugin");
                let mut node_children = KdlDocument::new();
                if let Some(plugin) = plugin {
                    node.push(plugin.clone());
                }
                if let Some(name) = name {
                    let mut name_node = KdlNode::new("name");
                    name_node.push(name.clone());
                    node_children.nodes_mut().push(name_node);
                }
                if let Some(cwd) = cwd {
                    let mut cwd_node = KdlNode::new("cwd");
                    cwd_node.push(cwd.display().to_string());
                    node_children.nodes_mut().push(cwd_node);
                }
                if let Some(payload) = payload {
                    let mut payload_node = KdlNode::new("payload");
                    payload_node.push(payload.clone());
                    node_children.nodes_mut().push(payload_node);
                }
                if *launch_new {
                    let mut launch_new_node = KdlNode::new("launch_new");
                    launch_new_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(launch_new_node);
                }
                if *skip_cache {
                    let mut skip_cache_node = KdlNode::new("skip_cache");
                    skip_cache_node.push(KdlValue::Bool(true));
                    node_children.nodes_mut().push(skip_cache_node);
                }
                if let Some(floating) = floating {
                    let mut floating_node = KdlNode::new("floating");
                    floating_node.push(KdlValue::Bool(*floating));
                    node_children.nodes_mut().push(floating_node);
                }
                if let Some(title) = pane_title {
                    let mut title_node = KdlNode::new("title");
                    title_node.push(title.clone());
                    node_children.nodes_mut().push(title_node);
                }
                if let Some(configuration) = configuration {
                    // we do this because the constructor removes the relevant config fields from
                    // above, otherwise we would have duplicates
                    let configuration = PluginUserConfiguration::new(configuration.clone());
                    let configuration = configuration.inner();
                    for (config_key, config_value) in configuration.iter() {
                        let mut node = KdlNode::new(config_key.clone());
                        node.push(config_value.clone());
                        node_children.nodes_mut().push(node);
                    }
                }
                if !node_children.nodes().is_empty() {
                    node.set_children(node_children);
                }
                Some(node)
            },
            Action::TogglePanePinned => Some(KdlNode::new("TogglePanePinned")),
            Action::TogglePaneInGroup => Some(KdlNode::new("TogglePaneInGroup")),
            Action::ToggleGroupMarking => Some(KdlNode::new("ToggleGroupMarking")),
            Action::SetDarkTheme => Some(KdlNode::new("SetDarkTheme")),
            Action::SetLightTheme => Some(KdlNode::new("SetLightTheme")),
            Action::ToggleTheme => Some(KdlNode::new("ToggleTheme")),
            Action::FocusHostSession => Some(KdlNode::new("FocusHostSession")),
            Action::FocusGuestSession => Some(KdlNode::new("FocusGuestSession")),
            Action::ToggleHostFullscreen => Some(KdlNode::new("ToggleHostFullscreen")),
            _ => None,
        }
    }
}

// Keep the parser's existing diagnostic payload and error propagation.
#[allow(clippy::result_large_err)]
pub fn palette_color_from_kdl(
    color_name: &str,
    theme_colors: &KdlDocument,
) -> Result<PaletteColor, ConfigError> {
    let color = theme_colors
        .get(color_name)
        .ok_or(ConfigError::new_kdl_error(
            format!("Missing theme color: {}", color_name),
            theme_colors.span().offset(),
            theme_colors.span().len(),
        ))?;
    let entry_count = entry_count!(color);
    let is_rgb = || entry_count == 3;
    // `#fff` or `#ffffff`. A malformed body (`#é1`, `#12`, `#ggg`) is `None` and
    // falls through to the `Err` arm the parse already models rather than
    // panicking on a mid-char slice — see `parse_hex_rgb`.
    let hex_rgb = || match kdl_first_entry_as_string!(color) {
        Some(s) if entry_count == 1 && s.starts_with('#') => crate::shared::parse_hex_rgb(s),
        _ => None,
    };
    let is_eight_bit = || kdl_first_entry_as_i64!(color).is_some() && entry_count == 1;
    if is_rgb() {
        let mut channels = kdl_entries_as_i64!(color);
        let r = channels.next().unwrap().ok_or(ConfigError::new_kdl_error(
            "invalid rgb color".to_string(),
            color.span().offset(),
            color.span().len(),
        ))? as u8;
        let g = channels.next().unwrap().ok_or(ConfigError::new_kdl_error(
            "invalid rgb color".to_string(),
            color.span().offset(),
            color.span().len(),
        ))? as u8;
        let b = channels.next().unwrap().ok_or(ConfigError::new_kdl_error(
            "invalid rgb color".to_string(),
            color.span().offset(),
            color.span().len(),
        ))? as u8;
        Ok(PaletteColor::Rgb((r, g, b)))
    } else if let Some(rgb) = hex_rgb() {
        Ok(PaletteColor::Rgb(rgb))
    } else if is_eight_bit() {
        let n = kdl_first_entry_as_i64!(color).ok_or(ConfigError::new_kdl_error(
            "Failed to parse color".into(),
            color.span().offset(),
            color.span().len(),
        ))?;
        Ok(PaletteColor::EightBit(n as u8))
    } else {
        Err(ConfigError::new_kdl_error(
            "Failed to parse color".into(),
            color.span().offset(),
            color.span().len(),
        ))
    }
}

pub fn palette_color_to_kdl(color: &PaletteColor, color_name: &str) -> KdlNode {
    let mut node = KdlNode::new(color_name);
    match color {
        PaletteColor::Rgb((r, g, b)) => {
            node.push(KdlValue::Base10(*r as i64));
            node.push(KdlValue::Base10(*g as i64));
            node.push(KdlValue::Base10(*b as i64));
        },
        PaletteColor::EightBit(color_index) => {
            node.push(KdlValue::Base10(*color_index as i64));
        },
    }
    node
}

pub fn style_declaration_to_kdl(style: &StyleDeclaration, declaration_name: &str) -> KdlNode {
    let mut node = KdlNode::new(declaration_name);
    let mut doc = KdlDocument::new();

    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.base, "base"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.background, "background"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.emphasis_0, "emphasis_0"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.emphasis_1, "emphasis_1"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.emphasis_2, "emphasis_2"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.emphasis_3, "emphasis_3"));
    node.set_children(doc);
    node
}

pub fn multiplayer_colors_to_kdl(style: &MultiplayerColors) -> KdlNode {
    let mut node = KdlNode::new("multiplayer_user_colors");
    let mut doc = KdlDocument::new();
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.player_1, "player_1"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.player_2, "player_2"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.player_3, "player_3"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.player_4, "player_4"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.player_5, "player_5"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.player_6, "player_6"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.player_7, "player_7"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.player_8, "player_8"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.player_9, "player_9"));
    doc.nodes_mut()
        .push(palette_color_to_kdl(&style.player_10, "player_10"));
    node.set_children(doc);
    node
}

impl TryFrom<(&KdlNode, &Options)> for Action {
    type Error = ConfigError;
    fn try_from((kdl_action, config_options): (&KdlNode, &Options)) -> Result<Self, Self::Error> {
        let action_name = kdl_name!(kdl_action);
        let action_arguments: Vec<&KdlEntry> = kdl_argument_values!(kdl_action);
        let action_children: Vec<&KdlDocument> = kdl_children!(kdl_action);
        match action_name {
            "Quit" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "FocusNextPane" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "FocusPreviousPane" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "FocusLastPane" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "FocusHostSession" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "FocusGuestSession" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "ToggleHostFullscreen" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "SwitchFocus" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "EditScrollback" => {
                let ansi = crate::kdl_get_bool_property_or_child_value!(kdl_action, "ansi")
                    .unwrap_or(false);
                Ok(Action::EditScrollback { ansi })
            },
            "ScrollUp" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "SwapTextSelectionEnds" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "ScrollDown" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "ScrollToBottom" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "ScrollToTop" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "ScrollToPreviousPrompt" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "ScrollToNextPrompt" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "SelectCommandAtScrollPosition" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "CopyLastCommandOutput" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "PageScrollUp" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "PageScrollDown" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "HalfPageScrollUp" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "HalfPageScrollDown" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "ToggleFocusFullscreen" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "ToggleFocusNoUiFullscreen" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "TogglePaneFrames" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "ToggleActiveSyncTab" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "TogglePaneEmbedOrFloating" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "ToggleFloatingPanes" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "ShowFloatingPanes" => {
                let tab_id = action_arguments
                    .first()
                    .and_then(|v| v.value().as_i64())
                    .map(|n| n as usize);
                Ok(Action::ShowFloatingPanes { tab_id })
            },
            "HideFloatingPanes" => {
                let tab_id = action_arguments
                    .first()
                    .and_then(|v| v.value().as_i64())
                    .map(|n| n as usize);
                Ok(Action::HideFloatingPanes { tab_id })
            },
            "CloseFocus" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "UndoRenamePane" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "NoOp" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "GoToNextTab" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "GoToPreviousTab" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "CloseTab" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "ToggleTab" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "UndoRenameTab" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "ToggleMouseMode" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "Detach" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "SetDarkTheme" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "SetLightTheme" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "ToggleTheme" => {
                parse_kdl_action_arguments!(action_name, action_arguments, kdl_action)
            },
            "SwitchSession" => {
                let name = kdl_get_string_property_or_child_value!(kdl_action, "name")
                    .map(|s| s.to_string())
                    .ok_or(ConfigError::new_kdl_error(
                        "SwitchSession action requires a 'name' property".into(),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    ))?;
                let tab_position =
                    crate::kdl_get_int_property_or_child_value!(kdl_action, "tab_position")
                        .map(|i| i as usize);
                let pane_id = crate::kdl_get_int_property_or_child_value!(kdl_action, "pane_id")
                    .map(|i| i as u32);
                let is_plugin =
                    crate::kdl_get_bool_property_or_child_value!(kdl_action, "is_plugin")
                        .unwrap_or(false);
                let pane_id_tuple = pane_id.map(|id| (id, is_plugin));

                // Parse layout
                let layout = if let Some(layout_str) =
                    kdl_get_string_property_or_child_value!(kdl_action, "layout")
                {
                    let layout_path = PathBuf::from(layout_str);
                    let layout_dir = config_options
                        .layout_dir
                        .clone()
                        .or_else(|| get_layout_dir(find_default_config_dir()));
                    LayoutInfo::from_config(&layout_dir, &Some(layout_path))
                } else {
                    None
                };

                // Parse cwd
                let cwd =
                    kdl_get_string_property_or_child_value!(kdl_action, "cwd").map(PathBuf::from);

                Ok(Action::SwitchSession {
                    name,
                    tab_position,
                    pane_id: pane_id_tuple,
                    layout,
                    cwd,
                })
            },
            "Copy" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "Clear" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "Confirm" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "Deny" => parse_kdl_action_arguments!(action_name, action_arguments, kdl_action),
            "Write" => parse_kdl_action_u8_arguments!(action_name, action_arguments, kdl_action),
            "WriteChars" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "SwitchToMode" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "SetPaneFrameStyle" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "Search" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "Resize" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "ResizeNew" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "MoveFocus" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "MoveTextSelection" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "MoveTab" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "MoveFocusOrTab" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "MovePane" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "MovePaneBackwards" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "DumpScreen" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "DumpLayout" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "NewPane" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "PaneNameInput" => {
                parse_kdl_action_u8_arguments!(action_name, action_arguments, kdl_action)
            },
            "NewTab" => {
                let command_metadata = action_children.first();
                if command_metadata.is_none() {
                    return Ok(Action::NewTab {
                        layout_source: None,
                        tiled_layout: None,
                        floating_layouts: vec![],
                        swap_tiled_layouts: None,
                        swap_floating_layouts: None,
                        tab_name: None,
                        should_change_focus_to_new_tab: true,
                        cwd: None,
                        initial_panes: None,
                        first_pane_unblock_condition: None,
                    });
                }

                let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

                // The reference is kept as written so that saving the configuration
                // writes it back unchanged; only loading resolves a relative path.
                let layout_source = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "layout"))
                    .map(PathBuf::from);
                let layout = layout_source
                    .clone()
                    .map(|path| {
                        if path.extension().is_some() || path.components().count() > 1 {
                            current_dir.join(path)
                        } else {
                            path
                        }
                    })
                    .or_else(|| config_options.default_layout.clone());
                let focus_override = command_metadata.and_then(|c_m| {
                    kdl_child_bool_value_for_entry(c_m, "should_change_focus_to_new_tab")
                });
                let cwd = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "cwd"))
                    .map(PathBuf::from)
                    .map(|cwd| current_dir.join(cwd));
                let name = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "name"))
                    .map(|name_string| name_string.to_string());

                let layout_dir = config_options
                    .layout_dir
                    .clone()
                    .or_else(|| get_layout_dir(find_default_config_dir()));
                let (path_to_raw_layout, raw_layout, swap_layouts) =
                    Layout::stringified_from_path_or_default(layout.as_ref(), layout_dir).map_err(
                        |e| {
                            ConfigError::new_kdl_error(
                                format!("Failed to load layout: {}", e),
                                kdl_action.span().offset(),
                                kdl_action.span().len(),
                            )
                        },
                    )?;

                let layout = Layout::from_str(
                    &raw_layout,
                    path_to_raw_layout,
                    swap_layouts.as_ref().map(|(f, p)| (f.as_str(), p.as_str())),
                    cwd.clone(),
                )
                .map_err(|e| {
                    ConfigError::new_kdl_error(
                        format!("Failed to load layout: {}", e),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    )
                })?;

                let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
                let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());

                let mut tabs = layout.tabs();
                #[allow(clippy::needless_return)]
                if tabs.len() > 1 {
                    return Err(ConfigError::new_kdl_error(
                        "Tab layout cannot itself have tabs".to_string(),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    ));
                } else if !tabs.is_empty() {
                    let (tab_name, layout, floating_panes_layout) = tabs.drain(..).next().unwrap();
                    let name = tab_name.or(name);
                    let should_change_focus_to_new_tab =
                        focus_override.or(layout.focus).unwrap_or(true);

                    Ok(Action::NewTab {
                        layout_source,
                        tiled_layout: Some(layout),
                        floating_layouts: floating_panes_layout,
                        swap_tiled_layouts,
                        swap_floating_layouts,
                        tab_name: name,
                        should_change_focus_to_new_tab,
                        cwd,
                        initial_panes: None,
                        first_pane_unblock_condition: None,
                    })
                } else {
                    let (layout, floating_panes_layout) = layout.new_tab();
                    let should_change_focus_to_new_tab =
                        focus_override.or(layout.focus).unwrap_or(true);

                    Ok(Action::NewTab {
                        layout_source,
                        tiled_layout: Some(layout),
                        floating_layouts: floating_panes_layout,
                        swap_tiled_layouts,
                        swap_floating_layouts,
                        tab_name: name,
                        should_change_focus_to_new_tab,
                        cwd,
                        initial_panes: None,
                        first_pane_unblock_condition: None,
                    })
                }
            },
            "OverrideLayout" => {
                let command_metadata = action_children.first();
                if command_metadata.is_none() {
                    return Ok(Action::OverrideLayout {
                        tabs: vec![],
                        retain_existing_terminal_panes: false,
                        retain_existing_plugin_panes: false,
                        apply_only_to_active_tab: false,
                    });
                }

                let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

                let layout = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "layout"))
                    .map(PathBuf::from)
                    .or_else(|| config_options.default_layout.clone());
                let cwd = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "cwd"))
                    .map(PathBuf::from)
                    .map(|cwd| current_dir.join(cwd));
                let name = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "name"))
                    .map(|name_string| name_string.to_string());
                let retain_existing_terminal_panes = command_metadata
                    .and_then(|c_m| {
                        kdl_child_bool_value_for_entry(c_m, "retain_existing_terminal_panes")
                    })
                    .unwrap_or(false);
                let retain_existing_plugin_panes = command_metadata
                    .and_then(|c_m| {
                        kdl_child_bool_value_for_entry(c_m, "retain_existing_plugin_panes")
                    })
                    .unwrap_or(false);
                let apply_only_to_active_tab = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "apply_only_to_active_tab"))
                    .unwrap_or(false);

                let layout_dir = config_options
                    .layout_dir
                    .clone()
                    .or_else(|| get_layout_dir(find_default_config_dir()));
                let (path_to_raw_layout, raw_layout, swap_layouts) =
                    Layout::stringified_from_path_or_default(layout.as_ref(), layout_dir).map_err(
                        |e| {
                            ConfigError::new_kdl_error(
                                format!("Failed to load layout: {}", e),
                                kdl_action.span().offset(),
                                kdl_action.span().len(),
                            )
                        },
                    )?;

                let layout = Layout::from_str(
                    &raw_layout,
                    path_to_raw_layout,
                    swap_layouts.as_ref().map(|(f, p)| (f.as_str(), p.as_str())),
                    cwd.clone(),
                )
                .map_err(|e| {
                    ConfigError::new_kdl_error(
                        format!("Failed to load layout: {}", e),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    )
                })?;

                let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
                let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());

                let mut tabs = layout.tabs();
                #[allow(clippy::needless_return)]
                if tabs.len() > 1 {
                    return Err(ConfigError::new_kdl_error(
                        "Tab layout cannot itself have tabs".to_string(),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    ));
                } else if !tabs.is_empty() {
                    let (tab_name, layout, floating_panes_layout) = tabs.drain(..).next().unwrap();
                    let name = tab_name.or(name);

                    let tab_layout_info = TabLayoutInfo {
                        tab_index: 0,
                        tab_name: name,
                        tiled_layout: layout,
                        floating_layouts: floating_panes_layout,
                        swap_tiled_layouts,
                        swap_floating_layouts,
                    };

                    Ok(Action::OverrideLayout {
                        tabs: vec![tab_layout_info],
                        retain_existing_terminal_panes,
                        retain_existing_plugin_panes,
                        apply_only_to_active_tab,
                    })
                } else {
                    let (layout, floating_panes_layout) = layout.new_tab();

                    let tab_layout_info = TabLayoutInfo {
                        tab_index: 0,
                        tab_name: name,
                        tiled_layout: layout,
                        floating_layouts: floating_panes_layout,
                        swap_tiled_layouts,
                        swap_floating_layouts,
                    };

                    Ok(Action::OverrideLayout {
                        tabs: vec![tab_layout_info],
                        retain_existing_terminal_panes,
                        retain_existing_plugin_panes,
                        apply_only_to_active_tab,
                    })
                }
            },
            "GoToTab" => parse_kdl_action_u8_arguments!(action_name, action_arguments, kdl_action),
            "TabNameInput" => {
                parse_kdl_action_u8_arguments!(action_name, action_arguments, kdl_action)
            },
            "SearchInput" => {
                parse_kdl_action_u8_arguments!(action_name, action_arguments, kdl_action)
            },
            "SearchToggleOption" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "Run" => {
                let arguments = action_arguments.iter().copied();
                let mut args = kdl_arguments_that_are_strings(arguments)?;
                if args.is_empty() {
                    return Err(ConfigError::new_kdl_error(
                        "No command found in Run action".into(),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    ));
                }
                let command = args.remove(0);
                let command_metadata = action_children.first();
                let cwd = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "cwd"))
                    .map(PathBuf::from);
                let name = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "name"))
                    .map(|name_string| name_string.to_string());
                let direction = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "direction"))
                    .and_then(|direction_string| Direction::from_str(direction_string).ok());
                let hold_on_close = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "close_on_exit"))
                    .map(|close_on_exit| !close_on_exit)
                    .unwrap_or(true);
                let hold_on_start = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "start_suspended"))
                    .unwrap_or(false);
                let floating = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "floating"))
                    .unwrap_or(false);
                let in_place = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "in_place"))
                    .unwrap_or(false);
                let close_replaced_pane = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "close_replaced_pane"))
                    .unwrap_or(false);
                let stacked = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "stacked"))
                    .unwrap_or(false);
                let firecracker_vm = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "firecracker_vm"));
                let firecracker_image = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "firecracker_image"));
                let firecracker_worktree = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "firecracker_worktree"))
                    .map(zellij_runtime::shared::expand_path)
                    .transpose()
                    .map_err(|error| {
                        ConfigError::new_kdl_error(
                            format!("firecracker_worktree: {error}"),
                            kdl_action.span().offset(),
                            kdl_action.span().len(),
                        )
                    })?;
                let target = PaneTarget::from_parts(
                    None,
                    firecracker_vm,
                    firecracker_image,
                    firecracker_worktree.as_deref(),
                )
                .map_err(|error| {
                    ConfigError::new_kdl_error(
                        error,
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    )
                })?;
                let run_command_action = RunCommandAction {
                    command: PathBuf::from(command),
                    args,
                    cwd,
                    direction,
                    hold_on_close,
                    hold_on_start,
                    target,
                    ..Default::default()
                };
                let x = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "x"))
                    .map(|s| s.to_owned());
                let y = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "y"))
                    .map(|s| s.to_owned());
                let width = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "width"))
                    .map(|s| s.to_owned());
                let height = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "height"))
                    .map(|s| s.to_owned());
                let pinned =
                    command_metadata.and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "pinned"));
                let borderless = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "borderless"));
                if floating {
                    Ok(Action::NewFloatingPane {
                        command: Some(run_command_action),
                        pane_name: name,
                        placement: NewPanePlacement::Floating(FloatingPaneCoordinates::new(
                            x, y, width, height, pinned, borderless,
                        )),
                        near_current_pane: false,
                        no_focus: false,
                        tab_id: None,
                    })
                } else if in_place {
                    Ok(Action::NewInPlacePane {
                        command: Some(run_command_action),
                        pane_name: name,
                        near_current_pane: false,
                        no_focus: false,
                        pane_id_to_replace: None,
                        close_replaced_pane,
                        tab_id: None,
                    })
                } else if stacked {
                    Ok(Action::NewStackedPane {
                        command: Some(run_command_action),
                        pane_name: name,
                        near_current_pane: false,
                        no_focus: false,
                        tab_id: None,
                    })
                } else {
                    Ok(Action::NewTiledPane {
                        direction,
                        command: Some(run_command_action),
                        pane_name: name,
                        near_current_pane: false,
                        no_focus: false,
                        borderless: None,
                        tab_id: None,
                    })
                }
            },
            "LaunchOrFocusPlugin" => {
                let arguments = action_arguments.iter().copied();
                let mut args = kdl_arguments_that_are_strings(arguments)?;
                if args.is_empty() {
                    return Err(ConfigError::new_kdl_error(
                        "No plugin found to launch in LaunchOrFocusPlugin".into(),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    ));
                }
                let plugin_path = args.remove(0);

                let command_metadata = action_children.first();
                let should_float = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "floating"))
                    .unwrap_or(false);
                let is_overlay = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "overlay"))
                    .unwrap_or(false);
                let move_to_focused_tab = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "move_to_focused_tab"))
                    .unwrap_or(false);
                let should_open_in_place = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "in_place"))
                    .unwrap_or(false);
                let close_replaced_pane = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "close_replaced_pane"))
                    .unwrap_or(false);
                let skip_plugin_cache = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "skip_plugin_cache"))
                    .unwrap_or(false);
                let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let mut configuration =
                    KdlLayoutParser::parse_plugin_user_configuration(kdl_action)?;
                if is_overlay {
                    configuration.insert("overlay", "true");
                }
                let initial_cwd =
                    kdl_get_string_property_or_child_value!(kdl_action, "cwd").map(PathBuf::from);
                let run_plugin_or_alias = RunPluginOrAlias::from_url(
                    &plugin_path,
                    &Some(configuration.inner().clone()),
                    None,
                    Some(current_dir),
                )
                .map_err(|e| {
                    ConfigError::new_kdl_error(
                        format!("Failed to parse plugin: {}", e),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    )
                })?
                .with_initial_cwd(initial_cwd);
                Ok(Action::LaunchOrFocusPlugin {
                    plugin: run_plugin_or_alias,
                    should_float,
                    move_to_focused_tab,
                    should_open_in_place,
                    close_replaced_pane,
                    skip_cache: skip_plugin_cache,
                    tab_id: None,
                })
            },
            "LaunchPlugin" => {
                let arguments = action_arguments.iter().copied();
                let mut args = kdl_arguments_that_are_strings(arguments)?;
                if args.is_empty() {
                    return Err(ConfigError::new_kdl_error(
                        "No plugin found to launch in LaunchPlugin".into(),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    ));
                }
                let plugin_path = args.remove(0);

                let command_metadata = action_children.first();
                let should_float = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "floating"))
                    .unwrap_or(false);
                let should_open_in_place = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "in_place"))
                    .unwrap_or(false);
                let close_replaced_pane = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "close_replaced_pane"))
                    .unwrap_or(false);
                let skip_plugin_cache = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "skip_plugin_cache"))
                    .unwrap_or(false);
                let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let configuration = KdlLayoutParser::parse_plugin_user_configuration(kdl_action)?;
                let run_plugin_or_alias = RunPluginOrAlias::from_url(
                    &plugin_path,
                    &Some(configuration.inner().clone()),
                    None,
                    Some(current_dir),
                )
                .map_err(|e| {
                    ConfigError::new_kdl_error(
                        format!("Failed to parse plugin: {}", e),
                        kdl_action.span().offset(),
                        kdl_action.span().len(),
                    )
                })?;
                Ok(Action::LaunchPlugin {
                    plugin: run_plugin_or_alias,
                    should_float,
                    should_open_in_place,
                    close_replaced_pane,
                    skip_cache: skip_plugin_cache,
                    cwd: None, // we explicitly do not send the current dir here so that it will be
                    // filled from the active pane == better UX
                    no_focus: false,
                    tab_id: None,
                })
            },
            "PreviousSwapLayout" => Ok(Action::PreviousSwapLayout),
            "NextSwapLayout" => Ok(Action::NextSwapLayout),
            "BreakPane" => Ok(Action::BreakPane),
            "BreakPaneRight" => Ok(Action::BreakPaneRight),
            "BreakPaneLeft" => Ok(Action::BreakPaneLeft),
            "RenameSession" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "Notify" => parse_kdl_action_char_or_string_arguments!(
                action_name,
                action_arguments,
                kdl_action
            ),
            "MessagePlugin" => {
                let arguments = action_arguments.iter().copied();
                let mut args = kdl_arguments_that_are_strings(arguments)?;
                let plugin_path = if args.is_empty() {
                    None
                } else {
                    Some(args.remove(0))
                };

                let command_metadata = action_children.first();
                let launch_new = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "launch_new"))
                    .unwrap_or(false);
                let skip_cache = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "skip_cache"))
                    .unwrap_or(false);
                let should_float = command_metadata
                    .and_then(|c_m| kdl_child_bool_value_for_entry(c_m, "floating"))
                    .unwrap_or(false);
                let name = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "name"))
                    .map(|n| n.to_owned());
                let payload = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "payload"))
                    .map(|p| p.to_owned());
                let title = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "title"))
                    .map(|t| t.to_owned());
                let configuration = KdlLayoutParser::parse_plugin_user_configuration(kdl_action)?;
                let configuration = if configuration.inner().is_empty() {
                    None
                } else {
                    Some(configuration.inner().clone())
                };
                let cwd =
                    kdl_get_string_property_or_child_value!(kdl_action, "cwd").map(PathBuf::from);

                let name = name
                    // first we try to take the explicitly supplied message name
                    // then we use the plugin, to facilitate using aliases
                    .or_else(|| plugin_path.clone())
                    // then we use a uuid to at least have some sort of identifier for this message
                    .or_else(|| Some(Uuid::new_v4().to_string()));

                Ok(Action::KeybindPipe {
                    name,
                    payload,
                    args: None, // TODO: consider supporting this if there's a need
                    plugin: plugin_path,
                    configuration,
                    launch_new,
                    skip_cache,
                    floating: Some(should_float),
                    in_place: None, // TODO: support this
                    cwd,
                    pane_title: title,
                    plugin_id: None,
                })
            },
            "MessagePluginId" => {
                let arguments = action_arguments.iter().copied();
                let mut args = kdl_arguments_that_are_digits(arguments)?;
                let plugin_id = if args.is_empty() {
                    None
                } else {
                    Some(args.remove(0) as u32)
                };

                let command_metadata = action_children.first();
                let launch_new = false;
                let skip_cache = false;
                let name = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "name"))
                    .map(|n| n.to_owned());
                let payload = command_metadata
                    .and_then(|c_m| kdl_child_string_value_for_entry(c_m, "payload"))
                    .map(|p| p.to_owned());
                let configuration = None;

                let name = name
                    // if no name is provided, we use a uuid to at least have some sort of identifier for this message
                    .or_else(|| Some(Uuid::new_v4().to_string()));

                Ok(Action::KeybindPipe {
                    name,
                    payload,
                    args: None, // TODO: consider supporting this if there's a need
                    plugin: None,
                    configuration,
                    launch_new,
                    skip_cache,
                    floating: None,
                    in_place: None, // TODO: support this
                    cwd: None,
                    pane_title: None,
                    plugin_id,
                })
            },
            "TogglePanePinned" => Ok(Action::TogglePanePinned),
            "TogglePaneInGroup" => Ok(Action::TogglePaneInGroup),
            "ToggleGroupMarking" => Ok(Action::ToggleGroupMarking),
            _ => Err(ConfigError::new_kdl_error(
                format!("Unsupported action: {}", action_name),
                kdl_action.span().offset(),
                kdl_action.span().len(),
            )),
        }
    }
}

#[macro_export]
macro_rules! kdl_property_first_arg_as_string {
    ( $kdl_node:expr, $property_name:expr ) => {
        $kdl_node
            .get($property_name)
            .and_then(|p| p.entries().iter().next())
            .and_then(|p| p.value().as_string())
    };
}

#[macro_export]
macro_rules! kdl_property_first_arg_as_string_or_error {
    ( $kdl_node:expr, $property_name:expr ) => {{
        match $kdl_node.get($property_name) {
            Some(property) => match property.entries().iter().next() {
                Some(first_entry) => match first_entry.value().as_string() {
                    Some(string_entry) => Some((string_entry, first_entry)),
                    None => {
                        return Err(ConfigError::new_kdl_error(
                            format!(
                                "Property {} must be a string, found: {}",
                                $property_name,
                                first_entry.value()
                            ),
                            property.span().offset(),
                            property.span().len(),
                        ));
                    },
                },
                None => {
                    return Err(ConfigError::new_kdl_error(
                        format!("Property {} must have a value", $property_name),
                        property.span().offset(),
                        property.span().len(),
                    ));
                },
            },
            None => None,
        }
    }};
}

#[macro_export]
macro_rules! kdl_property_first_arg_as_bool_or_error {
    ( $kdl_node:expr, $property_name:expr ) => {{
        match $kdl_node.get($property_name) {
            Some(property) => match property.entries().iter().next() {
                Some(first_entry) => match first_entry.value().as_bool() {
                    Some(bool_entry) => Some((bool_entry, first_entry)),
                    None => {
                        return Err(ConfigError::new_kdl_error(
                            format!(
                                "Property {} must be true or false, found {}",
                                $property_name,
                                first_entry.value()
                            ),
                            property.span().offset(),
                            property.span().len(),
                        ));
                    },
                },
                None => {
                    return Err(ConfigError::new_kdl_error(
                        format!("Property {} must have a value", $property_name),
                        property.span().offset(),
                        property.span().len(),
                    ));
                },
            },
            None => None,
        }
    }};
}

#[macro_export]
macro_rules! kdl_property_first_arg_as_i64_or_error {
    ( $kdl_node:expr, $property_name:expr ) => {{
        match $kdl_node.get($property_name) {
            Some(property) => match property.entries().iter().next() {
                Some(first_entry) => match first_entry.value().as_i64() {
                    Some(int_entry) => Some((int_entry, first_entry)),
                    None => {
                        return Err(ConfigError::new_kdl_error(
                            format!(
                                "Property {} must be numeric, found {}",
                                $property_name,
                                first_entry.value()
                            ),
                            property.span().offset(),
                            property.span().len(),
                        ));
                    },
                },
                None => {
                    return Err(ConfigError::new_kdl_error(
                        format!("Property {} must have a value", $property_name),
                        property.span().offset(),
                        property.span().len(),
                    ));
                },
            },
            None => None,
        }
    }};
}

#[macro_export]
macro_rules! kdl_has_string_argument {
    ( $kdl_node:expr, $string_argument:expr ) => {
        $kdl_node
            .entries()
            .iter()
            .find(|e| e.value().as_string() == Some($string_argument))
            .is_some()
    };
}

#[macro_export]
macro_rules! kdl_children_property_first_arg_as_string {
    ( $kdl_node:expr, $property_name:expr ) => {
        $kdl_node
            .children()
            .and_then(|c| c.get($property_name))
            .and_then(|p| p.entries().iter().next())
            .and_then(|p| p.value().as_string())
    };
}

#[macro_export]
macro_rules! kdl_property_first_arg_as_bool {
    ( $kdl_node:expr, $property_name:expr ) => {
        $kdl_node
            .get($property_name)
            .and_then(|p| p.entries().iter().next())
            .and_then(|p| p.value().as_bool())
    };
}

#[macro_export]
macro_rules! kdl_children_property_first_arg_as_bool {
    ( $kdl_node:expr, $property_name:expr ) => {
        $kdl_node
            .children()
            .and_then(|c| c.get($property_name))
            .and_then(|p| p.entries().iter().next())
            .and_then(|p| p.value().as_bool())
    };
}

#[macro_export]
macro_rules! kdl_property_first_arg_as_i64 {
    ( $kdl_node:expr, $property_name:expr ) => {
        $kdl_node
            .get($property_name)
            .and_then(|p| p.entries().iter().next())
            .and_then(|p| p.value().as_i64())
    };
}

#[macro_export]
macro_rules! kdl_get_child {
    ( $kdl_node:expr, $child_name:expr ) => {
        $kdl_node.children().and_then(|c| c.get($child_name))
    };
}

#[macro_export]
macro_rules! kdl_get_child_entry_bool_value {
    ( $kdl_node:expr, $child_name:expr ) => {
        $kdl_node
            .children()
            .and_then(|c| c.get($child_name))
            .and_then(|c| c.get(0))
            .and_then(|c| c.value().as_bool())
    };
}

#[macro_export]
macro_rules! kdl_get_child_entry_string_value {
    ( $kdl_node:expr, $child_name:expr ) => {
        $kdl_node
            .children()
            .and_then(|c| c.get($child_name))
            .and_then(|c| c.get(0))
            .and_then(|c| c.value().as_string())
    };
}

#[macro_export]
macro_rules! kdl_get_bool_property_or_child_value {
    ( $kdl_node:expr, $name:expr ) => {
        $kdl_node
            .get($name)
            .and_then(|e| e.value().as_bool())
            .or_else(|| {
                $kdl_node
                    .children()
                    .and_then(|c| c.get($name))
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.value().as_bool())
            })
    };
}

#[macro_export]
macro_rules! kdl_get_bool_property_or_child_value_with_error {
    ( $kdl_node:expr, $name:expr ) => {
        match $kdl_node.get($name) {
            Some(e) => match e.value().as_bool() {
                Some(bool_value) => Some(bool_value),
                None => {
                    return Err(kdl_parsing_error!(
                        format!(
                            "{} should be either true or false, found {}",
                            $name,
                            e.value()
                        ),
                        e
                    ))
                },
            },
            None => {
                let child_value = $kdl_node
                    .children()
                    .and_then(|c| c.get($name))
                    .and_then(|c| c.get(0));
                match child_value {
                    Some(e) => match e.value().as_bool() {
                        Some(bool_value) => Some(bool_value),
                        None => {
                            return Err(kdl_parsing_error!(
                                format!(
                                    "{} should be either true or false, found {}",
                                    $name,
                                    e.value()
                                ),
                                e
                            ))
                        },
                    },
                    None => {
                        if let Some(child_node) = kdl_child_with_name!($kdl_node, $name) {
                            return Err(kdl_parsing_error!(
                                format!(
                                    "{} must have a value, eg. '{} true'",
                                    child_node.name().value(),
                                    child_node.name().value()
                                ),
                                child_node
                            ));
                        }
                        None
                    },
                }
            },
        }
    };
}

#[macro_export]
macro_rules! kdl_property_or_child_value_node {
    ( $kdl_node:expr, $name:expr ) => {
        $kdl_node.get($name).or_else(|| {
            $kdl_node
                .children()
                .and_then(|c| c.get($name))
                .and_then(|c| c.get(0))
        })
    };
}

#[macro_export]
macro_rules! kdl_child_with_name {
    ( $kdl_node:expr, $name:expr ) => {{
        $kdl_node
            .children()
            .and_then(|children| children.nodes().iter().find(|c| c.name().value() == $name))
    }};
}

#[macro_export]
macro_rules! kdl_child_with_name_or_error {
    ( $kdl_node:expr, $name:expr) => {{
        $kdl_node
            .children()
            .and_then(|children| children.nodes().iter().find(|c| c.name().value() == $name))
            .ok_or(ConfigError::new_kdl_error(
                format!("Missing node {}", $name).into(),
                $kdl_node.span().offset(),
                $kdl_node.span().len(),
            ))
    }};
}

#[macro_export]
macro_rules! kdl_get_string_property_or_child_value_with_error {
    ( $kdl_node:expr, $name:expr ) => {
        match $kdl_node.get($name) {
            Some(e) => match e.value().as_string() {
                Some(string_value) => Some(string_value),
                None => {
                    return Err(kdl_parsing_error!(
                        format!(
                            "{} should be a string, found {} - not a string",
                            $name,
                            e.value()
                        ),
                        e
                    ))
                },
            },
            None => {
                let child_value = $kdl_node
                    .children()
                    .and_then(|c| c.get($name))
                    .and_then(|c| c.get(0));
                match child_value {
                    Some(e) => match e.value().as_string() {
                        Some(string_value) => Some(string_value),
                        None => {
                            return Err(kdl_parsing_error!(
                                format!(
                                    "{} should be a string, found {} - not a string",
                                    $name,
                                    e.value()
                                ),
                                e
                            ))
                        },
                    },
                    None => {
                        if let Some(child_node) = kdl_child_with_name!($kdl_node, $name) {
                            return Err(kdl_parsing_error!(
                                format!(
                                    "{} must have a value, eg. '{} \"foo\"'",
                                    child_node.name().value(),
                                    child_node.name().value()
                                ),
                                child_node
                            ));
                        }
                        None
                    },
                }
            },
        }
    };
}

#[macro_export]
macro_rules! kdl_get_property_or_child {
    ( $kdl_node:expr, $name:expr ) => {
        $kdl_node.get($name).or_else(|| {
            $kdl_node
                .children()
                .and_then(|c| c.get($name))
                .and_then(|c| c.get(0))
        })
    };
}

#[macro_export]
macro_rules! kdl_get_int_property_or_child_value {
    ( $kdl_node:expr, $name:expr ) => {
        $kdl_node
            .get($name)
            .and_then(|e| e.value().as_i64())
            .or_else(|| {
                $kdl_node
                    .children()
                    .and_then(|c| c.get($name))
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.value().as_i64())
            })
    };
}

#[macro_export]
macro_rules! kdl_get_string_entry {
    ( $kdl_node:expr, $entry_name:expr ) => {
        $kdl_node
            .get($entry_name)
            .and_then(|e| e.value().as_string())
    };
}

#[macro_export]
macro_rules! kdl_get_int_entry {
    ( $kdl_node:expr, $entry_name:expr ) => {
        $kdl_node.get($entry_name).and_then(|e| e.value().as_i64())
    };
}

impl Options {
    #[allow(clippy::result_large_err)]
    pub fn from_kdl(kdl_options: &KdlDocument) -> Result<Self, ConfigError> {
        let on_force_close =
            match kdl_property_first_arg_as_string_or_error!(kdl_options, "on_force_close") {
                Some((string, entry)) => Some(OnForceClose::from_str(string).map_err(|_| {
                    kdl_parsing_error!(
                        format!("Invalid value for on_force_close: '{}'", string),
                        entry
                    )
                })?),
                None => None,
            };
        let simplified_ui =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "simplified_ui").map(|(v, _)| v);
        let default_shell =
            kdl_property_first_arg_as_string_or_error!(kdl_options, "default_shell")
                .map(|(string, _entry)| PathBuf::from(string));
        let default_cwd = kdl_property_first_arg_as_string_or_error!(kdl_options, "default_cwd")
            .map(|(string, _entry)| PathBuf::from(string));
        let pane_frames =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "pane_frames").map(|(v, _)| v);
        let pane_frame_style =
            match kdl_property_first_arg_as_string_or_error!(kdl_options, "pane_frame_style") {
                Some((string, entry)) => Some(PaneFrameStyle::from_str(string).map_err(|_| {
                    kdl_parsing_error!(
                        format!("Invalid value for pane_frame_style: '{}'", string),
                        entry
                    )
                })?),
                None => None,
            };
        let notification_position = match kdl_property_first_arg_as_string_or_error!(
            kdl_options,
            "notification_position"
        ) {
            Some((string, entry)) => {
                Some(NotificationPosition::from_str(string).map_err(|_| {
                    kdl_parsing_error!(
                        format!("Invalid value for notification_position: '{}'", string),
                        entry
                    )
                })?)
            },
            None => None,
        };
        let notification_timeout =
            kdl_property_first_arg_as_i64_or_error!(kdl_options, "notification_timeout")
                .map(|(v, _)| v as u64);
        let auto_layout =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "auto_layout").map(|(v, _)| v);
        let theme = kdl_property_first_arg_as_string_or_error!(kdl_options, "theme")
            .map(|(theme, _entry)| theme.to_string());
        let theme_dark = kdl_property_first_arg_as_string_or_error!(kdl_options, "theme_dark")
            .map(|(theme, _entry)| theme.to_string());
        let theme_light = kdl_property_first_arg_as_string_or_error!(kdl_options, "theme_light")
            .map(|(theme, _entry)| theme.to_string());
        let explicit_theme_hue =
            match kdl_property_first_arg_as_string_or_error!(kdl_options, "explicit_theme_hue") {
                Some((string, entry)) => Some(ThemeHue::from_str(string).map_err(|_| {
                    kdl_parsing_error!(
                        format!("Invalid value for explicit_theme_hue: '{}'", string),
                        entry
                    )
                })?),
                None => None,
            };
        let default_mode =
            match kdl_property_first_arg_as_string_or_error!(kdl_options, "default_mode") {
                Some((string, entry)) => Some(InputMode::from_str(string).map_err(|_| {
                    kdl_parsing_error!(format!("Invalid input mode: '{}'", string), entry)
                })?),
                None => None,
            };
        let default_layout =
            kdl_property_first_arg_as_string_or_error!(kdl_options, "default_layout")
                .map(|(string, _entry)| PathBuf::from(string));
        let layout_dir = kdl_property_first_arg_as_string_or_error!(kdl_options, "layout_dir")
            .map(|(string, _entry)| PathBuf::from(string));
        let builtin_plugins_dir =
            kdl_property_first_arg_as_string_or_error!(kdl_options, "builtin_plugins_dir")
                .map(|(string, _entry)| PathBuf::from(string));
        let theme_dir = kdl_property_first_arg_as_string_or_error!(kdl_options, "theme_dir")
            .map(|(string, _entry)| PathBuf::from(string));
        let user_plugin_dir =
            kdl_property_first_arg_as_string_or_error!(kdl_options, "user_plugin_dir")
                .map(|(string, _entry)| PathBuf::from(string));
        let mouse_mode =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "mouse_mode").map(|(v, _)| v);
        let detachable_panes =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "detachable_panes")
                .map(|(v, _)| v);
        let firecracker_provider =
            match kdl_property_first_arg_as_string_or_error!(kdl_options, "firecracker_provider") {
                Some((value, entry)) => {
                    let path = zellij_runtime::shared::expand_path(value).map_err(|error| {
                        kdl_parsing_error!(format!("firecracker_provider: {error}"), entry)
                    })?;
                    if !path.is_absolute() {
                        return Err(kdl_parsing_error!(
                            format!("firecracker_provider must be an absolute path, got {value:?}"),
                            entry
                        ));
                    }
                    Some(path)
                },
                None => None,
            };
        let firecracker_provider_timeout_secs = match kdl_property_first_arg_as_i64_or_error!(
            kdl_options,
            "firecracker_provider_timeout_secs"
        ) {
            Some((value, _)) if value > 0 => Some(value as u64),
            Some((value, entry)) => {
                return Err(kdl_parsing_error!(
                    format!(
                        "firecracker_provider_timeout_secs must be greater than zero, got {value}"
                    ),
                    entry
                ))
            },
            None => None,
        };
        let scroll_buffer_size =
            kdl_property_first_arg_as_i64_or_error!(kdl_options, "scroll_buffer_size")
                .map(|(scroll_buffer_size, _entry)| scroll_buffer_size as usize);
        let locked_escape_timeout_ms = match kdl_property_first_arg_as_i64_or_error!(
            kdl_options,
            "locked_escape_timeout_ms"
        ) {
            Some((value, _)) if value >= 0 => Some(value as u64),
            Some((value, entry)) => {
                return Err(kdl_parsing_error!(
                    format!("locked_escape_timeout_ms cannot be negative, got {value}"),
                    entry
                ))
            },
            None => None,
        };
        let copy_command = kdl_property_first_arg_as_string_or_error!(kdl_options, "copy_command")
            .map(|(copy_command, _entry)| copy_command.to_string());
        let copy_clipboard =
            match kdl_property_first_arg_as_string_or_error!(kdl_options, "copy_clipboard") {
                Some((string, entry)) => Some(Clipboard::from_str(string).map_err(|_| {
                    kdl_parsing_error!(
                        format!("Invalid value for copy_clipboard: '{}'", string),
                        entry
                    )
                })?),
                None => None,
            };
        let copy_on_select =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "copy_on_select").map(|(v, _)| v);
        let osc8_hyperlinks =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "osc8_hyperlinks")
                .map(|(v, _)| v);
        let scrollback_editor =
            kdl_property_first_arg_as_string_or_error!(kdl_options, "scrollback_editor")
                .map(|(string, _entry)| PathBuf::from(string));
        let mirror_session =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "mirror_session").map(|(v, _)| v);
        let session_name = kdl_property_first_arg_as_string_or_error!(kdl_options, "session_name")
            .map(|(session_name, _entry)| session_name.to_string());
        let attach_to_session =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "attach_to_session")
                .map(|(v, _)| v);
        let session_serialization =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "session_serialization")
                .map(|(v, _)| v);
        let session_recording =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "session_recording")
                .map(|(v, _)| v);
        let plugin_data_url =
            match kdl_property_first_arg_as_string_or_error!(kdl_options, "plugin_data_url") {
                Some((value, entry)) => {
                    // The scheme is all that can be checked here: `kdl` is compiled for plugins
                    // too, and the Postgres client that parses the rest is host-only.
                    if !(value.starts_with("postgres://") || value.starts_with("postgresql://")) {
                        return Err(kdl_parsing_error!(
                            format!(
                                "plugin_data_url must be a postgres:// or postgresql:// URL, \
                                 got {value:?}; leave it unset for the SQLite file in the data dir"
                            ),
                            entry
                        ));
                    }
                    Some(value.to_owned())
                },
                None => None,
            };
        let plugin_data_schema =
            match kdl_property_first_arg_as_string_or_error!(kdl_options, "plugin_data_schema") {
                Some((value, entry)) => {
                    crate::plugin_data::validate_name(
                        value,
                        crate::plugin_data::NameKind::Namespace,
                    )
                    .map_err(|error| {
                        kdl_parsing_error!(format!("plugin_data_schema: {error}"), entry)
                    })?;
                    Some(value.to_owned())
                },
                None => None,
            };
        let host_id = match kdl_property_first_arg_as_string_or_error!(kdl_options, "host_id") {
            Some((value, entry)) => {
                if !crate::host_identity::is_valid_host_id(value) {
                    return Err(kdl_parsing_error!(
                        format!(
                            "host_id must be 1-{} characters of letters, digits, `_` or `-`",
                            crate::host_identity::MAX_HOST_ID_LEN
                        ),
                        entry
                    ));
                }
                Some(value.to_owned())
            },
            None => None,
        };
        let session_recording_input =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "session_recording_input")
                .map(|(v, _)| v);
        let serialize_pane_viewport =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "serialize_pane_viewport")
                .map(|(v, _)| v);
        let scrollback_lines_to_serialize =
            kdl_property_first_arg_as_i64_or_error!(kdl_options, "scrollback_lines_to_serialize")
                .map(|(v, _)| v as usize);
        let styled_underlines =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "styled_underlines")
                .map(|(v, _)| v);
        let serialization_interval =
            kdl_property_first_arg_as_i64_or_error!(kdl_options, "serialization_interval")
                .map(|(scroll_buffer_size, _entry)| scroll_buffer_size as u64);
        let disable_session_metadata =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "disable_session_metadata")
                .map(|(v, _)| v);
        let support_kitty_keyboard_protocol = kdl_property_first_arg_as_bool_or_error!(
            kdl_options,
            "support_kitty_keyboard_protocol"
        )
        .map(|(v, _)| v);
        let report_modifier_keys =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "report_modifier_keys")
                .map(|(v, _)| v);
        let support_kitty_graphics_protocol = kdl_property_first_arg_as_bool_or_error!(
            kdl_options,
            "support_kitty_graphics_protocol"
        )
        .map(|(v, _)| v);
        let stacked_resize =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "stacked_resize").map(|(v, _)| v);
        let agent_pane_min_columns =
            match kdl_property_first_arg_as_i64_or_error!(kdl_options, "agent_pane_min_columns") {
                Some((value, _)) if value >= 0 => Some(value as usize),
                Some((value, entry)) => {
                    return Err(kdl_parsing_error!(
                        format!("agent_pane_min_columns cannot be negative, got {value}"),
                        entry
                    ))
                },
                None => None,
            };
        let agent_pane_min_rows =
            match kdl_property_first_arg_as_i64_or_error!(kdl_options, "agent_pane_min_rows") {
                Some((value, _)) if value >= 0 => Some(value as usize),
                Some((value, entry)) => {
                    return Err(kdl_parsing_error!(
                        format!("agent_pane_min_rows cannot be negative, got {value}"),
                        entry
                    ))
                },
                None => None,
            };
        let stacked_pane_list =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "stacked_pane_list")
                .map(|(v, _)| v);
        let show_startup_tips =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "show_startup_tips")
                .map(|(v, _)| v);
        let show_release_notes =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "show_release_notes")
                .map(|(v, _)| v);
        let advanced_mouse_actions =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "advanced_mouse_actions")
                .map(|(v, _)| v);
        let mouse_scroll_resize =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "mouse_scroll_resize")
                .map(|(v, _)| v);
        let mouse_hover_effects =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "mouse_hover_effects")
                .map(|(v, _)| v);
        let post_command_discovery_hook =
            kdl_property_first_arg_as_string_or_error!(kdl_options, "post_command_discovery_hook")
                .map(|(hook, _entry)| hook.to_string());
        let visual_bell =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "visual_bell").map(|(v, _)| v);
        let focus_follows_mouse =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "focus_follows_mouse")
                .map(|(v, _)| v);
        let context_menu_on_right_click =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "context_menu_on_right_click")
                .map(|(v, _)| v);
        let mouse_click_through =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "mouse_click_through")
                .map(|(v, _)| v);
        let osc133_command_selection =
            kdl_property_first_arg_as_bool_or_error!(kdl_options, "osc133_command_selection")
                .map(|(v, _)| v);
        let word_separators =
            kdl_property_first_arg_as_string_or_error!(kdl_options, "word_separators")
                .map(|(separators, _entry)| separators.to_string());
        let nested_session_handling = match kdl_property_first_arg_as_string_or_error!(
            kdl_options,
            "nested_session_handling"
        ) {
            Some((value, entry)) => {
                use crate::input::options::NestedSessionHandling;
                match value.parse::<NestedSessionHandling>() {
                    Ok(v) => Some(v),
                    Err(e) => return Err(kdl_parsing_error!(e, entry)),
                }
            },
            None => None,
        };
        let host_notification_protocol = match kdl_property_first_arg_as_string_or_error!(
            kdl_options,
            "host_notification_protocol"
        ) {
            Some((value, entry)) => {
                use crate::input::options::HostNotificationProtocol;
                match value.parse::<HostNotificationProtocol>() {
                    Ok(v) => Some(v),
                    Err(e) => return Err(kdl_parsing_error!(e, entry)),
                }
            },
            None => None,
        };
        let dangerously_enable_paste_buffer_read = kdl_property_first_arg_as_bool_or_error!(
            kdl_options,
            "dangerously_enable_paste_buffer_read"
        )
        .map(|(v, _)| v);

        Ok(Options {
            simplified_ui,
            theme,
            theme_dark,
            theme_light,
            explicit_theme_hue,
            default_mode,
            default_shell,
            default_cwd,
            default_layout,
            layout_dir,
            builtin_plugins_dir,
            theme_dir,
            user_plugin_dir,
            mouse_mode,
            detachable_panes,
            firecracker_provider,
            firecracker_provider_timeout_secs,
            pane_frames,
            pane_frame_style,
            notification_position,
            notification_timeout,
            mirror_session,
            on_force_close,
            scroll_buffer_size,
            locked_escape_timeout_ms,
            copy_command,
            copy_clipboard,
            copy_on_select,
            osc8_hyperlinks,
            scrollback_editor,
            session_name,
            attach_to_session,
            auto_layout,
            session_serialization,
            session_recording,
            session_recording_input,
            plugin_data_url,
            plugin_data_schema,
            host_id,
            serialize_pane_viewport,
            scrollback_lines_to_serialize,
            styled_underlines,
            serialization_interval,
            disable_session_metadata,
            support_kitty_keyboard_protocol,
            report_modifier_keys,
            support_kitty_graphics_protocol,
            stacked_resize,
            agent_pane_min_columns,
            agent_pane_min_rows,
            stacked_pane_list,
            show_startup_tips,
            show_release_notes,
            advanced_mouse_actions,
            mouse_scroll_resize,
            mouse_hover_effects,
            visual_bell,
            focus_follows_mouse,
            mouse_click_through,
            context_menu_on_right_click,
            osc133_command_selection,
            word_separators,
            post_command_discovery_hook,
            nested_session_handling,
            host_notification_protocol,
            dangerously_enable_paste_buffer_read,
        })
    }
    #[allow(clippy::result_large_err)]
    pub fn from_string(stringified_keybindings: &str) -> Result<Self, ConfigError> {
        let document: KdlDocument = stringified_keybindings.parse()?;
        Options::from_kdl(&document)
    }
    fn simplified_ui_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Use a simplified UI without special fonts (arrow glyphs)",
            "// Options:",
            "//   - true",
            "//   - false (Default)",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("simplified_ui");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(simplified_ui) = self.simplified_ui {
            let mut node = create_node(simplified_ui);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(true);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn osc8_hyperlinks_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Enable OSC8 hyperlink output",
            "// Options:",
            "//   - true (Default)",
            "//   - false",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("osc8_hyperlinks");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(osc8_hyperlinks) = self.osc8_hyperlinks {
            let mut node = create_node(osc8_hyperlinks);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(true);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn theme_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// Choose the theme that is specified in the themes section.",
            "// Default: default",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("theme");
            node.push(node_value.to_owned());
            node
        };
        if let Some(theme) = &self.theme {
            let mut node = create_node(theme);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("dracula");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn theme_dark_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// Theme to use when the host terminal reports a dark color palette.",
            "// Requires `theme_light` to also be set; otherwise `theme` is used.",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("theme_dark");
            node.push(node_value.to_owned());
            node
        };
        if let Some(theme) = &self.theme_dark {
            let mut node = create_node(theme);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("dracula");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn theme_light_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// Theme to use when the host terminal reports a light color palette.",
            "// Requires `theme_dark` to also be set; otherwise `theme` is used.",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("theme_light");
            node.push(node_value.to_owned());
            node
        };
        if let Some(theme) = &self.theme_light {
            let mut node = create_node(theme);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("solarized-light");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn explicit_theme_hue_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}",
            " ",
            "// Pin the session to a dark or light appearance, ignoring what the",
            "// host terminal reports. When unset, the host terminal decides.",
            "// Options: dark, light",
            "// ",
        );

        let create_node = |hue: &ThemeHue| -> KdlNode {
            let mut node = KdlNode::new("explicit_theme_hue");
            node.push(format!("{}", hue));
            node
        };
        if let Some(explicit_theme_hue) = &self.explicit_theme_hue {
            let mut node = create_node(explicit_theme_hue);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(&ThemeHue::Dark);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn default_mode_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ", "// Choose the base input mode of zellij.", "// Default: normal", "// "
        );

        let create_node = |default_mode: &InputMode| -> KdlNode {
            let mut node = KdlNode::new("default_mode");
            node.push(format!("{:?}", default_mode).to_lowercase());
            node
        };
        if let Some(default_mode) = &self.default_mode {
            let mut node = create_node(default_mode);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(&InputMode::Locked);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn default_shell_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text =
            format!("{}\n{}\n{}\n{}",
            " ",
            "// Choose the path to the default shell that zellij will use for opening new panes",
            "// Default: $SHELL",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("default_shell");
            node.push(node_value.to_owned());
            node
        };
        if let Some(default_shell) = &self.default_shell {
            let mut node = create_node(&default_shell.display().to_string());
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("fish");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn default_cwd_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}",
            " ",
            "// Choose the path to override cwd that zellij will use for opening new panes",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("default_cwd");
            node.push(node_value.to_owned());
            node
        };
        if let Some(default_cwd) = &self.default_cwd {
            let mut node = create_node(&default_cwd.display().to_string());
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("/tmp");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn default_layout_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// The name of the default layout to load on startup",
            "// Default: \"default\"",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("default_layout");
            node.push(node_value.to_owned());
            node
        };
        if let Some(default_layout) = &self.default_layout {
            let mut node = create_node(&default_layout.display().to_string());
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("compact");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn layout_dir_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// The folder in which Zellij will look for layouts",
            "// (Requires restart)",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("layout_dir");
            node.push(node_value.to_owned());
            node
        };
        if let Some(layout_dir) = &self.layout_dir {
            let mut node = create_node(&layout_dir.display().to_string());
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("/tmp");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn builtin_plugins_dir_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// The folder in which Zellij will look for builtin plugin WASM files",
            "// It falls back to the installed builtin plugins when a file is not present",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("builtin_plugins_dir");
            node.push(node_value.to_owned());
            node
        };
        if let Some(builtin_plugins_dir) = &self.builtin_plugins_dir {
            let mut node = create_node(&builtin_plugins_dir.display().to_string());
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("/tmp");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn theme_dir_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// The folder in which Zellij will look for themes",
            "// (Requires restart)",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("theme_dir");
            node.push(node_value.to_owned());
            node
        };
        if let Some(theme_dir) = &self.theme_dir {
            let mut node = create_node(&theme_dir.display().to_string());
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("/tmp");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn user_plugin_dir_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// The folder in which Zellij will look for plugins you wrote yourself",
            "// (Requires restart)",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("user_plugin_dir");
            node.push(node_value.to_owned());
            node
        };
        if let Some(user_plugin_dir) = &self.user_plugin_dir {
            let mut node = create_node(&user_plugin_dir.display().to_string());
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("/tmp");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn detachable_panes_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Run each pane's process under a small supervisor process, so that",
            "// panes keep running when the session's server exits (for example if",
            "// it is killed or crashes).",
            "// Options:",
            "//   - true (default)",
            "//   - false",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("detachable_panes");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(detachable_panes) = self.detachable_panes {
            let mut node = create_node(detachable_panes);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(true);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn firecracker_provider_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let create_node = |value: &str| {
            let mut node = KdlNode::new("firecracker_provider");
            node.push(value.to_owned());
            node
        };
        match &self.firecracker_provider {
            Some(path) => Some(create_node(&path.display().to_string())),
            None if add_comments => {
                let mut node = create_node("/absolute/path/to/provider");
                node.set_leading("\n// Firecracker VM provider executable\n// ".to_owned());
                Some(node)
            },
            None => None,
        }
    }
    fn firecracker_provider_timeout_secs_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let create_node = |value: u64| {
            let mut node = KdlNode::new("firecracker_provider_timeout_secs");
            node.push(value as i64);
            node
        };
        match self.firecracker_provider_timeout_secs {
            Some(timeout) => Some(create_node(timeout)),
            None if add_comments => {
                let mut node = create_node(60);
                node.set_leading("\n// Firecracker provider request timeout\n// ".to_owned());
                Some(node)
            },
            None => None,
        }
    }
    fn locked_escape_timeout_ms_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}",
            " ",
            "// How long a locked-mode Ctrl+g waits for a plain g that would quote it into",
            "// the pane, in milliseconds. 0 unlocks immediately and never quotes Ctrl+g.",
        );
        let create_node = |value: u64| {
            let mut node = KdlNode::new("locked_escape_timeout_ms");
            node.push(value as i64);
            node
        };
        match self.locked_escape_timeout_ms {
            Some(timeout) => {
                let mut node = create_node(timeout);
                if add_comments {
                    node.set_leading(format!("{}\n", comment_text));
                }
                Some(node)
            },
            None if add_comments => {
                let mut node = create_node(250);
                node.set_leading(format!("{}\n// ", comment_text));
                Some(node)
            },
            None => None,
        }
    }

    fn mouse_mode_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Toggle enabling the mouse mode.",
            "// On certain configurations, or terminals this could",
            "// potentially interfere with copying text.",
            "// Options:",
            "//   - true (default)",
            "//   - false",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("mouse_mode");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(mouse_mode) = self.mouse_mode {
            let mut node = create_node(mouse_mode);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn pane_frames_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Toggle having pane frames around the panes",
            "// Options:",
            "//   - true (default, enabled)",
            "//   - false",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("pane_frames");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(pane_frames) = self.pane_frames {
            let mut node = create_node(pane_frames);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn pane_frame_style_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Set the pane frame style when pane_frames is enabled",
            "// Options:",
            "//   - full",
            "//   - titles (default)",
            "// ",
        );

        let style_as_str = |style: &PaneFrameStyle| -> &'static str {
            match style {
                PaneFrameStyle::Full => "full",
                PaneFrameStyle::Titles => "titles",
                PaneFrameStyle::None => "none",
            }
        };

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("pane_frame_style");
            node.push(node_value.to_owned());
            node
        };
        if let Some(pane_frame_style) = &self.pane_frame_style {
            let mut node = create_node(style_as_str(pane_frame_style));
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("titles");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn notification_position_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// The corner notifications appear in - they stack downwards from a top corner",
            "// and upwards from a bottom one",
            "// Options:",
            "//   - top-left",
            "//   - top-right (default)",
            "//   - bottom-left",
            "//   - bottom-right",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("notification_position");
            node.push(node_value.to_owned());
            node
        };
        if let Some(notification_position) = &self.notification_position {
            let mut node = create_node(notification_position.as_str());
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(NotificationPosition::default().as_str());
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn notification_timeout_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}",
            " ", "// How long a notification stays on screen, in seconds", "// Default: 4",
        );

        let create_node = |node_value: u64| -> KdlNode {
            let mut node = KdlNode::new("notification_timeout");
            node.push(node_value as i64);
            node
        };
        if let Some(notification_timeout) = self.notification_timeout {
            let mut node = create_node(notification_timeout);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(DEFAULT_NOTIFICATION_TIMEOUT_SECS);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn mirror_session_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// When attaching to an existing session with other users,",
            "// should the session be mirrored (true)",
            "// or should each user have their own cursor (false)",
            "// (Requires restart)",
            "// Default: false",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("mirror_session");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(mirror_session) = self.mirror_session {
            let mut node = create_node(mirror_session);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(true);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn on_force_close_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Choose what to do when zellij receives SIGTERM, SIGINT, SIGQUIT or SIGHUP",
            "// eg. when terminal window with an active zellij session is closed",
            "// (Requires restart)",
            "// Options:",
            "//   - detach (Default)",
            "//   - quit",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("on_force_close");
            node.push(node_value.to_owned());
            node
        };
        if let Some(on_force_close) = &self.on_force_close {
            let mut node = match on_force_close {
                OnForceClose::Detach => create_node("detach"),
                OnForceClose::Quit => create_node("quit"),
            };
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("quit");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn scroll_buffer_size_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Configure the scroll back buffer size",
            "// This is the number of lines zellij stores for each pane in the scroll back",
            "// buffer. Excess number of lines are discarded in a FIFO fashion.",
            "// (Requires restart)",
            "// Valid values: positive integers",
            "// Default value: 10000",
            "// ",
        );

        let create_node = |node_value: usize| -> KdlNode {
            let mut node = KdlNode::new("scroll_buffer_size");
            node.push(KdlValue::Base10(node_value as i64));
            node
        };
        if let Some(scroll_buffer_size) = self.scroll_buffer_size {
            let mut node = create_node(scroll_buffer_size);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(10000);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn copy_command_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Provide a command to execute when copying text. The text will be piped to",
            "// the stdin of the program to perform the copy. This can be used with",
            "// terminal emulators which do not support the OSC 52 ANSI control sequence",
            "// that will be used by default if this option is not set.",
            "// Examples:",
            "//",
            "// copy_command \"xclip -selection clipboard\" // x11",
            "// copy_command \"wl-copy\"                    // wayland",
            "// copy_command \"pbcopy\"                     // osx",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("copy_command");
            node.push(node_value.to_owned());
            node
        };
        if let Some(copy_command) = &self.copy_command {
            let mut node = create_node(copy_command);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("pbcopy");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn copy_clipboard_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!("{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Choose the destination for copied text",
            "// Allows using the primary selection buffer (on x11/wayland) instead of the system clipboard.",
            "// Does not apply when using copy_command.",
            "// Options:",
            "//   - system (default)",
            "//   - primary",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("copy_clipboard");
            node.push(node_value.to_owned());
            node
        };
        if let Some(copy_clipboard) = &self.copy_clipboard {
            let mut node = match copy_clipboard {
                Clipboard::Primary => create_node("primary"),
                Clipboard::System => create_node("system"),
            };
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("primary");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn copy_on_select_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// Enable automatic copying (and clearing) of selection when releasing mouse",
            "// Default: true",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("copy_on_select");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(copy_on_select) = self.copy_on_select {
            let mut node = create_node(copy_on_select);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(true);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn scrollback_editor_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}",
            " ",
            "// Path to the default editor to use to edit pane scrollbuffer",
            "// Default: $EDITOR or $VISUAL",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("scrollback_editor");
            node.push(node_value.to_owned());
            node
        };
        if let Some(scrollback_editor) = &self.scrollback_editor {
            let mut node = create_node(&scrollback_editor.display().to_string());
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("/usr/bin/vim");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn session_name_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// A fixed name to always give the Zellij session.",
            "// Consider also setting `attach_to_session true,`",
            "// otherwise this will error if such a session exists.",
            "// Default: <RANDOM>",
            "// ",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("session_name");
            node.push(node_value.to_owned());
            node
        };
        if let Some(session_name) = &self.session_name {
            let mut node = create_node(session_name);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("My singleton session");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn attach_to_session_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}",
            " ",
            "// When `session_name` is provided, attaches to that session",
            "// if it is already running or creates it otherwise.",
            "// Default: false",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("attach_to_session");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(attach_to_session) = self.attach_to_session {
            let mut node = create_node(attach_to_session);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(true);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn auto_layout_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!("{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Toggle between having Zellij lay out panes according to a predefined set of layouts whenever possible",
            "// Options:",
            "//   - true (default)",
            "//   - false",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("auto_layout");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(auto_layout) = self.auto_layout {
            let mut node = create_node(auto_layout);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn session_serialization_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!("{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Whether sessions should be serialized to the cache folder (including their tabs/panes, cwds and running commands) so that they can later be resurrected",
            "// Options:",
            "//   - true (default)",
            "//   - false",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("session_serialization");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(session_serialization) = self.session_serialization {
            let mut node = create_node(session_serialization);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn session_recording_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Record terminal pane output in the local recording database.",
            "// Options:",
            "//   - true",
            "//   - false (default)",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("session_recording");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(session_recording) = self.session_recording {
            let mut node = create_node(session_recording);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn session_recording_input_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Record raw terminal input. This can include passwords and other secrets.",
            "// This option has an effect only when session_recording is true.",
            "// Options:",
            "//   - true",
            "//   - false (default)",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("session_recording_input");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(session_recording_input) = self.session_recording_input {
            let mut node = create_node(session_recording_input);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    /// Emitted only when set. The commented-out template lives in `default.kdl`, next to the
    /// other options, and a URL with a password in it is not something to write out unasked.
    fn plugin_data_url_to_kdl(&self) -> Option<KdlNode> {
        self.plugin_data_url.as_ref().map(|url| {
            let mut node = KdlNode::new("plugin_data_url");
            node.push(url.clone());
            node
        })
    }
    fn plugin_data_schema_to_kdl(&self) -> Option<KdlNode> {
        self.plugin_data_schema.as_ref().map(|schema| {
            let mut node = KdlNode::new("plugin_data_schema");
            node.push(schema.clone());
            node
        })
    }
    fn host_id_to_kdl(&self) -> Option<KdlNode> {
        self.host_id.as_ref().map(|host_id| {
            let mut node = KdlNode::new("host_id");
            node.push(host_id.clone());
            node
        })
    }
    fn serialize_pane_viewport_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Whether pane viewports are serialized along with the session, default is false",
            "// Options:",
            "//   - true",
            "//   - false (default)",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("serialize_pane_viewport");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(serialize_pane_viewport) = self.serialize_pane_viewport {
            let mut node = create_node(serialize_pane_viewport);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn scrollback_lines_to_serialize_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!("{}\n{}\n{}\n{}\n{}",
            " ",
            "// Scrollback lines to serialize along with the pane viewport when serializing sessions, 0",
            "// defaults to the scrollback size. If this number is higher than the scrollback size, it will",
            "// also default to the scrollback size. This does nothing if `serialize_pane_viewport` is not true.",
            "// ",
        );

        let create_node = |node_value: usize| -> KdlNode {
            let mut node = KdlNode::new("scrollback_lines_to_serialize");
            node.push(KdlValue::Base10(node_value as i64));
            node
        };
        if let Some(scrollback_lines_to_serialize) = self.scrollback_lines_to_serialize {
            let mut node = create_node(scrollback_lines_to_serialize);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(10000);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn styled_underlines_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Enable or disable the rendering of styled and colored underlines (undercurl).",
            "// May need to be disabled for certain unsupported terminals",
            "// (Requires restart)",
            "// Default: true",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("styled_underlines");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(styled_underlines) = self.styled_underlines {
            let mut node = create_node(styled_underlines);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn serialization_interval_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}",
            " ", "// How often in seconds sessions are serialized", "// ",
        );

        let create_node = |node_value: u64| -> KdlNode {
            let mut node = KdlNode::new("serialization_interval");
            node.push(KdlValue::Base10(node_value as i64));
            node
        };
        if let Some(serialization_interval) = self.serialization_interval {
            let mut node = create_node(serialization_interval);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(10000);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn disable_session_metadata_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!("{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Enable or disable writing of session metadata to disk (if disabled, other sessions might not know",
            "// metadata info on this session)",
            "// (Requires restart)",
            "// Default: false",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("disable_session_metadata");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(disable_session_metadata) = self.disable_session_metadata {
            let mut node = create_node(disable_session_metadata);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn support_kitty_keyboard_protocol_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!("{}\n{}\n{}\n{}\n{}",
            " ",
            "// Enable or disable support for the enhanced Kitty Keyboard Protocol (the host terminal must also support it)",
            "// (Requires restart)",
            "// Default: true (if the host terminal supports it)",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("support_kitty_keyboard_protocol");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(support_kitty_keyboard_protocol) = self.support_kitty_keyboard_protocol {
            let mut node = create_node(support_kitty_keyboard_protocol);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn report_modifier_keys_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!("{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// Ask the host terminal to report the modifier keys themselves, so the UI can show",
            "// something for as long as eg. Alt is held. Needs Kitty Keyboard flags that also make",
            "// every printable key arrive as an escape code: AltGr glyphs, dead keys and IME input",
            "// will stop working. (Requires restart) Default: false",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("report_modifier_keys");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(report_modifier_keys) = self.report_modifier_keys {
            let mut node = create_node(report_modifier_keys);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn support_kitty_graphics_protocol_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!("{}\n{}\n{}\n{}\n{}",
            " ",
            "// Enable or disable support for the Kitty Graphics Protocol, used to display images (the host terminal must also support it)",
            "// (Requires restart)",
            "// Default: true (if the host terminal supports it)",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("support_kitty_graphics_protocol");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(support_kitty_graphics_protocol) = self.support_kitty_graphics_protocol {
            let mut node = create_node(support_kitty_graphics_protocol);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn agent_pane_min_columns_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// The smallest tiled pane worth giving a newly spawned coding agent, as a terminal",
            "// size. Before an agent pane joins a tab, zellij projects the area each tiled pane",
            "// would have if the tab's tiled panes (floating ones aside) shared the viewport",
            "// equally, and compares it with columns x rows. A tab that would fall below it",
            "// sends the agent to the roomiest other tab that stays above it, or to a new tab.",
            "// Set either to 0 to always open agents where they were asked for.",
            "// Default: 80 and 24",
            "// ",
            "// "
        );
        let create_node = |value: usize| {
            let mut node = KdlNode::new("agent_pane_min_columns");
            node.push(value as i64);
            node
        };
        match self.agent_pane_min_columns {
            Some(value) => Some(create_node(value)),
            None if add_comments => {
                let mut node = create_node(DEFAULT_AGENT_PANE_MIN_COLUMNS);
                node.set_leading(comment_text);
                Some(node)
            },
            None => None,
        }
    }
    fn agent_pane_min_rows_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let create_node = |value: usize| {
            let mut node = KdlNode::new("agent_pane_min_rows");
            node.push(value as i64);
            node
        };
        match self.agent_pane_min_rows {
            Some(value) => Some(create_node(value)),
            None if add_comments => {
                let mut node = create_node(DEFAULT_AGENT_PANE_MIN_ROWS);
                node.set_leading("// ".to_owned());
                Some(node)
            },
            None => None,
        }
    }
    fn stacked_resize_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// Whether to stack panes when resizing beyond a certain size",
            "// Default: true",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("stacked_resize");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(stacked_resize) = self.stacked_resize {
            let mut node = create_node(stacked_resize);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn stacked_pane_list_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// Whether stacked panes display as a list with the expanded pane pinned to the bottom",
            "// Default: true",
            "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("stacked_pane_list");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(stacked_pane_list) = self.stacked_pane_list {
            let mut node = create_node(stacked_pane_list);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn show_startup_tips_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ", "// Whether to show tips on startup", "// Default: true", "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("show_startup_tips");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(show_startup_tips) = self.show_startup_tips {
            let mut node = create_node(show_startup_tips);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn show_release_notes_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ", "// Whether to show release notes on first version run", "// Default: true", "// ",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("show_release_notes");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(show_release_notes) = self.show_release_notes {
            let mut node = create_node(show_release_notes);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn advanced_mouse_actions_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}",
            " ",
            "// Whether to enable mouse hover effects and pane grouping functionality",
            "// default is true",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("advanced_mouse_actions");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(advanced_mouse_actions) = self.advanced_mouse_actions {
            let mut node = create_node(advanced_mouse_actions);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn mouse_scroll_resize_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}",
            " ", "// Whether Ctrl+ScrollWheel resizes panes", "// default is true",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("mouse_scroll_resize");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(mouse_scroll_resize) = self.mouse_scroll_resize {
            let mut node = create_node(mouse_scroll_resize);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn mouse_hover_effects_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}",
            " ",
            "// Whether to enable mouse hover visual effects (frame highlight and help text)",
            "// default is true",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("mouse_hover_effects");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(mouse_hover_effects) = self.mouse_hover_effects {
            let mut node = create_node(mouse_hover_effects);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn visual_bell_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}",
            " ",
            "// Whether to show visual bell indicators (pane/tab frame flash and [!] suffix)",
            "// default is true",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("visual_bell");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(visual_bell) = self.visual_bell {
            let mut node = create_node(visual_bell);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(true);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn focus_follows_mouse_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}",
            " ", "// Whether to focus panes on mouse hover", "// default is false",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("focus_follows_mouse");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(focus_follows_mouse) = self.focus_follows_mouse {
            let mut node = create_node(focus_follows_mouse);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn mouse_click_through_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}",
            " ",
            "// Whether clicking a pane to focus it also sends the click into the pane",
            "// default is false",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("mouse_click_through");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(mouse_click_through) = self.mouse_click_through {
            let mut node = create_node(mouse_click_through);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn context_menu_on_right_click_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// Whether right-clicking a pane opens a context menu. A program that is tracking the",
            "// mouse itself keeps its own right-click either way.",
            "// default is false",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("context_menu_on_right_click");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(context_menu_on_right_click) = self.context_menu_on_right_click {
            let mut node = create_node(context_menu_on_right_click);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn osc133_command_selection_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// Whether triple-clicking inside command output marked by the shell (OSC 133) selects",
            "// the command and its output instead of the logical line",
            "// default is true",
        );

        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("osc133_command_selection");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(osc133_command_selection) = self.osc133_command_selection {
            let mut node = create_node(osc133_command_selection);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn word_separators_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}",
            " ",
            "// Characters that terminate a word when double-clicking to select it",
            "// whitespace is always a separator and need not be listed here",
            "// default is \"[]{}<>()\"",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("word_separators");
            node.push(node_value.to_owned());
            node
        };
        if let Some(word_separators) = &self.word_separators {
            let mut node = create_node(word_separators);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(DEFAULT_WORD_SEPARATORS);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn post_command_discovery_hook_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            " ",
            "// A command to run (will be wrapped with sh -c and provided the RESURRECT_COMMAND env variable) ",
            "// after Zellij attempts to discover a command inside a pane when resurrecting sessions, the STDOUT",
            "// of this command will be used instead of the discovered RESURRECT_COMMAND",
            "// can be useful for removing wrappers around commands",
            "// Note: be sure to escape backslashes and similar characters properly",
        );

        let create_node = |node_value: &str| -> KdlNode {
            let mut node = KdlNode::new("post_command_discovery_hook");
            node.push(node_value.to_owned());
            node
        };
        if let Some(post_command_discovery_hook) = &self.post_command_discovery_hook {
            let mut node = create_node(post_command_discovery_hook);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node("echo $RESURRECT_COMMAND | sed <your_regex_here>");
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn nested_session_handling_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        use crate::input::options::NestedSessionHandling;
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            "",
            "// How to handle a nested Zellij session detected inside a pane.",
            "// Options:",
            "//   - \"ask\" (Default — prompt with a modal)",
            "//   - \"fullscreen\" (always zoom into the nested session)",
            "//   - \"descend\" (always control the nested session on focus)",
            "//   - \"never\" (never prompt or descend; do it manually)",
            "// ",
        );
        let create_node = |value: NestedSessionHandling| -> KdlNode {
            let mut node = KdlNode::new("nested_session_handling");
            let s = match value {
                NestedSessionHandling::Ask => "ask",
                NestedSessionHandling::Fullscreen => "fullscreen",
                NestedSessionHandling::Descend => "descend",
                NestedSessionHandling::Never => "never",
            };
            node.push(KdlValue::String(s.to_string()));
            node
        };
        if let Some(value) = self.nested_session_handling {
            let mut node = create_node(value);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(NestedSessionHandling::Ask);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn host_notification_protocol_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        use crate::input::options::HostNotificationProtocol;
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            "",
            "// Which escape sequence desktop notifications coming from panes are",
            "// forwarded to the host terminal with.",
            "// Options:",
            "//   - \"auto\" (Default — detect from the host terminal environment)",
            "//   - \"osc9\" (the legacy iTerm2 protocol, understood by most terminals)",
            "//   - \"osc99\" (kitty's notification protocol)",
            "//   - \"bell\" (ring the terminal bell instead)",
            "//   - \"off\" (do not forward notifications to the host terminal)",
            "// ",
        );
        let create_node = |value: HostNotificationProtocol| -> KdlNode {
            let mut node = KdlNode::new("host_notification_protocol");
            node.push(KdlValue::String(value.as_str().to_string()));
            node
        };
        if let Some(value) = self.host_notification_protocol {
            let mut node = create_node(value);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(HostNotificationProtocol::Auto);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    fn dangerously_enable_paste_buffer_read_to_kdl(&self, add_comments: bool) -> Option<KdlNode> {
        let comment_text = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            "",
            "// Whether to let programs running inside panes read the paste buffer",
            "// (clipboard) with the OSC 52 escape sequence. When enabled, any program",
            "// in any pane - including one running on a remote machine over SSH - can",
            "// read the clipboard without the user being asked.",
            "// Default: false",
        );
        let create_node = |node_value: bool| -> KdlNode {
            let mut node = KdlNode::new("dangerously_enable_paste_buffer_read");
            node.push(KdlValue::Bool(node_value));
            node
        };
        if let Some(value) = self.dangerously_enable_paste_buffer_read {
            let mut node = create_node(value);
            if add_comments {
                node.set_leading(format!("{}\n", comment_text));
            }
            Some(node)
        } else if add_comments {
            let mut node = create_node(false);
            node.set_leading(format!("{}\n// ", comment_text));
            Some(node)
        } else {
            None
        }
    }
    pub fn to_kdl(&self, add_comments: bool) -> Vec<KdlNode> {
        let mut nodes = vec![];
        if let Some(simplified_ui_node) = self.simplified_ui_to_kdl(add_comments) {
            nodes.push(simplified_ui_node);
        }
        if let Some(osc8_hyperlinks_node) = self.osc8_hyperlinks_to_kdl(add_comments) {
            nodes.push(osc8_hyperlinks_node);
        }
        if let Some(theme_node) = self.theme_to_kdl(add_comments) {
            nodes.push(theme_node);
        }
        if let Some(theme_dark_node) = self.theme_dark_to_kdl(add_comments) {
            nodes.push(theme_dark_node);
        }
        if let Some(theme_light_node) = self.theme_light_to_kdl(add_comments) {
            nodes.push(theme_light_node);
        }
        if let Some(explicit_theme_hue_node) = self.explicit_theme_hue_to_kdl(add_comments) {
            nodes.push(explicit_theme_hue_node);
        }
        if let Some(default_mode) = self.default_mode_to_kdl(add_comments) {
            nodes.push(default_mode);
        }
        if let Some(default_shell) = self.default_shell_to_kdl(add_comments) {
            nodes.push(default_shell);
        }
        if let Some(default_cwd) = self.default_cwd_to_kdl(add_comments) {
            nodes.push(default_cwd);
        }
        if let Some(default_layout) = self.default_layout_to_kdl(add_comments) {
            nodes.push(default_layout);
        }
        if let Some(layout_dir) = self.layout_dir_to_kdl(add_comments) {
            nodes.push(layout_dir);
        }
        if let Some(builtin_plugins_dir) = self.builtin_plugins_dir_to_kdl(add_comments) {
            nodes.push(builtin_plugins_dir);
        }
        if let Some(theme_dir) = self.theme_dir_to_kdl(add_comments) {
            nodes.push(theme_dir);
        }
        if let Some(user_plugin_dir) = self.user_plugin_dir_to_kdl(add_comments) {
            nodes.push(user_plugin_dir);
        }
        if let Some(detachable_panes) = self.detachable_panes_to_kdl(add_comments) {
            nodes.push(detachable_panes);
        }
        if let Some(provider) = self.firecracker_provider_to_kdl(add_comments) {
            nodes.push(provider);
        }
        if let Some(timeout) = self.firecracker_provider_timeout_secs_to_kdl(add_comments) {
            nodes.push(timeout);
        }
        if let Some(mouse_mode) = self.mouse_mode_to_kdl(add_comments) {
            nodes.push(mouse_mode);
        }
        if let Some(pane_frames) = self.pane_frames_to_kdl(add_comments) {
            nodes.push(pane_frames);
        }
        if let Some(pane_frame_style) = self.pane_frame_style_to_kdl(add_comments) {
            nodes.push(pane_frame_style);
        }
        if let Some(notification_position) = self.notification_position_to_kdl(add_comments) {
            nodes.push(notification_position);
        }
        if let Some(notification_timeout) = self.notification_timeout_to_kdl(add_comments) {
            nodes.push(notification_timeout);
        }
        if let Some(mirror_session) = self.mirror_session_to_kdl(add_comments) {
            nodes.push(mirror_session);
        }
        if let Some(on_force_close) = self.on_force_close_to_kdl(add_comments) {
            nodes.push(on_force_close);
        }
        if let Some(scroll_buffer_size) = self.scroll_buffer_size_to_kdl(add_comments) {
            nodes.push(scroll_buffer_size);
        }
        if let Some(locked_escape_timeout_ms) = self.locked_escape_timeout_ms_to_kdl(add_comments) {
            nodes.push(locked_escape_timeout_ms);
        }
        if let Some(copy_command) = self.copy_command_to_kdl(add_comments) {
            nodes.push(copy_command);
        }
        if let Some(copy_clipboard) = self.copy_clipboard_to_kdl(add_comments) {
            nodes.push(copy_clipboard);
        }
        if let Some(copy_on_select) = self.copy_on_select_to_kdl(add_comments) {
            nodes.push(copy_on_select);
        }
        if let Some(scrollback_editor) = self.scrollback_editor_to_kdl(add_comments) {
            nodes.push(scrollback_editor);
        }
        if let Some(session_name) = self.session_name_to_kdl(add_comments) {
            nodes.push(session_name);
        }
        if let Some(attach_to_session) = self.attach_to_session_to_kdl(add_comments) {
            nodes.push(attach_to_session);
        }
        if let Some(auto_layout) = self.auto_layout_to_kdl(add_comments) {
            nodes.push(auto_layout);
        }
        if let Some(session_serialization) = self.session_serialization_to_kdl(add_comments) {
            nodes.push(session_serialization);
        }
        if let Some(session_recording) = self.session_recording_to_kdl(add_comments) {
            nodes.push(session_recording);
        }
        if let Some(session_recording_input) = self.session_recording_input_to_kdl(add_comments) {
            nodes.push(session_recording_input);
        }
        if let Some(plugin_data_url) = self.plugin_data_url_to_kdl() {
            nodes.push(plugin_data_url);
        }
        if let Some(plugin_data_schema) = self.plugin_data_schema_to_kdl() {
            nodes.push(plugin_data_schema);
        }
        if let Some(host_id) = self.host_id_to_kdl() {
            nodes.push(host_id);
        }
        if let Some(serialize_pane_viewport) = self.serialize_pane_viewport_to_kdl(add_comments) {
            nodes.push(serialize_pane_viewport);
        }
        if let Some(scrollback_lines_to_serialize) =
            self.scrollback_lines_to_serialize_to_kdl(add_comments)
        {
            nodes.push(scrollback_lines_to_serialize);
        }
        if let Some(styled_underlines) = self.styled_underlines_to_kdl(add_comments) {
            nodes.push(styled_underlines);
        }
        if let Some(serialization_interval) = self.serialization_interval_to_kdl(add_comments) {
            nodes.push(serialization_interval);
        }
        if let Some(disable_session_metadata) = self.disable_session_metadata_to_kdl(add_comments) {
            nodes.push(disable_session_metadata);
        }
        if let Some(support_kitty_keyboard_protocol) =
            self.support_kitty_keyboard_protocol_to_kdl(add_comments)
        {
            nodes.push(support_kitty_keyboard_protocol);
        }
        if let Some(report_modifier_keys) = self.report_modifier_keys_to_kdl(add_comments) {
            nodes.push(report_modifier_keys);
        }
        if let Some(support_kitty_graphics_protocol) =
            self.support_kitty_graphics_protocol_to_kdl(add_comments)
        {
            nodes.push(support_kitty_graphics_protocol);
        }
        if let Some(stacked_resize) = self.stacked_resize_to_kdl(add_comments) {
            nodes.push(stacked_resize);
        }
        if let Some(agent_pane_min_columns) = self.agent_pane_min_columns_to_kdl(add_comments) {
            nodes.push(agent_pane_min_columns);
        }
        if let Some(agent_pane_min_rows) = self.agent_pane_min_rows_to_kdl(add_comments) {
            nodes.push(agent_pane_min_rows);
        }
        if let Some(stacked_pane_list) = self.stacked_pane_list_to_kdl(add_comments) {
            nodes.push(stacked_pane_list);
        }
        if let Some(show_startup_tips) = self.show_startup_tips_to_kdl(add_comments) {
            nodes.push(show_startup_tips);
        }
        if let Some(show_release_notes) = self.show_release_notes_to_kdl(add_comments) {
            nodes.push(show_release_notes);
        }
        if let Some(advanced_mouse_actions) = self.advanced_mouse_actions_to_kdl(add_comments) {
            nodes.push(advanced_mouse_actions);
        }
        if let Some(mouse_scroll_resize) = self.mouse_scroll_resize_to_kdl(add_comments) {
            nodes.push(mouse_scroll_resize);
        }
        if let Some(mouse_hover_effects) = self.mouse_hover_effects_to_kdl(add_comments) {
            nodes.push(mouse_hover_effects);
        }
        if let Some(visual_bell) = self.visual_bell_to_kdl(add_comments) {
            nodes.push(visual_bell);
        }
        if let Some(focus_follows_mouse) = self.focus_follows_mouse_to_kdl(add_comments) {
            nodes.push(focus_follows_mouse);
        }
        if let Some(mouse_click_through) = self.mouse_click_through_to_kdl(add_comments) {
            nodes.push(mouse_click_through);
        }
        if let Some(context_menu_on_right_click) =
            self.context_menu_on_right_click_to_kdl(add_comments)
        {
            nodes.push(context_menu_on_right_click);
        }
        if let Some(osc133_command_selection) = self.osc133_command_selection_to_kdl(add_comments) {
            nodes.push(osc133_command_selection);
        }
        if let Some(word_separators) = self.word_separators_to_kdl(add_comments) {
            nodes.push(word_separators);
        }
        if let Some(post_command_discovery_hook) =
            self.post_command_discovery_hook_to_kdl(add_comments)
        {
            nodes.push(post_command_discovery_hook);
        }
        if let Some(dangerously_enable_paste_buffer_read) =
            self.dangerously_enable_paste_buffer_read_to_kdl(add_comments)
        {
            nodes.push(dangerously_enable_paste_buffer_read);
        }
        if let Some(nested_session_handling) = self.nested_session_handling_to_kdl(add_comments) {
            nodes.push(nested_session_handling);
        }
        if let Some(host_notification_protocol) =
            self.host_notification_protocol_to_kdl(add_comments)
        {
            nodes.push(host_notification_protocol);
        }
        nodes
    }
}

impl Layout {
    #[allow(clippy::result_large_err)]
    pub fn from_kdl(
        raw_layout: &str,
        file_name: Option<String>,
        raw_swap_layouts: Option<(&str, &str)>, // raw_swap_layouts swap_layouts_file_name
        cwd: Option<PathBuf>,
    ) -> Result<Self, ConfigError> {
        let mut kdl_layout_parser = KdlLayoutParser::new(raw_layout, cwd, file_name.clone());
        let layout = kdl_layout_parser.parse().map_err(|e| match e {
            ConfigError::KdlError(kdl_error) => ConfigError::KdlError(kdl_error.add_src(
                file_name.unwrap_or_else(|| "N/A".to_owned()),
                String::from(raw_layout),
            )),
            ConfigError::KdlDeserializationError(kdl_error) => kdl_layout_error(
                kdl_error,
                file_name.unwrap_or_else(|| "N/A".to_owned()),
                raw_layout,
            ),
            e => e,
        })?;
        match raw_swap_layouts {
            Some((raw_swap_layout_filename, raw_swap_layout)) => {
                // here we use the same parser to parse the swap layout so that we can reuse assets
                // (eg. pane and tab templates)
                kdl_layout_parser
                    .parse_external_swap_layouts(raw_swap_layout, layout)
                    .map_err(|e| match e {
                        ConfigError::KdlError(kdl_error) => {
                            ConfigError::KdlError(kdl_error.add_src(
                                String::from(raw_swap_layout_filename),
                                String::from(raw_swap_layout),
                            ))
                        },
                        ConfigError::KdlDeserializationError(kdl_error) => kdl_layout_error(
                            kdl_error,
                            raw_swap_layout_filename.into(),
                            raw_swap_layout,
                        ),
                        e => e,
                    })
            },
            None => Ok(layout),
        }
    }
}

fn kdl_layout_error(kdl_error: kdl::KdlError, file_name: String, raw_layout: &str) -> ConfigError {
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
        src: Some(NamedSource::new(file_name, String::from(raw_layout))),
        offset: Some(kdl_error.span.offset()),
        len: Some(kdl_error.span.len()),
        help_message: None,
    };
    ConfigError::KdlError(kdl_error)
}

#[allow(clippy::result_large_err)]
pub fn environment_variables_from_kdl(
    kdl_env_variables: &KdlNode,
) -> Result<EnvironmentVariables, ConfigError> {
    let mut env: HashMap<String, String> = HashMap::new();
    for env_var in kdl_children_nodes_or_error!(kdl_env_variables, "empty env variable block") {
        let env_var_name = kdl_name!(env_var);
        let env_var_str_value = kdl_first_entry_as_string!(env_var).map(|s| s.to_string());
        let env_var_int_value = kdl_first_entry_as_i64!(env_var).map(|s| s.to_string());
        let env_var_value =
            env_var_str_value
                .or(env_var_int_value)
                .ok_or(ConfigError::new_kdl_error(
                    format!("Failed to parse env var: {:?}", env_var_name),
                    env_var.span().offset(),
                    env_var.span().len(),
                ))?;
        env.insert(env_var_name.into(), env_var_value);
    }
    Ok(EnvironmentVariables::from_data(env))
}
pub fn environment_variables_to_kdl(variables: &EnvironmentVariables) -> Option<KdlNode> {
    let mut has_env_vars = false;
    let mut env = KdlNode::new("env");
    let mut env_vars = KdlDocument::new();

    let mut stable_sorted = BTreeMap::new();
    for (env_var_name, env_var_value) in variables.inner() {
        stable_sorted.insert(env_var_name, env_var_value);
    }
    for (env_key, env_value) in stable_sorted {
        has_env_vars = true;
        let mut variable_key = KdlNode::new(env_key.to_owned());
        variable_key.push(env_value.to_owned());
        env_vars.nodes_mut().push(variable_key);
    }

    if has_env_vars {
        env.set_children(env_vars);
        Some(env)
    } else {
        None
    }
}

impl Keybinds {
    #[allow(clippy::result_large_err)]
    fn bind_keys_in_block(
        block: &KdlNode,
        input_mode_keybinds: &mut HashMap<KeyWithModifier, Vec<Action>>,
        config_options: &Options,
    ) -> Result<(), ConfigError> {
        let all_nodes = kdl_children_nodes_or_error!(block, "no keybinding block for mode");
        let bind_nodes = all_nodes.iter().filter(|n| kdl_name!(n) == "bind");
        let unbind_nodes = all_nodes.iter().filter(|n| kdl_name!(n) == "unbind");
        for key_block in bind_nodes {
            Keybinds::bind_actions_for_each_key(key_block, input_mode_keybinds, config_options)?;
        }
        // we loop a second time so that the unbinds always happen after the binds
        for key_block in unbind_nodes {
            Keybinds::unbind_keys(key_block, input_mode_keybinds)?;
        }
        for key_block in all_nodes {
            if kdl_name!(key_block) != "bind" && kdl_name!(key_block) != "unbind" {
                return Err(ConfigError::new_kdl_error(
                    format!("Unknown keybind instruction: '{}'", kdl_name!(key_block)),
                    key_block.span().offset(),
                    key_block.span().len(),
                ));
            }
        }
        Ok(())
    }
    #[allow(clippy::result_large_err)]
    pub fn from_kdl(
        kdl_keybinds: &KdlNode,
        base_keybinds: Keybinds,
        config_options: &Options,
    ) -> Result<Self, ConfigError> {
        let clear_defaults = kdl_arg_is_truthy!(kdl_keybinds, "clear-defaults");
        let mut keybinds_from_config = if clear_defaults {
            Keybinds::default()
        } else {
            base_keybinds
        };
        for block in kdl_children_nodes_or_error!(kdl_keybinds, "keybindings with no children") {
            if kdl_name!(block) == "shared_except" || kdl_name!(block) == "shared" {
                let mut modes_to_exclude = vec![];
                for mode_name in kdl_string_arguments!(block) {
                    modes_to_exclude.push(InputMode::from_str(mode_name).map_err(|_| {
                        ConfigError::new_kdl_error(
                            format!("Invalid mode: '{}'", mode_name),
                            block.name().span().offset(),
                            block.name().span().len(),
                        )
                    })?);
                }
                for mode in InputMode::iter() {
                    if modes_to_exclude.contains(&mode) {
                        continue;
                    }
                    let input_mode_keybinds = keybinds_from_config.get_input_mode_mut(&mode);
                    Keybinds::bind_keys_in_block(block, input_mode_keybinds, config_options)?;
                }
            }
            if kdl_name!(block) == "shared_among" {
                let mut modes_to_include = vec![];
                for mode_name in kdl_string_arguments!(block) {
                    modes_to_include.push(InputMode::from_str(mode_name)?);
                }
                for mode in InputMode::iter() {
                    if !modes_to_include.contains(&mode) {
                        continue;
                    }
                    let input_mode_keybinds = keybinds_from_config.get_input_mode_mut(&mode);
                    Keybinds::bind_keys_in_block(block, input_mode_keybinds, config_options)?;
                }
            }
        }
        for mode in kdl_children_nodes_or_error!(kdl_keybinds, "keybindings with no children") {
            if kdl_name!(mode) == "unbind"
                || kdl_name!(mode) == "shared_except"
                || kdl_name!(mode) == "shared_among"
                || kdl_name!(mode) == "shared"
            {
                continue;
            }
            let input_mode_keybinds =
                Keybinds::input_mode_keybindings(mode, &mut keybinds_from_config)?;
            Keybinds::bind_keys_in_block(mode, input_mode_keybinds, config_options)?;
        }
        if let Some(global_unbind) = kdl_keybinds.children().and_then(|c| c.get("unbind")) {
            Keybinds::unbind_keys_in_all_modes(global_unbind, &mut keybinds_from_config)?;
        };
        Ok(keybinds_from_config)
    }
    #[allow(clippy::result_large_err)]
    fn bind_actions_for_each_key(
        key_block: &KdlNode,
        input_mode_keybinds: &mut HashMap<KeyWithModifier, Vec<Action>>,
        config_options: &Options,
    ) -> Result<(), ConfigError> {
        let keys: Vec<KeyWithModifier> = keys_from_kdl!(key_block);
        let actions: Vec<Action> = actions_from_kdl!(key_block, config_options);
        for key in keys {
            input_mode_keybinds.insert(key, actions.clone());
        }
        Ok(())
    }
    #[allow(clippy::result_large_err)]
    fn unbind_keys(
        key_block: &KdlNode,
        input_mode_keybinds: &mut HashMap<KeyWithModifier, Vec<Action>>,
    ) -> Result<(), ConfigError> {
        let keys: Vec<KeyWithModifier> = keys_from_kdl!(key_block);
        for key in keys {
            input_mode_keybinds.remove(&key);
        }
        Ok(())
    }
    #[allow(clippy::result_large_err)]
    fn unbind_keys_in_all_modes(
        global_unbind: &KdlNode,
        keybinds_from_config: &mut Keybinds,
    ) -> Result<(), ConfigError> {
        let keys: Vec<KeyWithModifier> = keys_from_kdl!(global_unbind);
        for mode in keybinds_from_config.0.values_mut() {
            for key in &keys {
                mode.remove(key);
            }
        }
        Ok(())
    }
    #[allow(clippy::result_large_err)]
    fn input_mode_keybindings<'a>(
        mode: &KdlNode,
        keybinds_from_config: &'a mut Keybinds,
    ) -> Result<&'a mut HashMap<KeyWithModifier, Vec<Action>>, ConfigError> {
        let mode_name = kdl_name!(mode);
        let input_mode = InputMode::from_str(mode_name).map_err(|_| {
            ConfigError::new_kdl_error(
                format!("Invalid mode: '{}'", mode_name),
                mode.name().span().offset(),
                mode.name().span().len(),
            )
        })?;
        let input_mode_keybinds = keybinds_from_config.get_input_mode_mut(&input_mode);
        let clear_defaults_for_mode = kdl_arg_is_truthy!(mode, "clear-defaults");
        if clear_defaults_for_mode {
            input_mode_keybinds.clear();
        }
        Ok(input_mode_keybinds)
    }
    #[allow(clippy::result_large_err)]
    pub fn from_string(
        stringified_keybindings: String,
        base_keybinds: Keybinds,
        config_options: &Options,
    ) -> Result<Self, ConfigError> {
        let document: KdlDocument = stringified_keybindings.parse()?;
        if let Some(kdl_keybinds) = document.get("keybinds") {
            Keybinds::from_kdl(kdl_keybinds, base_keybinds, config_options)
        } else {
            Err(ConfigError::new_kdl_error(
                "Could not find keybinds node".to_string(),
                document.span().offset(),
                document.span().len(),
            ))
        }
    }
    // minimize keybind entries for serialization, so that duplicate entries will appear in
    // "shared" nodes later rather than once per mode
    fn minimize_entries(
        &self,
    ) -> BTreeMap<BTreeSet<InputMode>, BTreeMap<KeyWithModifier, Vec<Action>>> {
        let mut minimized: BTreeMap<BTreeSet<InputMode>, BTreeMap<KeyWithModifier, Vec<Action>>> =
            BTreeMap::new();
        let mut flattened: Vec<BTreeMap<KeyWithModifier, Vec<Action>>> = self
            .0
            .values()
            .map(|keybind| keybind.clone().into_iter().collect())
            .collect();
        for keybind in flattened.drain(..) {
            for (key, actions) in keybind.into_iter() {
                let mut appears_in_modes: BTreeSet<InputMode> = BTreeSet::new();
                for (input_mode, keybinds) in self.0.iter() {
                    if keybinds.get(&key) == Some(&actions) {
                        appears_in_modes.insert(*input_mode);
                    }
                }
                minimized
                    .entry(appears_in_modes)
                    .or_default()
                    .insert(key, actions);
            }
        }
        minimized
    }
    fn serialize_mode_title_node(&self, input_modes: &BTreeSet<InputMode>) -> KdlNode {
        let all_modes: Vec<InputMode> = InputMode::iter().collect();
        let total_input_mode_count = all_modes.len();
        if input_modes.len() == 1 {
            let input_mode_name =
                format!("{:?}", input_modes.iter().next().unwrap()).to_lowercase();
            KdlNode::new(input_mode_name)
        } else if input_modes.len() == total_input_mode_count {
            KdlNode::new("shared")
        } else if input_modes.len() < total_input_mode_count / 2 {
            let mut node = KdlNode::new("shared_among");
            for input_mode in input_modes {
                node.push(format!("{:?}", input_mode).to_lowercase());
            }
            node
        } else {
            let mut node = KdlNode::new("shared_except");
            let mut modes = all_modes.clone();
            for input_mode in input_modes {
                modes.retain(|m| m != input_mode)
            }
            for mode in modes {
                node.push(format!("{:?}", mode).to_lowercase());
            }
            node
        }
    }
    fn serialize_mode_keybinds(
        &self,
        keybinds: &BTreeMap<KeyWithModifier, Vec<Action>>,
    ) -> KdlDocument {
        let mut mode_keybinds = KdlDocument::new();
        for keybind in keybinds {
            let mut keybind_node = KdlNode::new("bind");
            keybind_node.push(keybind.0.to_keybind_string());
            let mut actions = KdlDocument::new();
            let mut actions_have_children = false;
            for action in keybind.1 {
                if let Some(kdl_action) = action.to_kdl() {
                    if kdl_action.children().is_some() {
                        actions_have_children = true;
                    }
                    actions.nodes_mut().push(kdl_action);
                }
            }
            if !actions_have_children {
                for action in actions.nodes_mut() {
                    action.set_leading("");
                    action.set_trailing("; ");
                }
                actions.set_leading(" ");
                actions.set_trailing("");
            }
            keybind_node.set_children(actions);
            mode_keybinds.nodes_mut().push(keybind_node);
        }
        mode_keybinds
    }
    pub fn to_kdl(&self, should_clear_defaults: bool) -> KdlNode {
        let mut keybinds_node = KdlNode::new("keybinds");
        if should_clear_defaults {
            keybinds_node.insert("clear-defaults", true);
        }
        let mut minimized = self.minimize_entries();
        let mut keybinds_children = KdlDocument::new();

        macro_rules! encode_single_input_mode {
            ($mode_name:ident) => {{
                if let Some(keybinds) = minimized.remove(&BTreeSet::from([InputMode::$mode_name])) {
                    let mut mode_node =
                        KdlNode::new(format!("{:?}", InputMode::$mode_name).to_lowercase());
                    let mode_keybinds = self.serialize_mode_keybinds(&keybinds);
                    mode_node.set_children(mode_keybinds);
                    keybinds_children.nodes_mut().push(mode_node);
                }
            }};
        }
        // we do this explicitly so that the sorting order of modes in the config is more Human
        // readable - this is actually less code (and clearer) than implementing Ord in this case
        encode_single_input_mode!(Normal);
        encode_single_input_mode!(Locked);
        encode_single_input_mode!(Pane);
        encode_single_input_mode!(Tab);
        encode_single_input_mode!(Resize);
        encode_single_input_mode!(Move);
        encode_single_input_mode!(Scroll);
        encode_single_input_mode!(Search);
        encode_single_input_mode!(Session);
        encode_single_input_mode!(Agent);

        for (input_modes, keybinds) in minimized {
            if input_modes.is_empty() {
                log::error!("invalid input mode for keybinds: {:#?}", keybinds);
                continue;
            }
            let mut mode_node = self.serialize_mode_title_node(&input_modes);
            let mode_keybinds = self.serialize_mode_keybinds(&keybinds);
            mode_node.set_children(mode_keybinds);
            keybinds_children.nodes_mut().push(mode_node);
        }
        keybinds_node.set_children(keybinds_children);
        keybinds_node
    }
}

/// The one `name` block of a document, or an error naming the second when there are more.
///
/// `KdlDocument::get` returns the first match, so a second `keybinds` (or `plugins`, …) block
/// in the same file used to parse successfully and silently contribute nothing. Merging across
/// *files* is unaffected: each `config.d` fragment and layout is its own `Config::from_kdl` call
/// over `base_config`.
#[allow(clippy::result_large_err)]
fn single_top_level_block<'a>(
    document: &'a KdlDocument,
    name: &str,
) -> Result<Option<&'a KdlNode>, ConfigError> {
    let mut blocks = document.nodes().iter().filter(|n| n.name().value() == name);
    let first = blocks.next();
    if let Some(second) = blocks.next() {
        return Err(kdl_parsing_error!(
            format!("'{name}' is defined more than once in this file; merge the blocks into one"),
            second
        ));
    }
    Ok(first)
}

impl Config {
    #[allow(clippy::result_large_err)]
    pub fn from_kdl(kdl_config: &str, base_config: Option<Config>) -> Result<Config, ConfigError> {
        let mut config = base_config.unwrap_or_default();
        let kdl_config: KdlDocument = kdl_config.parse()?;

        let config_options = Options::from_kdl(&kdl_config)?;
        config.options = config.options.merge(config_options);

        if let Some(kdl_keybinds) = single_top_level_block(&kdl_config, "keybinds")? {
            config.keybinds = Keybinds::from_kdl(kdl_keybinds, config.keybinds, &config.options)?;
        }
        if let Some(kdl_themes) = single_top_level_block(&kdl_config, "themes")? {
            let sourced_from_external_file = false;
            let config_themes = Themes::from_kdl(kdl_themes, sourced_from_external_file)?;
            config.themes = config.themes.merge(config_themes);
        }
        if let Some(kdl_plugin_aliases) = single_top_level_block(&kdl_config, "plugins")? {
            let config_plugins = PluginAliases::from_kdl(kdl_plugin_aliases)?;
            config.plugins.merge(config_plugins);
        }
        if let Some(kdl_load_plugins) = single_top_level_block(&kdl_config, "load_plugins")? {
            // Additive, like every other block here: a config written before a
            // default background plugin existed must still get it, otherwise the
            // plugin silently never starts.
            // ponytail: no way to drop a single default, only all of them with
            // clear-defaults - per-plugin removal if anyone ever asks
            if kdl_arg_is_truthy!(kdl_load_plugins, "clear-defaults") {
                config.background_plugins.clear();
                config.background_plugins_cleared = true;
            }
            config
                .background_plugins
                .extend(load_plugins_from_kdl(kdl_load_plugins)?);
        }
        if let Some(kdl_context_menu) = single_top_level_block(&kdl_config, "context_menu")? {
            config.context_menu = context_menu_from_kdl(kdl_context_menu, &config.options)?;
        }
        if let Some(kdl_hooks) = single_top_level_block(&kdl_config, "hooks")? {
            let parsed_hooks = hooks_from_kdl(kdl_hooks, &mut config.hooks)?;
            config.hooks.merge(parsed_hooks);
        }
        if let Some(kdl_ui_config) = single_top_level_block(&kdl_config, "ui")? {
            let config_ui = UiConfig::from_kdl(kdl_ui_config)?;
            config.ui = config.ui.merge(config_ui);
        }
        if let Some(env_config) = single_top_level_block(&kdl_config, "env")? {
            let config_env = environment_variables_from_kdl(env_config)?;
            config.env = config.env.merge(config_env);
        }
        Ok(config)
    }
    pub fn to_string(&self, add_comments: bool) -> String {
        let mut document = KdlDocument::new();

        let clear_defaults = true;
        let keybinds = self.keybinds.to_kdl(clear_defaults);
        document.nodes_mut().push(keybinds);

        if let Some(themes) = self.themes.to_kdl() {
            document.nodes_mut().push(themes);
        }

        let plugins = self.plugins.to_kdl(add_comments);
        document.nodes_mut().push(plugins);

        let load_plugins = load_plugins_to_kdl(
            &self.background_plugins,
            self.background_plugins_cleared,
            add_comments,
        );
        document.nodes_mut().push(load_plugins);

        document.nodes_mut().push(hooks_to_kdl(&self.hooks));
        document
            .nodes_mut()
            .push(context_menu_to_kdl(&self.context_menu));

        if let Some(ui_config) = self.ui.to_kdl() {
            document.nodes_mut().push(ui_config);
        }

        if let Some(env) = environment_variables_to_kdl(&self.env) {
            document.nodes_mut().push(env);
        }

        document
            .nodes_mut()
            .append(&mut self.options.to_kdl(add_comments));

        document.to_string()
    }
}

impl PluginAliases {
    #[allow(clippy::result_large_err)]
    pub fn from_kdl(kdl_plugin_aliases: &KdlNode) -> Result<PluginAliases, ConfigError> {
        let mut aliases: BTreeMap<String, RunPlugin> = BTreeMap::new();
        if let Some(kdl_plugin_aliases) = kdl_children_nodes!(kdl_plugin_aliases) {
            for alias_definition in kdl_plugin_aliases {
                let alias_name = kdl_name!(alias_definition);
                if let Some(string_url) =
                    kdl_get_string_property_or_child_value!(alias_definition, "location")
                {
                    let configuration =
                        KdlLayoutParser::parse_plugin_user_configuration(alias_definition)?;
                    let initial_cwd =
                        kdl_get_string_property_or_child_value!(alias_definition, "cwd")
                            .map(PathBuf::from);
                    let run_plugin = RunPlugin::from_url(string_url)?
                        .with_configuration(configuration.inner().clone())
                        .with_initial_cwd(initial_cwd);
                    aliases.insert(alias_name.to_owned(), run_plugin);
                }
            }
        }
        Ok(PluginAliases { aliases })
    }
    pub fn to_kdl(&self, add_comments: bool) -> KdlNode {
        let mut plugins = KdlNode::new("plugins");
        let mut plugins_children = KdlDocument::new();
        for (alias_name, plugin_alias) in self.aliases.iter() {
            let mut plugin_alias_node = KdlNode::new(alias_name.clone());
            let mut plugin_alias_children = KdlDocument::new();
            let location_string = plugin_alias.location.display();

            plugin_alias_node.insert("location", location_string);
            let cwd = plugin_alias.initial_cwd.as_ref();
            let mut has_children = false;
            if let Some(cwd) = cwd {
                has_children = true;
                let mut cwd_node = KdlNode::new("cwd");
                cwd_node.push(cwd.display().to_string());
                plugin_alias_children.nodes_mut().push(cwd_node);
            }
            let configuration = plugin_alias.configuration.inner();
            if !configuration.is_empty() {
                has_children = true;
                for (config_key, config_value) in configuration {
                    let mut node = KdlNode::new(config_key.to_owned());
                    if config_value == "true" {
                        node.push(KdlValue::Bool(true));
                    } else if config_value == "false" {
                        node.push(KdlValue::Bool(false));
                    } else {
                        node.push(config_value.to_string());
                    }
                    plugin_alias_children.nodes_mut().push(node);
                }
            }
            if has_children {
                plugin_alias_node.set_children(plugin_alias_children);
            }
            plugins_children.nodes_mut().push(plugin_alias_node);
        }
        plugins.set_children(plugins_children);

        if add_comments {
            plugins.set_leading(format!(
                "\n{}\n{}\n",
                "// Plugin aliases - can be used to change the implementation of Zellij",
                "// changing these requires a restart to take effect",
            ));
        }
        plugins
    }
}

/// The `cwd` and user-configuration children shared by every place a plugin is written as a
/// KDL node (`load_plugins`, `hooks`). `None` when the plugin has nothing to write, so the
/// caller can leave the node childless.
fn plugin_settings_to_kdl(run_plugin_or_alias: &RunPluginOrAlias) -> Option<KdlDocument> {
    let mut children = KdlDocument::new();

    let cwd = match run_plugin_or_alias {
        RunPluginOrAlias::RunPlugin(run_plugin) => run_plugin.initial_cwd.clone(),
        RunPluginOrAlias::Alias(plugin_alias) => plugin_alias.initial_cwd.clone(),
    };
    if let Some(cwd) = cwd.as_ref() {
        let mut cwd_node = KdlNode::new("cwd");
        cwd_node.push(cwd.display().to_string());
        children.nodes_mut().push(cwd_node);
    }
    let configuration = match run_plugin_or_alias {
        RunPluginOrAlias::RunPlugin(run_plugin) => Some(run_plugin.configuration.inner().clone()),
        RunPluginOrAlias::Alias(plugin_alias) => plugin_alias
            .configuration
            .as_ref()
            .map(|c| c.inner().clone()),
    };
    if let Some(configuration) = configuration {
        for (config_key, config_value) in configuration {
            // `cwd` is not a reserved plugin property, so parsing puts it in the configuration
            // as well as in `initial_cwd`. Writing both would duplicate the node, and the copy
            // would multiply on every round trip.
            if config_key == "cwd" && cwd.is_some() {
                continue;
            }
            let mut node = KdlNode::new(config_key.to_owned());
            if config_value == "true" {
                node.push(KdlValue::Bool(true));
            } else if config_value == "false" {
                node.push(KdlValue::Bool(false));
            } else {
                node.push(config_value.to_string());
            }
            children.nodes_mut().push(node);
        }
    }

    if children.nodes().is_empty() {
        None
    } else {
        Some(children)
    }
}

pub fn load_plugins_to_kdl(
    background_plugins: &BTreeSet<RunPluginOrAlias>,
    cleared: bool,
    add_comments: bool,
) -> KdlNode {
    let mut load_plugins = KdlNode::new("load_plugins");
    if cleared {
        load_plugins.insert("clear-defaults", true);
    }
    let mut load_plugins_children = KdlDocument::new();
    for run_plugin_or_alias in background_plugins.iter() {
        let mut background_plugin_node = KdlNode::new(run_plugin_or_alias.location_string());
        if let Some(children) = plugin_settings_to_kdl(run_plugin_or_alias) {
            background_plugin_node.set_children(children);
        }
        load_plugins_children
            .nodes_mut()
            .push(background_plugin_node);
    }
    load_plugins.set_children(load_plugins_children);

    if add_comments {
        load_plugins.set_leading(format!(
            "\n{}\n{}\n{}\n",
            "// Plugins to load in the background when a new session starts",
            "// eg. \"file:/path/to/my-plugin.wasm\"",
            "// eg. \"https://example.com/my-plugin.wasm\"",
        ));
    }
    load_plugins
}

/// Parse a `hooks { }` block.
///
/// A hook point takes one plugin url. The empty string unbinds it, which is how a user turns
/// off a hook that has a default - there is nothing else to write there that means "none", and
/// deleting the line would only fall back to the default again.
///
/// Unbinding has to be applied to the config being built rather than returned, because the
/// result is merged additively over it and an absent key means "unchanged", not "removed".
#[allow(clippy::result_large_err)]
fn hooks_from_kdl(kdl_hooks: &KdlNode, config_hooks: &mut Hooks) -> Result<Hooks, ConfigError> {
    let mut hooks = Hooks::from_data(BTreeMap::new());
    if let Some(kdl_hooks) = kdl_children_nodes!(kdl_hooks) {
        for hook_node in kdl_hooks {
            let name_node = hook_node.name();
            let hook_point = HookPoint::from_str(name_node.value()).map_err(|e| {
                ConfigError::new_kdl_error(e, name_node.span().offset(), name_node.span().len())
            })?;
            let string_url = kdl_first_entry_as_string!(hook_node).ok_or_else(|| {
                ConfigError::new_kdl_error(
                    format!(
                        "hook '{}' needs a plugin url, eg. `{} \"file:~/hook.lua\"`",
                        hook_point, hook_point
                    ),
                    name_node.span().offset(),
                    name_node.span().len(),
                )
            })?;
            if string_url.trim().is_empty() {
                config_hooks.clear(hook_point);
                continue;
            }
            let configuration = KdlLayoutParser::parse_plugin_user_configuration(hook_node)?;
            let cwd = kdl_get_string_property_or_child_value!(&hook_node, "cwd").map(PathBuf::from);
            let run_plugin_or_alias = RunPluginOrAlias::from_url(
                string_url,
                &Some(configuration.inner().clone()),
                None,
                cwd.clone(),
            )
            .map_err(|e| {
                ConfigError::new_kdl_error(
                    format!("Failed to parse hook plugin: {}", e),
                    name_node.span().offset(),
                    name_node.span().len(),
                )
            })?
            .with_initial_cwd(cwd);
            hooks.set(hook_point, run_plugin_or_alias);
        }
    }
    Ok(hooks)
}

/// Parse a `context_menu { }` block.
///
/// Each child is one item: its name is the label, its children are actions. `separator` is the one
/// reserved name, and takes no children.
///
/// The block *replaces* the default menu rather than adding to it - see `input::context_menu` for
/// why - so an empty block is meaningful and is not an error.
#[allow(clippy::result_large_err)]
fn context_menu_from_kdl(
    kdl_context_menu: &KdlNode,
    config_options: &Options,
) -> Result<ContextMenu, ConfigError> {
    let mut items = vec![];
    if let Some(item_nodes) = kdl_children_nodes!(kdl_context_menu) {
        for item_node in item_nodes {
            let name_node = item_node.name();
            let label = name_node.value();
            if label == "separator" {
                if item_node.children().is_some() || !item_node.entries().is_empty() {
                    return Err(ConfigError::new_kdl_error(
                        "'separator' takes no children or properties".to_string(),
                        name_node.span().offset(),
                        name_node.span().len(),
                    ));
                }
                items.push(ContextMenuItem::separator());
                continue;
            }
            // An item with no actions can never do anything, and is far more likely to be a
            // half-written line than a deliberate no-op - so it is named rather than drawn.
            let actions: Vec<Action> = kdl_children_nodes!(item_node)
                .map(|action_nodes| {
                    action_nodes
                        .iter()
                        .map(|action_node| Action::try_from((action_node, config_options)))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?
                .unwrap_or_default();
            if actions.is_empty() {
                return Err(ConfigError::new_kdl_error(
                    format!(
                        "context menu item '{label}' has no actions. Give it one, eg. \
                         `\"{label}\" {{ Copy; }}`, or write `separator` for a divider"
                    ),
                    name_node.span().offset(),
                    name_node.span().len(),
                ));
            }
            let mut item = ContextMenuItem::new(label, actions);
            if let Some(shortcut) = kdl_get_string_property_or_child_value!(&item_node, "shortcut")
            {
                item = item.with_shortcut(shortcut);
            }
            items.push(item);
        }
    }
    Ok(ContextMenu::from_items(items))
}

pub fn context_menu_to_kdl(context_menu: &ContextMenu) -> KdlNode {
    let mut menu_node = KdlNode::new("context_menu");
    let mut children = KdlDocument::new();
    for item in context_menu.items() {
        if item.separator {
            children.nodes_mut().push(KdlNode::new("separator"));
            continue;
        }
        let mut item_node = KdlNode::new(item.label.as_str());
        if let Some(shortcut) = item.shortcut.as_ref() {
            item_node.push(("shortcut", KdlValue::String(shortcut.clone())));
        }
        let mut actions = KdlDocument::new();
        for action in &item.actions {
            // An action with no KDL spelling is dropped rather than written wrong - the same rule
            // keybinding serialization already follows.
            if let Some(action_node) = action.to_kdl() {
                actions.nodes_mut().push(action_node);
            }
        }
        item_node.set_children(actions);
        children.nodes_mut().push(item_node);
    }
    menu_node.set_children(children);
    menu_node
}

pub fn hooks_to_kdl(hooks: &Hooks) -> KdlNode {
    let mut hooks_node = KdlNode::new("hooks");
    let mut children = KdlDocument::new();
    for point in HookPoint::ALL {
        // Every known point is written, bound or not, so that a round trip preserves an
        // unbinding: an absent line would be read back as "use the default".
        let mut node = KdlNode::new(point.as_str());
        match hooks.get(*point) {
            Some(plugin) => {
                node.push(KdlValue::String(plugin.location_string()));
                if let Some(children) = plugin_settings_to_kdl(plugin) {
                    node.set_children(children);
                }
            },
            None => node.push(KdlValue::String(String::new())),
        }
        children.nodes_mut().push(node);
    }
    hooks_node.set_children(children);
    hooks_node
}

#[allow(clippy::result_large_err)]
fn load_plugins_from_kdl(
    kdl_load_plugins: &KdlNode,
) -> Result<BTreeSet<RunPluginOrAlias>, ConfigError> {
    let mut load_plugins: BTreeSet<RunPluginOrAlias> = BTreeSet::new();
    if let Some(kdl_load_plugins) = kdl_children_nodes!(kdl_load_plugins) {
        for plugin_block in kdl_load_plugins {
            let url_node = plugin_block.name();
            let string_url = url_node.value();
            let configuration = KdlLayoutParser::parse_plugin_user_configuration(plugin_block)?;
            let cwd =
                kdl_get_string_property_or_child_value!(&plugin_block, "cwd").map(PathBuf::from);
            let run_plugin_or_alias = RunPluginOrAlias::from_url(
                string_url,
                &Some(configuration.inner().clone()),
                None,
                cwd.clone(),
            )
            .map_err(|e| {
                ConfigError::new_kdl_error(
                    format!("Failed to parse plugin: {}", e),
                    url_node.span().offset(),
                    url_node.span().len(),
                )
            })?
            .with_initial_cwd(cwd);
            load_plugins.insert(run_plugin_or_alias);
        }
    }
    Ok(load_plugins)
}

impl UiConfig {
    #[allow(clippy::result_large_err)]
    pub fn from_kdl(kdl_ui_config: &KdlNode) -> Result<UiConfig, ConfigError> {
        let mut ui_config = UiConfig::default();
        if let Some(pane_frames) = kdl_get_child!(kdl_ui_config, "pane_frames") {
            let rounded_corners =
                kdl_children_property_first_arg_as_bool!(pane_frames, "rounded_corners")
                    .unwrap_or(false);
            let hide_session_name =
                kdl_get_child_entry_bool_value!(pane_frames, "hide_session_name").unwrap_or(false);
            let frame_config = FrameConfig {
                rounded_corners,
                hide_session_name,
            };
            ui_config.pane_frames = frame_config;
        }
        Ok(ui_config)
    }
    pub fn to_kdl(&self) -> Option<KdlNode> {
        let mut ui_config = KdlNode::new("ui");
        let mut ui_config_children = KdlDocument::new();
        let mut frame_config = KdlNode::new("pane_frames");
        let mut frame_config_children = KdlDocument::new();
        let mut has_ui_config = false;
        if self.pane_frames.rounded_corners {
            has_ui_config = true;
            let mut rounded_corners = KdlNode::new("rounded_corners");
            rounded_corners.push(KdlValue::Bool(true));
            frame_config_children.nodes_mut().push(rounded_corners);
        }
        if self.pane_frames.hide_session_name {
            has_ui_config = true;
            let mut hide_session_name = KdlNode::new("hide_session_name");
            hide_session_name.push(KdlValue::Bool(true));
            frame_config_children.nodes_mut().push(hide_session_name);
        }
        if has_ui_config {
            frame_config.set_children(frame_config_children);
            ui_config_children.nodes_mut().push(frame_config);
            ui_config.set_children(ui_config_children);
            Some(ui_config)
        } else {
            None
        }
    }
}

impl Themes {
    #[allow(clippy::result_large_err)]
    fn style_declaration_from_node(
        style_node: &KdlNode,
        style_descriptor: &str,
    ) -> Result<Option<StyleDeclaration>, ConfigError> {
        let descriptor_node = kdl_child_with_name!(style_node, style_descriptor);

        match descriptor_node {
            Some(descriptor) => {
                let colors = kdl_children_or_error!(
                    descriptor,
                    format!("Missing colors for {}", style_descriptor)
                );
                Ok(Some(StyleDeclaration {
                    base: crate::kdl::palette_color_from_kdl("base", colors)?,
                    background: crate::kdl::palette_color_from_kdl("background", colors)
                        .unwrap_or_default(),
                    emphasis_0: crate::kdl::palette_color_from_kdl("emphasis_0", colors)?,
                    emphasis_1: crate::kdl::palette_color_from_kdl("emphasis_1", colors)?,
                    emphasis_2: crate::kdl::palette_color_from_kdl("emphasis_2", colors)?,
                    emphasis_3: crate::kdl::palette_color_from_kdl("emphasis_3", colors)?,
                }))
            },
            None => Ok(None),
        }
    }

    #[allow(clippy::result_large_err)]
    fn multiplayer_colors(style_node: &KdlNode) -> Result<MultiplayerColors, ConfigError> {
        let descriptor_node = kdl_child_with_name!(style_node, "multiplayer_user_colors");
        match descriptor_node {
            Some(descriptor) => {
                let colors = kdl_children_or_error!(
                    descriptor,
                    format!("Missing colors for {}", "multiplayer_user_colors")
                );
                Ok(MultiplayerColors {
                    player_1: crate::kdl::palette_color_from_kdl("player_1", colors)
                        .unwrap_or(DEFAULT_STYLES.multiplayer_user_colors.player_1),
                    player_2: crate::kdl::palette_color_from_kdl("player_2", colors)
                        .unwrap_or(DEFAULT_STYLES.multiplayer_user_colors.player_2),
                    player_3: crate::kdl::palette_color_from_kdl("player_3", colors)
                        .unwrap_or(DEFAULT_STYLES.multiplayer_user_colors.player_3),
                    player_4: crate::kdl::palette_color_from_kdl("player_4", colors)
                        .unwrap_or(DEFAULT_STYLES.multiplayer_user_colors.player_4),
                    player_5: crate::kdl::palette_color_from_kdl("player_5", colors)
                        .unwrap_or(DEFAULT_STYLES.multiplayer_user_colors.player_5),
                    player_6: crate::kdl::palette_color_from_kdl("player_6", colors)
                        .unwrap_or(DEFAULT_STYLES.multiplayer_user_colors.player_6),
                    player_7: crate::kdl::palette_color_from_kdl("player_7", colors)
                        .unwrap_or(DEFAULT_STYLES.multiplayer_user_colors.player_7),
                    player_8: crate::kdl::palette_color_from_kdl("player_8", colors)
                        .unwrap_or(DEFAULT_STYLES.multiplayer_user_colors.player_8),
                    player_9: crate::kdl::palette_color_from_kdl("player_9", colors)
                        .unwrap_or(DEFAULT_STYLES.multiplayer_user_colors.player_9),
                    player_10: crate::kdl::palette_color_from_kdl("player_10", colors)
                        .unwrap_or(DEFAULT_STYLES.multiplayer_user_colors.player_10),
                })
            },
            None => Ok(DEFAULT_STYLES.multiplayer_user_colors),
        }
    }

    #[allow(clippy::result_large_err)]
    pub fn from_kdl(
        themes_from_kdl: &KdlNode,
        sourced_from_external_file: bool,
    ) -> Result<Self, ConfigError> {
        let mut themes: HashMap<String, Theme> = HashMap::new();
        for theme_config in kdl_children_nodes_or_error!(themes_from_kdl, "no themes found") {
            let theme_name = kdl_name!(theme_config);
            let theme_colors = kdl_children_or_error!(theme_config, "empty theme");
            let palette_color_names = HashSet::from([
                "fg", "bg", "red", "green", "blue", "yellow", "magenta", "orange", "cyan", "black",
                "white",
            ]);
            let theme = if theme_colors
                .nodes()
                .iter()
                .all(|n| palette_color_names.contains(n.name().value()))
            {
                // Older palette based theme definition
                let palette = Palette {
                    fg: crate::kdl::palette_color_from_kdl("fg", theme_colors)?,
                    bg: crate::kdl::palette_color_from_kdl("bg", theme_colors)?,
                    red: crate::kdl::palette_color_from_kdl("red", theme_colors)?,
                    green: crate::kdl::palette_color_from_kdl("green", theme_colors)?,
                    yellow: crate::kdl::palette_color_from_kdl("yellow", theme_colors)?,
                    blue: crate::kdl::palette_color_from_kdl("blue", theme_colors)?,
                    magenta: crate::kdl::palette_color_from_kdl("magenta", theme_colors)?,
                    orange: crate::kdl::palette_color_from_kdl("orange", theme_colors)?,
                    cyan: crate::kdl::palette_color_from_kdl("cyan", theme_colors)?,
                    black: crate::kdl::palette_color_from_kdl("black", theme_colors)?,
                    white: crate::kdl::palette_color_from_kdl("white", theme_colors)?,
                    ..Default::default()
                };
                Theme {
                    palette: palette.into(),
                    sourced_from_external_file,
                }
            } else {
                // Newer theme definition with named styles
                let s = Styling {
                    text_unselected: Themes::style_declaration_from_node(
                        theme_config,
                        "text_unselected",
                    )
                    .map(|maybe_style| maybe_style.unwrap_or(DEFAULT_STYLES.text_unselected))?,
                    text_selected: Themes::style_declaration_from_node(
                        theme_config,
                        "text_selected",
                    )
                    .map(|maybe_style| maybe_style.unwrap_or(DEFAULT_STYLES.text_selected))?,
                    ribbon_unselected: Themes::style_declaration_from_node(
                        theme_config,
                        "ribbon_unselected",
                    )
                    .map(|maybe_style| maybe_style.unwrap_or(DEFAULT_STYLES.ribbon_unselected))?,
                    ribbon_selected: Themes::style_declaration_from_node(
                        theme_config,
                        "ribbon_selected",
                    )
                    .map(|maybe_style| maybe_style.unwrap_or(DEFAULT_STYLES.ribbon_selected))?,
                    table_title: Themes::style_declaration_from_node(theme_config, "table_title")
                        .map(|maybe_style| {
                        maybe_style.unwrap_or(DEFAULT_STYLES.table_title)
                    })?,
                    table_cell_unselected: Themes::style_declaration_from_node(
                        theme_config,
                        "table_cell_unselected",
                    )
                    .map(|maybe_style| {
                        maybe_style.unwrap_or(DEFAULT_STYLES.table_cell_unselected)
                    })?,
                    table_cell_selected: Themes::style_declaration_from_node(
                        theme_config,
                        "table_cell_selected",
                    )
                    .map(|maybe_style| maybe_style.unwrap_or(DEFAULT_STYLES.table_cell_selected))?,
                    list_unselected: Themes::style_declaration_from_node(
                        theme_config,
                        "list_unselected",
                    )
                    .map(|maybe_style| maybe_style.unwrap_or(DEFAULT_STYLES.list_unselected))?,
                    list_selected: Themes::style_declaration_from_node(
                        theme_config,
                        "list_selected",
                    )
                    .map(|maybe_style| maybe_style.unwrap_or(DEFAULT_STYLES.list_selected))?,
                    frame_unselected: Themes::style_declaration_from_node(
                        theme_config,
                        "frame_unselected",
                    )?,
                    frame_selected: Themes::style_declaration_from_node(
                        theme_config,
                        "frame_selected",
                    )
                    .map(|maybe_style| maybe_style.unwrap_or(DEFAULT_STYLES.frame_selected))?,
                    frame_highlight: Themes::style_declaration_from_node(
                        theme_config,
                        "frame_highlight",
                    )
                    .map(|maybe_style| maybe_style.unwrap_or(DEFAULT_STYLES.frame_highlight))?,
                    exit_code_success: Themes::style_declaration_from_node(
                        theme_config,
                        "exit_code_success",
                    )
                    .map(|maybe_style| maybe_style.unwrap_or(DEFAULT_STYLES.exit_code_success))?,
                    exit_code_error: Themes::style_declaration_from_node(
                        theme_config,
                        "exit_code_error",
                    )
                    .map(|maybe_style| maybe_style.unwrap_or(DEFAULT_STYLES.exit_code_error))?,
                    multiplayer_user_colors: Themes::multiplayer_colors(theme_config)
                        .unwrap_or_default(),
                };

                Theme {
                    palette: s,
                    sourced_from_external_file,
                }
            };
            themes.insert(theme_name.into(), theme);
        }
        let themes = Themes::from_data(themes);
        Ok(themes)
    }

    #[allow(clippy::result_large_err)]
    pub fn from_string(
        raw_string: &str,
        sourced_from_external_file: bool,
    ) -> Result<Self, ConfigError> {
        let kdl_config: KdlDocument = raw_string.parse()?;
        let kdl_themes =
            single_top_level_block(&kdl_config, "themes")?.ok_or(ConfigError::new_kdl_error(
                "No theme node found in file".into(),
                kdl_config.span().offset(),
                kdl_config.span().len(),
            ))?;
        let all_themes_in_file = Themes::from_kdl(kdl_themes, sourced_from_external_file)?;
        Ok(all_themes_in_file)
    }

    #[allow(clippy::result_large_err)]
    pub fn from_path(path_to_theme_file: PathBuf) -> Result<Self, ConfigError> {
        // String is the theme name
        let kdl_config = std::fs::read_to_string(&path_to_theme_file)
            .map_err(|e| ConfigError::IoPath(e, path_to_theme_file.clone()))?;
        let sourced_from_external_file = true;
        Themes::from_string(&kdl_config, sourced_from_external_file).map_err(|e| match e {
            ConfigError::KdlError(kdl_error) => ConfigError::KdlError(
                kdl_error.add_src(path_to_theme_file.display().to_string(), kdl_config),
            ),
            e => e,
        })
    }

    #[allow(clippy::result_large_err)]
    pub fn from_dir(path_to_theme_dir: PathBuf) -> Result<Self, ConfigError> {
        let mut themes = Themes::default();
        for entry in std::fs::read_dir(&path_to_theme_dir)
            .map_err(|e| ConfigError::IoPath(e, path_to_theme_dir.clone()))?
        {
            let entry = entry.map_err(|e| ConfigError::IoPath(e, path_to_theme_dir.clone()))?;
            let path = entry.path();
            if let Some(extension) = path.extension() {
                if extension == "kdl" {
                    themes = themes.merge(Themes::from_path(path)?);
                }
            }
        }
        Ok(themes)
    }
    pub fn to_kdl(&self) -> Option<KdlNode> {
        let mut theme_node = KdlNode::new("themes");
        let mut themes = KdlDocument::new();
        let mut has_themes = false;
        let sorted_themes: BTreeMap<String, Theme> = self.inner().clone().into_iter().collect();
        for (theme_name, theme) in sorted_themes {
            if theme.sourced_from_external_file {
                // we do not serialize themes that have been defined in external files so as not to
                // clog up the configuration file definitions
                continue;
            }
            has_themes = true;
            let mut current_theme_node = KdlNode::new(theme_name.clone());
            let mut current_theme_node_children = KdlDocument::new();

            current_theme_node_children
                .nodes_mut()
                .push(style_declaration_to_kdl(
                    &theme.palette.text_unselected,
                    "text_unselected",
                ));
            current_theme_node_children
                .nodes_mut()
                .push(style_declaration_to_kdl(
                    &theme.palette.text_selected,
                    "text_selected",
                ));
            current_theme_node_children
                .nodes_mut()
                .push(style_declaration_to_kdl(
                    &theme.palette.ribbon_selected,
                    "ribbon_selected",
                ));
            current_theme_node_children
                .nodes_mut()
                .push(style_declaration_to_kdl(
                    &theme.palette.ribbon_unselected,
                    "ribbon_unselected",
                ));
            current_theme_node_children
                .nodes_mut()
                .push(style_declaration_to_kdl(
                    &theme.palette.table_title,
                    "table_title",
                ));
            current_theme_node_children
                .nodes_mut()
                .push(style_declaration_to_kdl(
                    &theme.palette.table_cell_selected,
                    "table_cell_selected",
                ));
            current_theme_node_children
                .nodes_mut()
                .push(style_declaration_to_kdl(
                    &theme.palette.table_cell_unselected,
                    "table_cell_unselected",
                ));
            current_theme_node_children
                .nodes_mut()
                .push(style_declaration_to_kdl(
                    &theme.palette.list_selected,
                    "list_selected",
                ));
            current_theme_node_children
                .nodes_mut()
                .push(style_declaration_to_kdl(
                    &theme.palette.list_unselected,
                    "list_unselected",
                ));
            current_theme_node_children
                .nodes_mut()
                .push(style_declaration_to_kdl(
                    &theme.palette.frame_selected,
                    "frame_selected",
                ));

            match theme.palette.frame_unselected {
                None => {},
                Some(frame_unselected_style) => {
                    current_theme_node_children
                        .nodes_mut()
                        .push(style_declaration_to_kdl(
                            &frame_unselected_style,
                            "frame_unselected",
                        ));
                },
            }
            current_theme_node_children
                .nodes_mut()
                .push(style_declaration_to_kdl(
                    &theme.palette.frame_highlight,
                    "frame_highlight",
                ));
            current_theme_node_children
                .nodes_mut()
                .push(style_declaration_to_kdl(
                    &theme.palette.exit_code_success,
                    "exit_code_success",
                ));
            current_theme_node_children
                .nodes_mut()
                .push(style_declaration_to_kdl(
                    &theme.palette.exit_code_error,
                    "exit_code_error",
                ));
            current_theme_node_children
                .nodes_mut()
                .push(multiplayer_colors_to_kdl(
                    &theme.palette.multiplayer_user_colors,
                ));
            current_theme_node.set_children(current_theme_node_children);
            themes.nodes_mut().push(current_theme_node);
        }
        if has_themes {
            theme_node.set_children(themes);
            Some(theme_node)
        } else {
            None
        }
    }
}

impl PermissionCache {
    #[allow(clippy::result_large_err)]
    pub fn from_string(raw_string: String) -> Result<GrantedPermission, ConfigError> {
        let kdl_document: KdlDocument = raw_string.parse()?;

        let mut granted_permission = GrantedPermission::default();

        for node in kdl_document.nodes() {
            if let Some(children) = node.children() {
                let key = kdl_name!(node);
                let permissions: Vec<PermissionType> = children
                    .nodes()
                    .iter()
                    .filter_map(|p| {
                        let v = kdl_name!(p);
                        PermissionType::from_str(v).ok()
                    })
                    .collect();

                granted_permission.insert(key.into(), permissions);
            }
        }

        Ok(granted_permission)
    }

    pub fn to_string(granted: &GrantedPermission) -> String {
        let mut kdl_doucment = KdlDocument::new();

        granted.iter().for_each(|(k, v)| {
            let mut node = KdlNode::new(k.as_str());
            let mut children = KdlDocument::new();

            let permissions: HashSet<PermissionType> = v.clone().into_iter().collect();
            permissions.iter().for_each(|f| {
                let n = KdlNode::new(f.to_string().as_str());
                children.nodes_mut().push(n);
            });

            node.set_children(children);
            kdl_doucment.nodes_mut().push(node);
        });

        kdl_doucment.fmt();
        kdl_doucment.to_string()
    }
}

impl SessionInfo {
    pub fn from_string(raw_session_info: &str, current_session_name: &str) -> Result<Self, String> {
        let kdl_document: KdlDocument = raw_session_info
            .parse()
            .map_err(|e| format!("Failed to parse kdl document: {}", e))?;
        let name = kdl_document
            .get("name")
            .and_then(|n| n.entries().iter().next())
            .and_then(|e| e.value().as_string())
            .map(|s| s.to_owned())
            .ok_or("Failed to parse session name")?;
        let connected_clients = kdl_document
            .get("connected_clients")
            .and_then(|n| n.entries().iter().next())
            .and_then(|e| e.value().as_i64())
            .map(|c| c as usize)
            .ok_or("Failed to parse connected_clients")?;
        let tabs: Vec<TabInfo> = kdl_document
            .get("tabs")
            .and_then(|t| t.children())
            .and_then(|c| {
                let mut tab_nodes = vec![];
                for tab_node in c.nodes() {
                    if let Some(tab) = tab_node.children() {
                        tab_nodes.push(TabInfo::decode_from_kdl(tab).ok()?);
                    }
                }
                Some(tab_nodes)
            })
            .ok_or("Failed to parse tabs")?;
        let panes: PaneManifest = kdl_document
            .get("panes")
            .and_then(|p| p.children())
            .map(PaneManifest::decode_from_kdl)
            .ok_or("Failed to parse panes")?;
        let available_layouts: Vec<LayoutInfo> = kdl_document
            .get("available_layouts")
            .and_then(|p| p.children())
            .map(|e| {
                e.nodes()
                    .iter()
                    .filter_map(|n| {
                        let layout_name = n.name().value().to_owned();
                        let layout_source = n
                            .entries()
                            .iter()
                            .find(|e| e.name().map(|n| n.value()) == Some("source"))
                            .and_then(|e| e.value().as_string());
                        match layout_source {
                            Some(layout_source) => match layout_source {
                                "built-in" => Some(LayoutInfo::BuiltIn(layout_name)),
                                "file" => {
                                    Some(LayoutInfo::File(layout_name, LayoutMetadata::default()))
                                },
                                _ => None,
                            },
                            None => None,
                        }
                    })
                    .collect()
            })
            .ok_or("Failed to parse available_layouts")?;
        let is_current_session = name == current_session_name;
        let mut tab_history = BTreeMap::new();
        if let Some(kdl_tab_history) = kdl_document.get("tab_history").and_then(|p| p.children()) {
            for client_node in kdl_tab_history.nodes() {
                if let Some(client_id) = client_node.children().and_then(|c| {
                    c.get("id")
                        .and_then(|c| c.entries().iter().next().and_then(|e| e.value().as_i64()))
                }) {
                    let mut history = vec![];
                    if let Some(history_entries) = client_node
                        .children()
                        .and_then(|c| c.get("history"))
                        .map(|h| h.entries())
                    {
                        for entry in history_entries {
                            if let Some(entry) = entry.value().as_i64() {
                                history.push(entry as usize);
                            }
                        }
                    }
                    tab_history.insert(
                        ClientId::try_from(client_id)
                            .map_err(|_| "client_id is outside u16 range")?,
                        history,
                    );
                }
            }
        }
        let mut pane_history = BTreeMap::new();
        if let Some(kdl_pane_history) = kdl_document.get("pane_history").and_then(|p| p.children())
        {
            for client_node in kdl_pane_history.nodes() {
                if let Some(client_id) = client_node.children().and_then(|c| {
                    c.get("id")
                        .and_then(|c| c.entries().iter().next().and_then(|e| e.value().as_i64()))
                }) {
                    let mut history = vec![];
                    if let Some(history_node) =
                        client_node.children().and_then(|c| c.get("history"))
                    {
                        if let Some(history_children) = history_node.children() {
                            for pane_id_node in history_children.nodes() {
                                if pane_id_node.name().value() == "pane_id" {
                                    let pane_type = pane_id_node
                                        .entries()
                                        .iter()
                                        .find(|e| e.name().map(|n| n.value()) == Some("type"))
                                        .and_then(|e| e.value().as_string());
                                    let id = pane_id_node
                                        .entries()
                                        .iter()
                                        .find(|e| e.name().is_none())
                                        .and_then(|e| e.value().as_i64())
                                        .map(|i| i as u32);
                                    if let (Some(pane_type), Some(id)) = (pane_type, id) {
                                        let pane_id = match pane_type {
                                            "terminal" => Some(PaneId::Terminal(
                                                zellij_types::internal_ids::TerminalId(id),
                                            )),
                                            "plugin" => Some(PaneId::Plugin(
                                                zellij_types::internal_ids::PluginId(id),
                                            )),
                                            _ => None,
                                        };
                                        if let Some(pane_id) = pane_id {
                                            history.push(pane_id);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    pane_history.insert(
                        ClientId::try_from(client_id)
                            .map_err(|_| "client_id is outside u16 range")?,
                        history,
                    );
                }
            }
        }
        let creation_time = kdl_document
            .get("creation_time")
            .and_then(|n| n.entries().iter().next())
            .and_then(|e| e.value().as_i64())
            .map(|c| Duration::from_secs(c as u64))
            .unwrap_or_default();
        Ok(SessionInfo {
            name,
            tabs,
            panes,
            connected_clients,
            is_current_session,
            available_layouts,
            plugins: Default::default(), // we do not serialize plugin information
            tab_history,
            pane_history,
            creation_time,
        })
    }
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        let mut kdl_document = KdlDocument::new();

        let mut name = KdlNode::new("name");
        name.push(self.name.clone());

        let mut connected_clients = KdlNode::new("connected_clients");
        connected_clients.push(self.connected_clients as i64);

        let mut tabs = KdlNode::new("tabs");
        let mut tab_children = KdlDocument::new();
        for tab_info in &self.tabs {
            let mut tab = KdlNode::new("tab");
            let kdl_tab_info = tab_info.encode_to_kdl();
            tab.set_children(kdl_tab_info);
            tab_children.nodes_mut().push(tab);
        }
        tabs.set_children(tab_children);

        let mut panes = KdlNode::new("panes");
        panes.set_children(self.panes.encode_to_kdl());

        let mut available_layouts = KdlNode::new("available_layouts");
        let mut available_layouts_children = KdlDocument::new();
        for layout_info in &self.available_layouts {
            let (layout_name, layout_source) = match layout_info {
                LayoutInfo::File(name, _layout_metadata) => (name.clone(), "file"),
                LayoutInfo::BuiltIn(name) => (name.clone(), "built-in"),
                LayoutInfo::Url(url) => (url.clone(), "url"),
                LayoutInfo::Stringified(_stringified) => ("stringified-layout".to_owned(), "N/A"),
            };
            let mut layout_node = KdlNode::new(layout_name.to_string());
            let layout_source = KdlEntry::new_prop("source", layout_source);
            layout_node.entries_mut().push(layout_source);
            available_layouts_children.nodes_mut().push(layout_node);
        }
        available_layouts.set_children(available_layouts_children);

        let mut tab_history = KdlNode::new("tab_history");
        let mut tab_history_children = KdlDocument::new();
        for (client_id, client_tab_history) in &self.tab_history {
            let mut client_document = KdlDocument::new();
            let mut client_node = KdlNode::new("client");
            let mut id = KdlNode::new("id");
            id.push(i64::from(client_id.get()));
            client_document.nodes_mut().push(id);
            let mut history = KdlNode::new("history");
            for entry in client_tab_history {
                history.push(*entry as i64);
            }
            client_document.nodes_mut().push(history);
            client_node.set_children(client_document);
            tab_history_children.nodes_mut().push(client_node);
        }
        tab_history.set_children(tab_history_children);

        let mut pane_history = KdlNode::new("pane_history");
        let mut pane_history_children = KdlDocument::new();
        for (client_id, client_pane_history) in &self.pane_history {
            let mut client_document = KdlDocument::new();
            let mut client_node = KdlNode::new("client");
            let mut id = KdlNode::new("id");
            id.push(i64::from(client_id.get()));
            client_document.nodes_mut().push(id);
            let mut history = KdlNode::new("history");
            for pane_id in client_pane_history {
                let mut pane_id_node = KdlNode::new("pane_id");
                match pane_id {
                    PaneId::Terminal(id) => {
                        pane_id_node.push(KdlEntry::new_prop("type", "terminal"));
                        pane_id_node.push(i64::from(id.get()));
                    },
                    PaneId::Plugin(id) => {
                        pane_id_node.push(KdlEntry::new_prop("type", "plugin"));
                        pane_id_node.push(i64::from(id.get()));
                    },
                }
                history.ensure_children().nodes_mut().push(pane_id_node);
            }
            client_document.nodes_mut().push(history);
            client_node.set_children(client_document);
            pane_history_children.nodes_mut().push(client_node);
        }
        pane_history.set_children(pane_history_children);

        kdl_document.nodes_mut().push(name);
        kdl_document.nodes_mut().push(tabs);
        kdl_document.nodes_mut().push(panes);
        kdl_document.nodes_mut().push(connected_clients);
        kdl_document.nodes_mut().push(available_layouts);
        kdl_document.nodes_mut().push(tab_history);
        kdl_document.nodes_mut().push(pane_history);

        let mut creation_time_node = KdlNode::new("creation_time");
        creation_time_node.push(self.creation_time.as_secs() as i64);
        kdl_document.nodes_mut().push(creation_time_node);

        kdl_document.fmt();
        kdl_document.to_string()
    }
}

impl TabInfo {
    pub fn decode_from_kdl(kdl_document: &KdlDocument) -> Result<Self, String> {
        macro_rules! int_node {
            ($name:expr, $type:ident) => {{
                kdl_document
                    .get($name)
                    .and_then(|n| n.entries().iter().next())
                    .and_then(|e| e.value().as_i64())
                    .map(|e| e as $type)
                    .ok_or(format!("Failed to parse tab {}", $name))?
            }};
        }
        macro_rules! string_node {
            ($name:expr) => {{
                kdl_document
                    .get($name)
                    .and_then(|n| n.entries().iter().next())
                    .and_then(|e| e.value().as_string())
                    .map(|s| s.to_owned())
                    .ok_or(format!("Failed to parse tab {}", $name))?
            }};
        }
        macro_rules! optional_string_node {
            ($name:expr) => {{
                kdl_document
                    .get($name)
                    .and_then(|n| n.entries().iter().next())
                    .and_then(|e| e.value().as_string())
                    .map(|s| s.to_owned())
            }};
        }
        macro_rules! optional_int_node {
            ($name:expr, $type:ident) => {{
                kdl_document
                    .get($name)
                    .and_then(|n| n.entries().iter().next())
                    .and_then(|e| e.value().as_i64())
                    .map(|e| e as $type)
            }};
        }
        macro_rules! bool_node {
            ($name:expr) => {{
                kdl_document
                    .get($name)
                    .and_then(|n| n.entries().iter().next())
                    .and_then(|e| e.value().as_bool())
                    .ok_or(format!("Failed to parse tab {}", $name))?
            }};
        }

        let position = int_node!("position", usize);
        let name = string_node!("name");
        let active = bool_node!("active");
        let panes_to_hide = int_node!("panes_to_hide", usize);
        let is_fullscreen_active = bool_node!("is_fullscreen_active");
        let is_sync_panes_active = bool_node!("is_sync_panes_active");
        let are_floating_panes_visible = bool_node!("are_floating_panes_visible");
        let mut other_focused_clients = vec![];
        if let Some(tab_other_focused_clients) = kdl_document
            .get("other_focused_clients")
            .map(|n| n.entries())
        {
            for entry in tab_other_focused_clients {
                if let Some(entry_parsed) = entry.value().as_i64() {
                    other_focused_clients.push(
                        ClientId::try_from(entry_parsed)
                            .map_err(|_| "client_id is outside u16 range")?,
                    );
                }
            }
        }
        let active_swap_layout_name = optional_string_node!("active_swap_layout_name");
        let viewport_rows = optional_int_node!("viewport_rows", usize).unwrap_or(0);
        let viewport_columns = optional_int_node!("viewport_columns", usize).unwrap_or(0);
        let display_area_rows = optional_int_node!("display_area_rows", usize).unwrap_or(0);
        let display_area_columns = optional_int_node!("display_area_columns", usize).unwrap_or(0);
        let is_swap_layout_dirty = bool_node!("is_swap_layout_dirty");
        let selectable_tiled_panes_count =
            optional_int_node!("selectable_tiled_panes_count", usize).unwrap_or(0);
        let selectable_floating_panes_count =
            optional_int_node!("selectable_floating_panes_count", usize).unwrap_or(0);
        let tab_id = optional_int_node!("tab_id", usize).unwrap_or(0);
        Ok(TabInfo {
            position,
            name,
            active,
            panes_to_hide,
            is_fullscreen_active,
            is_sync_panes_active,
            are_floating_panes_visible,
            other_focused_clients,
            active_swap_layout_name,
            is_swap_layout_dirty,
            viewport_rows,
            viewport_columns,
            display_area_rows,
            display_area_columns,
            selectable_tiled_panes_count,
            selectable_floating_panes_count,
            tab_id,
            has_bell_notification: false,
            is_flashing_bell: false,
        })
    }
    pub fn encode_to_kdl(&self) -> KdlDocument {
        let mut kdl_doucment = KdlDocument::new();

        let mut position = KdlNode::new("position");
        position.push(self.position as i64);
        kdl_doucment.nodes_mut().push(position);

        let mut name = KdlNode::new("name");
        name.push(self.name.clone());
        kdl_doucment.nodes_mut().push(name);

        let mut active = KdlNode::new("active");
        active.push(self.active);
        kdl_doucment.nodes_mut().push(active);

        let mut panes_to_hide = KdlNode::new("panes_to_hide");
        panes_to_hide.push(self.panes_to_hide as i64);
        kdl_doucment.nodes_mut().push(panes_to_hide);

        let mut is_fullscreen_active = KdlNode::new("is_fullscreen_active");
        is_fullscreen_active.push(self.is_fullscreen_active);
        kdl_doucment.nodes_mut().push(is_fullscreen_active);

        let mut is_sync_panes_active = KdlNode::new("is_sync_panes_active");
        is_sync_panes_active.push(self.is_sync_panes_active);
        kdl_doucment.nodes_mut().push(is_sync_panes_active);

        let mut are_floating_panes_visible = KdlNode::new("are_floating_panes_visible");
        are_floating_panes_visible.push(self.are_floating_panes_visible);
        kdl_doucment.nodes_mut().push(are_floating_panes_visible);

        if !self.other_focused_clients.is_empty() {
            let mut other_focused_clients = KdlNode::new("other_focused_clients");
            for client_id in &self.other_focused_clients {
                other_focused_clients.push(i64::from(client_id.get()));
            }
            kdl_doucment.nodes_mut().push(other_focused_clients);
        }

        if let Some(active_swap_layout_name) = self.active_swap_layout_name.as_ref() {
            let mut active_swap_layout = KdlNode::new("active_swap_layout_name");
            active_swap_layout.push(active_swap_layout_name.to_string());
            kdl_doucment.nodes_mut().push(active_swap_layout);
        }

        let mut viewport_rows = KdlNode::new("viewport_rows");
        viewport_rows.push(self.viewport_rows as i64);
        kdl_doucment.nodes_mut().push(viewport_rows);

        let mut viewport_columns = KdlNode::new("viewport_columns");
        viewport_columns.push(self.viewport_columns as i64);
        kdl_doucment.nodes_mut().push(viewport_columns);

        let mut display_area_columns = KdlNode::new("display_area_columns");
        display_area_columns.push(self.display_area_columns as i64);
        kdl_doucment.nodes_mut().push(display_area_columns);

        let mut display_area_rows = KdlNode::new("display_area_rows");
        display_area_rows.push(self.display_area_rows as i64);
        kdl_doucment.nodes_mut().push(display_area_rows);

        let mut is_swap_layout_dirty = KdlNode::new("is_swap_layout_dirty");
        is_swap_layout_dirty.push(self.is_swap_layout_dirty);
        kdl_doucment.nodes_mut().push(is_swap_layout_dirty);

        let mut selectable_tiled_panes_count = KdlNode::new("selectable_tiled_panes_count");
        selectable_tiled_panes_count.push(self.selectable_tiled_panes_count as i64);
        kdl_doucment.nodes_mut().push(selectable_tiled_panes_count);

        let mut selectable_floating_panes_count = KdlNode::new("selectable_floating_panes_count");
        selectable_floating_panes_count.push(self.selectable_floating_panes_count as i64);
        kdl_doucment
            .nodes_mut()
            .push(selectable_floating_panes_count);

        let mut tab_id = KdlNode::new("tab_id");
        tab_id.push(self.tab_id as i64);
        kdl_doucment.nodes_mut().push(tab_id);

        kdl_doucment
    }
}

impl PaneManifest {
    pub fn decode_from_kdl(kdl_doucment: &KdlDocument) -> Self {
        let mut panes: HashMap<usize, Vec<PaneInfo>> = HashMap::new();
        for node in kdl_doucment.nodes() {
            if node.name().to_string() == "pane" {
                if let Some(pane_document) = node.children() {
                    if let Ok((Some(tab_position), pane_info)) =
                        PaneInfo::decode_from_kdl(pane_document)
                    {
                        let panes_in_tab_position = panes.entry(tab_position).or_default();
                        panes_in_tab_position.push(pane_info);
                    }
                }
            }
        }
        PaneManifest { panes }
    }
    pub fn encode_to_kdl(&self) -> KdlDocument {
        let mut kdl_doucment = KdlDocument::new();
        for (tab_position, panes) in &self.panes {
            for pane in panes {
                let mut pane_node = KdlNode::new("pane");
                let mut pane = pane.encode_to_kdl();

                let mut position_node = KdlNode::new("tab_position");
                position_node.push(*tab_position as i64);
                pane.nodes_mut().push(position_node);

                pane_node.set_children(pane);
                kdl_doucment.nodes_mut().push(pane_node);
            }
        }
        kdl_doucment
    }
}

impl PaneInfo {
    pub fn decode_from_kdl(kdl_document: &KdlDocument) -> Result<(Option<usize>, Self), String> {
        macro_rules! int_node {
            ($name:expr, $type:ident) => {{
                kdl_document
                    .get($name)
                    .and_then(|n| n.entries().iter().next())
                    .and_then(|e| e.value().as_i64())
                    .map(|e| e as $type)
                    .ok_or(format!("Failed to parse pane {}", $name))?
            }};
        }
        macro_rules! optional_int_node {
            ($name:expr, $type:ident) => {{
                kdl_document
                    .get($name)
                    .and_then(|n| n.entries().iter().next())
                    .and_then(|e| e.value().as_i64())
                    .map(|e| e as $type)
            }};
        }
        macro_rules! bool_node {
            ($name:expr) => {{
                kdl_document
                    .get($name)
                    .and_then(|n| n.entries().iter().next())
                    .and_then(|e| e.value().as_bool())
                    .ok_or(format!("Failed to parse pane {}", $name))?
            }};
        }
        macro_rules! string_node {
            ($name:expr) => {{
                kdl_document
                    .get($name)
                    .and_then(|n| n.entries().iter().next())
                    .and_then(|e| e.value().as_string())
                    .map(|s| s.to_owned())
                    .ok_or(format!("Failed to parse pane {}", $name))?
            }};
        }
        macro_rules! optional_string_node {
            ($name:expr) => {{
                kdl_document
                    .get($name)
                    .and_then(|n| n.entries().iter().next())
                    .and_then(|e| e.value().as_string())
                    .map(|s| s.to_owned())
            }};
        }
        let tab_position = optional_int_node!("tab_position", usize);
        let id = int_node!("id", u32);

        let is_plugin = bool_node!("is_plugin");
        let is_focused = bool_node!("is_focused");
        let is_fullscreen = bool_node!("is_fullscreen");
        let is_floating = bool_node!("is_floating");
        let is_suppressed = bool_node!("is_suppressed");
        let title = string_node!("title");
        let exited = bool_node!("exited");
        let exit_status = optional_int_node!("exit_status", i32);
        let is_held = bool_node!("is_held");
        let pane_x = int_node!("pane_x", usize);
        let pane_content_x = int_node!("pane_content_x", usize);
        let pane_y = int_node!("pane_y", usize);
        let pane_content_y = int_node!("pane_content_y", usize);
        let pane_rows = int_node!("pane_rows", usize);
        let pane_content_rows = int_node!("pane_content_rows", usize);
        let pane_columns = int_node!("pane_columns", usize);
        let pane_content_columns = int_node!("pane_content_columns", usize);
        let cursor_coordinates_in_pane = kdl_document
            .get("cursor_coordinates_in_pane")
            .map(|n| {
                let mut entries = n.entries().iter();
                (entries.next(), entries.next())
            })
            .and_then(|(x, y)| {
                let x = x.and_then(|x| x.value().as_i64()).map(|x| x as usize);
                let y = y.and_then(|y| y.value().as_i64()).map(|y| y as usize);
                match (x, y) {
                    (Some(x), Some(y)) => Some((x, y)),
                    _ => None,
                }
            });
        let terminal_command = optional_string_node!("terminal_command");
        let cwd = optional_string_node!("cwd");
        let plugin_url = optional_string_node!("plugin_url");
        let is_selectable = bool_node!("is_selectable");
        let pane_info = PaneInfo {
            host: None,
            target: Default::default(),
            agent: None,
            id,
            is_plugin,
            is_focused,
            is_fullscreen,
            is_floating,
            is_suppressed,
            title,
            exited,
            exit_status,
            is_held,
            pane_x,
            pane_content_x,
            pane_y,
            pane_content_y,
            pane_rows,
            pane_content_rows,
            pane_columns,
            pane_content_columns,
            cursor_coordinates_in_pane,
            terminal_command,
            plugin_url,
            is_selectable,
            index_in_pane_group: Default::default(), // we don't serialize this
            default_fg: None,
            default_bg: None,
            cwd,
        };
        Ok((tab_position, pane_info))
    }
    pub fn encode_to_kdl(&self) -> KdlDocument {
        let mut kdl_doucment = KdlDocument::new();
        macro_rules! int_node {
            ($name:expr, $val:expr) => {{
                let mut att = KdlNode::new($name);
                att.push($val as i64);
                kdl_doucment.nodes_mut().push(att);
            }};
        }
        macro_rules! bool_node {
            ($name:expr, $val:expr) => {{
                let mut att = KdlNode::new($name);
                att.push($val);
                kdl_doucment.nodes_mut().push(att);
            }};
        }
        macro_rules! string_node {
            ($name:expr, $val:expr) => {{
                let mut att = KdlNode::new($name);
                att.push($val);
                kdl_doucment.nodes_mut().push(att);
            }};
        }

        int_node!("id", self.id);
        bool_node!("is_plugin", self.is_plugin);
        bool_node!("is_focused", self.is_focused);
        bool_node!("is_fullscreen", self.is_fullscreen);
        bool_node!("is_floating", self.is_floating);
        bool_node!("is_suppressed", self.is_suppressed);
        string_node!("title", self.title.to_string());
        bool_node!("exited", self.exited);
        if let Some(exit_status) = self.exit_status {
            int_node!("exit_status", exit_status);
        }
        bool_node!("is_held", self.is_held);
        int_node!("pane_x", self.pane_x);
        int_node!("pane_content_x", self.pane_content_x);
        int_node!("pane_y", self.pane_y);
        int_node!("pane_content_y", self.pane_content_y);
        int_node!("pane_rows", self.pane_rows);
        int_node!("pane_content_rows", self.pane_content_rows);
        int_node!("pane_columns", self.pane_columns);
        int_node!("pane_content_columns", self.pane_content_columns);
        if let Some((cursor_x, cursor_y)) = self.cursor_coordinates_in_pane {
            let mut cursor_coordinates_in_pane = KdlNode::new("cursor_coordinates_in_pane");
            cursor_coordinates_in_pane.push(cursor_x as i64);
            cursor_coordinates_in_pane.push(cursor_y as i64);
            kdl_doucment.nodes_mut().push(cursor_coordinates_in_pane);
        }
        if let Some(terminal_command) = &self.terminal_command {
            string_node!("terminal_command", terminal_command.to_string());
        }
        if let Some(cwd) = &self.cwd {
            string_node!("cwd", cwd.to_string());
        }
        if let Some(plugin_url) = &self.plugin_url {
            string_node!("plugin_url", plugin_url.to_string());
        }
        bool_node!("is_selectable", self.is_selectable);
        kdl_doucment
    }
}

#[allow(clippy::result_large_err)]
pub fn parse_plugin_user_configuration(
    plugin_block: &KdlNode,
) -> Result<BTreeMap<String, String>, ConfigError> {
    let mut configuration = BTreeMap::new();
    for user_configuration_entry in plugin_block.entries() {
        let name = user_configuration_entry.name();
        let value = user_configuration_entry.value();
        if let Some(name) = name {
            let name = name.to_string();
            if KdlLayoutParser::is_a_reserved_plugin_property(&name) {
                continue;
            }
            configuration.insert(name, value.to_string());
        }
    }
    if let Some(user_config) = kdl_children_nodes!(plugin_block) {
        for user_configuration_entry in user_config {
            let config_entry_name = kdl_name!(user_configuration_entry);
            if KdlLayoutParser::is_a_reserved_plugin_property(config_entry_name) {
                continue;
            }
            let config_entry_str_value =
                kdl_first_entry_as_string!(user_configuration_entry).map(|s| s.to_string());
            let config_entry_int_value =
                kdl_first_entry_as_i64!(user_configuration_entry).map(|s| s.to_string());
            let config_entry_bool_value =
                kdl_first_entry_as_bool!(user_configuration_entry).map(|s| s.to_string());
            let config_entry_children = user_configuration_entry
                .children()
                .map(|s| s.to_string().trim().to_string());
            let config_entry_value = config_entry_str_value
                .or(config_entry_int_value)
                .or(config_entry_bool_value)
                .or(config_entry_children)
                .ok_or(ConfigError::new_kdl_error(
                    format!(
                        "Failed to parse plugin block configuration: {:?}",
                        user_configuration_entry
                    ),
                    plugin_block.span().offset(),
                    plugin_block.span().len(),
                ))?;
            configuration.insert(config_entry_name.into(), config_entry_value);
        }
    }
    Ok(configuration)
}

#[cfg(test)]
#[path = "./unit/mod.rs"]
mod kdl_tests;

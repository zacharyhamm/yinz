mod kdl_layout_parser;
use kdl_layout_parser::KdlLayoutParser;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use strum::IntoEnumIterator;
use uuid::Uuid;
use yinz_types::data::ContextMenuItem;
use yinz_types::envs::EnvironmentVariables;
use yinz_types::geometry::PercentOrFixed;
use yinz_types::geometry::{Direction, FloatingPaneCoordinates, Resize};
use yinz_types::input::context_menu::ContextMenu;
use yinz_types::input::keybinds::Keybinds;
use yinz_types::input::plugins::PluginAliases;
use yinz_types::keys::KeyWithModifier;
use yinz_types::style::{
    MultiplayerColors, Palette, StyleDeclaration, Styling, ThemeHue, DEFAULT_STYLES,
};
use {
    crate::config::Config, crate::config::ConfigError, crate::config::KdlError,
    crate::config::NotWritableIn,
};
use {crate::permission::GrantedPermission, crate::permission::PermissionCache};
use {
    crate::theme::FrameConfig, crate::theme::Theme, crate::theme::Themes, crate::theme::UiConfig,
};
use {yinz_runtime::home::find_default_config_dir, yinz_runtime::home::get_layout_dir};
use {
    yinz_types::data::InputMode, yinz_types::data::NewPanePlacement,
    yinz_types::data::PaletteColor, yinz_types::data::PermissionType,
};
use {yinz_types::input::hooks::HookPoint, yinz_types::input::hooks::Hooks};
use {
    yinz_types::input::layout::Layout, yinz_types::input::layout::PluginUserConfiguration,
    yinz_types::input::layout::RunPlugin, yinz_types::input::layout::RunPluginOrAlias,
    yinz_types::input::layout::TabLayoutInfo,
};
use {
    yinz_types::input::options::Clipboard, yinz_types::input::options::NotificationPosition,
    yinz_types::input::options::OnForceClose, yinz_types::input::options::Options,
    yinz_types::input::options::PaneFrameStyle,
    yinz_types::input::options::DEFAULT_AGENT_PANE_MIN_COLUMNS,
    yinz_types::input::options::DEFAULT_AGENT_PANE_MIN_ROWS,
    yinz_types::input::options::DEFAULT_NOTIFICATION_TIMEOUT_SECS,
    yinz_types::input::options::DEFAULT_WORD_SEPARATORS,
};

use miette::NamedSource;

use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};

use std::path::PathBuf;
use std::str::FromStr;

use yinz_types::command::PaneTarget;
use yinz_types::input::command::RunCommandAction;
use {
    yinz_types::input::actions::Action, yinz_types::input::actions::SearchDirection,
    yinz_types::input::actions::SearchOption,
};

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
                "FocusPaneTabs" => Ok(Action::FocusPaneTabs),
                "PreviousPaneTab" => Ok(Action::PreviousPaneTab),
                "NextPaneTab" => Ok(Action::NextPaneTab),
                "FocusLastPane" => Ok(Action::FocusLastPane),
                "FocusHostSession" => Ok(Action::FocusHostSession),
                "FocusGuestSession" => Ok(Action::FocusGuestSession),
                "ToggleHostFullscreen" => Ok(Action::ToggleHostFullscreen),
                "SwitchFocus" => Ok(Action::SwitchFocus),
                "EditScrollback" => Ok(Action::EditScrollback { ansi: false }),
                "ScrollUp" => Ok(Action::ScrollUp),
                "SwapTextSelectionEnds" => Ok(Action::SwapTextSelectionEnds),
                "ActivateNotification" => Ok(Action::ActivateNotification),
                "ToggleNotificationDetail" => Ok(Action::ToggleNotificationDetail),
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
                "ToggleCommandPalette" => Ok(Action::ToggleCommandPalette),
                // Keep saved Mother bindings compatible after Mother's removal.
                "ToggleMother" => Ok(Action::NoOp),
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
        $crate::kdl::action::new_from_bytes($action_name, bytes, $action_node)
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
        $crate::kdl::action::new_from_string($action_name, chars_to_write, $action_node)
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
            .map(|kdl_action| $crate::kdl::action::from_kdl((kdl_action, $config_options)))
            .collect::<Result<_, _>>()?
    };
}
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

fn terminal_theme_from_kdl(
    document: &KdlDocument,
) -> Result<Option<yinz_types::style::TerminalTheme>, ConfigError> {
    use yinz_types::style::TerminalTheme;
    let Some(node) = single_top_level_block(document, "terminal")? else {
        return Ok(None);
    };
    let colors = kdl_children_or_error!(node, "empty terminal palette");
    let error = || {
        ConfigError::new_kdl_error(
            "Terminal colors require hex RGB or three channels from 0 to 255".into(),
            node.span().offset(),
            node.span().len(),
        )
    };
    for color in colors.nodes() {
        let name = color.name().value();
        if !["foreground", "background"].contains(&name)
            && !TerminalTheme::ANSI_NAMES.contains(&name)
        {
            return Err(error());
        }
        if colors
            .nodes()
            .iter()
            .filter(|n| n.name() == color.name())
            .count()
            != 1
            || color.children().is_some()
            || color.entries().iter().any(|e| {
                e.name().is_some() || e.value().as_i64().is_some_and(|v| !(0..=255).contains(&v))
            })
        {
            return Err(error());
        }
    }
    let rgb = |name| match palette_color_from_kdl(name, colors)? {
        PaletteColor::Rgb(rgb) => Ok(rgb),
        _ => Err(error()),
    };
    let foreground = rgb("foreground")?;
    let background = rgb("background")?;
    let mut ansi = [(0, 0, 0); 16];
    for (index, name) in TerminalTheme::ANSI_NAMES.into_iter().enumerate() {
        ansi[index] = rgb(name)?;
    }
    Ok(Some(TerminalTheme {
        foreground,
        background,
        ansi,
    }))
}

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
        Some(s) if entry_count == 1 && s.starts_with('#') => yinz_types::colors::parse_hex_rgb(s),
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

/// The one `name` block of a document, or an error naming the second when there are more.
///
/// `KdlDocument::get` returns the first match, so a second `keybinds` (or `plugins`, …) block
/// in the same file used to parse successfully and silently contribute nothing. Merging across
/// *files* is unaffected: each `config.d` fragment and layout is its own `Config::from_kdl` call
/// over `base_config`.
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
    pub fn from_kdl(kdl_config: &str, base_config: Option<Config>) -> Result<Config, ConfigError> {
        let mut config = base_config.unwrap_or_default();
        let kdl_config: KdlDocument = kdl_config.parse()?;

        let config_options = crate::kdl::options::from_kdl(&kdl_config)?;
        config.options = config.options.merge(config_options);

        if let Some(kdl_keybinds) = single_top_level_block(&kdl_config, "keybinds")? {
            config.keybinds =
                crate::kdl::keybinds::from_kdl(kdl_keybinds, config.keybinds, &config.options)?;
        }
        if let Some(kdl_themes) = single_top_level_block(&kdl_config, "themes")? {
            let sourced_from_external_file = false;
            let config_themes = Themes::from_kdl(kdl_themes, sourced_from_external_file)?;
            config.themes = config.themes.merge(config_themes);
        }
        if let Some(kdl_plugin_aliases) = single_top_level_block(&kdl_config, "plugins")? {
            let config_plugins = crate::kdl::plugin_aliases::from_kdl(kdl_plugin_aliases)?;
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
    /// The whole configuration as KDL text. `Err` when a keybinding or context menu item holds
    /// an action with no config spelling: the text is refused rather than written without it.
    pub fn to_string(&self, add_comments: bool) -> Result<String, ConfigError> {
        let mut document = KdlDocument::new();

        let clear_defaults = true;
        let keybinds = keybinds::to_kdl(&self.keybinds, clear_defaults)?;
        document.nodes_mut().push(keybinds);

        if let Some(themes) = self.themes.to_kdl() {
            document.nodes_mut().push(themes);
        }

        let plugins = plugin_aliases::to_kdl(&self.plugins, add_comments);
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
            .push(context_menu_to_kdl(&self.context_menu)?);

        if let Some(ui_config) = self.ui.to_kdl() {
            document.nodes_mut().push(ui_config);
        }

        if let Some(env) = environment_variables_to_kdl(&self.env) {
            document.nodes_mut().push(env);
        }

        document
            .nodes_mut()
            .append(&mut options::to_kdl(&self.options, add_comments));

        Ok(document.to_string())
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
        let mut background_plugin_node =
            KdlNode::new(run_plugin_or_alias.location_for_serialization());
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
            "\n{}\n{}\n",
            "// Plugins to load in the background when a new session starts",
            "// eg. \"file:/path/to/my-plugin.wasm\" or \"file:/path/to/my-plugin.ts\"",
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
                        "hook '{}' needs a plugin url, eg. `{} \"file:~/hook.ts\"`",
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
            let run_plugin_or_alias = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
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
                        .map(|action_node| {
                            crate::kdl::action::from_kdl((action_node, config_options))
                        })
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

pub fn context_menu_to_kdl(context_menu: &ContextMenu) -> Result<KdlNode, NotWritableIn> {
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
        for (index, action) in item.actions.iter().enumerate() {
            // An action with no KDL spelling is an error naming the item, never a line left
            // out - the same rule keybinding serialization follows.
            let action_node =
                action::to_kdl(action).map_err(|source| NotWritableIn::ContextMenu {
                    label: item.label.clone(),
                    index,
                    source,
                })?;
            actions.nodes_mut().push(action_node);
        }
        item_node.set_children(actions);
        children.nodes_mut().push(item_node);
    }
    menu_node.set_children(children);
    Ok(menu_node)
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
                node.push(KdlValue::String(plugin.location_for_serialization()));
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
            let run_plugin_or_alias = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
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
            let pane_surround = single_top_level_block(theme_colors, "pane_surround")?
                .map(|node| {
                    if node.children().is_some() || node.entries().iter().any(|entry| {
                        entry.name().is_some() || entry.value().as_i64().is_some_and(|v| !(0..=255).contains(&v))
                    }) {
                        return Err(ConfigError::new_kdl_error(
                            "pane_surround requires a hex color, an index, or three channels from 0 to 255".into(),
                            node.span().offset(), node.span().len(),
                        ));
                    }
                    palette_color_from_kdl("pane_surround", theme_colors)
                }).transpose()?;
            let terminal = terminal_theme_from_kdl(theme_colors)?;
            let theme = if theme_colors
                .nodes()
                .iter()
                .any(|n| palette_color_names.contains(n.name().value()))
                && theme_colors.nodes().iter().all(|n| {
                    ["terminal", "pane_surround"].contains(&n.name().value())
                        || palette_color_names.contains(n.name().value())
                }) {
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
                    terminal,
                    palette: Styling {
                        pane_surround,
                        ..palette.into()
                    },
                    sourced_from_external_file,
                }
            } else {
                // Newer theme definition with named styles
                let s = Styling {
                    pane_surround,
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
                    terminal,
                    palette: s,
                    sourced_from_external_file,
                }
            };
            themes.insert(theme_name.into(), theme);
        }
        let themes = Themes::from_data(themes);
        Ok(themes)
    }
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
            if let Some(color) = theme.palette.pane_surround {
                current_theme_node_children
                    .nodes_mut()
                    .push(palette_color_to_kdl(&color, "pane_surround"));
            }
            if let Some(terminal) = theme.terminal {
                let mut node = KdlNode::new("terminal");
                let mut colors = KdlDocument::new();
                for (name, rgb) in [
                    ("foreground", terminal.foreground),
                    ("background", terminal.background),
                ]
                .into_iter()
                .chain(
                    yinz_types::style::TerminalTheme::ANSI_NAMES
                        .into_iter()
                        .zip(terminal.ansi),
                ) {
                    colors
                        .nodes_mut()
                        .push(palette_color_to_kdl(&PaletteColor::Rgb(rgb), name));
                }
                node.set_children(colors);
                current_theme_node_children.nodes_mut().push(node);
            }
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

pub mod action;

pub mod options;

pub mod layout;

pub mod keybinds;

pub mod plugin_aliases;

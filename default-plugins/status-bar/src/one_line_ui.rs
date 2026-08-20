use ansi_term::{ANSIString, ANSIStrings};
use ansi_term::{
    Color::{Fixed, RGB},
    Style,
};
use std::collections::{BTreeSet, HashMap};
use zellij_tile::prelude::actions::Action;
use zellij_tile::prelude::*;
use zellij_tile_utils::{palette_match, style};

use crate::first_line::{is_generic_to_normal_key, to_char, KeyAction, KeyMode, KeyShortcut};
use crate::second_line::{
    ascended_to_host_session_hint, descended_into_nested_session_hint, system_clipboard_error,
    text_copied_hint,
};
use crate::{action_key, action_key_group, color_elements, plugin_launch_key, MORE_MSG, TO_NORMAL};
use crate::{ColoredElements, LinePart};
use unicode_width::UnicodeWidthStr;

/// A rendered line plus the ranges the new-pane and floating-pane buttons occupy in it, for hover
/// and click hit-testing.
type LineWithButtonRanges = (LinePart, Option<(usize, usize)>, Option<(usize, usize)>);

// the render inputs of the whole status line
#[allow(clippy::too_many_arguments)]
pub fn one_line_ui(
    help: &ModeInfo,
    tab_info: Option<&TabInfo>,
    mut max_len: usize,
    separator: &str,
    base_mode_is_locked: bool,
    text_copied_to_clipboard_destination: Option<CopyDestination>,
    clipboard_failure: bool,
    new_pane_hovered: bool,
    floating_hovered: bool,
) -> LineWithButtonRanges {
    if help.session_dimmed.unwrap_or(false) {
        return (
            descended_into_nested_session_hint(help, max_len),
            None,
            None,
        );
    }
    if help.session_ascended.unwrap_or(false) {
        return (ascended_to_host_session_hint(help, max_len), None, None);
    }
    if let Some(text_copied_to_clipboard_destination) = text_copied_to_clipboard_destination {
        return (
            text_copied_hint(text_copied_to_clipboard_destination),
            None,
            None,
        );
    }
    if clipboard_failure {
        return (system_clipboard_error(&help.style.colors), None, None);
    }
    let mut line_part_to_render = LinePart::default();
    let mut new_pane_range = None;
    let mut floating_range = None;

    let left_part = render_mode_key_indicators(help, max_len, separator, base_mode_is_locked);
    if let Some(left) = left_part {
        line_part_to_render.append(&left);
        max_len = max_len.saturating_sub(left.len);
        match help.mode {
            InputMode::Normal | InputMode::Locked => {
                let secondary_offset = line_part_to_render.len;
                if let Some((secondary_info, new_pane, floating)) = render_secondary_info(
                    help,
                    tab_info,
                    max_len,
                    new_pane_hovered,
                    floating_hovered,
                ) {
                    new_pane_range = new_pane
                        .map(|(start, end)| (secondary_offset + start, secondary_offset + end));
                    floating_range = floating
                        .map(|(start, end)| (secondary_offset + start, secondary_offset + end));
                    line_part_to_render.append(&secondary_info);
                }
            },
            _ => {
                if let Some(key_group_separator) = add_keygroup_separator(help, max_len) {
                    line_part_to_render.append(&key_group_separator);
                    max_len = max_len.saturating_sub(key_group_separator.len);
                    if let Some(keybinds) = keybinds(help, max_len) {
                        line_part_to_render.append(&keybinds);
                    }
                }
            },
        }
    }
    (line_part_to_render, new_pane_range, floating_range)
}

fn to_base_mode(base_mode: InputMode) -> Action {
    Action::SwitchToMode {
        input_mode: base_mode,
    }
}

fn base_mode_locked_mode_indicators(help: &ModeInfo) -> HashMap<InputMode, Vec<KeyShortcut>> {
    let locked_binds = &help.get_keybinds_for_mode(InputMode::Locked);
    let normal_binds = &help.get_keybinds_for_mode(InputMode::Normal);
    let pane_binds = &help.get_keybinds_for_mode(InputMode::Pane);
    let tab_binds = &help.get_keybinds_for_mode(InputMode::Tab);
    let resize_binds = &help.get_keybinds_for_mode(InputMode::Resize);
    let move_binds = &help.get_keybinds_for_mode(InputMode::Move);
    let scroll_binds = &help.get_keybinds_for_mode(InputMode::Scroll);
    let session_binds = &help.get_keybinds_for_mode(InputMode::Session);
    let agent_binds = &help.get_keybinds_for_mode(InputMode::Agent);
    HashMap::from([
        (
            InputMode::Locked,
            vec![KeyShortcut::new(
                KeyMode::Unselected,
                KeyAction::Unlock,
                to_char(action_key(
                    locked_binds,
                    &[Action::SwitchToMode {
                        input_mode: InputMode::Normal,
                    }],
                )),
            )],
        ),
        (
            InputMode::Normal,
            vec![
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Unlock,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Locked,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::UnselectedAlternate,
                    KeyAction::Pane,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Pane,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Unselected,
                    KeyAction::Tab,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Tab,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::UnselectedAlternate,
                    KeyAction::Resize,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Resize,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Unselected,
                    KeyAction::Move,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Move,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::UnselectedAlternate,
                    KeyAction::Search,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Scroll,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Unselected,
                    KeyAction::Session,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Session,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::UnselectedAlternate,
                    KeyAction::Agent,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Agent,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Unselected,
                    KeyAction::Quit,
                    to_char(action_key(normal_binds, &[Action::Quit])),
                ),
            ],
        ),
        (
            InputMode::Agent,
            vec![
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Unlock,
                    to_char(action_key(
                        agent_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Locked,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Agent,
                    to_char(action_key(
                        agent_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Normal,
                        }],
                    )),
                ),
            ],
        ),
        (
            InputMode::Pane,
            vec![
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Unlock,
                    to_char(action_key(
                        pane_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Locked,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Pane,
                    to_char(action_key(
                        pane_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Normal,
                        }],
                    )),
                ),
            ],
        ),
        (
            InputMode::Tab,
            vec![
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Unlock,
                    to_char(action_key(
                        tab_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Locked,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Tab,
                    to_char(action_key(
                        tab_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Normal,
                        }],
                    )),
                ),
            ],
        ),
        (
            InputMode::Resize,
            vec![
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Unlock,
                    to_char(action_key(
                        resize_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Locked,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Resize,
                    to_char(action_key(
                        resize_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Normal,
                        }],
                    )),
                ),
            ],
        ),
        (
            InputMode::Move,
            vec![
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Unlock,
                    to_char(action_key(
                        move_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Locked,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Move,
                    to_char(action_key(
                        move_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Normal,
                        }],
                    )),
                ),
            ],
        ),
        (
            InputMode::Scroll,
            vec![
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Unlock,
                    to_char(action_key(
                        scroll_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Locked,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Search,
                    to_char(action_key(
                        scroll_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Normal,
                        }],
                    )),
                ),
            ],
        ),
        (
            InputMode::Session,
            vec![
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Unlock,
                    to_char(action_key(
                        session_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Locked,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Session,
                    to_char(action_key(
                        session_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Normal,
                        }],
                    )),
                ),
            ],
        ),
    ])
}

fn base_mode_normal_mode_indicators(help: &ModeInfo) -> HashMap<InputMode, Vec<KeyShortcut>> {
    let locked_binds = &help.get_keybinds_for_mode(InputMode::Locked);
    let normal_binds = &help.get_keybinds_for_mode(InputMode::Normal);
    let pane_binds = &help.get_keybinds_for_mode(InputMode::Pane);
    let tab_binds = &help.get_keybinds_for_mode(InputMode::Tab);
    let resize_binds = &help.get_keybinds_for_mode(InputMode::Resize);
    let move_binds = &help.get_keybinds_for_mode(InputMode::Move);
    let scroll_binds = &help.get_keybinds_for_mode(InputMode::Scroll);
    let session_binds = &help.get_keybinds_for_mode(InputMode::Session);
    let agent_binds = &help.get_keybinds_for_mode(InputMode::Agent);
    HashMap::from([
        (
            InputMode::Locked,
            vec![KeyShortcut::new(
                KeyMode::Selected,
                KeyAction::Lock,
                to_char(action_key(
                    locked_binds,
                    &[Action::SwitchToMode {
                        input_mode: InputMode::Normal,
                    }],
                )),
            )],
        ),
        (
            InputMode::Normal,
            vec![
                KeyShortcut::new(
                    KeyMode::Unselected,
                    KeyAction::Lock,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Locked,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::UnselectedAlternate,
                    KeyAction::Pane,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Pane,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Unselected,
                    KeyAction::Tab,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Tab,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::UnselectedAlternate,
                    KeyAction::Resize,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Resize,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Unselected,
                    KeyAction::Move,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Move,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::UnselectedAlternate,
                    KeyAction::Search,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Scroll,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Unselected,
                    KeyAction::Session,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Session,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::UnselectedAlternate,
                    KeyAction::Agent,
                    to_char(action_key(
                        normal_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Agent,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Unselected,
                    KeyAction::Quit,
                    to_char(action_key(normal_binds, &[Action::Quit])),
                ),
            ],
        ),
        (
            InputMode::Agent,
            vec![KeyShortcut::new(
                KeyMode::Selected,
                KeyAction::Agent,
                to_char(action_key(
                    agent_binds,
                    &[Action::SwitchToMode {
                        input_mode: InputMode::Normal,
                    }],
                )),
            )],
        ),
        (
            InputMode::Pane,
            vec![KeyShortcut::new(
                KeyMode::Selected,
                KeyAction::Pane,
                to_char(action_key(
                    pane_binds,
                    &[Action::SwitchToMode {
                        input_mode: InputMode::Normal,
                    }],
                )),
            )],
        ),
        (
            InputMode::Tab,
            vec![KeyShortcut::new(
                KeyMode::Selected,
                KeyAction::Tab,
                to_char(action_key(
                    tab_binds,
                    &[Action::SwitchToMode {
                        input_mode: InputMode::Normal,
                    }],
                )),
            )],
        ),
        (
            InputMode::Resize,
            vec![KeyShortcut::new(
                KeyMode::Selected,
                KeyAction::Resize,
                to_char(action_key(
                    resize_binds,
                    &[Action::SwitchToMode {
                        input_mode: InputMode::Normal,
                    }],
                )),
            )],
        ),
        (
            InputMode::Move,
            vec![KeyShortcut::new(
                KeyMode::Selected,
                KeyAction::Move,
                to_char(action_key(
                    move_binds,
                    &[Action::SwitchToMode {
                        input_mode: InputMode::Normal,
                    }],
                )),
            )],
        ),
        (
            InputMode::Scroll,
            vec![KeyShortcut::new(
                KeyMode::Selected,
                KeyAction::Search,
                to_char(action_key(
                    scroll_binds,
                    &[Action::SwitchToMode {
                        input_mode: InputMode::Normal,
                    }],
                )),
            )],
        ),
        (
            InputMode::Session,
            vec![KeyShortcut::new(
                KeyMode::Selected,
                KeyAction::Session,
                to_char(action_key(
                    session_binds,
                    &[Action::SwitchToMode {
                        input_mode: InputMode::Normal,
                    }],
                )),
            )],
        ),
    ])
}

fn render_mode_key_indicators(
    help: &ModeInfo,
    max_len: usize,
    separator: &str,
    base_mode_is_locked: bool,
) -> Option<LinePart> {
    let mut line_part_to_render = LinePart::default();
    let supports_arrow_fonts = !help.capabilities.arrow_fonts;
    let colored_elements = color_elements(
        help.style.colors,
        !supports_arrow_fonts,
        help.session_dimmed.unwrap_or(false),
    );
    let default_keys = if base_mode_is_locked {
        base_mode_locked_mode_indicators(help)
    } else {
        base_mode_normal_mode_indicators(help)
    };
    match common_modifiers_in_all_modes(&default_keys) {
        Some(modifiers) => {
            if let Some(default_keys) = default_keys.get(&help.mode) {
                let keys_without_common_modifiers: Vec<KeyShortcut> = default_keys
                    .iter()
                    .map(|key_shortcut| {
                        let key = key_shortcut
                            .get_key()
                            .map(|k| k.strip_common_modifiers(&modifiers));
                        let mode = key_shortcut.get_mode();
                        let action = key_shortcut.get_action();
                        KeyShortcut::new(mode, action, key)
                    })
                    .collect();
                render_common_modifiers(
                    &colored_elements,
                    help,
                    &modifiers,
                    &mut line_part_to_render,
                    separator,
                );

                let full_shortcut_list =
                    full_inline_keys_modes_shortcut_list(&keys_without_common_modifiers, help);

                if line_part_to_render.len + full_shortcut_list.len <= max_len {
                    line_part_to_render.append(&full_shortcut_list);
                } else {
                    let shortened_shortcut_list = shortened_inline_keys_modes_shortcut_list(
                        &keys_without_common_modifiers,
                        help,
                    );
                    if line_part_to_render.len + shortened_shortcut_list.len <= max_len {
                        line_part_to_render.append(&shortened_shortcut_list);
                    }
                }
            }
        },
        None => {
            if let Some(default_keys) = default_keys.get(&help.mode) {
                let full_shortcut_list = full_modes_shortcut_list(default_keys, help);
                if line_part_to_render.len + full_shortcut_list.len <= max_len {
                    line_part_to_render.append(&full_shortcut_list);
                } else {
                    let shortened_shortcut_list = shortened_modes_shortcut_list(default_keys, help);
                    if line_part_to_render.len + shortened_shortcut_list.len <= max_len {
                        line_part_to_render.append(&shortened_shortcut_list);
                    }
                }
            }
        },
    }
    if line_part_to_render.len <= max_len {
        Some(line_part_to_render)
    } else {
        None
    }
}

fn full_inline_keys_modes_shortcut_list(
    keys_without_common_modifiers: &Vec<KeyShortcut>,
    help: &ModeInfo,
) -> LinePart {
    let mut full_shortcut_list = LinePart::default();
    for key in keys_without_common_modifiers {
        let is_selected = key.is_selected();
        let shortcut = add_shortcut_with_inline_key(
            help,
            &key.full_text(),
            key.key
                .as_ref()
                .map(|k| vec![k.clone()])
                .unwrap_or_else(std::vec::Vec::new),
            is_selected,
        );
        full_shortcut_list.append(&shortcut);
    }
    full_shortcut_list
}

fn shortened_inline_keys_modes_shortcut_list(
    keys_without_common_modifiers: &Vec<KeyShortcut>,
    help: &ModeInfo,
) -> LinePart {
    let mut shortened_shortcut_list = LinePart::default();
    for key in keys_without_common_modifiers {
        let is_selected = key.is_selected();
        let shortcut = add_shortcut_with_key_only(
            help,
            key.key
                .as_ref()
                .map(|k| vec![k.clone()])
                .unwrap_or_else(std::vec::Vec::new),
            is_selected,
        );
        shortened_shortcut_list.append(&shortcut);
    }
    shortened_shortcut_list
}

fn full_modes_shortcut_list(default_keys: &Vec<KeyShortcut>, help: &ModeInfo) -> LinePart {
    let mut full_shortcut_list = LinePart::default();
    for key in default_keys {
        let is_selected = key.is_selected();
        full_shortcut_list.append(&add_shortcut(
            help,
            &key.full_text(),
            &key.key
                .as_ref()
                .map(|k| vec![k.clone()])
                .unwrap_or_else(std::vec::Vec::new),
            is_selected,
            Some(3),
        ));
    }
    full_shortcut_list
}

fn shortened_modes_shortcut_list(default_keys: &Vec<KeyShortcut>, help: &ModeInfo) -> LinePart {
    let mut shortened_shortcut_list = LinePart::default();
    for key in default_keys {
        let is_selected = key.is_selected();
        shortened_shortcut_list.append(&add_shortcut(
            help,
            &key.short_text(),
            &key.key
                .as_ref()
                .map(|k| vec![k.clone()])
                .unwrap_or_else(std::vec::Vec::new),
            is_selected,
            Some(3),
        ));
    }
    shortened_shortcut_list
}

/// The modifier every mode shortcut shares, if there is one, so the ribbon can print it once as a
/// `Ctrl +` prefix and leave bare letters in the tiles.
///
/// Two kinds of shortcut contribute nothing here rather than cancelling the prefix:
///
/// - Ones the user has no key for. Such a tile renders as nothing at all (see the empty-key paths
///   in `full_modes_shortcut_list` and friends), so letting one collapse the intersection would
///   cost every *other* tile its prefix over a tile that isn't even drawn.
/// - Ones that resolved to `Enter`/`Esc`/`Space`. Those are the generic get-back-to-normal keys,
///   which `to_char` falls back to when a mode has nothing else; being unmodified, a single one
///   empties the intersection. `mode_switch_keys` skips them for the same reason.
///
/// A config with no `agent` keybind block hit both at once when the AGENT tile was added: the
/// Normal-mode AGENT tile has no key, and agent mode itself inherits only the shared `Enter`/`Esc`.
fn common_modifiers_in_all_modes(
    key_shortcuts: &HashMap<InputMode, Vec<KeyShortcut>>,
) -> Option<Vec<KeyModifier>> {
    let mut common_modifiers: Option<BTreeSet<KeyModifier>> = None;
    for key_shortcuts in key_shortcuts.values() {
        for key in key_shortcuts
            .iter()
            .filter_map(|k| k.get_key())
            .filter(|k| !is_generic_to_normal_key(k))
        {
            common_modifiers = Some(match common_modifiers {
                Some(common_modifiers) => common_modifiers
                    .intersection(&key.key_modifiers)
                    .cloned()
                    .collect(),
                None => key.key_modifiers.clone(),
            });
        }
    }
    let common_modifiers = common_modifiers?;
    if common_modifiers.is_empty() {
        return None;
    }
    Some(common_modifiers.into_iter().collect())
}

fn render_common_modifiers(
    palette: &ColoredElements,
    mode_info: &ModeInfo,
    common_modifiers: &[KeyModifier],
    line_part_to_render: &mut LinePart,
    separator: &str,
) {
    let prefix_text = if mode_info.capabilities.arrow_fonts {
        // Add extra space in simplified ui
        format!(
            " {} + ",
            common_modifiers
                .iter()
                .map(|m| m.to_string())
                .collect::<Vec<_>>()
                .join("-")
        )
    } else {
        format!(
            " {} +",
            common_modifiers
                .iter()
                .map(|m| m.to_string())
                .collect::<Vec<_>>()
                .join("-")
        )
    };

    let prefix = if mode_info.session_dimmed.unwrap_or(false) {
        serialize_text(&Text::new(&prefix_text).disabled().opaque())
    } else {
        serialize_text(&Text::new(&prefix_text).opaque())
    };
    let suffix_separator = palette.superkey_suffix_separator.paint(separator);
    line_part_to_render.part =
        format!("{}{}{}", line_part_to_render.part, prefix, suffix_separator);
    line_part_to_render.len += prefix_text.chars().count() + separator.chars().count();
}

fn render_secondary_info(
    help: &ModeInfo,
    tab_info: Option<&TabInfo>,
    max_len: usize,
    new_pane_hovered: bool,
    floating_hovered: bool,
) -> Option<LineWithButtonRanges> {
    let mut secondary_info = LinePart::default();
    let supports_arrow_fonts = !help.capabilities.arrow_fonts;
    let colored_elements = color_elements(
        help.style.colors,
        !supports_arrow_fonts,
        help.session_dimmed.unwrap_or(false),
    );
    let (secondary_keybinds, new_pane_range, floating_range) =
        secondary_keybinds(help, tab_info, max_len, new_pane_hovered, floating_hovered);
    secondary_info.append(&secondary_keybinds);
    let remaining_space = max_len.saturating_sub(secondary_info.len).saturating_sub(1); // 1 for the end padding of the line
    let mut padding = String::new();
    let mut padding_len = 0;
    for _ in 0..remaining_space {
        padding.push_str(&ANSIStrings(&[colored_elements.superkey_prefix.paint(" ")]).to_string());
        padding_len += 1;
    }
    secondary_info.part = format!("{}{}", padding, secondary_info.part);
    secondary_info.len += padding_len;
    if secondary_info.len <= max_len {
        let shift = |range: Option<(usize, usize)>| {
            range.map(|(start, end)| (start + padding_len, end + padding_len))
        };
        Some((secondary_info, shift(new_pane_range), shift(floating_range)))
    } else {
        None
    }
}

fn should_show_focus_and_resize_shortcuts(tab_info: Option<&TabInfo>) -> bool {
    let Some(tab_info) = tab_info else {
        return false;
    };
    let are_floating_panes_visible = tab_info.are_floating_panes_visible;
    if are_floating_panes_visible {
        tab_info.selectable_floating_panes_count > 1
    } else {
        tab_info.selectable_tiled_panes_count > 1
    }
}

fn secondary_keybinds(
    help: &ModeInfo,
    tab_info: Option<&TabInfo>,
    max_len: usize,
    new_pane_hovered: bool,
    floating_hovered: bool,
) -> LineWithButtonRanges {
    let mut secondary_info = LinePart::default();
    let mut new_pane_range;
    let mut floating_range;
    let binds = &help.get_mode_keybinds();
    let should_show_focus_and_resize_shortcuts = should_show_focus_and_resize_shortcuts(tab_info);
    // New Pane
    let new_pane_action_key = action_key(
        binds,
        &[Action::NewPane {
            direction: None,
            pane_name: None,
            start_suppressed: false,
            global_placement: None,
        }],
    );
    let mut new_pane_key_to_display = new_pane_action_key
        .iter()
        .find(|k| k.is_key_with_alt_modifier(BareKey::Char('n')))
        .or_else(|| new_pane_action_key.first());
    let new_pane_key_to_display =
        if let Some(new_pane_key_to_display) = new_pane_key_to_display.take() {
            vec![new_pane_key_to_display.clone()]
        } else {
            vec![]
        };

    // Resize
    let resize_increase_action_key = action_key(
        binds,
        &[Action::Resize {
            resize: Resize::Increase,
            direction: None,
        }],
    );
    let resize_increase_key = resize_increase_action_key
        .iter()
        .find(|k| k.bare_key == BareKey::Char('+'))
        .or_else(|| resize_increase_action_key.first());
    let resize_decrease_action_key = action_key(
        binds,
        &[Action::Resize {
            resize: Resize::Decrease,
            direction: None,
        }],
    );
    let resize_decrease_key = resize_decrease_action_key
        .iter()
        .find(|k| k.bare_key == BareKey::Char('-'))
        .or_else(|| resize_increase_action_key.first());
    let mut resize_shortcuts = vec![];
    if let Some(resize_increase_key) = resize_increase_key {
        resize_shortcuts.push(resize_increase_key.clone());
    }
    if let Some(resize_decrease_key) = resize_decrease_key {
        resize_shortcuts.push(resize_decrease_key.clone());
    }

    // Move focus
    let mut move_focus_shortcuts: Vec<KeyWithModifier> = vec![];

    // Left
    let move_focus_left_action_key = action_key(
        binds,
        &[Action::MoveFocusOrTab {
            direction: Direction::Left,
        }],
    );
    let move_focus_left_key = move_focus_left_action_key
        .iter()
        .find(|k| k.bare_key == BareKey::Left)
        .or_else(|| move_focus_left_action_key.first());
    if let Some(move_focus_left_key) = move_focus_left_key {
        move_focus_shortcuts.push(move_focus_left_key.clone());
    }
    // Down
    let move_focus_left_action_key = action_key(
        binds,
        &[Action::MoveFocus {
            direction: Direction::Down,
        }],
    );
    let move_focus_left_key = move_focus_left_action_key
        .iter()
        .find(|k| k.bare_key == BareKey::Down)
        .or_else(|| move_focus_left_action_key.first());
    if let Some(move_focus_left_key) = move_focus_left_key {
        move_focus_shortcuts.push(move_focus_left_key.clone());
    }
    // Up
    let move_focus_left_action_key = action_key(
        binds,
        &[Action::MoveFocus {
            direction: Direction::Up,
        }],
    );
    let move_focus_left_key = move_focus_left_action_key
        .iter()
        .find(|k| k.bare_key == BareKey::Up)
        .or_else(|| move_focus_left_action_key.first());
    if let Some(move_focus_left_key) = move_focus_left_key {
        move_focus_shortcuts.push(move_focus_left_key.clone());
    }
    // Right
    let move_focus_left_action_key = action_key(
        binds,
        &[Action::MoveFocusOrTab {
            direction: Direction::Right,
        }],
    );
    let move_focus_left_key = move_focus_left_action_key
        .iter()
        .find(|k| k.bare_key == BareKey::Right)
        .or_else(|| move_focus_left_action_key.first());
    if let Some(move_focus_left_key) = move_focus_left_key {
        move_focus_shortcuts.push(move_focus_left_key.clone());
    }

    let toggle_floating_action_key = action_key(binds, &[Action::ToggleFloatingPanes]);
    let mut toggle_floating_action_key = toggle_floating_action_key
        .iter()
        .find(|k| k.is_key_with_alt_modifier(BareKey::Char('f')))
        .or_else(|| toggle_floating_action_key.first());
    let toggle_floating_key_to_display =
        if let Some(toggle_floating_key_to_display) = toggle_floating_action_key.take() {
            vec![toggle_floating_key_to_display.clone()]
        } else {
            vec![]
        };
    let are_floating_panes_visible = tab_info
        .map(|t| t.are_floating_panes_visible)
        .unwrap_or(false);

    let common_modifiers = get_common_modifiers(
        [
            new_pane_key_to_display.clone(),
            move_focus_shortcuts.clone(),
            resize_shortcuts.clone(),
            toggle_floating_key_to_display.clone(),
        ]
        .iter()
        .flatten()
        .collect(),
    );
    let no_common_modifier = common_modifiers.is_empty();

    if no_common_modifier {
        let new_pane_start = secondary_info.len;
        if new_pane_hovered {
            secondary_info.append(&add_shortcut_hovered(
                help,
                "New Pane",
                &new_pane_key_to_display,
                Some(0),
            ));
        } else {
            secondary_info.append(&add_shortcut(
                help,
                "New Pane",
                &new_pane_key_to_display,
                false,
                Some(0),
            ));
        }
        new_pane_range = ribbon_range(new_pane_start, secondary_info.len);
        if should_show_focus_and_resize_shortcuts {
            secondary_info.append(&add_shortcut(
                help,
                "Change Focus",
                &move_focus_shortcuts,
                false,
                Some(0),
            ));
            secondary_info.append(&add_shortcut(
                help,
                "Resize",
                &resize_shortcuts,
                false,
                Some(0),
            ));
        }
        let floating_start = secondary_info.len;
        if floating_hovered {
            secondary_info.append(&add_shortcut_hovered(
                help,
                "Floating",
                &toggle_floating_key_to_display,
                Some(0),
            ));
        } else {
            secondary_info.append(&add_shortcut(
                help,
                "Floating",
                &toggle_floating_key_to_display,
                are_floating_panes_visible,
                Some(0),
            ));
        }
        floating_range = ribbon_range(floating_start, secondary_info.len);
    } else {
        let modifier_str = text_as_line_part_with_emphasis(
            format!(
                "{} + ",
                common_modifiers
                    .iter()
                    .map(|m| m.to_string())
                    .collect::<Vec<_>>()
                    .join("-")
            ),
            0,
            help.session_dimmed.unwrap_or(false),
        );
        secondary_info.append(&modifier_str);
        let new_pane_key_to_display: Vec<KeyWithModifier> = new_pane_key_to_display
            .iter()
            .map(|k| k.strip_common_modifiers(&common_modifiers))
            .collect();
        let move_focus_shortcuts: Vec<KeyWithModifier> = move_focus_shortcuts
            .iter()
            .map(|k| k.strip_common_modifiers(&common_modifiers))
            .collect();
        let resize_shortcuts: Vec<KeyWithModifier> = resize_shortcuts
            .iter()
            .map(|k| k.strip_common_modifiers(&common_modifiers))
            .collect();
        let toggle_floating_key_to_display: Vec<KeyWithModifier> = toggle_floating_key_to_display
            .iter()
            .map(|k| k.strip_common_modifiers(&common_modifiers))
            .collect();
        let new_pane_start = secondary_info.len;
        if new_pane_hovered {
            secondary_info.append(&add_shortcut_with_inline_key_hovered(
                help,
                "New Pane",
                new_pane_key_to_display,
            ));
        } else {
            secondary_info.append(&add_shortcut_with_inline_key(
                help,
                "New Pane",
                new_pane_key_to_display,
                false,
            ));
        }
        new_pane_range = ribbon_range(new_pane_start, secondary_info.len);
        if should_show_focus_and_resize_shortcuts {
            secondary_info.append(&add_shortcut_with_inline_key(
                help,
                "Change Focus",
                move_focus_shortcuts,
                false,
            ));
            secondary_info.append(&add_shortcut_with_inline_key(
                help,
                "Resize",
                resize_shortcuts,
                false,
            ));
        }
        let floating_start = secondary_info.len;
        if floating_hovered {
            secondary_info.append(&add_shortcut_with_inline_key_hovered(
                help,
                "Floating",
                toggle_floating_key_to_display,
            ));
        } else {
            secondary_info.append(&add_shortcut_with_inline_key(
                help,
                "Floating",
                toggle_floating_key_to_display,
                are_floating_panes_visible,
            ));
        }
        floating_range = ribbon_range(floating_start, secondary_info.len);
    }

    if secondary_info.len <= max_len {
        return (secondary_info, new_pane_range, floating_range);
    }
    let mut short_line = LinePart::default();
    if no_common_modifier {
        let new_pane_start = short_line.len;
        if new_pane_hovered {
            short_line.append(&add_shortcut_hovered(
                help,
                "New",
                &new_pane_key_to_display,
                Some(0),
            ));
        } else {
            short_line.append(&add_shortcut(
                help,
                "New",
                &new_pane_key_to_display,
                false,
                Some(0),
            ));
        }
        new_pane_range = ribbon_range(new_pane_start, short_line.len);
        if should_show_focus_and_resize_shortcuts {
            short_line.append(&add_shortcut(
                help,
                "Focus",
                &move_focus_shortcuts,
                false,
                Some(0),
            ));
            short_line.append(&add_shortcut(
                help,
                "Resize",
                &resize_shortcuts,
                false,
                Some(0),
            ));
        }
        let floating_start = short_line.len;
        if floating_hovered {
            short_line.append(&add_shortcut_hovered(
                help,
                "Floating",
                &toggle_floating_key_to_display,
                Some(0),
            ));
        } else {
            short_line.append(&add_shortcut(
                help,
                "Floating",
                &toggle_floating_key_to_display,
                are_floating_panes_visible,
                Some(0),
            ));
        }
        floating_range = ribbon_range(floating_start, short_line.len);
    } else {
        let modifier_str = text_as_line_part_with_emphasis(
            format!(
                "{} + ",
                common_modifiers
                    .iter()
                    .map(|m| m.to_string())
                    .collect::<Vec<_>>()
                    .join("-")
            ),
            0,
            help.session_dimmed.unwrap_or(false),
        );
        short_line.append(&modifier_str);
        let new_pane_key_to_display: Vec<KeyWithModifier> = new_pane_key_to_display
            .iter()
            .map(|k| k.strip_common_modifiers(&common_modifiers))
            .collect();
        let move_focus_shortcuts: Vec<KeyWithModifier> = move_focus_shortcuts
            .iter()
            .map(|k| k.strip_common_modifiers(&common_modifiers))
            .collect();
        let resize_shortcuts: Vec<KeyWithModifier> = resize_shortcuts
            .iter()
            .map(|k| k.strip_common_modifiers(&common_modifiers))
            .collect();
        let toggle_floating_key_to_display: Vec<KeyWithModifier> = toggle_floating_key_to_display
            .iter()
            .map(|k| k.strip_common_modifiers(&common_modifiers))
            .collect();
        let new_pane_start = short_line.len;
        if new_pane_hovered {
            short_line.append(&add_shortcut_with_inline_key_hovered(
                help,
                "New",
                new_pane_key_to_display,
            ));
        } else {
            short_line.append(&add_shortcut_with_inline_key(
                help,
                "New",
                new_pane_key_to_display,
                false,
            ));
        }
        new_pane_range = ribbon_range(new_pane_start, short_line.len);
        if should_show_focus_and_resize_shortcuts {
            short_line.append(&add_shortcut_with_inline_key(
                help,
                "Focus",
                move_focus_shortcuts,
                false,
            ));
            short_line.append(&add_shortcut_with_inline_key(
                help,
                "Resize",
                resize_shortcuts,
                false,
            ));
        }
        let floating_start = short_line.len;
        if floating_hovered {
            short_line.append(&add_shortcut_with_inline_key_hovered(
                help,
                "Floating",
                toggle_floating_key_to_display,
            ));
        } else {
            short_line.append(&add_shortcut_with_inline_key(
                help,
                "Floating",
                toggle_floating_key_to_display,
                are_floating_panes_visible,
            ));
        }
        floating_range = ribbon_range(floating_start, short_line.len);
    }
    if short_line.len <= max_len {
        (short_line, new_pane_range, floating_range)
    } else if max_len >= 3 {
        let overflow_text = Text::new(format!("{:>width$}", "...", width = max_len));
        let part = if help.session_dimmed.unwrap_or(false) {
            serialize_text(&overflow_text.disabled().opaque())
        } else {
            serialize_text(&overflow_text.color_range(0, ..).opaque())
        };
        let len = max_len;
        (LinePart { part, len }, None, None)
    } else {
        (
            LinePart {
                part: "".to_owned(),
                len: 0,
            },
            None,
            None,
        )
    }
}

fn text_as_line_part_with_emphasis(text: String, emphases_index: usize, dimmed: bool) -> LinePart {
    let part = if dimmed {
        serialize_text(&Text::new(&text).disabled().opaque())
    } else {
        serialize_text(&Text::new(&text).color_range(emphases_index, ..).opaque())
    };
    LinePart {
        part,
        len: text.width(),
    }
}

fn keybinds(help: &ModeInfo, max_width: usize) -> Option<LinePart> {
    let full_shortcut_list = full_shortcut_list(help);
    if full_shortcut_list.len <= max_width {
        return Some(full_shortcut_list);
    }
    let shortened_shortcut_list = shortened_shortcut_list(help);
    if shortened_shortcut_list.len <= max_width {
        return Some(shortened_shortcut_list);
    }
    Some(best_effort_shortcut_list(help, max_width))
}

fn ribbon_range(start: usize, end: usize) -> Option<(usize, usize)> {
    if end > start {
        Some((start, end))
    } else {
        None
    }
}

fn hovered_ribbon_wrap(body: String, palette: Styling, supports_arrow_fonts: bool) -> String {
    let ribbon_bg = palette.ribbon_unselected.emphasis_1;
    let outer_bg = palette.text_unselected.background;
    let arrow = if supports_arrow_fonts {
        crate::ARROW_SEPARATOR
    } else {
        ""
    };
    let left = style!(outer_bg, ribbon_bg).paint(arrow).to_string();
    let right = style!(ribbon_bg, outer_bg).paint(arrow).to_string();
    format!("{}{}{}", left, body, right)
}

fn add_shortcut_hovered(
    help: &ModeInfo,
    text: &str,
    keys: &[KeyWithModifier],
    key_color_index: Option<usize>,
) -> LinePart {
    if help.session_dimmed.unwrap_or(false) {
        return add_shortcut(help, text, keys, false, key_color_index);
    }
    let mut ret = LinePart::default();
    if keys.is_empty() {
        return ret;
    }
    ret.append(&style_key_with_modifier(keys, key_color_index, false));
    let palette = help.style.colors;
    let supports_arrow_fonts = !help.capabilities.arrow_fonts;
    let ribbon_bg = palette.ribbon_unselected.emphasis_1;
    let body = style!(palette.ribbon_unselected.base, ribbon_bg)
        .bold()
        .paint(format!(" {} ", text))
        .to_string();
    let ribbon = hovered_ribbon_wrap(body, palette, supports_arrow_fonts);
    ret.part = format!("{}{}", ret.part, ribbon);
    ret.len += if supports_arrow_fonts {
        text.width() + 4
    } else {
        text.width() + 2
    };
    ret
}

fn add_shortcut_with_inline_key_hovered(
    help: &ModeInfo,
    text: &str,
    key: Vec<KeyWithModifier>,
) -> LinePart {
    if help.session_dimmed.unwrap_or(false) {
        return add_shortcut_with_inline_key(help, text, key, false);
    }
    let mut ret = LinePart::default();
    if key.is_empty() {
        return ret;
    }
    let key_separator = match key
        .iter()
        .map(|k| k.to_string())
        .collect::<Vec<_>>()
        .join("")
        .as_str()
    {
        "HJKL" => "",
        "hjkl" => "",
        "←↓↑→" => "",
        "←→" => "",
        "↓↑" => "",
        "[]" => "",
        "+-" => "",
        _ => "|",
    };
    let key_string = key
        .iter()
        .map(|k| k.to_string())
        .collect::<Vec<_>>()
        .join(key_separator)
        .to_string();
    let palette = help.style.colors;
    let supports_arrow_fonts = !help.capabilities.arrow_fonts;
    let ribbon_bg = palette.ribbon_unselected.emphasis_1;
    let base = style!(palette.ribbon_unselected.base, ribbon_bg).bold();
    let emph = style!(palette.ribbon_unselected.emphasis_0, ribbon_bg).bold();
    let body = format!(
        "{}{}{}",
        base.paint(" <"),
        emph.paint(key_string.clone()),
        base.paint(format!("> {} ", text)),
    );
    ret.part = hovered_ribbon_wrap(body, palette, supports_arrow_fonts);
    ret.len += if supports_arrow_fonts {
        text.width() + key_string.width() + 7
    } else {
        text.width() + key_string.width() + 5
    };
    ret
}

fn add_shortcut(
    help: &ModeInfo,
    text: &str,
    keys: &[KeyWithModifier],
    selected: bool,
    key_color_index: Option<usize>,
) -> LinePart {
    let dimmed = help.session_dimmed.unwrap_or(false);
    let mut ret = LinePart::default();
    if keys.is_empty() {
        return ret;
    }

    ret.append(&style_key_with_modifier(keys, key_color_index, dimmed)); // TODO: alternate
                                                                         //
    let ribbon = if dimmed {
        serialize_ribbon(&Text::new(text.to_string()).disabled())
    } else if selected {
        serialize_ribbon(&Text::new(text.to_string()).selected())
    } else {
        serialize_ribbon(&Text::new(text.to_string()))
    };
    ret.part = format!("{}{}", ret.part, ribbon);
    let supports_arrow_fonts = !help.capabilities.arrow_fonts;
    ret.len += if supports_arrow_fonts {
        text.width() + 4 // padding and arrow fonts
    } else {
        text.width() + 2 // padding
    };
    ret
}

fn add_shortcut_with_inline_key(
    help: &ModeInfo,
    text: &str,
    key: Vec<KeyWithModifier>,
    is_selected: bool,
) -> LinePart {
    let capabilities = help.capabilities;

    let mut ret = LinePart::default();
    if key.is_empty() {
        return ret;
    }

    let key_separator = match key
        .iter()
        .map(|k| k.to_string())
        .collect::<Vec<_>>()
        .join("")
        .as_str()
    {
        "HJKL" => "",
        "hjkl" => "",
        "←↓↑→" => "",
        "←→" => "",
        "↓↑" => "",
        "[]" => "",
        "+-" => "",
        _ => "|",
    };

    let key_string = key
        .iter()
        .map(|k| k.to_string())
        .collect::<Vec<_>>()
        .join(key_separator)
        .to_string();

    let ribbon = if help.session_dimmed.unwrap_or(false) {
        serialize_ribbon(&Text::new(format!("<{}> {}", key_string, text)).disabled())
    } else if is_selected {
        serialize_ribbon(
            &Text::new(format!("<{}> {}", key_string, text))
                .color_range(0, 1..key_string.width() + 1)
                .selected(),
        )
    } else {
        serialize_ribbon(
            &Text::new(format!("<{}> {}", key_string, text))
                .color_range(0, 1..key_string.width() + 1),
        )
    };
    ret.part = ribbon;
    let supports_arrow_fonts = !capabilities.arrow_fonts;
    ret.len += if supports_arrow_fonts {
        text.width() + key_string.width() + 7 // padding, group boundaries and arrow fonts
    } else {
        text.width() + key_string.width() + 5 // padding and group boundaries
    };

    ret
}

fn add_shortcut_with_key_only(
    help: &ModeInfo,
    key: Vec<KeyWithModifier>,
    is_selected: bool,
) -> LinePart {
    let mut ret = LinePart::default();
    if key.is_empty() {
        return ret;
    }

    let key_string = key
        .iter()
        .map(|k| k.to_string())
        .collect::<Vec<_>>()
        .join("-")
        .to_string();

    let ribbon = if help.session_dimmed.unwrap_or(false) {
        serialize_ribbon(&Text::new(key_string.to_string()).disabled())
    } else if is_selected {
        serialize_ribbon(
            &Text::new(key_string.to_string())
                .color_range(0, ..)
                .selected(),
        )
    } else {
        serialize_ribbon(&Text::new(key_string.to_string()).color_range(0, ..))
    };
    ret.part = ribbon;
    let supports_arrow_fonts = !help.capabilities.arrow_fonts;
    ret.len += if supports_arrow_fonts {
        key_string.width() + 4 // 4 => arrow fonts + padding
    } else {
        key_string.width() + 2 // 2 => padding
    };
    ret
}

fn add_keygroup_separator(help: &ModeInfo, max_len: usize) -> Option<LinePart> {
    let supports_arrow_fonts = !help.capabilities.arrow_fonts;
    let separator = if supports_arrow_fonts {
        crate::ARROW_SEPARATOR
    } else {
        " "
    };
    let palette = help.style.colors;

    if help.session_dimmed.unwrap_or(false) {
        let mut ret = LinePart::default();
        let dim_style = Style::new()
            .fg(palette_match!(palette.text_unselected.base))
            .on(palette_match!(palette.text_unselected.background))
            .italic();
        let mut bits: Vec<ANSIString> = vec![];
        let mode_help_text = match help.mode {
            InputMode::RenamePane => Some("RENAMING PANE"),
            InputMode::RenameTab => Some("RENAMING TAB"),
            InputMode::EnterSearch => Some("ENTERING SEARCH TERM"),
            InputMode::Search => Some("SEARCHING"),
            _ => None,
        };
        if let Some(mode_help_text) = mode_help_text {
            bits.push(dim_style.paint(format!(" {} ", mode_help_text)));
            ret.len += mode_help_text.width() + 2;
        }
        bits.push(dim_style.paint(separator.to_string()));
        bits.push(dim_style.paint(" ".to_string()));
        bits.push(dim_style.paint(separator.to_string()));
        ret.part = format!("{}{}", ret.part, ANSIStrings(&bits));
        ret.len += 3;

        if ret.len <= max_len {
            return Some(ret);
        } else {
            return None;
        }
    }

    let mut ret = LinePart::default();

    let separator_color = palette_match!(palette.text_unselected.emphasis_0);
    let bg_color = palette_match!(palette.ribbon_selected.base);
    let mut bits: Vec<ANSIString> = vec![];
    let mode_help_text = match help.mode {
        InputMode::RenamePane => Some("RENAMING PANE"),
        InputMode::RenameTab => Some("RENAMING TAB"),
        InputMode::EnterSearch => Some("ENTERING SEARCH TERM"),
        InputMode::Search => Some("SEARCHING"),
        _ => None,
    };
    if let Some(mode_help_text) = mode_help_text {
        bits.push(
            Style::new()
                .fg(separator_color)
                .on(bg_color)
                .bold()
                .paint(format!(" {} ", mode_help_text)),
        );
        ret.len += mode_help_text.width() + 2; // 2 => padding
    }
    bits.push(
        Style::new()
            .fg(bg_color)
            .on(separator_color)
            .bold()
            .paint(separator.to_string()),
    );
    bits.push(
        Style::new()
            .fg(separator_color)
            .on(separator_color)
            .bold()
            .paint(" ".to_string()),
    );
    bits.push(
        Style::new()
            .fg(separator_color)
            .on(bg_color)
            .bold()
            .paint(separator.to_string()),
    );
    ret.part = format!("{}{}", ret.part, ANSIStrings(&bits));
    ret.len += 3; // padding and arrow fonts

    if ret.len <= max_len {
        Some(ret)
    } else {
        None
    }
}

fn full_shortcut_list(help: &ModeInfo) -> LinePart {
    match help.mode {
        InputMode::Normal => LinePart::default(),
        InputMode::Locked => LinePart::default(),
        _ => full_shortcut_list_nonstandard_mode(help),
    }
}

fn full_shortcut_list_nonstandard_mode(help: &ModeInfo) -> LinePart {
    let mut line_part = LinePart::default();
    let keys_and_hints = get_keys_and_hints(help);

    for (long, _short, keys) in keys_and_hints.into_iter() {
        line_part.append(&add_shortcut(help, &long, &keys.to_vec(), false, Some(2)));
    }
    line_part
}

#[rustfmt::skip]
fn get_keys_and_hints(mi: &ModeInfo) -> Vec<(String, String, Vec<KeyWithModifier>)> {
    use Action as A;
    use InputMode as IM;
    use Direction as Dir;
    use actions::SearchDirection as SDir;
    use actions::SearchOption as SOpt;

    let mut old_keymap = mi.get_mode_keybinds();
    let s = |string: &str| string.to_string();

    // Find a keybinding to get back to "Normal" input mode. In this case we prefer '\n' over other
    // choices. Do it here before we dedupe the keymap below!
    let base_mode = mi.base_mode;
    let to_basemode_keys = base_mode.map(|b| action_key(&old_keymap, &[to_base_mode(b)])).unwrap_or_else(|| action_key(&old_keymap, &[TO_NORMAL]));
    let to_basemode_key = if to_basemode_keys.contains(&KeyWithModifier::new(BareKey::Enter)) {
        vec![KeyWithModifier::new(BareKey::Enter)]
    } else {
        // Yield `vec![key]` if `to_normal_keys` has at least one key, or an empty vec otherwise.
        to_basemode_keys.into_iter().take(1).collect()
    };

    // Sort and deduplicate the keybindings first. We sort after the `Key`s, and deduplicate by
    // their `Action` vectors. An unstable sort is fine here because if the user maps anything to
    // the same key again, anything will happen...
    old_keymap.sort_unstable_by(|(keya, _), (keyb, _)| keya.partial_cmp(keyb).unwrap());

    let mut known_actions: Vec<Vec<Action>> = vec![];
    let mut km = vec![];
    for (key, acvec) in old_keymap {
        if known_actions.contains(&acvec) {
            // This action is known already
            continue;
        } else {
            known_actions.push(acvec.to_vec());
            km.push((key, acvec));
        }
    }

    if mi.mode == IM::Pane { vec![
        (s("New"), s("New"), single_action_key(&km, &[A::NewPane{direction: None, pane_name: None, start_suppressed: false, global_placement: None}, TO_NORMAL])),
        (s("Change Focus"), s("Move"),
            action_key_group(&km, &[&[A::MoveFocus{direction: Dir::Left}], &[A::MoveFocus{direction: Dir::Down}],
                &[A::MoveFocus{direction: Dir::Up}], &[A::MoveFocus{direction: Dir::Right}]])),
        (s("Close"), s("Close"), single_action_key(&km, &[A::CloseFocus, TO_NORMAL])),
        (s("Rename"), s("Rename"),
            single_action_key(&km, &[A::SwitchToMode{input_mode: IM::RenamePane}, A::PaneNameInput{input: vec![0]}])),
        (s("Toggle Fullscreen"), s("Fullscreen"), single_action_key(&km, &[A::ToggleFocusFullscreen, TO_NORMAL])),
        (s("Toggle Floating"), s("Floating"),
            single_action_key(&km, &[A::ToggleFloatingPanes, TO_NORMAL])),
        (s("Toggle Embed"), s("Embed"), single_action_key(&km, &[A::TogglePaneEmbedOrFloating, TO_NORMAL])),
        (s("Split Right"), s("Right"), single_action_key(&km, &[A::NewPane{direction: Some(Direction::Right), pane_name: None, start_suppressed: false, global_placement: None}, TO_NORMAL])),
        (s("Split Down"), s("Down"), single_action_key(&km, &[A::NewPane{direction: Some(Direction::Down), pane_name: None, start_suppressed: false, global_placement: None}, TO_NORMAL])),
        (s("Stack"), s("Stack"), single_action_key(&km, &[A::NewStackedPane{command: None, pane_name: None, near_current_pane: false, no_focus: false, tab_id: None, global_placement: None}, TO_NORMAL])),
        (s("Select pane"), s("Select"), to_basemode_key),
    ]} else if mi.mode == IM::Tab {
        // With the default bindings, "Move focus" for tabs is tricky: It binds all the arrow keys
        // to moving tabs focus (left/up go left, right/down go right). Since we sort the keys
        // above and then dedpulicate based on the actions, we will end up with LeftArrow for
        // "left" and DownArrow for "right". What we really expect is to see LeftArrow and
        // RightArrow.
        // FIXME: So for lack of a better idea we just check this case manually here.
        let old_keymap = mi.get_mode_keybinds();
        let focus_keys_full: Vec<KeyWithModifier> = action_key_group(&old_keymap,
            &[&[A::GoToPreviousTab], &[A::GoToNextTab]]);
        let focus_keys = if focus_keys_full.contains(&KeyWithModifier::new(BareKey::Left))
            && focus_keys_full.contains(&KeyWithModifier::new(BareKey::Right)) {
            vec![KeyWithModifier::new(BareKey::Left), KeyWithModifier::new(BareKey::Right)]
        } else {
            action_key_group(&km, &[&[A::GoToPreviousTab], &[A::GoToNextTab]])
        };

        vec![
        (s("New"), s("New"), single_action_key(&km, &[A::NewTab{
            tiled_layout: None,
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: true,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        }, TO_NORMAL])),
        (s("Change focus"), s("Move"), focus_keys),
        (s("Close"), s("Close"), single_action_key(&km, &[A::CloseTab, TO_NORMAL])),
        (s("Rename"), s("Rename"),
            single_action_key(&km, &[A::SwitchToMode{input_mode: IM::RenameTab}, A::TabNameInput{input: vec![0]}])),
        (s("Sync"), s("Sync"), single_action_key(&km, &[A::ToggleActiveSyncTab, TO_NORMAL])),
        (s("Break pane to new tab"), s("Break out"), single_action_key(&km, &[A::BreakPane, TO_NORMAL])),
        (s("Break pane left/right"), s("Break"), action_key_group(&km, &[
            &[Action::BreakPaneLeft, TO_NORMAL],
            &[Action::BreakPaneRight, TO_NORMAL],
        ])),
        (s("Toggle"), s("Toggle"), single_action_key(&km, &[A::ToggleTab])),
        (s("Select pane"), s("Select"), to_basemode_key),
    ]} else if mi.mode == IM::Resize { vec![
        (s("Increase/Decrease size"), s("Increase/Decrease"),
            action_key_group(&km, &[
                &[A::Resize{resize: Resize::Increase, direction: None}],
                &[A::Resize{resize: Resize::Decrease, direction: None}]
            ])),
        (s("Increase to"), s("Increase"), action_key_group(&km, &[
            &[A::Resize{resize: Resize::Increase, direction: Some(Dir::Left)}],
            &[A::Resize{resize: Resize::Increase, direction: Some(Dir::Down)}],
            &[A::Resize{resize: Resize::Increase, direction: Some(Dir::Up)}],
            &[A::Resize{resize: Resize::Increase, direction: Some(Dir::Right)}]
            ])),
        (s("Decrease from"), s("Decrease"), action_key_group(&km, &[
            &[A::Resize{resize: Resize::Decrease, direction: Some(Dir::Left)}],
            &[A::Resize{resize: Resize::Decrease, direction: Some(Dir::Down)}],
            &[A::Resize{resize: Resize::Decrease, direction: Some(Dir::Up)}],
            &[A::Resize{resize: Resize::Decrease, direction: Some(Dir::Right)}]
            ])),
        (s("Select pane"), s("Select"), to_basemode_key),
    ]} else if mi.mode == IM::Move { vec![
        (s("Switch Location"), s("Move"), action_key_group(&km, &[
            &[Action::MovePane{direction: Some(Dir::Left)}], &[Action::MovePane{direction: Some(Dir::Down)}],
            &[Action::MovePane{direction: Some(Dir::Up)}], &[Action::MovePane{direction: Some(Dir::Right)}]])),
        (s("When done"), s("Back"), to_basemode_key),
    ]} else if mi.mode == IM::Scroll { vec![
        (s("Enter search term"), s("Search"),
            action_key(&km, &[A::SwitchToMode{input_mode: IM::EnterSearch}, A::SearchInput{input: vec![0]}])),
        (s("Scroll"), s("Scroll"),
            action_key_group(&km, &[&[Action::ScrollDown], &[Action::ScrollUp]])),
        (s("Scroll page"), s("Scroll"),
            action_key_group(&km, &[&[Action::PageScrollDown], &[Action::PageScrollUp]])),
        (s("Scroll half page"), s("Scroll"),
            action_key_group(&km, &[&[Action::HalfPageScrollDown], &[Action::HalfPageScrollUp]])),
        (s("Edit scrollback in default editor"), s("Edit"),
            single_action_key(&km, &[Action::EditScrollback { ansi: false }, TO_NORMAL])),
        (s("Select pane"), s("Select"), to_basemode_key),
    ]} else if mi.mode == IM::EnterSearch { vec![
        (s("When done"), s("Done"), action_key(&km, &[A::SwitchToMode{input_mode: IM::Search}])),
        (s("Cancel"), s("Cancel"),
            action_key(&km, &[A::SearchInput{input: vec![27]}, A::SwitchToMode{input_mode: IM::Scroll}])),
    ]} else if mi.mode == IM::Search { vec![
        (s("Enter Search term"), s("Search"),
            action_key(&km, &[A::SwitchToMode{input_mode: IM::EnterSearch}, A::SearchInput{input: vec![0]}])),
        (s("Scroll"), s("Scroll"),
            action_key_group(&km, &[&[Action::ScrollDown], &[Action::ScrollUp]])),
        (s("Scroll page"), s("Scroll"),
            action_key_group(&km, &[&[Action::PageScrollDown], &[Action::PageScrollUp]])),
        (s("Scroll half page"), s("Scroll"),
            action_key_group(&km, &[&[Action::HalfPageScrollDown], &[Action::HalfPageScrollUp]])),
        (s("Search down"), s("Down"), action_key(&km, &[A::Search{direction: SDir::Down}])),
        (s("Search up"), s("Up"), action_key(&km, &[A::Search{direction: SDir::Up}])),
        (s("Case sensitive"), s("Case"),
            action_key(&km, &[A::SearchToggleOption{option: SOpt::CaseSensitivity}])),
        (s("Wrap"), s("Wrap"),
            action_key(&km, &[A::SearchToggleOption{option: SOpt::Wrap}])),
        (s("Whole words"), s("Whole"),
            action_key(&km, &[A::SearchToggleOption{option: SOpt::WholeWord}])),
    ]} else if mi.mode == IM::Session { vec![
        (s("Detach"), s("Detach"), action_key(&km, &[Action::Detach])),
        (s("Session Manager"), s("Manager"), session_manager_key(&km)),
        (s("Share"), s("Share"), share_key(&km)),
        (s("Configure"), s("Config"), configuration_key(&km)),
        (s("Layout Manager"), s("Layouts"), layout_manager_key(&km)),
        (s("Plugin Manager"), s("Plugins"), plugin_manager_key(&km)),
        (s("About"), s("About"), about_key(&km)),
        (s("Select pane"), s("Select"), to_basemode_key),
    ]} else if mi.mode == IM::Agent { vec![
        (s("New agent"), s("New"), plugin_launch_key(&km, "hocus-new-agent")),
        (s("Duplicate agent"), s("Duplicate"), plugin_launch_key(&km, "hocus-duplicate-agent")),
        (s("Agents"), s("Agents"), plugin_launch_key(&km, "hocus-agents")),
        (s("Chat"), s("Chat"), plugin_launch_key(&km, "hocus")),
        (s("Ask"), s("Ask"), plugin_launch_key(&km, "hocus-ask")),
        (s("Select pane"), s("Select"), to_basemode_key),
    ]} else if mi.mode == IM::Tmux { vec![
        (s("Move focus"), s("Move"), action_key_group(&km, &[
            &[A::MoveFocus{direction: Dir::Left}], &[A::MoveFocus{direction: Dir::Down}],
            &[A::MoveFocus{direction: Dir::Up}], &[A::MoveFocus{direction: Dir::Right}]])),
        (s("Split down"), s("Down"), action_key(&km, &[A::NewPane{direction: Some(Dir::Down), pane_name: None, start_suppressed: false, global_placement: None}, TO_NORMAL])),
        (s("Split right"), s("Right"), action_key(&km, &[A::NewPane{direction: Some(Dir::Right), pane_name: None, start_suppressed: false, global_placement: None}, TO_NORMAL])),
        (s("Fullscreen"), s("Fullscreen"), action_key(&km, &[A::ToggleFocusFullscreen, TO_NORMAL])),
        (s("New tab"), s("New"), action_key(&km, &[A::NewTab{
            tiled_layout: None,
            floating_layouts: vec![],
            swap_tiled_layouts: None,
            swap_floating_layouts: None,
            tab_name: None,
            should_change_focus_to_new_tab: true,
            cwd: None,
            initial_panes: None,
            first_pane_unblock_condition: None,
        }, TO_NORMAL])),
        (s("Rename tab"), s("Rename"),
            action_key(&km, &[A::SwitchToMode{input_mode: IM::RenameTab}, A::TabNameInput{input: vec![0]}])),
        (s("Previous Tab"), s("Previous"), action_key(&km, &[A::GoToPreviousTab, TO_NORMAL])),
        (s("Next Tab"), s("Next"), action_key(&km, &[A::GoToNextTab, TO_NORMAL])),
        (s("Select pane"), s("Select"), to_basemode_key),
    ]} else if matches!(mi.mode, IM::RenamePane | IM::RenameTab) { vec![
        (s("When done"), s("Done"), to_basemode_key),
    ]} else { vec![] }
}

fn shortened_shortcut_list_nonstandard_mode(help: &ModeInfo) -> LinePart {
    let mut line_part = LinePart::default();
    let keys_and_hints = get_keys_and_hints(help);

    for (_, short, keys) in keys_and_hints.into_iter() {
        line_part.append(&add_shortcut(help, &short, &keys.to_vec(), false, Some(2)));
    }
    line_part
}

fn shortened_shortcut_list(help: &ModeInfo) -> LinePart {
    match help.mode {
        InputMode::Normal => LinePart::default(),
        InputMode::Locked => LinePart::default(),
        _ => shortened_shortcut_list_nonstandard_mode(help),
    }
}

fn best_effort_shortcut_list(help: &ModeInfo, max_len: usize) -> LinePart {
    let mut line_part = LinePart::default();
    let keys_and_hints = get_keys_and_hints(help);
    for (_, short, keys) in keys_and_hints.into_iter() {
        let shortcut = add_shortcut(help, &short, &keys.to_vec(), false, Some(2));
        if line_part.len + shortcut.len + MORE_MSG.chars().count() > max_len {
            line_part.part = format!("{}{}", line_part.part, MORE_MSG);
            line_part.len += MORE_MSG.chars().count();
            break;
        } else {
            line_part.append(&shortcut);
        }
    }
    line_part
}

fn single_action_key(
    keymap: &[(KeyWithModifier, Vec<Action>)],
    action: &[Action],
) -> Vec<KeyWithModifier> {
    let mut matching = keymap.iter().find_map(|(key, acvec)| {
        if acvec.iter().next() == action.iter().next() {
            Some(key.clone())
        } else {
            None
        }
    });
    if let Some(matching) = matching.take() {
        vec![matching]
    } else {
        vec![]
    }
}

fn session_manager_key(keymap: &[(KeyWithModifier, Vec<Action>)]) -> Vec<KeyWithModifier> {
    let mut matching = keymap.iter().find_map(|(key, acvec)| {
        let has_match = acvec
            .iter()
            .find(|a| a.launches_plugin("session-manager"))
            .is_some();
        if has_match {
            Some(key.clone())
        } else {
            None
        }
    });
    if let Some(matching) = matching.take() {
        vec![matching]
    } else {
        vec![]
    }
}

fn share_key(keymap: &[(KeyWithModifier, Vec<Action>)]) -> Vec<KeyWithModifier> {
    let mut matching = keymap.iter().find_map(|(key, acvec)| {
        let has_match = acvec
            .iter()
            .find(|a| a.launches_plugin("zellij:share"))
            .is_some();
        if has_match {
            Some(key.clone())
        } else {
            None
        }
    });
    if let Some(matching) = matching.take() {
        vec![matching]
    } else {
        vec![]
    }
}

fn plugin_manager_key(keymap: &[(KeyWithModifier, Vec<Action>)]) -> Vec<KeyWithModifier> {
    let mut matching = keymap.iter().find_map(|(key, acvec)| {
        let has_match = acvec
            .iter()
            .find(|a| a.launches_plugin("plugin-manager"))
            .is_some();
        if has_match {
            Some(key.clone())
        } else {
            None
        }
    });
    if let Some(matching) = matching.take() {
        vec![matching]
    } else {
        vec![]
    }
}

fn layout_manager_key(keymap: &[(KeyWithModifier, Vec<Action>)]) -> Vec<KeyWithModifier> {
    let mut matching = keymap.iter().find_map(|(key, acvec)| {
        let has_match = acvec
            .iter()
            .find(|a| a.launches_plugin("zellij:layout-manager"))
            .is_some();
        if has_match {
            Some(key.clone())
        } else {
            None
        }
    });
    if let Some(matching) = matching.take() {
        vec![matching]
    } else {
        vec![]
    }
}

fn about_key(keymap: &[(KeyWithModifier, Vec<Action>)]) -> Vec<KeyWithModifier> {
    let mut matching = keymap.iter().find_map(|(key, acvec)| {
        let has_match = acvec
            .iter()
            .find(|a| a.launches_plugin("zellij:about"))
            .is_some();
        if has_match {
            Some(key.clone())
        } else {
            None
        }
    });
    if let Some(matching) = matching.take() {
        vec![matching]
    } else {
        vec![]
    }
}

fn configuration_key(keymap: &[(KeyWithModifier, Vec<Action>)]) -> Vec<KeyWithModifier> {
    let mut matching = keymap.iter().find_map(|(key, acvec)| {
        let has_match = acvec
            .iter()
            .find(|a| a.launches_plugin("configuration"))
            .is_some();
        if has_match {
            Some(key.clone())
        } else {
            None
        }
    });
    if let Some(matching) = matching.take() {
        vec![matching]
    } else {
        vec![]
    }
}

fn style_key_with_modifier(
    keyvec: &[KeyWithModifier],
    color_index: Option<usize>,
    dimmed: bool,
) -> LinePart {
    if keyvec.is_empty() {
        return LinePart::default();
    }

    let common_modifiers = get_common_modifiers(keyvec.iter().collect());

    let no_common_modifier = common_modifiers.is_empty();
    let modifier_str = common_modifiers
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join("-");

    // Prints the keys
    let key = keyvec
        .iter()
        .map(|key| {
            if no_common_modifier || keyvec.len() == 1 {
                format!("{}", key)
            } else {
                format!("{}", key.strip_common_modifiers(&common_modifiers))
            }
        })
        .collect::<Vec<String>>();

    // Special handling of some pre-defined keygroups
    let key_string = key.join("");
    let key_separator = match &key_string[..] {
        "HJKL" => "",
        "hjkl" => "",
        "←↓↑→" => "",
        "←→" => "",
        "↓↑" => "",
        "[]" => "",
        _ => "|",
    };

    if no_common_modifier || key.len() == 1 {
        let key_string_text = format!(" {} ", key.join(key_separator));
        let text = if dimmed {
            Text::new(&key_string_text).disabled().opaque()
        } else if let Some(color_index) = color_index {
            Text::new(&key_string_text)
                .color_range(color_index, ..)
                .opaque()
        } else {
            Text::new(&key_string_text).opaque()
        };
        LinePart {
            part: serialize_text(&text),
            len: key_string_text.width(),
        }
    } else {
        let key_string_without_modifier = key.join(key_separator).to_string();
        let key_string_text = format!(" {} <{}> ", modifier_str, key_string_without_modifier);
        let text = if dimmed {
            Text::new(&key_string_text).disabled().opaque()
        } else if let Some(color_index) = color_index {
            Text::new(&key_string_text)
                .color_range(color_index, ..modifier_str.width() + 1)
                .color_range(
                    color_index,
                    modifier_str.width() + 3
                        ..modifier_str.width() + 3 + key_string_without_modifier.width(),
                )
                .opaque()
        } else {
            Text::new(&key_string_text).opaque()
        };
        LinePart {
            part: serialize_text(&text),
            len: key_string_text.width(),
        }
    }
}

fn get_common_modifiers(mut keyvec: Vec<&KeyWithModifier>) -> Vec<KeyModifier> {
    if keyvec.is_empty() {
        return vec![];
    }
    let mut common_modifiers = keyvec.pop().unwrap().key_modifiers.clone();
    for key in keyvec {
        common_modifiers = common_modifiers
            .intersection(&key.key_modifiers)
            .cloned()
            .collect();
    }
    common_modifiers.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl(character: char) -> Option<KeyWithModifier> {
        Some(KeyWithModifier::new_with_modifiers(
            BareKey::Char(character),
            BTreeSet::from([KeyModifier::Ctrl]),
        ))
    }

    fn alt(character: char) -> Option<KeyWithModifier> {
        Some(KeyWithModifier::new_with_modifiers(
            BareKey::Char(character),
            BTreeSet::from([KeyModifier::Alt]),
        ))
    }

    fn shortcut(action: KeyAction, key: Option<KeyWithModifier>) -> KeyShortcut {
        KeyShortcut::new(KeyMode::Unselected, action, key)
    }

    // The regression this guards: an AGENT tile with nothing bound to it short-circuited the whole
    // intersection, and every other tile then printed its own "Ctrl".
    #[test]
    fn an_unbound_shortcut_does_not_cost_the_others_their_common_modifier() {
        let key_shortcuts = HashMap::from([
            (
                InputMode::Normal,
                vec![
                    shortcut(KeyAction::Pane, ctrl('p')),
                    shortcut(KeyAction::Tab, ctrl('t')),
                    shortcut(KeyAction::Agent, None),
                ],
            ),
            (InputMode::Pane, vec![shortcut(KeyAction::Pane, ctrl('p'))]),
        ]);

        assert_eq!(
            common_modifiers_in_all_modes(&key_shortcuts),
            Some(vec![KeyModifier::Ctrl])
        );
    }

    // A mode whose only way back to normal is the shared `Enter`/`Esc` resolves to that key, which
    // carries no modifier. It must not empty the intersection either - agent mode looks exactly
    // like this in a config that has no `agent` block but does have the shared Enter/Esc binding.
    #[test]
    fn a_shortcut_that_fell_back_to_enter_does_not_cost_the_others_their_common_modifier() {
        let key_shortcuts = HashMap::from([
            (
                InputMode::Normal,
                vec![
                    shortcut(KeyAction::Pane, ctrl('p')),
                    shortcut(KeyAction::Tab, ctrl('t')),
                ],
            ),
            (
                InputMode::Agent,
                vec![shortcut(
                    KeyAction::Agent,
                    Some(KeyWithModifier::new(BareKey::Enter)),
                )],
            ),
        ]);

        assert_eq!(
            common_modifiers_in_all_modes(&key_shortcuts),
            Some(vec![KeyModifier::Ctrl])
        );
    }

    #[test]
    fn a_mode_with_no_shortcuts_does_not_cost_the_others_their_common_modifier() {
        let key_shortcuts = HashMap::from([
            (
                InputMode::Normal,
                vec![
                    shortcut(KeyAction::Pane, ctrl('p')),
                    shortcut(KeyAction::Tab, ctrl('t')),
                ],
            ),
            (InputMode::Agent, vec![]),
        ]);

        assert_eq!(
            common_modifiers_in_all_modes(&key_shortcuts),
            Some(vec![KeyModifier::Ctrl])
        );
    }

    // Modifiers that genuinely differ still cancel the prefix - there is nothing to factor out.
    #[test]
    fn a_bound_key_with_a_different_modifier_cancels_the_common_modifier() {
        let key_shortcuts = HashMap::from([(
            InputMode::Normal,
            vec![
                shortcut(KeyAction::Pane, ctrl('p')),
                shortcut(KeyAction::Tab, ctrl('t')),
                shortcut(KeyAction::Agent, alt('e')),
            ],
        )]);

        assert_eq!(common_modifiers_in_all_modes(&key_shortcuts), None);
    }

    #[test]
    fn nothing_bound_anywhere_has_no_common_modifier() {
        let key_shortcuts = HashMap::from([
            (
                InputMode::Normal,
                vec![
                    shortcut(KeyAction::Pane, None),
                    shortcut(KeyAction::Tab, None),
                ],
            ),
            (InputMode::Agent, vec![]),
        ]);

        assert_eq!(common_modifiers_in_all_modes(&key_shortcuts), None);
    }
}

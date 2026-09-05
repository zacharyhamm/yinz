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
use crate::keybind_hints::render_hints;
use crate::second_line::{
    ascended_to_host_session_hint, descended_into_nested_session_hint, system_clipboard_error,
    text_copied_hint,
};
use crate::status_block::{status_block, AgentCounts, Clock};
use crate::{action_key, color_elements};
use crate::{ColoredElements, LinePart};
use unicode_width::UnicodeWidthStr;

/// A rendered line plus the ranges the new-pane and floating-pane buttons occupy in it, for hover
/// and click hit-testing.
type LineWithButtonRanges = (LinePart, Option<(usize, usize)>, Option<(usize, usize)>);

// the render inputs of the whole status line
#[allow(clippy::too_many_arguments)]
/// What the right-hand slot draws, and which of its two tenants has it.
///
/// The Alt hint and the clock/agent block are alternatives in the same space: the hint is what
/// you want while you are reaching for a binding, the block is what you want the rest of the
/// time. `hint_revealed` is the whole of that decision - the plugin works out whether Alt is
/// down (or was, just now), or whether the pointer is on the bar, and this only obeys.
pub struct SecondaryInfo {
    pub hint_revealed: bool,
    pub agent_counts: AgentCounts,
    pub clock: Clock,
}

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
    secondary_info: &SecondaryInfo,
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
                    secondary_info,
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
                    line_part_to_render.append(&keybinds(help, max_len));
                }
            },
        }
    }
    (line_part_to_render, new_pane_range, floating_range)
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
    let select_binds = &help.get_keybinds_for_mode(InputMode::Select);
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
            InputMode::Select,
            vec![
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Unlock,
                    to_char(action_key(
                        select_binds,
                        &[Action::SwitchToMode {
                            input_mode: InputMode::Locked,
                        }],
                    )),
                ),
                KeyShortcut::new(
                    KeyMode::Selected,
                    KeyAction::Pane,
                    to_char(action_key(
                        select_binds,
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
    let select_binds = &help.get_keybinds_for_mode(InputMode::Select);
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
            InputMode::Select,
            vec![KeyShortcut::new(
                KeyMode::Selected,
                KeyAction::Pane,
                to_char(action_key(
                    select_binds,
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
    info: &SecondaryInfo,
) -> Option<LineWithButtonRanges> {
    let mut secondary_info = LinePart::default();
    let supports_arrow_fonts = !help.capabilities.arrow_fonts;
    let colored_elements = color_elements(
        help.style.colors,
        !supports_arrow_fonts,
        help.session_dimmed.unwrap_or(false),
    );
    // The two tenants of the right-hand slot. The hint only gets it when it has something to
    // draw - a mode with none of these actions bound, or a slot too narrow for the short form,
    // leaves it to the block rather than blanking it. Only the keybinds are clickable, so the
    // block hands back no hit ranges - the click handling already no-ops on `None`.
    let hint = info
        .hint_revealed
        .then(|| secondary_keybinds(help, tab_info, max_len, new_pane_hovered, floating_hovered))
        .flatten();
    let (content, new_pane_range, floating_range) = match hint {
        Some(hint) => hint,
        None => (
            status_block(help, &info.agent_counts, &info.clock, max_len),
            None,
            None,
        ),
    };
    secondary_info.append(&content);
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

/// The keybind hint for the right-hand slot, or `None` when it has nothing to say.
///
/// Two ways to have nothing: the current mode binds none of these actions - locked mode binds
/// only its way out, and everything here lives under `shared_except "locked"` - or the slot is
/// too narrow for even the short form. Either way the slot goes back to the clock rather than
/// being blanked, so a hover in locked mode is a no-op; see [`render_secondary_info`].
fn secondary_keybinds(
    help: &ModeInfo,
    tab_info: Option<&TabInfo>,
    max_len: usize,
    new_pane_hovered: bool,
    floating_hovered: bool,
) -> Option<LineWithButtonRanges> {
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
        return hint_with_content(secondary_info, new_pane_range, floating_range);
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
        hint_with_content(short_line, new_pane_range, floating_range)
    } else {
        // Neither form fits. An ellipsis in the slot says less than the clock does, and the
        // clock has its own narrower forms to fall back through.
        None
    }
}

/// A hint that drew nothing is not a hint - hand the slot back so the clock keeps it.
fn hint_with_content(
    line: LinePart,
    new_pane_range: Option<(usize, usize)>,
    floating_range: Option<(usize, usize)>,
) -> Option<LineWithButtonRanges> {
    (line.len > 0).then_some((line, new_pane_range, floating_range))
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

pub(crate) fn keybinds(help: &ModeInfo, max_width: usize) -> LinePart {
    render_hints(help, max_width, &|_is_first, text, keys| {
        add_shortcut(help, text, keys, false, Some(2))
    })
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

    use crate::test_support::{plain_text, with_shipped_keybinds};

    /// An arbitrary but fixed instant, read at UTC so the test does not depend on the host.
    const FIXED_MS: i64 = 1_757_000_000_000;

    /// The right-hand slot as text, for a mode and a width.
    fn secondary_slot(mode: InputMode, hint_revealed: bool, max_len: usize) -> String {
        let mode_info = with_shipped_keybinds(mode);
        let info = SecondaryInfo {
            hint_revealed,
            agent_counts: AgentCounts {
                busy: 2,
                waiting: 0,
                idle: 0,
            },
            clock: Clock::at(FIXED_MS, 0),
        };
        let (line, _, _) = render_secondary_info(&mode_info, None, max_len, false, false, &info)
            .expect("the slot fits at these widths");
        plain_text(line)
    }

    // The regression this guards: hovering the bar in locked mode revealed a hint that had
    // nothing to draw - locked mode binds none of these actions - and the slot went blank
    // instead of keeping the clock.
    #[test]
    fn a_hint_with_nothing_to_say_leaves_the_slot_to_the_clock() {
        let clock = Clock::at(FIXED_MS, 0);
        let revealed = secondary_slot(InputMode::Locked, true, 200);
        assert!(
            revealed.contains(&clock.time),
            "expected the clock in the slot, got {:?}",
            revealed
        );
        assert_eq!(revealed, secondary_slot(InputMode::Locked, false, 200));
    }

    #[test]
    fn a_hint_that_has_something_to_say_still_takes_the_slot() {
        let clock = Clock::at(FIXED_MS, 0);
        let revealed = secondary_slot(InputMode::Normal, true, 200);
        assert!(
            revealed.contains("New Pane") && revealed.contains("Floating"),
            "expected the shortcuts in the slot, got {:?}",
            revealed
        );
        assert!(!revealed.contains(&clock.time));
    }

    // A slot too narrow for even the short form used to draw "...", which says less than the
    // time does - and the clock has its own narrower forms to fall back through.
    #[test]
    fn a_hint_too_wide_for_the_slot_leaves_it_to_the_clock() {
        let clock = Clock::at(FIXED_MS, 0);
        let revealed = secondary_slot(InputMode::Normal, true, 12);
        assert!(
            revealed.contains(&clock.time) && !revealed.contains("..."),
            "expected the clock in the narrow slot, got {:?}",
            revealed
        );
    }

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

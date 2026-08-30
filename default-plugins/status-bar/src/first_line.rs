use ansi_term::{unstyled_len, ANSIStrings};
use zellij_tile::prelude::actions::Action;
use zellij_tile::prelude::*;

use crate::color_elements;
use crate::{
    action_key, action_key_group, get_common_modifiers, style_key_with_modifier, TO_NORMAL,
};
use crate::{ColoredElements, LinePart};

#[derive(Debug)]
pub struct KeyShortcut {
    pub mode: KeyMode,
    pub action: KeyAction,
    pub key: Option<KeyWithModifier>,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum KeyAction {
    Unlock,
    Lock,
    Pane,
    Tab,
    Resize,
    Search,
    Quit,
    Session,
    Move,
    Tmux,
    Agent,
}

#[derive(Debug, Copy, Clone)]
pub enum KeyMode {
    Unselected,
    UnselectedAlternate,
    Selected,
    Disabled,
}

impl KeyShortcut {
    pub fn new(mode: KeyMode, action: KeyAction, key: Option<KeyWithModifier>) -> Self {
        KeyShortcut { mode, action, key }
    }

    pub fn full_text(&self) -> String {
        match self.action {
            KeyAction::Lock => String::from("LOCK"),
            KeyAction::Unlock => String::from("UNLOCK"),
            KeyAction::Pane => String::from("PANE"),
            KeyAction::Tab => String::from("TAB"),
            KeyAction::Resize => String::from("SIZE"),
            KeyAction::Search => String::from("SEARCH"),
            KeyAction::Quit => String::from("QUIT"),
            // "SESS" rather than "SESSION", and "SIZE" above rather than "RESIZE": the AGENT tile
            // pushes the full-label ribbon over budget at 120 columns, and the renderer's answer to
            // that is to drop every label and show bare key letters. These two abbreviations buy
            // back enough width to keep the labels in both the one-line and the classic UI.
            KeyAction::Session => String::from("SESS"),
            KeyAction::Move => String::from("MOVE"),
            KeyAction::Tmux => String::from("TMUX"),
            KeyAction::Agent => String::from("AGENT"),
        }
    }
    pub fn with_shortened_modifiers(&self, common_modifiers: &[KeyModifier]) -> String {
        let key = match &self.key {
            Some(k) => k.strip_common_modifiers(common_modifiers),
            None => return String::from("?"),
        };
        let shortened_modifiers = key
            .key_modifiers
            .iter()
            .map(|m| match m {
                KeyModifier::Ctrl => "^C",
                KeyModifier::Alt => "^A",
                KeyModifier::Super => "^Su",
                KeyModifier::Shift => "^Sh",
            })
            .collect::<Vec<_>>()
            .join("-");
        if shortened_modifiers.is_empty() {
            format!("{}", key)
        } else {
            format!("{} {}", shortened_modifiers, key.bare_key)
        }
    }
    pub fn letter_shortcut(&self, common_modifiers: &[KeyModifier]) -> String {
        let key = match &self.key {
            Some(k) => k.strip_common_modifiers(common_modifiers),
            None => return String::from("?"),
        };
        format!("{}", key)
    }
    pub fn get_key(&self) -> Option<KeyWithModifier> {
        self.key.clone()
    }
    pub fn get_mode(&self) -> KeyMode {
        self.mode
    }
    pub fn get_action(&self) -> KeyAction {
        self.action
    }
    pub fn is_selected(&self) -> bool {
        matches!(self.mode, KeyMode::Selected)
    }
    pub fn short_text(&self) -> String {
        match self.action {
            KeyAction::Lock => String::from("Lo"),
            KeyAction::Unlock => String::from("Un"),
            KeyAction::Pane => String::from("Pa"),
            KeyAction::Tab => String::from("Ta"),
            KeyAction::Resize => String::from("Re"),
            KeyAction::Search => String::from("Se"),
            KeyAction::Quit => String::from("Qu"),
            KeyAction::Session => String::from("Se"),
            KeyAction::Move => String::from("Mo"),
            KeyAction::Tmux => String::from("Tm"),
            KeyAction::Agent => String::from("Ag"),
        }
    }
}

/// Generate long mode shortcut tile.
///
/// A long mode shortcut tile consists of a leading and trailing `separator`, a keybinding enclosed
/// in `<>` brackets and the name of the mode displayed in capitalized letters next to it. For
/// example, the default long mode shortcut tile for "Locked" mode is: ` <g> LOCK `.
///
/// # Arguments
///
/// - `key`: A [`KeyShortcut`] that defines how the tile is displayed (active/disabled/...), what
///   action it belongs to (roughly equivalent to [`InputMode`]s) and the keybinding to trigger
///   this action.
/// - `palette`: A structure holding styling information.
/// - `separator`: The separator printed before and after the mode shortcut tile. The default is an
///   arrow head-like separator.
/// - `shared_super`: If set to true, all mode shortcut keybindings share a common modifier (see
///   [`get_common_modifier`]) and the modifier belonging to the keybinding is **not** printed in
///   the shortcut tile.
/// - `first_tile`: If set to true, the leading separator for this tile will be ommited so no gap
///   appears on the screen.
fn long_mode_shortcut(
    key: &KeyShortcut,
    palette: ColoredElements,
    separator: &str,
    common_modifiers: &[KeyModifier],
    first_tile: bool,
) -> LinePart {
    let key_hint = key.full_text();
    let has_common_modifiers = !common_modifiers.is_empty();
    let key_binding = match (&key.mode, &key.key) {
        (KeyMode::Disabled, None) => "".to_string(),
        (_, None) => return LinePart::default(),
        (_, Some(_)) => key.letter_shortcut(common_modifiers),
    };

    let colors = match key.mode {
        KeyMode::Unselected => palette.unselected,
        KeyMode::UnselectedAlternate => palette.unselected_alternate,
        KeyMode::Selected => palette.selected,
        KeyMode::Disabled => palette.disabled,
    };
    let start_separator = if !has_common_modifiers && first_tile {
        ""
    } else {
        separator
    };
    let prefix_separator = colors.prefix_separator.paint(start_separator);
    let char_left_separator = colors.char_left_separator.paint(" <".to_string());
    let char_shortcut = colors.char_shortcut.paint(key_binding.to_string());
    let char_right_separator = colors.char_right_separator.paint("> ".to_string());
    let styled_text = colors.styled_text.paint(format!("{} ", key_hint));
    let suffix_separator = colors.suffix_separator.paint(separator);
    LinePart {
        part: ANSIStrings(&[
            prefix_separator,
            char_left_separator,
            char_shortcut,
            char_right_separator,
            styled_text,
            suffix_separator,
        ])
        .to_string(),
        len: start_separator.chars().count() // Separator
            + 2                              // " <"
            + key_binding.chars().count()    // Key binding
            + 2                              // "> "
            + key_hint.chars().count()       // Key hint (mode)
            + 1                              // " "
            + separator.chars().count(), // Separator
    }
}

fn shortened_modifier_shortcut(
    key: &KeyShortcut,
    palette: ColoredElements,
    separator: &str,
    common_modifiers: &[KeyModifier],
    first_tile: bool,
) -> LinePart {
    let key_hint = key.full_text();
    let has_common_modifiers = !common_modifiers.is_empty();
    let key_binding = match (&key.mode, &key.key) {
        (KeyMode::Disabled, None) => "".to_string(),
        (_, None) => return LinePart::default(),
        (_, Some(_)) => key.with_shortened_modifiers(common_modifiers),
    };

    let colors = match key.mode {
        KeyMode::Unselected => palette.unselected,
        KeyMode::UnselectedAlternate => palette.unselected_alternate,
        KeyMode::Selected => palette.selected,
        KeyMode::Disabled => palette.disabled,
    };
    let start_separator = if !has_common_modifiers && first_tile {
        ""
    } else {
        separator
    };
    let prefix_separator = colors.prefix_separator.paint(start_separator);
    let char_left_separator = colors.char_left_separator.paint(" <".to_string());
    let char_shortcut = colors.char_shortcut.paint(key_binding.to_string());
    let char_right_separator = colors.char_right_separator.paint("> ".to_string());
    let styled_text = colors.styled_text.paint(format!("{} ", key_hint));
    let suffix_separator = colors.suffix_separator.paint(separator);
    LinePart {
        part: ANSIStrings(&[
            prefix_separator,
            char_left_separator,
            char_shortcut,
            char_right_separator,
            styled_text,
            suffix_separator,
        ])
        .to_string(),
        len: start_separator.chars().count() // Separator
            + 2                              // " <"
            + key_binding.chars().count()    // Key binding
            + 2                              // "> "
            + key_hint.chars().count()       // Key hint (mode)
            + 1                              // " "
            + separator.chars().count(), // Separator
    }
}

/// Generate short mode shortcut tile.
///
/// A short mode shortcut tile consists of a leading and trailing `separator` and a keybinding. For
/// example, the default short mode shortcut tile for "Locked" mode is: ` g `.
///
/// # Arguments
///
/// - `key`: A [`KeyShortcut`] that defines how the tile is displayed (active/disabled/...), what
///   action it belongs to (roughly equivalent to [`InputMode`]s) and the keybinding to trigger
///   this action.
/// - `palette`: A structure holding styling information.
/// - `separator`: The separator printed before and after the mode shortcut tile. The default is an
///   arrow head-like separator.
/// - `shared_super`: If set to true, all mode shortcut keybindings share a common modifier (see
///   [`get_common_modifier`]) and the modifier belonging to the keybinding is **not** printed in
///   the shortcut tile.
/// - `first_tile`: If set to true, the leading separator for this tile will be ommited so no gap
///   appears on the screen.
fn short_mode_shortcut(
    key: &KeyShortcut,
    palette: ColoredElements,
    separator: &str,
    common_modifiers: &[KeyModifier],
    first_tile: bool,
) -> LinePart {
    let has_common_modifiers = !common_modifiers.is_empty();
    let key_binding = match (&key.mode, &key.key) {
        (KeyMode::Disabled, None) => "".to_string(),
        (_, None) => return LinePart::default(),
        (_, Some(_)) => key.letter_shortcut(common_modifiers),
    };

    let colors = match key.mode {
        KeyMode::Unselected => palette.unselected,
        KeyMode::UnselectedAlternate => palette.unselected_alternate,
        KeyMode::Selected => palette.selected,
        KeyMode::Disabled => palette.disabled,
    };
    let start_separator = if !has_common_modifiers && first_tile {
        ""
    } else {
        separator
    };
    let prefix_separator = colors.prefix_separator.paint(start_separator);
    let char_shortcut = colors.char_shortcut.paint(format!(" {} ", key_binding));
    let suffix_separator = colors.suffix_separator.paint(separator);
    LinePart {
        part: ANSIStrings(&[prefix_separator, char_shortcut, suffix_separator]).to_string(),
        len: start_separator.chars().count() // Separator (empty for the first tile)
            + 1                             // " "
            + key_binding.chars().count()   // Key binding
            + 1                             // " "
            + separator.chars().count(), // Separator
    }
}

fn key_indicators(
    max_len: usize,
    keys: &[KeyShortcut],
    palette: ColoredElements,
    separator: &str,
    mode_info: &ModeInfo,
) -> LinePart {
    // Print full-width hints
    let (shared_modifiers, mut line_part) = superkey(palette, separator, mode_info);
    for key in keys {
        let line_empty = line_part.len == 0;
        let key = long_mode_shortcut(key, palette, separator, &shared_modifiers, line_empty);
        line_part.part = format!("{}{}", line_part.part, key.part);
        line_part.len += key.len;
    }
    if line_part.len < max_len {
        return line_part;
    }

    // Full-width doesn't fit, try shortened modifiers (eg. "^C" instead of "Ctrl")
    line_part = superkey(palette, separator, mode_info).1;
    for key in keys {
        let line_empty = line_part.len == 0;
        let key =
            shortened_modifier_shortcut(key, palette, separator, &shared_modifiers, line_empty);
        line_part.part = format!("{}{}", line_part.part, key.part);
        line_part.len += key.len;
    }
    if line_part.len < max_len {
        return line_part;
    }

    // Full-width doesn't fit, try shortened hints (just keybindings, no meanings/actions)
    line_part = superkey(palette, separator, mode_info).1;
    for key in keys {
        let line_empty = line_part.len == 0;
        let key = short_mode_shortcut(key, palette, separator, &shared_modifiers, line_empty);
        line_part.part = format!("{}{}", line_part.part, key.part);
        line_part.len += key.len;
    }
    if line_part.len < max_len {
        return line_part;
    }

    // Shortened doesn't fit, print nothing
    line_part = LinePart::default();
    line_part
}

fn swap_layout_keycode(mode_info: &ModeInfo) -> LinePart {
    let mode_keybinds = mode_info.get_mode_keybinds();
    let prev_next_keys = action_key_group(
        &mode_keybinds,
        &[&[Action::PreviousSwapLayout], &[Action::NextSwapLayout]],
    );
    let prev_next_keys_indicator = style_key_with_modifier(
        &prev_next_keys,
        &mode_info.style.colors,
        Some(mode_info.style.colors.text_unselected.background),
    );
    let keycode = ANSIStrings(&prev_next_keys_indicator);
    let len = unstyled_len(&keycode);
    let part = keycode.to_string();
    LinePart { part, len }
}

fn swap_layout_status(
    max_len: usize,
    swap_layout_name: &Option<String>,
    is_swap_layout_damaged: bool,
    mode_info: &ModeInfo,
    colored_elements: ColoredElements,
    separator: &str,
) -> Option<LinePart> {
    match swap_layout_name {
        Some(swap_layout_name) => {
            let mut swap_layout_name = format!(" {} ", swap_layout_name);
            swap_layout_name.make_ascii_uppercase();
            let keycode = swap_layout_keycode(mode_info);
            // width of the indicator itself: the name between the two separators. Count chars,
            // not bytes, and count the separator rather than assuming it is one column - it is
            // empty when the terminal has no arrow fonts.
            let swap_layout_name_len =
                swap_layout_name.chars().count() + 2 * separator.chars().count();
            macro_rules! style_swap_layout_indicator {
                ($style_name:ident) => {{
                    (
                        colored_elements
                            .$style_name
                            .prefix_separator
                            .paint(separator),
                        colored_elements
                            .$style_name
                            .styled_text
                            .paint(&swap_layout_name),
                        colored_elements
                            .$style_name
                            .suffix_separator
                            .paint(separator),
                    )
                }};
            }
            let (prefix_separator, swap_layout_name, suffix_separator) =
                if mode_info.mode == InputMode::Locked {
                    style_swap_layout_indicator!(disabled)
                } else if is_swap_layout_damaged {
                    style_swap_layout_indicator!(unselected)
                } else {
                    style_swap_layout_indicator!(selected)
                };
            let swap_layout_indicator = format!(
                "{}{}{}",
                prefix_separator, swap_layout_name, suffix_separator
            );
            let (part, full_len) = if mode_info.mode == InputMode::Locked {
                (swap_layout_indicator.clone(), swap_layout_name_len)
            } else {
                (
                    format!(
                        "{}{}{}{}",
                        keycode,
                        colored_elements.superkey_prefix.paint(" "),
                        swap_layout_indicator,
                        colored_elements.superkey_prefix.paint(" ")
                    ),
                    keycode.len + swap_layout_name_len + 2, // the spaces on either side
                )
            };
            let short_len = swap_layout_name_len;
            if full_len <= max_len {
                Some(LinePart {
                    part,
                    len: full_len,
                })
            } else if short_len <= max_len && mode_info.mode != InputMode::Locked {
                Some(LinePart {
                    part: swap_layout_indicator,
                    len: short_len,
                })
            } else {
                None
            }
        },
        None => None,
    }
}

/// Get the keybindings for switching `InputMode`s and `Quit` visible in status bar.
///
/// Return a Vector of `Key`s where each `Key` is a shortcut to switch to some `InputMode` or Quit
/// zellij. Given the vast amount of things a user can configure in their zellij config, this
/// function has some limitations to keep in mind:
///
/// - The vector is not deduplicated: If switching to a certain `InputMode` is bound to multiple
///   `Key`s, all of these bindings will be part of the returned vector. There is also no
///   guaranteed sort order. Which key ends up in the status bar in such a situation isn't defined.
/// - The vector will **not** contain the ' ', '\n' and 'Esc' keys: These are the default bindings
///   to get back to normal mode from any input mode, but they aren't of interest when searching
///   for the super key. If for any input mode the user has bound only these keys to switching back
///   to `InputMode::Normal`, a '?' will be displayed as keybinding instead.
///
/// They say nothing about how the user reaches a mode, so anything reasoning about mode shortcuts -
/// which key to show for a mode, which modifier they all share - has to look past them.
pub fn is_generic_to_normal_key(key: &KeyWithModifier) -> bool {
    matches!(
        key,
        KeyWithModifier {
            bare_key: BareKey::Char(' '),
            ..
        } | KeyWithModifier {
            bare_key: BareKey::Enter,
            ..
        } | KeyWithModifier {
            bare_key: BareKey::Esc,
            ..
        }
    )
}

pub fn mode_switch_keys(mode_info: &ModeInfo) -> Vec<KeyWithModifier> {
    mode_info
        .get_mode_keybinds()
        .iter()
        .filter_map(|(key, vac)| match vac.first() {
            // No actions defined, ignore
            None => None,
            Some(vac) => {
                if is_generic_to_normal_key(key) {
                    return None;
                }
                if let actions::Action::SwitchToMode { input_mode: mode } = vac {
                    return match mode {
                        // Store the keys that switch to displayed modes
                        InputMode::Normal
                        | InputMode::Locked
                        | InputMode::Pane
                        | InputMode::Tab
                        | InputMode::Resize
                        | InputMode::Move
                        | InputMode::Scroll
                        | InputMode::Session
                        | InputMode::Agent => Some(key.clone()),
                        _ => None,
                    };
                }
                if let actions::Action::Quit = vac {
                    return Some(key.clone());
                }
                // Not a `SwitchToMode` or `Quit` action, ignore
                None
            },
        })
        .collect()
}

pub fn superkey(
    palette: ColoredElements,
    separator: &str,
    mode_info: &ModeInfo,
) -> (Vec<KeyModifier>, LinePart) {
    // Find a common modifier if any
    let common_modifiers = get_common_modifiers(mode_switch_keys(mode_info).iter().collect());
    if common_modifiers.is_empty() {
        return (common_modifiers, LinePart::default());
    }

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

    let prefix = palette.superkey_prefix.paint(&prefix_text);
    let suffix_separator = palette.superkey_suffix_separator.paint(separator);
    (
        common_modifiers,
        LinePart {
            part: ANSIStrings(&[prefix, suffix_separator]).to_string(),
            len: prefix_text.chars().count() + separator.chars().count(),
        },
    )
}

pub fn to_char(kv: Vec<KeyWithModifier>) -> Option<KeyWithModifier> {
    let key = kv.iter().find(|key| !is_generic_to_normal_key(key));
    // Maybe the user bound one of the ignored keys?
    if key.is_none() {
        return kv.first().cloned();
    }
    key.cloned()
}

/// Get the [`KeyShortcut`] for a specific [`InputMode`].
///
/// Iterates over the contents of `shortcuts` to find the [`KeyShortcut`] with the [`KeyAction`]
/// matching the [`InputMode`]. Returns a mutable reference to the entry in `shortcuts` if a match
/// is found or `None` otherwise.
///
/// In case multiple entries in `shortcuts` match `mode` (which shouldn't happen), the first match
/// is returned.
fn get_key_shortcut_for_mode<'a>(
    shortcuts: &'a mut [KeyShortcut],
    mode: &InputMode,
) -> Option<&'a mut KeyShortcut> {
    let key_action = match mode {
        InputMode::Normal | InputMode::Prompt | InputMode::Tmux => return None,
        InputMode::Locked => KeyAction::Lock,
        InputMode::Pane | InputMode::RenamePane => KeyAction::Pane,
        InputMode::Tab | InputMode::RenameTab => KeyAction::Tab,
        InputMode::Resize => KeyAction::Resize,
        InputMode::Move => KeyAction::Move,
        InputMode::Scroll | InputMode::Search | InputMode::EnterSearch => KeyAction::Search,
        InputMode::Session => KeyAction::Session,
        InputMode::Agent => KeyAction::Agent,
    };
    shortcuts
        .iter_mut()
        .find(|shortcut| shortcut.action == key_action)
}

pub fn first_line(
    help: &ModeInfo,
    tab_info: Option<&TabInfo>,
    max_len: usize,
    separator: &str,
) -> LinePart {
    // no terminal is this wide; the cap stops an absurd `max_len` from building a string of
    // that length one space at a time below
    const MAX_LINE_WIDTH: usize = 10_000;
    let max_len = max_len.min(MAX_LINE_WIDTH);
    let supports_arrow_fonts = !help.capabilities.arrow_fonts;
    let colored_elements = color_elements(help.style.colors, !supports_arrow_fonts, false);
    let binds = &help.get_mode_keybinds();
    // Unselect all by default
    let mut default_keys = vec![
        KeyShortcut::new(
            KeyMode::Unselected,
            KeyAction::Lock,
            to_char(action_key(
                binds,
                &[Action::SwitchToMode {
                    input_mode: InputMode::Locked,
                }],
            )),
        ),
        KeyShortcut::new(
            KeyMode::UnselectedAlternate,
            KeyAction::Pane,
            to_char(action_key(
                binds,
                &[Action::SwitchToMode {
                    input_mode: InputMode::Pane,
                }],
            )),
        ),
        KeyShortcut::new(
            KeyMode::Unselected,
            KeyAction::Tab,
            to_char(action_key(
                binds,
                &[Action::SwitchToMode {
                    input_mode: InputMode::Tab,
                }],
            )),
        ),
        KeyShortcut::new(
            KeyMode::UnselectedAlternate,
            KeyAction::Resize,
            to_char(action_key(
                binds,
                &[Action::SwitchToMode {
                    input_mode: InputMode::Resize,
                }],
            )),
        ),
        KeyShortcut::new(
            KeyMode::Unselected,
            KeyAction::Move,
            to_char(action_key(
                binds,
                &[Action::SwitchToMode {
                    input_mode: InputMode::Move,
                }],
            )),
        ),
        KeyShortcut::new(
            KeyMode::UnselectedAlternate,
            KeyAction::Search,
            to_char(action_key(
                binds,
                &[Action::SwitchToMode {
                    input_mode: InputMode::Scroll,
                }],
            )),
        ),
        KeyShortcut::new(
            KeyMode::Unselected,
            KeyAction::Session,
            to_char(action_key(
                binds,
                &[Action::SwitchToMode {
                    input_mode: InputMode::Session,
                }],
            )),
        ),
        KeyShortcut::new(
            KeyMode::UnselectedAlternate,
            KeyAction::Agent,
            to_char(action_key(
                binds,
                &[Action::SwitchToMode {
                    input_mode: InputMode::Agent,
                }],
            )),
        ),
        KeyShortcut::new(
            KeyMode::Unselected,
            KeyAction::Quit,
            to_char(action_key(binds, &[Action::Quit])),
        ),
    ];

    if let Some(key_shortcut) = get_key_shortcut_for_mode(&mut default_keys, &help.mode) {
        key_shortcut.mode = KeyMode::Selected;
        key_shortcut.key = to_char(action_key(binds, &[TO_NORMAL]));
    }

    // In locked mode we must disable all other mode keybindings
    if help.mode == InputMode::Locked {
        for key in default_keys.iter_mut().skip(1) {
            key.mode = KeyMode::Disabled;
        }
    }

    if help.mode == InputMode::Tmux {
        // Tmux tile is hidden by default
        default_keys.push(KeyShortcut::new(
            KeyMode::Selected,
            KeyAction::Tmux,
            to_char(action_key(binds, &[TO_NORMAL])),
        ));
    }

    let mut key_indicators =
        key_indicators(max_len, &default_keys, colored_elements, separator, help);
    if key_indicators.len < max_len {
        if let Some(tab_info) = tab_info {
            let mut remaining_space = max_len - key_indicators.len;
            if let Some(swap_layout_status) = swap_layout_status(
                remaining_space,
                &tab_info.active_swap_layout_name,
                tab_info.is_swap_layout_dirty,
                help,
                colored_elements,
                separator,
            ) {
                remaining_space -= swap_layout_status.len;
                for _ in 0..remaining_space {
                    key_indicators.part.push_str(
                        &ANSIStrings(&[colored_elements.superkey_prefix.paint(" ")]).to_string(),
                    );
                    key_indicators.len += 1;
                }
                key_indicators.append(&swap_layout_status);
            }
        }
    }
    key_indicators
}

#[cfg(test)]
/// Unit tests.
///
/// Note that we cheat a little here, because the number of things one may want to test is endless,
/// and creating a Mockup of [`ModeInfo`] by hand for all these testcases is nothing less than
/// torture. Hence, we test the most atomic units thoroughly ([`long_mode_shortcut`] and
/// [`short_mode_shortcut`]) and then test the public API ([`first_line`]) to ensure correct
/// operation.
mod tests {
    use super::*;
    use hegel::generators;

    fn all_input_modes() -> Vec<InputMode> {
        vec![
            InputMode::Normal,
            InputMode::Locked,
            InputMode::Resize,
            InputMode::Pane,
            InputMode::Tab,
            InputMode::Scroll,
            InputMode::EnterSearch,
            InputMode::Search,
            InputMode::RenameTab,
            InputMode::RenamePane,
            InputMode::Session,
            InputMode::Move,
            InputMode::Prompt,
            InputMode::Tmux,
            InputMode::Agent,
        ]
    }

    /// Hand-mocking [`ModeInfo`] is, as the module doc above says, torture -- so build it once
    /// here and reuse it.
    #[hegel::composite]
    fn any_mode_info(tc: &hegel::TestCase) -> ModeInfo {
        let mode = tc.draw(generators::sampled_from(all_input_modes()));
        let bindings = tc.draw(generators::vecs(generators::tuples!(
            generators::text().min_size(1).max_size(1),
            generators::sampled_from(all_input_modes()),
            generators::booleans(),
        )));
        let keybinds = vec![(
            mode,
            bindings
                .into_iter()
                .map(|(key, target, ctrl)| {
                    let bare = KeyWithModifier::new(BareKey::Char(key.chars().next().unwrap()));
                    let bare = if ctrl {
                        bare.with_ctrl_modifier()
                    } else {
                        bare
                    };
                    (bare, vec![Action::SwitchToMode { input_mode: target }])
                })
                .collect(),
        )];
        ModeInfo {
            mode,
            keybinds,
            capabilities: PluginCapabilities {
                arrow_fonts: tc.draw(generators::booleans()),
                // The status bar draws no images, so this never changes what it renders.
                kitty_graphics: false,
            },
            ..ModeInfo::default()
        }
    }

    /// The swap layout name is the interesting part: it is drawn from full Unicode, while
    /// `swap_layout_status` measures it in bytes.
    #[hegel::composite]
    fn any_tab_info(tc: &hegel::TestCase) -> TabInfo {
        TabInfo {
            active_swap_layout_name: tc.draw(generators::optional(generators::text())),
            is_swap_layout_dirty: tc.draw(generators::booleans()),
            ..TabInfo::default()
        }
    }

    /// A `max_len` no terminal could produce must not be taken literally - the padding loop
    /// would build a string of that length.
    #[test]
    fn first_line_clamps_an_absurd_width() {
        let part = first_line(&ModeInfo::default(), None, usize::MAX, ">");
        assert!(
            part.len <= 10_000,
            "first_line claimed {} columns for max_len usize::MAX",
            part.len
        );
    }

    /// `first_line` must never claim more columns than it was given, and the width it reports
    /// must be the width it actually renders.
    #[hegel::test]
    fn first_line_respects_the_width_it_is_given(tc: hegel::TestCase) {
        let mode_info = tc.draw(any_mode_info());
        let tab_info = tc.draw(generators::optional(any_tab_info()));
        // bounded for speed, not to dodge a defect: the padding loop emits one styled space per
        // column, so a 10_000-column draw dominates the runtime of the whole suite.
        // `first_line_clamps_an_absurd_width` covers the top end.
        let max_len = tc.draw(generators::integers::<usize>().max_value(500));
        let separator = tc.draw(generators::sampled_from(vec![">", "", "|", "\u{e0b0}"]));

        let part = first_line(&mode_info, tab_info.as_ref(), max_len, separator);
        let claimed_len = part.len;
        let rendered = unstyle(part);
        let actual_len = rendered.chars().count();

        assert!(
            claimed_len <= max_len,
            "first_line claimed {} columns but was given {} (mode {:?}, swap layout {:?}, separator {:?})",
            claimed_len,
            max_len,
            mode_info.mode,
            tab_info.as_ref().and_then(|t| t.active_swap_layout_name.clone()),
            separator
        );
        assert_eq!(
            claimed_len,
            actual_len,
            "first_line claimed {} columns but rendered {} ({:?}) (max_len {}, mode {:?}, swap layout {:?}, separator {:?})",
            claimed_len,
            actual_len,
            rendered,
            max_len,
            mode_info.mode,
            tab_info.as_ref().and_then(|t| t.active_swap_layout_name.clone()),
            separator
        );
    }

    fn colored_elements() -> ColoredElements {
        let palette = Styling::default();
        color_elements(palette, false, false)
    }

    // Strip style information from `LinePart` and return a raw String instead
    fn unstyle(line_part: LinePart) -> String {
        let string = line_part.to_string();

        let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
        let string = re.replace_all(&string, "".to_string());

        string.to_string()
    }

    #[test]
    fn long_mode_shortcut_selected_with_binding() {
        let key = KeyShortcut::new(
            KeyMode::Selected,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0'))),
        );
        let color = colored_elements();

        let ret = long_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ <0> SESS +".to_string());
    }

    #[test]
    // Displayed like selected(alternate), but different styling
    fn long_mode_shortcut_unselected_with_binding() {
        let key = KeyShortcut::new(
            KeyMode::Unselected,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0'))),
        );
        let color = colored_elements();

        let ret = long_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ <0> SESS +".to_string());
    }

    #[test]
    // Treat exactly like "unselected" variant
    fn long_mode_shortcut_unselected_alternate_with_binding() {
        let key = KeyShortcut::new(
            KeyMode::UnselectedAlternate,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0'))),
        );
        let color = colored_elements();

        let ret = long_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ <0> SESS +".to_string());
    }

    #[test]
    // KeyShortcuts without binding are only displayed when "disabled" (for locked mode indications)
    fn long_mode_shortcut_selected_without_binding() {
        let key = KeyShortcut::new(KeyMode::Selected, KeyAction::Session, None);
        let color = colored_elements();

        let ret = long_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "".to_string());
    }

    #[test]
    // First tile doesn't print a starting separator
    fn long_mode_shortcut_selected_with_binding_first_tile() {
        let key = KeyShortcut::new(
            KeyMode::Selected,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0'))),
        );
        let color = colored_elements();

        let ret = long_mode_shortcut(&key, color, "+", &[], true);
        let ret = unstyle(ret);

        assert_eq!(ret, " <0> SESS +".to_string());
    }

    #[test]
    // Modifier is the superkey, mustn't appear in angled brackets
    fn long_mode_shortcut_selected_with_ctrl_binding_shared_superkey() {
        let key = KeyShortcut::new(
            KeyMode::Selected,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0')).with_ctrl_modifier()),
        );
        let color = colored_elements();

        let ret = long_mode_shortcut(&key, color, "+", &[KeyModifier::Ctrl], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ <0> SESS +".to_string());
    }

    #[test]
    // Modifier must be in the angled brackets
    fn long_mode_shortcut_selected_with_ctrl_binding_no_shared_superkey() {
        let key = KeyShortcut::new(
            KeyMode::Selected,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0')).with_ctrl_modifier()),
        );
        let color = colored_elements();

        let ret = long_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ <Ctrl 0> SESS +".to_string());
    }

    #[test]
    // Must be displayed as usual, but it is styled to be greyed out which we don't test here
    fn long_mode_shortcut_disabled_with_binding() {
        let key = KeyShortcut::new(
            KeyMode::Disabled,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0'))),
        );
        let color = colored_elements();

        let ret = long_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ <0> SESS +".to_string());
    }

    #[test]
    // Must be displayed but without keybinding
    fn long_mode_shortcut_disabled_without_binding() {
        let key = KeyShortcut::new(KeyMode::Disabled, KeyAction::Session, None);
        let color = colored_elements();

        let ret = long_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ <> SESS +".to_string());
    }

    #[test]
    // Test all at once
    // Note that when "shared_super" is true, the tile **cannot** be the first on the line, so we
    // ignore **first** here.
    fn long_mode_shortcut_selected_with_ctrl_binding_and_shared_super_and_first_tile() {
        let key = KeyShortcut::new(
            KeyMode::Selected,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0')).with_ctrl_modifier()),
        );
        let color = colored_elements();

        let ret = long_mode_shortcut(&key, color, "+", &[KeyModifier::Ctrl], true);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ <0> SESS +".to_string());
    }

    #[test]
    fn short_mode_shortcut_selected_with_binding() {
        let key = KeyShortcut::new(
            KeyMode::Selected,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0'))),
        );
        let color = colored_elements();

        let ret = short_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ 0 +".to_string());
    }

    #[test]
    fn short_mode_shortcut_selected_with_ctrl_binding_no_shared_super() {
        let key = KeyShortcut::new(
            KeyMode::Selected,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0')).with_ctrl_modifier()),
        );
        let color = colored_elements();

        let ret = short_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ Ctrl 0 +".to_string());
    }

    #[test]
    fn short_mode_shortcut_selected_with_ctrl_binding_shared_super() {
        let key = KeyShortcut::new(
            KeyMode::Selected,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0')).with_ctrl_modifier()),
        );
        let color = colored_elements();

        let ret = short_mode_shortcut(&key, color, "+", &[KeyModifier::Ctrl], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ 0 +".to_string());
    }

    #[test]
    fn short_mode_shortcut_selected_with_binding_first_tile() {
        let key = KeyShortcut::new(
            KeyMode::Selected,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0'))),
        );
        let color = colored_elements();

        let ret = short_mode_shortcut(&key, color, "+", &[], true);
        let ret = unstyle(ret);

        assert_eq!(ret, " 0 +".to_string());
    }

    #[test]
    fn short_mode_shortcut_unselected_with_binding() {
        let key = KeyShortcut::new(
            KeyMode::Unselected,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0'))),
        );
        let color = colored_elements();

        let ret = short_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ 0 +".to_string());
    }

    #[test]
    fn short_mode_shortcut_unselected_alternate_with_binding() {
        let key = KeyShortcut::new(
            KeyMode::UnselectedAlternate,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0'))),
        );
        let color = colored_elements();

        let ret = short_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ 0 +".to_string());
    }

    #[test]
    fn short_mode_shortcut_disabled_with_binding() {
        let key = KeyShortcut::new(
            KeyMode::Selected,
            KeyAction::Session,
            Some(KeyWithModifier::new(BareKey::Char('0'))),
        );
        let color = colored_elements();

        let ret = short_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "+ 0 +".to_string());
    }

    #[test]
    fn short_mode_shortcut_selected_without_binding() {
        let key = KeyShortcut::new(KeyMode::Selected, KeyAction::Session, None);
        let color = colored_elements();

        let ret = short_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "".to_string());
    }

    #[test]
    fn short_mode_shortcut_unselected_without_binding() {
        let key = KeyShortcut::new(KeyMode::Unselected, KeyAction::Session, None);
        let color = colored_elements();

        let ret = short_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "".to_string());
    }

    #[test]
    fn short_mode_shortcut_unselected_alternate_without_binding() {
        let key = KeyShortcut::new(KeyMode::UnselectedAlternate, KeyAction::Session, None);
        let color = colored_elements();

        let ret = short_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "".to_string());
    }

    #[test]
    fn short_mode_shortcut_disabled_without_binding() {
        let key = KeyShortcut::new(KeyMode::Selected, KeyAction::Session, None);
        let color = colored_elements();

        let ret = short_mode_shortcut(&key, color, "+", &[], false);
        let ret = unstyle(ret);

        assert_eq!(ret, "".to_string());
    }

    #[test]
    // Observe: Modes missing in between aren't displayed!
    fn first_line_default_layout_shared_super() {
        #[rustfmt::skip]
        let mode_info = ModeInfo{
            mode: InputMode::Normal,
            keybinds : vec![
                (InputMode::Normal, vec![
                    (KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Pane}]),
                    (KeyWithModifier::new(BareKey::Char('b')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Resize}]),
                    (KeyWithModifier::new(BareKey::Char('c')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Move}]),
                ]),
            ],
            ..ModeInfo::default()
        };

        let ret = first_line(&mode_info, None, 500, ">");
        let ret = unstyle(ret);

        assert_eq!(
            ret,
            " Ctrl + >> <a> PANE >> <b> SIZE >> <c> MOVE >".to_string()
        );
    }

    #[test]
    fn first_line_default_layout_no_shared_super() {
        #[rustfmt::skip]
        let mode_info = ModeInfo{
            mode: InputMode::Normal,
            keybinds : vec![
                (InputMode::Normal, vec![
                    (KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Pane}]),
                    (KeyWithModifier::new(BareKey::Char('b')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Resize}]),
                    (KeyWithModifier::new(BareKey::Char('c')), vec![Action::SwitchToMode{input_mode: InputMode::Move}]),
                ]),
            ],
            ..ModeInfo::default()
        };

        let ret = first_line(&mode_info, None, 500, ">");
        let ret = unstyle(ret);

        assert_eq!(
            ret,
            " <Ctrl a> PANE >> <Ctrl b> SIZE >> <c> MOVE >".to_string()
        );
    }

    #[test]
    fn first_line_default_layout_unprintables() {
        #[rustfmt::skip]
        let mode_info = ModeInfo{
            mode: InputMode::Normal,
            keybinds : vec![
                (InputMode::Normal, vec![
                    (KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Locked}]),
                    (KeyWithModifier::new(BareKey::Backspace), vec![Action::SwitchToMode{input_mode: InputMode::Pane}]),
                    (KeyWithModifier::new(BareKey::Enter), vec![Action::SwitchToMode{input_mode: InputMode::Tab}]),
                    (KeyWithModifier::new(BareKey::Tab), vec![Action::SwitchToMode{input_mode: InputMode::Resize}]),
                    (KeyWithModifier::new(BareKey::Left), vec![Action::SwitchToMode{input_mode: InputMode::Move}]),
                ]),
            ],
            ..ModeInfo::default()
        };

        let ret = first_line(&mode_info, None, 500, ">");
        let ret = unstyle(ret);

        assert_eq!(
            ret,
            " <Ctrl a> LOCK >> <BACKSPACE> PANE >> <ENTER> TAB >> <TAB> SIZE >> <←> MOVE >"
                .to_string()
        );
    }

    #[test]
    fn first_line_short_layout_shared_super() {
        #[rustfmt::skip]
        let mode_info = ModeInfo{
            mode: InputMode::Normal,
            keybinds : vec![
                (InputMode::Normal, vec![
                    (KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Locked}]),
                    (KeyWithModifier::new(BareKey::Char('b')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Pane}]),
                    (KeyWithModifier::new(BareKey::Char('c')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Tab}]),
                    (KeyWithModifier::new(BareKey::Char('d')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Resize}]),
                    (KeyWithModifier::new(BareKey::Char('e')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Move}]),
                ]),
            ],
            ..ModeInfo::default()
        };

        let ret = first_line(&mode_info, None, 50, ">");
        let ret = unstyle(ret);

        assert_eq!(ret, " Ctrl + >> a >> b >> c >> d >> e >".to_string());
    }

    #[test]
    fn first_line_short_simplified_ui_shared_super() {
        #[rustfmt::skip]
        let mode_info = ModeInfo{
            mode: InputMode::Normal,
            keybinds : vec![
                (InputMode::Normal, vec![
                    (KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Pane}]),
                    (KeyWithModifier::new(BareKey::Char('b')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Resize}]),
                    (KeyWithModifier::new(BareKey::Char('c')).with_ctrl_modifier(), vec![Action::SwitchToMode{input_mode: InputMode::Move}]),
                ]),
            ],
            ..ModeInfo::default()
        };

        let ret = first_line(&mode_info, None, 30, "");
        let ret = unstyle(ret);

        assert_eq!(ret, " Ctrl +  a  b  c ".to_string());
    }
}

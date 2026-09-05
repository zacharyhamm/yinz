mod first_line;
mod keybind_hints;
mod one_line_ui;
mod second_line;
mod status_block;
mod tip;

use ansi_term::{
    ANSIString,
    Colour::{Fixed, RGB},
    Style,
};

use std::collections::BTreeMap;
use std::fmt::{Display, Error, Formatter};
use zellij_tile::prelude::actions::Action;
use zellij_tile::prelude::*;
use zellij_tile_utils::{palette_match, style};

use first_line::first_line;
use one_line_ui::{one_line_ui, SecondaryInfo};
use second_line::{
    ascended_to_host_session_hint, descended_into_nested_session_hint, floating_panes_are_visible,
    fullscreen_panes_to_hide, keybinds, locked_floating_panes_are_visible,
    locked_fullscreen_panes_to_hide, system_clipboard_error, text_copied_hint,
};
use status_block::{AgentCounts, Clock};
use tip::utils::get_cached_tip_name;

// for more of these, copy paste from: https://en.wikipedia.org/wiki/Box-drawing_character
static ARROW_SEPARATOR: &str = "";
static MORE_MSG: &str = " ... ";
/// Shorthand for `Action::SwitchToMode{input_mode: InputMode::Normal}`.
const TO_NORMAL: Action = Action::SwitchToMode {
    input_mode: InputMode::Normal,
};

#[derive(Default)]
struct State {
    tabs: Vec<TabInfo>,
    tip_name: String,
    mode_info: ModeInfo,
    text_copy_destination: Option<CopyDestination>,
    display_system_clipboard_failure: bool,
    classic_ui: bool,
    base_mode_is_locked: bool,
    cached_keybinds: KeybindsVec,
    new_pane_ribbon_range: Option<(usize, usize)>,
    floating_ribbon_range: Option<(usize, usize)>,
    new_pane_ribbon_hovered: bool,
    floating_ribbon_hovered: bool,
    /// What the bottom-right draws when the Alt hint is not covering it.
    agent_counts: AgentCounts,
    clock: Clock,
    /// Seconds east of UTC. Only the host can say (`get_local_utc_offset`), and it is a
    /// synchronous round trip, so it is asked for on load and then only when the hour it was
    /// taken in has passed - which is the granularity a DST change moves at.
    utc_offset_secs: i32,
    utc_offset_taken_at_ms: i64,
    /// While this is in the future, the Alt hint has the right-hand slot instead of the clock.
    /// Set by `Event::ModifiersHeld`; see `alt_hint_revealed`.
    alt_hint_until_ms: i64,
}

register_plugin!(State);

#[derive(Default)]
pub struct LinePart {
    part: String,
    len: usize,
}

impl LinePart {
    pub fn append(&mut self, to_append: &LinePart) {
        self.part.push_str(&to_append.part);
        self.len += to_append.len;
    }
}

impl Display for LinePart {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        write!(f, "{}", self.part)
    }
}

#[derive(Clone, Copy)]
pub struct ColoredElements {
    pub selected: SegmentStyle,
    pub unselected: SegmentStyle,
    pub unselected_alternate: SegmentStyle,
    pub disabled: SegmentStyle,
    // superkey
    pub superkey_prefix: Style,
    pub superkey_suffix_separator: Style,
}

#[derive(Clone, Copy)]
pub struct SegmentStyle {
    pub prefix_separator: Style,
    pub char_left_separator: Style,
    pub char_shortcut: Style,
    pub char_right_separator: Style,
    pub styled_text: Style,
    pub suffix_separator: Style,
}

// I really hate this, but I can't come up with a good solution for this,
// we need different colors from palette for the default theme
// plus here we can add new sources in the future, like Theme
// that can be defined in the config perhaps
fn color_elements(
    palette: Styling,
    different_color_alternates: bool,
    dimmed: bool,
) -> ColoredElements {
    if dimmed {
        let background = palette.text_unselected.background;
        let foreground = palette.text_unselected.base;
        let ribbon_background = palette.ribbon_unselected.background;
        let italic_on_ribbon = style!(palette.ribbon_unselected.base, ribbon_background).italic();
        let dim_segment = SegmentStyle {
            prefix_separator: style!(background, ribbon_background),
            char_left_separator: italic_on_ribbon,
            char_shortcut: italic_on_ribbon,
            char_right_separator: italic_on_ribbon,
            styled_text: italic_on_ribbon,
            suffix_separator: style!(ribbon_background, background),
        };
        return ColoredElements {
            selected: dim_segment,
            unselected: dim_segment,
            unselected_alternate: dim_segment,
            disabled: dim_segment,
            superkey_prefix: style!(foreground, background).italic(),
            superkey_suffix_separator: style!(background, background),
        };
    }
    let background = palette.text_unselected.background;
    let foreground = palette.text_unselected.base;
    let alternate_background_color = if different_color_alternates {
        palette.ribbon_unselected.base
    } else {
        palette.ribbon_unselected.background
    };
    ColoredElements {
        selected: SegmentStyle {
            prefix_separator: style!(background, palette.ribbon_selected.background),
            char_left_separator: style!(
                palette.ribbon_selected.base,
                palette.ribbon_selected.background
            )
            .bold(),
            char_shortcut: style!(
                palette.ribbon_selected.emphasis_0,
                palette.ribbon_selected.background
            )
            .bold(),
            char_right_separator: style!(
                palette.ribbon_selected.base,
                palette.ribbon_selected.background
            )
            .bold(),
            styled_text: style!(
                palette.ribbon_selected.base,
                palette.ribbon_selected.background
            )
            .bold(),
            suffix_separator: style!(palette.ribbon_selected.background, background).bold(),
        },
        unselected: SegmentStyle {
            prefix_separator: style!(background, palette.ribbon_unselected.background),
            char_left_separator: style!(
                palette.ribbon_unselected.base,
                palette.ribbon_unselected.background
            )
            .bold(),
            char_shortcut: style!(
                palette.ribbon_unselected.emphasis_0,
                palette.ribbon_unselected.background
            )
            .bold(),
            char_right_separator: style!(
                palette.ribbon_unselected.base,
                palette.ribbon_unselected.background
            )
            .bold(),
            styled_text: style!(
                palette.ribbon_unselected.base,
                palette.ribbon_unselected.background
            )
            .bold(),
            suffix_separator: style!(palette.ribbon_unselected.background, background).bold(),
        },
        unselected_alternate: SegmentStyle {
            prefix_separator: style!(background, alternate_background_color),
            char_left_separator: style!(background, alternate_background_color).bold(),
            char_shortcut: style!(
                palette.ribbon_unselected.emphasis_0,
                alternate_background_color
            )
            .bold(),
            char_right_separator: style!(background, alternate_background_color).bold(),
            styled_text: style!(palette.ribbon_unselected.base, alternate_background_color).bold(),
            suffix_separator: style!(alternate_background_color, background).bold(),
        },
        disabled: SegmentStyle {
            prefix_separator: style!(background, palette.ribbon_unselected.background),
            char_left_separator: style!(
                palette.ribbon_unselected.base,
                palette.ribbon_unselected.background
            )
            .dimmed()
            .italic(),
            char_shortcut: style!(
                palette.ribbon_unselected.base,
                palette.ribbon_unselected.background
            )
            .dimmed()
            .italic(),
            char_right_separator: style!(
                palette.ribbon_unselected.base,
                palette.ribbon_unselected.background
            )
            .dimmed()
            .italic(),
            styled_text: style!(
                palette.ribbon_unselected.base,
                palette.ribbon_unselected.background
            )
            .dimmed()
            .italic(),
            suffix_separator: style!(palette.ribbon_unselected.background, background),
        },
        superkey_prefix: style!(foreground, background).bold(),
        superkey_suffix_separator: style!(background, background),
    }
}

fn col_in_range(range: Option<(usize, usize)>, col: usize) -> bool {
    match range {
        Some((start, end)) => col >= start && col < end,
        None => false,
    }
}

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        // TODO: Should be able to choose whether to use the cache through config.
        self.tip_name = get_cached_tip_name();
        self.classic_ui = configuration
            .get("classic")
            .map(|c| c == "true")
            .unwrap_or(false);
        set_selectable(false);
        self.refresh_clock();
        subscribe(&[
            EventType::Timer,
            EventType::ModifiersHeld,
            EventType::ModeUpdate,
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::CopyToClipboard,
            EventType::InputReceived,
            EventType::SystemClipboardFailure,
            EventType::InitialKeybinds,
            EventType::Mouse,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        let mut should_render = false;
        match event {
            Event::InitialKeybinds(keybinds) => {
                self.cached_keybinds = keybinds;
                if !self.cached_keybinds.is_empty() {
                    self.mode_info.keybinds = self.cached_keybinds.clone();
                }
                should_render = true;
            },
            Event::ModeUpdate(mut mode_info) => {
                if mode_info.keybinds.is_empty() && !self.cached_keybinds.is_empty() {
                    mode_info.keybinds = self.cached_keybinds.clone();
                } else if !mode_info.keybinds.is_empty() {
                    self.cached_keybinds = mode_info.keybinds.clone();
                }
                if self.mode_info != mode_info {
                    should_render = true;
                }
                self.mode_info = mode_info;
                self.base_mode_is_locked = self.mode_info.base_mode == Some(InputMode::Locked);
            },
            Event::TabUpdate(tabs) => {
                if self.tabs != tabs {
                    should_render = true;
                }
                self.tabs = tabs;
            },
            Event::PaneUpdate(manifest) => {
                let agent_counts = AgentCounts::from_manifest(&manifest);
                if self.agent_counts != agent_counts {
                    self.agent_counts = agent_counts;
                    should_render = true;
                }
            },
            Event::ModifiersHeld { modifiers, is_live } => {
                let alt_is_down = modifiers.contains(&KeyModifier::Alt);
                let was_revealed = self.alt_hint_revealed();
                self.alt_hint_until_ms = match (alt_is_down, is_live) {
                    // A terminal that reports the key itself will report it coming back up, so
                    // this only needs a safety net against a release that never arrives - a
                    // window stolen mid-hold, say. Long enough not to blink out under a hold.
                    (true, true) => now_ms() + LIVE_ALT_HINT_SAFETY_MS,
                    // Inferred from a chord: nothing will say when Alt came up, so the window
                    // is the whole of the reveal, and every further chord pushes it out.
                    (true, false) => now_ms() + ALT_HINT_WINDOW_MS,
                    // An empty live set is the release. An empty inferred set never arrives -
                    // the server only sends a chord that had modifiers on it.
                    (false, true) => 0,
                    (false, false) => self.alt_hint_until_ms,
                };
                if self.alt_hint_revealed() != was_revealed {
                    // Re-aim the tick: it is what takes an inferred hold back down.
                    self.refresh_clock();
                    should_render = true;
                }
            },
            Event::Timer(_) => {
                // The tick does two jobs, and either can want a frame: it advances the clock,
                // and it is what takes the Alt hint back down. Both are re-armed here because
                // `set_timeout` is one-shot.
                let clock_changed = self.refresh_clock();
                let hint_expired = self.alt_hint_until_ms != 0 && !self.alt_hint_revealed();
                if hint_expired {
                    self.alt_hint_until_ms = 0;
                }
                should_render = clock_changed || hint_expired;
            },
            Event::CopyToClipboard(copy_destination) => {
                match self.text_copy_destination {
                    Some(text_copy_destination) => {
                        if text_copy_destination != copy_destination {
                            should_render = true;
                        }
                    },
                    None => {
                        should_render = true;
                    },
                }
                self.text_copy_destination = Some(copy_destination);
            },
            Event::SystemClipboardFailure => {
                should_render = true;
                self.display_system_clipboard_failure = true;
            },
            Event::InputReceived => {
                if self.text_copy_destination.is_some() || self.display_system_clipboard_failure {
                    should_render = true;
                }
                self.text_copy_destination = None;
                self.display_system_clipboard_failure = false;
            },
            Event::Mouse(mouse_event) => match mouse_event {
                Mouse::LeftClick(_, col) => {
                    if col_in_range(self.new_pane_ribbon_range, col) {
                        new_pane();
                    } else if col_in_range(self.floating_ribbon_range, col) {
                        toggle_floating_panes(None);
                    }
                },
                Mouse::Hover(_, col) => {
                    let new_pane_hovered = col_in_range(self.new_pane_ribbon_range, col);
                    let floating_hovered =
                        !new_pane_hovered && col_in_range(self.floating_ribbon_range, col);
                    if self.new_pane_ribbon_hovered != new_pane_hovered
                        || self.floating_ribbon_hovered != floating_hovered
                    {
                        self.new_pane_ribbon_hovered = new_pane_hovered;
                        self.floating_ribbon_hovered = floating_hovered;
                        should_render = true;
                    }
                },
                _ => {},
            },
            _ => {},
        };
        should_render
    }

    fn render(&mut self, rows: usize, cols: usize) {
        let supports_arrow_fonts = !self.mode_info.capabilities.arrow_fonts;
        let separator = if supports_arrow_fonts {
            ARROW_SEPARATOR
        } else {
            ""
        };

        let background = self.mode_info.style.colors.text_unselected.background;

        if rows == 1 && !self.classic_ui {
            let fill_bg = match background {
                PaletteColor::Rgb((r, g, b)) => format!("\u{1b}[48;2;{};{};{}m\u{1b}[0K", r, g, b),
                PaletteColor::EightBit(color) => format!("\u{1b}[48;5;{}m\u{1b}[0K", color),
            };
            let active_tab = self.tabs.iter().find(|t| t.active);
            let (line, new_pane_ribbon_range, floating_ribbon_range) = one_line_ui(
                &self.mode_info,
                active_tab,
                cols,
                separator,
                self.base_mode_is_locked,
                self.text_copy_destination,
                self.display_system_clipboard_failure,
                self.new_pane_ribbon_hovered,
                self.floating_ribbon_hovered,
                &SecondaryInfo {
                    alt_hint_revealed: self.alt_hint_revealed(),
                    agent_counts: self.agent_counts,
                    clock: self.clock.clone(),
                },
            );
            self.new_pane_ribbon_range = new_pane_ribbon_range;
            self.floating_ribbon_range = floating_ribbon_range;
            print!("{}{}", line, fill_bg);
            return;
        }
        self.new_pane_ribbon_range = None;
        self.floating_ribbon_range = None;

        //TODO: Switch to UI components here
        let active_tab = self.tabs.iter().find(|t| t.active);
        let first_line = first_line(&self.mode_info, active_tab, cols, separator);
        let second_line = self.second_line(cols);
        let show_nested_session_hint = self.mode_info.session_dimmed.unwrap_or(false)
            || self.mode_info.session_ascended.unwrap_or(false);

        // [48;5;238m is white background, [0K is so that it fills the rest of the line
        // [m is background reset, [0K is so that it clears the rest of the line
        match background {
            PaletteColor::Rgb((r, g, b)) => {
                if rows > 1 {
                    println!("{}\u{1b}[48;2;{};{};{}m\u{1b}[0K", first_line, r, g, b);
                } else {
                    if show_nested_session_hint {
                        print!("\u{1b}[m{}\u{1b}[0K", second_line);
                    } else if self.mode_info.mode == InputMode::Normal {
                        print!("{}\u{1b}[48;2;{};{};{}m\u{1b}[0K", first_line, r, g, b);
                    } else {
                        print!("\u{1b}[m{}\u{1b}[0K", second_line);
                    }
                }
            },
            PaletteColor::EightBit(color) => {
                if rows > 1 {
                    println!("{}\u{1b}[48;5;{}m\u{1b}[0K", first_line, color);
                } else {
                    if show_nested_session_hint {
                        print!("\u{1b}[m{}\u{1b}[0K", second_line);
                    } else if self.mode_info.mode == InputMode::Normal {
                        print!("{}\u{1b}[48;5;{}m\u{1b}[0K", first_line, color);
                    } else {
                        print!("\u{1b}[m{}\u{1b}[0K", second_line);
                    }
                }
            },
        }

        if rows > 1 {
            print!("\u{1b}[m{}\u{1b}[0K", second_line);
        }
    }
}

/// How long the Alt hint stays up after an Alt chord, on a terminal that cannot tell us when
/// the key came back up. Long enough to read the hint, short enough not to sit on the clock.
const ALT_HINT_WINDOW_MS: i64 = 3000;

/// The safety net under a *live* hold, where the terminal does report the release. Kitty makes
/// no promise about a release for a window that loses focus mid-hold, and without this a
/// dropped release would pin the hint up for the rest of the session.
const LIVE_ALT_HINT_SAFETY_MS: i64 = 30_000;

/// How often the host is re-asked for the session's UTC offset. Only a daylight-saving change
/// moves it, and that happens on an hour boundary, so an hour is as often as it can matter.
const UTC_OFFSET_TTL_MS: i64 = 60 * 60 * 1000;

impl State {
    /// Whether the Alt hint currently owns the right-hand slot.
    fn alt_hint_revealed(&self) -> bool {
        now_ms() < self.alt_hint_until_ms
    }

    /// Re-read the clock, re-arm the tick, and say whether anything visible moved.
    ///
    /// The tick is aimed at the next minute boundary rather than set to a flat 60s, so the
    /// displayed minute turns over when the real one does instead of drifting away from it by
    /// however long the plugin took to get scheduled. It is also capped so a hint waiting to
    /// expire is not left up until the next minute.
    fn refresh_clock(&mut self) -> bool {
        let now = now_ms();
        if now.saturating_sub(self.utc_offset_taken_at_ms) >= UTC_OFFSET_TTL_MS {
            // An offset the host would not give us is `0` - UTC, stated as such, which is a
            // better answer than refusing to draw a clock.
            self.utc_offset_secs = get_local_utc_offset(now).unwrap_or(0);
            self.utc_offset_taken_at_ms = now;
        }
        let clock = Clock::at(now, self.utc_offset_secs);
        let changed = self.clock != clock;
        self.clock = clock;

        let to_next_minute = 60.0 - (now.rem_euclid(60_000) as f64 / 1000.0);
        let to_hint_expiry = match self.alt_hint_until_ms > now {
            true => (self.alt_hint_until_ms - now) as f64 / 1000.0,
            false => f64::MAX,
        };
        set_timeout(to_next_minute.min(to_hint_expiry).max(0.1));
        changed
    }

    fn second_line(&self, cols: usize) -> LinePart {
        let active_tab = self.tabs.iter().find(|t| t.active);

        if self.mode_info.session_dimmed.unwrap_or(false) {
            return descended_into_nested_session_hint(&self.mode_info, cols);
        }
        if self.mode_info.session_ascended.unwrap_or(false) {
            return ascended_to_host_session_hint(&self.mode_info, cols);
        }
        if let Some(copy_destination) = self.text_copy_destination {
            text_copied_hint(copy_destination)
        } else if self.display_system_clipboard_failure {
            system_clipboard_error(&self.mode_info.style.colors)
        } else if let Some(active_tab) = active_tab {
            if active_tab.is_fullscreen_active {
                match self.mode_info.mode {
                    InputMode::Normal => fullscreen_panes_to_hide(
                        &self.mode_info.style.colors,
                        active_tab.panes_to_hide,
                    ),
                    InputMode::Locked => locked_fullscreen_panes_to_hide(
                        &self.mode_info.style.colors,
                        active_tab.panes_to_hide,
                    ),
                    _ => keybinds(&self.mode_info, &self.tip_name, cols),
                }
            } else if active_tab.are_floating_panes_visible {
                match self.mode_info.mode {
                    InputMode::Normal => floating_panes_are_visible(&self.mode_info),
                    InputMode::Locked => {
                        locked_floating_panes_are_visible(&self.mode_info.style.colors)
                    },
                    _ => keybinds(&self.mode_info, &self.tip_name, cols),
                }
            } else {
                keybinds(&self.mode_info, &self.tip_name, cols)
            }
        } else {
            LinePart::default()
        }
    }
}

pub fn get_common_modifiers(mut keyvec: Vec<&KeyWithModifier>) -> Vec<KeyModifier> {
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

/// Get key from action pattern(s).
///
/// This function takes as arguments a `keymap` that is a `Vec<(Key, Vec<Action>)>` and contains
/// all keybindings for the current mode and one or more `p` patterns which match a sequence of
/// actions to search for. If within the keymap a sequence of actions matching `p` is found, all
/// keys that trigger the action pattern are returned as vector of `Vec<Key>`.
pub fn action_key(
    keymap: &[(KeyWithModifier, Vec<Action>)],
    action: &[Action],
) -> Vec<KeyWithModifier> {
    keymap
        .iter()
        .filter_map(|(key, acvec)| {
            let matching = acvec
                .iter()
                .zip(action)
                .filter(|(a, b)| a.shallow_eq(b))
                .count();

            if matching == acvec.len() && matching == action.len() {
                Some(key.clone())
            } else {
                None
            }
        })
        .collect::<Vec<KeyWithModifier>>()
}

/// Get the key bound to launching a particular plugin, by alias.
///
/// [`action_key`] cannot do this: it compares actions with `shallow_eq`, which only looks at the
/// variant, so every `LaunchOrFocusPlugin` binding in a mode looks identical to it. Matching on the
/// alias is what keeps each hint pointing at its own key.
///
/// Of the keys that do the same thing, the one a hint should name — see [`the_modes_own_key`].
pub fn plugin_launch_key(
    keymap: &[(KeyWithModifier, Vec<Action>)],
    alias: &str,
) -> Vec<KeyWithModifier> {
    the_modes_own_key(
        keymap
            .iter()
            .filter(|(_, acvec)| acvec.iter().any(|a| a.launches_plugin(alias)))
            .map(|(key, _)| key),
    )
}

/// Pick the key a hint should name out of several that do the same thing.
///
/// A thing can be reachable by more than one key — the mode's own leader-then-letter binding plus,
/// say, a global `Alt <something>` the user keeps for it. Both work, but the hint is shown
/// *inside* the mode, so the mode's own letter is the one that belongs there: prefer the fewest
/// modifiers, and break a tie on the key itself so the choice does not ride on keymap order.
///
/// Yields `vec![key]`, or an empty vec when there is no candidate, which is how the renderers
/// spell "nothing bound".
pub fn the_modes_own_key<'a>(
    candidates: impl Iterator<Item = &'a KeyWithModifier>,
) -> Vec<KeyWithModifier> {
    candidates
        .min_by(|left, right| {
            left.key_modifiers
                .len()
                .cmp(&right.key_modifiers.len())
                .then_with(|| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal))
        })
        .map(|key| vec![key.clone()])
        .unwrap_or_default()
}

/// Get multiple keys for multiple actions.
///
/// An extension of [`action_key`] that iterates over all action tuples and collects the results.
pub fn action_key_group(
    keymap: &[(KeyWithModifier, Vec<Action>)],
    actions: &[&[Action]],
) -> Vec<KeyWithModifier> {
    let mut ret = vec![];
    for action in actions {
        ret.extend(action_key(keymap, action));
    }
    ret
}

/// Style a vector of [`Key`]s with the given [`Palette`].
///
/// Creates a line segment of style `<KEYS>`, with correct theming applied: The brackets have the
/// regular text color, the enclosed keys are painted green and bold. If the keys share a common
/// modifier (See [`get_common_modifier`]), it is printed in front of the keys, painted green and
/// bold, separated with a `+`: `MOD + <KEYS>`.
///
/// If multiple [`Key`]s are given, the individual keys are separated with a `|` char. This does
/// not apply to the following groups of keys which are treated specially and don't have a
/// separator between them:
///
/// - "hjkl"
/// - "HJKL"
/// - "←↓↑→"
/// - "←→"
/// - "↓↑"
///
/// The returned Vector of [`ANSIString`] is suitable for transformation into an [`ANSIStrings`]
/// type.
pub fn style_key_with_modifier(
    keyvec: &[KeyWithModifier],
    palette: &Styling,
    background: Option<PaletteColor>,
) -> Vec<ANSIString<'static>> {
    if keyvec.is_empty() {
        return vec![];
    }

    let text_color = palette_match!(palette.text_unselected.base);
    let green_color = palette_match!(palette.text_unselected.emphasis_2);
    let orange_color = palette_match!(palette.text_unselected.emphasis_0);
    let mut ret = vec![];

    let common_modifiers = get_common_modifiers(keyvec.iter().collect());

    let no_common_modifier = common_modifiers.is_empty();
    let modifier_str = common_modifiers
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join("-");
    let painted_modifier = if modifier_str.is_empty() {
        Style::new().paint("")
    } else {
        if let Some(background) = background {
            let background = palette_match!(background);
            Style::new()
                .fg(orange_color)
                .on(background)
                .bold()
                .paint(modifier_str)
        } else {
            Style::new().fg(orange_color).bold().paint(modifier_str)
        }
    };
    ret.push(painted_modifier);

    // Prints key group start
    let group_start_str = if no_common_modifier { "<" } else { " + <" };
    if let Some(background) = background {
        let background = palette_match!(background);
        ret.push(
            Style::new()
                .fg(text_color)
                .on(background)
                .paint(group_start_str),
        );
    } else {
        ret.push(Style::new().fg(text_color).paint(group_start_str));
    }

    // Prints the keys
    let key = keyvec
        .iter()
        .map(|key| {
            if no_common_modifier {
                format!("{}", key)
            } else {
                let key_modifier_for_key = key
                    .key_modifiers
                    .iter()
                    .filter(|m| !common_modifiers.contains(m))
                    .map(|m| m.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                if key_modifier_for_key.is_empty() {
                    format!("{}", key.bare_key)
                } else {
                    format!("{} {}", key_modifier_for_key, key.bare_key)
                }
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

    for (idx, key) in key.iter().enumerate() {
        if idx > 0 && !key_separator.is_empty() {
            if let Some(background) = background {
                let background = palette_match!(background);
                ret.push(
                    Style::new()
                        .fg(text_color)
                        .on(background)
                        .paint(key_separator),
                );
            } else {
                ret.push(Style::new().fg(text_color).paint(key_separator));
            }
        }
        if let Some(background) = background {
            let background = palette_match!(background);
            ret.push(
                Style::new()
                    .fg(green_color)
                    .on(background)
                    .bold()
                    .paint(key.clone()),
            );
        } else {
            ret.push(Style::new().fg(green_color).bold().paint(key.clone()));
        }
    }

    let group_end_str = ">";
    if let Some(background) = background {
        let background = palette_match!(background);
        ret.push(
            Style::new()
                .fg(text_color)
                .on(background)
                .paint(group_end_str),
        );
    } else {
        ret.push(Style::new().fg(text_color).paint(group_end_str));
    }

    ret
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use ansi_term::unstyle;
    use ansi_term::ANSIStrings;

    fn big_keymap() -> Vec<(KeyWithModifier, Vec<Action>)> {
        vec![
            (KeyWithModifier::new(BareKey::Char('a')), vec![Action::Quit]),
            (
                KeyWithModifier::new(BareKey::Char('b')).with_ctrl_modifier(),
                vec![Action::ScrollUp],
            ),
            (
                KeyWithModifier::new(BareKey::Char('d')).with_ctrl_modifier(),
                vec![Action::ScrollDown],
            ),
            (
                KeyWithModifier::new(BareKey::Char('c')).with_alt_modifier(),
                vec![
                    Action::ScrollDown,
                    Action::SwitchToMode {
                        input_mode: InputMode::Normal,
                    },
                ],
            ),
            (
                KeyWithModifier::new(BareKey::Char('1')),
                vec![
                    TO_NORMAL,
                    Action::SwitchToMode {
                        input_mode: InputMode::Locked,
                    },
                ],
            ),
        ]
    }

    fn launches(alias: &str) -> Action {
        use zellij_utils::input::layout::{PluginAlias, RunPluginOrAlias};
        Action::LaunchOrFocusPlugin {
            plugin: RunPluginOrAlias::Alias(PluginAlias {
                name: alias.to_string(),
                ..Default::default()
            }),
            should_float: true,
            move_to_focused_tab: true,
            should_open_in_place: false,
            close_replaced_pane: false,
            skip_cache: false,
            tab_id: None,
        }
    }

    // A mode hint has to name the key that belongs to the mode. When the same plugin also hangs off
    // a global shortcut - here a user's own `Ctrl \`` for the chat - the leader-then-letter binding
    // is the one to show, not whichever of the two the keymap happens to list first.
    #[test]
    fn plugin_launch_key_prefers_the_modes_own_letter_over_a_global_shortcut() {
        let keymap = vec![
            (
                KeyWithModifier::new(BareKey::Char('n')),
                vec![launches("hocus-new-agent"), TO_NORMAL],
            ),
            (
                KeyWithModifier::new(BareKey::Char('`')).with_ctrl_modifier(),
                vec![launches("hocus")],
            ),
            (
                KeyWithModifier::new(BareKey::Char('c')),
                vec![launches("hocus"), TO_NORMAL],
            ),
        ];

        assert_eq!(
            plugin_launch_key(&keymap, "hocus-new-agent"),
            vec![KeyWithModifier::new(BareKey::Char('n'))]
        );

        assert_eq!(
            plugin_launch_key(&keymap, "hocus"),
            vec![KeyWithModifier::new(BareKey::Char('c'))]
        );
    }

    #[test]
    fn plugin_launch_key_falls_back_to_a_modified_key_when_that_is_all_there_is() {
        let keymap = vec![(
            KeyWithModifier::new(BareKey::Char('`')).with_ctrl_modifier(),
            vec![launches("hocus")],
        )];

        assert_eq!(
            plugin_launch_key(&keymap, "hocus"),
            vec![KeyWithModifier::new(BareKey::Char('`')).with_ctrl_modifier()]
        );
    }

    #[test]
    fn plugin_launch_key_keeps_different_plugins_on_different_keys() {
        let keymap = vec![
            (
                KeyWithModifier::new(BareKey::Char('a')),
                vec![launches("hocus-agents"), TO_NORMAL],
            ),
            (
                KeyWithModifier::new(BareKey::Char('c')),
                vec![launches("hocus"), TO_NORMAL],
            ),
            (
                KeyWithModifier::new(BareKey::Char('k')),
                vec![launches("hocus-new-agent"), TO_NORMAL],
            ),
        ];

        assert_eq!(
            plugin_launch_key(&keymap, "hocus-agents"),
            vec![KeyWithModifier::new(BareKey::Char('a'))]
        );
        assert_eq!(
            plugin_launch_key(&keymap, "hocus"),
            vec![KeyWithModifier::new(BareKey::Char('c'))]
        );
        assert_eq!(
            plugin_launch_key(&keymap, "hocus-new-agent"),
            vec![KeyWithModifier::new(BareKey::Char('k'))]
        );
    }

    #[test]
    fn common_modifier_with_ctrl_keys() {
        let keyvec = [
            KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Char('b')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Char('c')).with_ctrl_modifier(),
        ];
        let ret = get_common_modifiers(keyvec.iter().collect());
        assert_eq!(ret, vec![KeyModifier::Ctrl]);
    }

    #[test]
    fn common_modifier_with_alt_keys_chars() {
        let keyvec = [
            KeyWithModifier::new(BareKey::Char('1')).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Char('t')).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Char('z')).with_alt_modifier(),
        ];
        let ret = get_common_modifiers(keyvec.iter().collect());
        assert_eq!(ret, vec![KeyModifier::Alt]);
    }

    #[test]
    fn common_modifier_with_mixed_alt_ctrl_keys() {
        let keyvec = [
            KeyWithModifier::new(BareKey::Char('1')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Char('t')).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Char('z')).with_alt_modifier(),
        ];
        let ret = get_common_modifiers(keyvec.iter().collect());
        assert_eq!(ret, vec![]); // no common modifiers
    }

    #[test]
    fn common_modifier_with_any_keys() {
        let keyvec = [
            KeyWithModifier::new(BareKey::Char('1')),
            KeyWithModifier::new(BareKey::Char('t')).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Char('z')).with_alt_modifier(),
        ];
        let ret = get_common_modifiers(keyvec.iter().collect());
        assert_eq!(ret, vec![]); // no common modifiers
    }

    #[test]
    fn action_key_simple_pattern_match_exact() {
        let keymap = &[(KeyWithModifier::new(BareKey::Char('f')), vec![Action::Quit])];
        let ret = action_key(keymap, &[Action::Quit]);
        assert_eq!(ret, vec![KeyWithModifier::new(BareKey::Char('f'))]);
    }

    #[test]
    fn action_key_simple_pattern_match_pattern_too_long() {
        let keymap = &[(KeyWithModifier::new(BareKey::Char('f')), vec![Action::Quit])];
        let ret = action_key(keymap, &[Action::Quit, Action::ScrollUp]);
        assert_eq!(ret, Vec::new());
    }

    #[test]
    fn action_key_simple_pattern_match_pattern_empty() {
        let keymap = &[(KeyWithModifier::new(BareKey::Char('f')), vec![Action::Quit])];
        let ret = action_key(keymap, &[]);
        assert_eq!(ret, Vec::new());
    }

    #[test]
    fn action_key_long_pattern_match_exact() {
        let keymap = big_keymap();
        let ret = action_key(&keymap, &[Action::ScrollDown, TO_NORMAL]);
        assert_eq!(
            ret,
            vec![KeyWithModifier::new(BareKey::Char('c')).with_alt_modifier()]
        );
    }

    #[test]
    fn action_key_long_pattern_match_too_short() {
        let keymap = big_keymap();
        let ret = action_key(&keymap, &[TO_NORMAL]);
        assert_eq!(ret, Vec::new());
    }

    #[test]
    fn action_key_group_single_pattern() {
        let keymap = big_keymap();
        let ret = action_key_group(&keymap, &[&[Action::Quit]]);
        assert_eq!(ret, vec![KeyWithModifier::new(BareKey::Char('a'))]);
    }

    #[test]
    fn action_key_group_two_patterns() {
        let keymap = big_keymap();
        let ret = action_key_group(&keymap, &[&[Action::ScrollDown], &[Action::ScrollUp]]);
        // Mind the order!
        assert_eq!(
            ret,
            vec![
                KeyWithModifier::new(BareKey::Char('d')).with_ctrl_modifier(),
                KeyWithModifier::new(BareKey::Char('b')).with_ctrl_modifier()
            ]
        );
    }

    #[test]
    fn style_key_with_modifier_only_chars() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Char('a')),
            KeyWithModifier::new(BareKey::Char('b')),
            KeyWithModifier::new(BareKey::Char('c')),
        ];
        let palette = Styling::default();

        let ret = style_key_with_modifier(&keyvec, &palette, None);
        let ret = unstyle(&ANSIStrings(&ret));

        assert_eq!(ret, "<a|b|c>".to_string())
    }

    #[test]
    fn style_key_with_modifier_special_group_hjkl() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Char('h')),
            KeyWithModifier::new(BareKey::Char('j')),
            KeyWithModifier::new(BareKey::Char('k')),
            KeyWithModifier::new(BareKey::Char('l')),
        ];
        let palette = Styling::default();

        let ret = style_key_with_modifier(&keyvec, &palette, None);
        let ret = unstyle(&ANSIStrings(&ret));

        assert_eq!(ret, "<hjkl>".to_string())
    }

    #[test]
    fn style_key_with_modifier_special_group_all_arrows() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Left),
            KeyWithModifier::new(BareKey::Down),
            KeyWithModifier::new(BareKey::Up),
            KeyWithModifier::new(BareKey::Right),
        ];
        let palette = Styling::default();

        let ret = style_key_with_modifier(&keyvec, &palette, None);
        let ret = unstyle(&ANSIStrings(&ret));

        assert_eq!(ret, "<←↓↑→>".to_string())
    }

    #[test]
    fn style_key_with_modifier_special_group_left_right_arrows() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Left),
            KeyWithModifier::new(BareKey::Right),
        ];
        let palette = Styling::default();

        let ret = style_key_with_modifier(&keyvec, &palette, None);
        let ret = unstyle(&ANSIStrings(&ret));

        assert_eq!(ret, "<←→>".to_string())
    }

    #[test]
    fn style_key_with_modifier_special_group_down_up_arrows() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Down),
            KeyWithModifier::new(BareKey::Up),
        ];
        let palette = Styling::default();

        let ret = style_key_with_modifier(&keyvec, &palette, None);
        let ret = unstyle(&ANSIStrings(&ret));

        assert_eq!(ret, "<↓↑>".to_string())
    }

    #[test]
    fn style_key_with_modifier_common_ctrl_modifier_chars() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Char('b')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Char('c')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Char('d')).with_ctrl_modifier(),
        ];
        let palette = Styling::default();

        let ret = style_key_with_modifier(&keyvec, &palette, None);
        let ret = unstyle(&ANSIStrings(&ret));

        assert_eq!(ret, "Ctrl + <a|b|c|d>".to_string())
    }

    #[test]
    fn style_key_with_modifier_common_alt_modifier_chars() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Char('a')).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Char('b')).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Char('c')).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Char('d')).with_alt_modifier(),
        ];
        let palette = Styling::default();

        let ret = style_key_with_modifier(&keyvec, &palette, None);
        let ret = unstyle(&ANSIStrings(&ret));

        assert_eq!(ret, "Alt + <a|b|c|d>".to_string())
    }

    #[test]
    fn style_key_with_modifier_common_alt_modifier_with_special_group_all_arrows() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Left).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Down).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Up).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Right).with_alt_modifier(),
        ];
        let palette = Styling::default();

        let ret = style_key_with_modifier(&keyvec, &palette, None);
        let ret = unstyle(&ANSIStrings(&ret));

        assert_eq!(ret, "Alt + <←↓↑→>".to_string())
    }

    #[test]
    fn style_key_with_modifier_ctrl_alt_char_mixed() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Char('a')).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Char('b')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Char('c')),
        ];
        let palette = Styling::default();

        let ret = style_key_with_modifier(&keyvec, &palette, None);
        let ret = unstyle(&ANSIStrings(&ret));

        assert_eq!(ret, "<Alt a|Ctrl b|c>".to_string())
    }

    #[test]
    fn style_key_with_modifier_unprintables() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Backspace),
            KeyWithModifier::new(BareKey::Enter),
            KeyWithModifier::new(BareKey::Char(' ')),
            KeyWithModifier::new(BareKey::Tab),
            KeyWithModifier::new(BareKey::PageDown),
            KeyWithModifier::new(BareKey::Delete),
            KeyWithModifier::new(BareKey::Home),
            KeyWithModifier::new(BareKey::End),
            KeyWithModifier::new(BareKey::Insert),
            KeyWithModifier::new(BareKey::Tab),
            KeyWithModifier::new(BareKey::Esc),
        ];
        let palette = Styling::default();

        let ret = style_key_with_modifier(&keyvec, &palette, None);
        let ret = unstyle(&ANSIStrings(&ret));

        assert_eq!(
            ret,
            "<BACKSPACE|ENTER|SPACE|TAB|PgDn|DEL|HOME|END|INS|TAB|ESC>".to_string()
        )
    }

    #[test]
    fn style_key_with_modifier_unprintables_with_common_ctrl_modifier() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Enter).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Char(' ')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Tab).with_ctrl_modifier(),
        ];
        let palette = Styling::default();

        let ret = style_key_with_modifier(&keyvec, &palette, None);
        let ret = unstyle(&ANSIStrings(&ret));

        assert_eq!(ret, "Ctrl + <ENTER|SPACE|TAB>".to_string())
    }

    #[test]
    fn style_key_with_modifier_unprintables_with_common_alt_modifier() {
        let keyvec = vec![
            KeyWithModifier::new(BareKey::Enter).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Char(' ')).with_alt_modifier(),
            KeyWithModifier::new(BareKey::Tab).with_alt_modifier(),
        ];
        let palette = Styling::default();

        let ret = style_key_with_modifier(&keyvec, &palette, None);
        let ret = unstyle(&ANSIStrings(&ret));

        assert_eq!(ret, "Alt + <ENTER|SPACE|TAB>".to_string())
    }
}

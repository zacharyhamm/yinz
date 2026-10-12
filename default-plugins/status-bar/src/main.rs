mod first_line;
mod keybind_hints;
mod one_line_ui;
mod second_line;
mod status_block;
#[cfg(test)]
mod test_support;
mod tip;

use ansi_term::{
    ANSIString,
    Colour::{Fixed, RGB},
    Style,
};

use std::collections::BTreeMap;
use std::fmt::{Display, Error, Formatter};
use yinz_tile::prelude::actions::Action;
use yinz_tile::prelude::*;
use yinz_tile_utils::{palette_match, style};

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
    /// synchronous round trip, so it is asked for on load and then once per whole hour - which
    /// is the granularity a DST change moves at.
    utc_offset_secs: i32,
    /// The hour boundary this offset stops being trusted at - see [`next_utc_hour`]. A
    /// deadline rather than an age, so the re-ask lands on the boundary a DST change happens
    /// on instead of on whatever phase the session was loaded at.
    utc_offset_valid_until_ms: i64,
    /// While this is in the future, the Alt hint has the right-hand slot instead of the clock.
    /// Set by `Event::ModifiersHeld`; see `hint_revealed`.
    alt_hint_until_ms: i64,
    /// The same slot, claimed by the pointer being on the bar rather than by Alt. Set by
    /// `Mouse::Hover`; see `note_pointer`.
    hover_hint_until_ms: i64,
    /// The width of the last frame, which is what says whether a hover column is on the bar at
    /// all - see `pointer_left_the_bar`.
    last_render_cols: usize,
    last_render_rows: usize,
    /// When the `set_timeout` believed to be outstanding will fire, if there is one. The host
    /// has no way to cancel a timer - each `set_timeout` spawns a task that will fire once, no
    /// matter what happens here - so the plugin has to stop asking for ones it already has.
    pending_tick_at_ms: Option<i64>,
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

impl YinzPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.tip_name = get_cached_tip_name();
        self.classic_ui = configuration
            .get("classic")
            .map(|c| c == "true")
            .unwrap_or(false);
        set_selectable(false);
        self.tick();
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
                    self.arm_tick(now_ms().get());
                    should_render = true;
                }
            },
            Event::ModifiersHeld { modifiers, is_live } => {
                let now = now_ms().get();
                let was_revealed = self.hint_revealed();
                self.alt_hint_until_ms = next_alt_hint_until(
                    self.alt_hint_until_ms,
                    now,
                    modifiers.contains(&KeyModifier::Alt),
                    is_live,
                );
                if self.hint_revealed() != was_revealed {
                    // Re-aim the tick: it is what takes an inferred hold back down.
                    self.arm_tick(now);
                    should_render = true;
                }
            },
            Event::Timer(_) => {
                // The tick advances the clock and agent animation, and expires hints.
                // All are re-armed here because `set_timeout` is one-shot.
                let now = now_ms().get();
                // Only the tick that was actually waited for clears the record. A timer armed
                // earlier than one already outstanding leaves a straggler behind it, and
                // letting the straggler clear the record would have every fire arm a
                // replacement; this way the count decays back to one.
                if self.pending_tick_at_ms.is_some_and(|at| now >= at) {
                    self.pending_tick_at_ms = None;
                }
                let clock_changed = self.refresh_clock(now);
                let hint_expired = self.hint_until_ms() != 0 && now >= self.hint_until_ms();
                if hint_expired {
                    self.alt_hint_until_ms = 0;
                    self.hover_hint_until_ms = 0;
                }
                self.arm_tick(now);
                should_render =
                    clock_changed || hint_expired || self.animation_interval_ms().is_some();
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
                    // A click is a sample of where the pointer is too, and re-arms the
                    // safety net under a reveal the pointer has been sitting still in.
                    should_render = self.note_pointer(col);
                    if col_in_range(self.new_pane_ribbon_range, col) {
                        new_pane();
                    } else if col_in_range(self.floating_ribbon_range, col) {
                        toggle_floating_panes(None);
                    }
                },
                Mouse::Hover(_, col) => {
                    should_render = self.note_pointer(col);
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
        // Kept so a hover can be told from the leave the server sends when the pointer moves
        // off the bar, which arrives as a column past the end of it.
        self.last_render_cols = cols;
        if self.last_render_rows != rows {
            self.last_render_rows = rows;
            self.arm_tick(now_ms().get());
        }
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
                    hint_revealed: self.hint_revealed(),
                    agent_counts: self.agent_counts,
                    clock: self.clock.clone(),
                    now_ms: now_ms().get(),
                },
            );
            self.new_pane_ribbon_range = new_pane_ribbon_range;
            self.floating_ribbon_range = floating_ribbon_range;
            print!("{}{}", line, fill_bg);
            return;
        }
        self.new_pane_ribbon_range = None;
        self.floating_ribbon_range = None;

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

/// The safety net under a pointer reveal, for a pointer that leaves without saying so.
///
/// A plugin is sent `Mouse::Hover` for motion over its own surface, and the server also sends
/// it one column `u16::MAX` when the pointer moves off it onto something else - so the reveal
/// normally ends the moment the pointer leaves, not on a clock. What has no event is the
/// pointer leaving the *terminal*, and this is what stops that from pinning the hint up for
/// the rest of the session.
const HOVER_HINT_SAFETY_MS: i64 = 30_000;

/// The grain the session's UTC offset is re-asked for on. Only a daylight-saving change moves
/// it, and that happens on an hour boundary, so an hour is as often as it can matter.
const UTC_OFFSET_HOUR_MS: i64 = 60 * 60 * 1000;

/// The next whole UTC hour strictly after `now`.
///
/// The offset is re-asked for *on* the boundary rather than an hour after it was last read,
/// because the read's phase is whenever the session happened to load: a session loaded at
/// 14:37 that re-asked every hour would go on drawing the old offset until 15:37, an hour
/// after the change. Anchored here, the staleness is bounded by the tick interval instead.
fn next_utc_hour(now: i64) -> i64 {
    (now.div_euclid(UTC_OFFSET_HOUR_MS) + 1) * UTC_OFFSET_HOUR_MS
}

/// Where the Alt reveal deadline goes on a `ModifiersHeld`.
///
/// Live evidence is authoritative in both directions: a live hold restarts the safety net, a
/// live release takes the hint down at once. Inferred evidence can only ever extend - a chord
/// says Alt was down when it was struck and nothing about when the hold ends, so it must not
/// cut a live hold's window down to its own.
fn next_alt_hint_until(current: i64, now: i64, alt_is_down: bool, is_live: bool) -> i64 {
    match (alt_is_down, is_live) {
        // A terminal that reports the key itself will report it coming back up, so this only
        // needs a safety net against a release that never arrives - a window stolen mid-hold,
        // say. Long enough not to blink out under a hold.
        (true, true) => now + LIVE_ALT_HINT_SAFETY_MS,
        // Inferred from a chord: nothing will say when Alt came up, so the window is the whole
        // of the reveal, and every further chord pushes it out.
        (true, false) => current.max(now + ALT_HINT_WINDOW_MS),
        // An empty live set is the release. An empty inferred set never arrives - the server
        // only sends a chord that had modifiers on it.
        (false, true) => 0,
        (false, false) => current,
    }
}

/// The next animation or minute boundary, or a nearer hint expiry.
///
/// Aimed at the boundary rather than set to a flat 60s so the displayed minute turns over when
/// the real one does instead of drifting away from it by however long the plugin took to get
/// scheduled; capped so a hint waiting to expire is not left up until the next minute.
fn next_tick_at(now: i64, hint_until_ms: i64, animation_interval_ms: Option<i64>) -> i64 {
    let interval = animation_interval_ms.unwrap_or(60_000);
    let next_boundary = now + (interval - now.rem_euclid(interval));
    match hint_until_ms > now {
        true => next_boundary.min(hint_until_ms),
        false => next_boundary,
    }
}

/// Whether a hover column means the pointer has gone somewhere else.
///
/// The server tells a plugin the pointer has left it by sending one last buttonless motion at
/// column `u16::MAX`, which is to say a column that is not on the bar. Read as "not on the
/// bar" rather than as the sentinel itself: any column past the end of what was drawn is not
/// somewhere the pointer can be, and a frame we have not drawn yet cannot say either way.
fn pointer_left_the_bar(last_render_cols: usize, col: usize) -> bool {
    last_render_cols != 0 && col >= last_render_cols
}

/// Whether a tick wanted at `at` needs a fresh `set_timeout`, given the one thought to be
/// outstanding. A timer that already fires at or before `at` will do: it wakes the plugin,
/// which re-reads the clock and re-arms, so an early wake costs a frame at worst while a
/// second timer costs one per minute for the rest of the session.
fn should_arm_tick(pending: Option<i64>, at: i64) -> bool {
    match pending {
        Some(pending) => at < pending,
        None => true,
    }
}

impl State {
    /// Whether the hint currently owns the right-hand slot, by either claim on it.
    fn hint_revealed(&self) -> bool {
        now_ms().get() < self.hint_until_ms()
    }

    /// The later of the two reveal deadlines - Alt and the pointer claim the same slot, and
    /// whichever holds it longest is when it goes back to the clock.
    fn hint_until_ms(&self) -> i64 {
        self.alt_hint_until_ms.max(self.hover_hint_until_ms)
    }

    /// Take in where the pointer is, and say whether that changed hands on the slot.
    ///
    /// The pointer reveal is the whole of the mouse's way in: with the clock in the slot the
    /// ribbons are not drawn and so cannot be clicked, which under the default
    /// `report_modifier_keys false` would leave them unreachable by mouse entirely - an Alt
    /// chord performs its action rather than revealing anything.
    fn note_pointer(&mut self, col: usize) -> bool {
        let now = now_ms().get();
        let was_revealed = self.hint_revealed();
        self.hover_hint_until_ms = match pointer_left_the_bar(self.last_render_cols, col) {
            true => 0,
            false => now + HOVER_HINT_SAFETY_MS,
        };
        self.arm_tick(now);
        self.hint_revealed() != was_revealed
    }

    /// Re-read the clock, re-arm the tick, and say whether anything visible moved.
    fn tick(&mut self) -> bool {
        let now = now_ms().get();
        let changed = self.refresh_clock(now);
        self.arm_tick(now);
        changed
    }

    /// Re-read the clock and say whether anything visible moved.
    fn refresh_clock(&mut self, now: i64) -> bool {
        if now >= self.utc_offset_valid_until_ms {
            // An offset the host would not give us is `0` - UTC, stated as such, which is a
            // better answer than refusing to draw a clock.
            self.utc_offset_secs = get_local_utc_offset(SignedUnixMillis::new(now)).unwrap_or(0);
            self.utc_offset_valid_until_ms = next_utc_hour(now);
        }
        let clock = Clock::at(now, self.utc_offset_secs);
        let changed = self.clock != clock;
        self.clock = clock;
        changed
    }

    fn animation_interval_ms(&self) -> Option<i64> {
        if !self.classic_ui && self.last_render_rows == 1 {
            self.agent_counts.animation_interval_ms()
        } else {
            None
        }
    }

    /// Ask the host for the next tick, unless one already outstanding will do.
    ///
    /// `set_timeout` cannot be cancelled, so every call that is not needed is a timer firing
    /// for the rest of the session: this runs on every reveal-state change, and arming
    /// unconditionally meant every Alt chord left one more behind it.
    fn arm_tick(&mut self, now: i64) {
        let at = next_tick_at(now, self.hint_until_ms(), self.animation_interval_ms());
        if should_arm_tick(self.pending_tick_at_ms, at) {
            set_timeout(yinz_tile::prelude::FractionalSeconds::new(
                ((at - now) as f64 / 1000.0).max(0.001),
            ));
            self.pending_tick_at_ms = Some(at);
        }
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

/// Get the key bound to sending a particular message to the plugins already running, by the
/// message's name: a `MessagePlugin` binding, which the keymap holds as an [`Action::KeybindPipe`].
///
/// [`action_key`] cannot tell one message from another any more than it can tell one plugin
/// launch from another, so this matches on the name instead.
///
/// The config always gives a message a name, but the plugin API's wire encoding of a
/// `KeybindPipe` carries none of its fields, so a Wasm plugin gets every such binding with
/// `name: None`. A nameless pipe is therefore one whose name was lost on the way, not one that
/// never had one, and it counts: otherwise this bar could never find the key that the Deno bar,
/// whose keymap arrives whole, finds and shows. The price is that where a mode binds more than one
/// message this bar cannot tell them apart; the shipped keymap binds one at most.
///
/// Of the keys that do the same thing, the one a hint should name — see [`the_modes_own_key`].
pub fn keybind_pipe_key(
    keymap: &[(KeyWithModifier, Vec<Action>)],
    name: &str,
) -> Vec<KeyWithModifier> {
    the_modes_own_key(
        keymap
            .iter()
            .filter(|(_, acvec)| {
                acvec.iter().any(|a| {
                    matches!(a, Action::KeybindPipe { name: sent, .. }
                        if sent.as_deref().is_none_or(|sent| sent == name))
                })
            })
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
        use {yinz_types::input::layout::PluginAlias, yinz_types::input::layout::RunPluginOrAlias};
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
                vec![launches("new-agent"), TO_NORMAL],
            ),
            (
                KeyWithModifier::new(BareKey::Char('`')).with_ctrl_modifier(),
                vec![launches("agents")],
            ),
            (
                KeyWithModifier::new(BareKey::Char('c')),
                vec![launches("agents"), TO_NORMAL],
            ),
        ];

        assert_eq!(
            plugin_launch_key(&keymap, "new-agent"),
            vec![KeyWithModifier::new(BareKey::Char('n'))]
        );

        assert_eq!(
            plugin_launch_key(&keymap, "agents"),
            vec![KeyWithModifier::new(BareKey::Char('c'))]
        );
    }

    #[test]
    fn plugin_launch_key_falls_back_to_a_modified_key_when_that_is_all_there_is() {
        let keymap = vec![(
            KeyWithModifier::new(BareKey::Char('`')).with_ctrl_modifier(),
            vec![launches("agents")],
        )];

        assert_eq!(
            plugin_launch_key(&keymap, "agents"),
            vec![KeyWithModifier::new(BareKey::Char('`')).with_ctrl_modifier()]
        );
    }

    #[test]
    fn plugin_launch_key_keeps_different_plugins_on_different_keys() {
        let keymap = vec![
            (
                KeyWithModifier::new(BareKey::Char('a')),
                vec![launches("agents"), TO_NORMAL],
            ),
            (
                KeyWithModifier::new(BareKey::Char('c')),
                vec![launches("todos"), TO_NORMAL],
            ),
            (
                KeyWithModifier::new(BareKey::Char('k')),
                vec![launches("new-agent"), TO_NORMAL],
            ),
        ];

        assert_eq!(
            plugin_launch_key(&keymap, "agents"),
            vec![KeyWithModifier::new(BareKey::Char('a'))]
        );
        assert_eq!(
            plugin_launch_key(&keymap, "todos"),
            vec![KeyWithModifier::new(BareKey::Char('c'))]
        );
        assert_eq!(
            plugin_launch_key(&keymap, "new-agent"),
            vec![KeyWithModifier::new(BareKey::Char('k'))]
        );
    }

    /// A `MessagePlugin { name "…" }` binding as the keymap holds it; `None` is how it reaches a
    /// Wasm plugin, whose wire carries no name.
    fn pipes(name: Option<&str>) -> Action {
        Action::KeybindPipe {
            name: name.map(str::to_string),
            payload: None,
            args: None,
            plugin: None,
            plugin_id: None,
            configuration: None,
            launch_new: false,
            skip_cache: false,
            floating: Some(false),
            in_place: None,
            cwd: None,
            pane_title: None,
        }
    }

    #[test]
    fn keybind_pipe_key_finds_the_message_by_name() {
        let keymap = vec![
            (
                KeyWithModifier::new(BareKey::Char('m')),
                vec![pipes(Some("something_else")), TO_NORMAL],
            ),
            (
                KeyWithModifier::new(BareKey::Char('o')),
                vec![pipes(Some("agent_strip_reposition")), TO_NORMAL],
            ),
            (
                KeyWithModifier::new(BareKey::Char('c')),
                vec![launches("agents"), TO_NORMAL],
            ),
        ];

        assert_eq!(
            keybind_pipe_key(&keymap, "agent_strip_reposition"),
            vec![KeyWithModifier::new(BareKey::Char('o'))]
        );
        assert_eq!(keybind_pipe_key(&keymap, "nothing_sends_this"), vec![]);
        // A plugin launch is not a message, whatever it is called.
        assert_eq!(keybind_pipe_key(&keymap[2..], "agents"), vec![]);
    }

    // The Wasm bar's keymap comes over the plugin API, which drops a pipe's name. Were a nameless
    // pipe not to count, this bar would never show the Move panel key the Deno bar shows, and the
    // two would draw Agent mode differently.
    #[test]
    fn keybind_pipe_key_counts_a_pipe_whose_name_the_wire_dropped() {
        let keymap = vec![
            (
                KeyWithModifier::new(BareKey::Char('o')),
                vec![pipes(None), TO_NORMAL],
            ),
            (KeyWithModifier::new(BareKey::Char('q')), vec![TO_NORMAL]),
        ];

        assert_eq!(
            keybind_pipe_key(&keymap, "agent_strip_reposition"),
            vec![KeyWithModifier::new(BareKey::Char('o'))]
        );
        assert_eq!(
            keybind_pipe_key(&keymap[1..], "agent_strip_reposition"),
            vec![]
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

    /// A minute boundary, so `next_tick_at`'s arithmetic is readable in the tests below.
    const MINUTE: i64 = 60_000;

    #[test]
    fn an_inferred_chord_never_shortens_a_live_hold() {
        let live_hold = next_alt_hint_until(0, MINUTE, true, true);
        assert_eq!(live_hold, MINUTE + LIVE_ALT_HINT_SAFETY_MS);

        // The chord arrives mid-hold: it is weaker evidence than the hold itself and must not
        // pull the deadline back to its own, much shorter, window.
        let after_chord = next_alt_hint_until(live_hold, MINUTE + 100, true, false);
        assert_eq!(after_chord, live_hold);
    }

    #[test]
    fn inferred_chords_extend_each_other() {
        let first = next_alt_hint_until(0, MINUTE, true, false);
        assert_eq!(first, MINUTE + ALT_HINT_WINDOW_MS);

        let second = next_alt_hint_until(first, MINUTE + 1000, true, false);
        assert_eq!(second, MINUTE + 1000 + ALT_HINT_WINDOW_MS);
    }

    #[test]
    fn a_live_release_takes_the_hint_down_and_an_inferred_one_is_a_no_op() {
        let held = next_alt_hint_until(0, MINUTE, true, true);
        assert_eq!(next_alt_hint_until(held, MINUTE + 100, false, true), 0);
        assert_eq!(next_alt_hint_until(held, MINUTE + 100, false, false), held);
    }

    #[test]
    fn the_tick_lands_on_the_next_minute_unless_a_hint_expires_first() {
        // No hint: the next boundary, however far into the minute we are.
        assert_eq!(next_tick_at(MINUTE + 100, 0, None), 2 * MINUTE);
        // A hint expiring inside the minute pulls the tick in; one expiring past it does not.
        assert_eq!(
            next_tick_at(MINUTE + 100, MINUTE + 3000, None),
            MINUTE + 3000
        );
        assert_eq!(next_tick_at(MINUTE + 100, 5 * MINUTE, None), 2 * MINUTE);
        // An already-expired deadline is not in the future and cannot pull anything in.
        assert_eq!(next_tick_at(MINUTE + 100, MINUTE, None), 2 * MINUTE);
    }

    #[test]
    fn agent_deadlines_follow_counts_and_leave_existing_ticks_alone() {
        let mut state = State {
            last_render_rows: 1,
            ..State::default()
        };
        state.agent_counts = AgentCounts {
            busy: 2,
            waiting: 1,
            idle: 3,
        };
        assert_eq!(state.animation_interval_ms(), Some(125));
        let busy = next_tick_at(MINUTE + 100, 0, state.animation_interval_ms());
        assert_eq!(busy, MINUTE + 125);
        assert!(should_arm_tick(Some(2 * MINUTE), busy));
        assert!(!should_arm_tick(Some(busy), busy));
        // Both Alt and hover use this same deadline; hints do not stop animation.
        assert_eq!(
            next_tick_at(MINUTE + 100, MINUTE + 110, Some(125)),
            MINUTE + 110
        );
        assert_eq!(next_tick_at(MINUTE + 100, MINUTE + 3000, Some(125)), busy);
        assert_eq!(next_tick_at(MINUTE + 110, MINUTE + 110, Some(125)), busy);
        state.agent_counts.busy = 0;
        assert_eq!(state.animation_interval_ms(), Some(500));
        let waiting = next_tick_at(MINUTE + 100, 0, state.animation_interval_ms());
        assert_eq!(waiting, MINUTE + 500);
        assert!(!should_arm_tick(Some(busy), waiting));
        state.agent_counts.waiting = 0;
        assert_eq!(state.animation_interval_ms(), None);
        let clock = next_tick_at(MINUTE + 500, 0, state.animation_interval_ms());
        assert_eq!(clock, 2 * MINUTE);
        assert!(!should_arm_tick(Some(waiting), clock));
        assert!(should_arm_tick(None, clock));
        state.agent_counts.busy = 1;
        state.classic_ui = true;
        assert_eq!(state.animation_interval_ms(), None);
        state.classic_ui = false;
        state.last_render_rows = 2;
        assert_eq!(state.animation_interval_ms(), None);
    }

    #[test]
    fn a_tick_already_outstanding_is_not_armed_again() {
        // Nothing outstanding, or one that fires too late to serve: arm.
        assert!(should_arm_tick(None, MINUTE));
        assert!(should_arm_tick(Some(2 * MINUTE), MINUTE));
        // One that fires at or before the tick we want will wake us, and waking early only
        // costs a frame - where a second timer costs one every minute from here on.
        assert!(!should_arm_tick(Some(MINUTE), MINUTE));
        assert!(!should_arm_tick(Some(MINUTE), 2 * MINUTE));
    }

    #[test]
    fn a_hover_column_past_the_end_of_the_bar_is_the_pointer_leaving() {
        // What the server actually sends on a leave, and every column that is really on the
        // bar - the bar being 80 wide here.
        assert!(pointer_left_the_bar(80, u16::MAX as usize));
        assert!(pointer_left_the_bar(80, 80));
        assert!(!pointer_left_the_bar(80, 79));
        assert!(!pointer_left_the_bar(80, 0));
        // Before the first frame there is no width to judge against, so nothing is a leave.
        assert!(!pointer_left_the_bar(0, u16::MAX as usize));
    }

    #[test]
    fn a_run_of_alt_chords_arms_no_extra_timers() {
        // What used to leak: every chord re-aimed the tick, and every re-aim spawned a timer
        // the host cannot cancel. Each chord pushes the deadline *later*, so after the first
        // the outstanding tick already covers it.
        let mut pending = None;
        let mut hint_until = 0;
        let mut armed = 0;
        for chord in 0..100 {
            let now = MINUTE + chord * 10;
            hint_until = next_alt_hint_until(hint_until, now, true, false);
            let at = next_tick_at(now, hint_until, None);
            if should_arm_tick(pending, at) {
                pending = Some(at);
                armed += 1;
            }
        }
        assert_eq!(armed, 1);
    }

    #[test]
    fn the_utc_offset_deadline_is_the_next_whole_hour() {
        // The point of the deadline: whatever phase of the hour the session loads at, the
        // re-ask lands on the boundary a DST change happens on.
        for offset_into_the_hour in [0, 1, 37 * 60 * 1000, UTC_OFFSET_HOUR_MS - 1] {
            let now = 480_000 * UTC_OFFSET_HOUR_MS + offset_into_the_hour;
            let next = next_utc_hour(now);
            assert!(next > now, "{now} -> {next} did not move forward");
            assert_eq!(next % UTC_OFFSET_HOUR_MS, 0, "{now} -> {next} is off-hour");
            assert!(
                next - now <= UTC_OFFSET_HOUR_MS,
                "{now} -> {next} is more than an hour out"
            );
        }
    }
}

use crate::output::CharacterChunk;
use crate::pane::GuestChoiceIndicator;
use crate::panes::{
    AnsiCode, CharacterStyles, RcCharacterStyles, TerminalCharacter, EMPTY_TERMINAL_CHARACTER,
};
use crate::ui::boundaries::boundary_type;
use crate::ui::hint_text::{
    exit_code_segments, hover_segments, rerun_segments, resize_segments, suspended_marker_segments,
    suspended_segments, HintExitStatus, HintLevel, HintSegment, HintTier,
};
use crate::ClientId;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::PaletteColor;
use yinz_types::multiplayer::ColorSlot;
use yinz_types::pane_size::{Offset, PaneGeom, Viewport};
use yinz_types::position::Position;
use yinz_types::style::Style;

use std::iter::repeat_n;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

fn styled_characters(
    characters: &str,
    style_character: impl Fn(&mut CharacterStyles),
) -> Vec<TerminalCharacter> {
    characters
        .chars()
        .map(|character| {
            let mut styles = RcCharacterStyles::reset();
            styles.update(|styles| style_character(styles));
            TerminalCharacter::new_styled(character, styles)
        })
        .collect()
}

fn foreground_color(characters: &str, color: Option<PaletteColor>) -> Vec<TerminalCharacter> {
    styled_characters(characters, |styles| {
        styles.bold = Some(AnsiCode::On);
        if let Some(palette_color) = color {
            styles.foreground = Some(AnsiCode::from(palette_color));
        }
    })
}

fn dimmed_foreground_color(
    characters: &str,
    color: Option<PaletteColor>,
) -> Vec<TerminalCharacter> {
    styled_characters(characters, |styles| {
        styles.dim = Some(AnsiCode::On);
        if let Some(palette_color) = color {
            styles.foreground = Some(AnsiCode::from(palette_color));
        }
    })
}

fn background_color(characters: &str, color: Option<PaletteColor>) -> Vec<TerminalCharacter> {
    styled_characters(characters, |styles| {
        if let Some(palette_color) = color {
            styles.background = Some(AnsiCode::from(palette_color));
            styles.bold(Some(AnsiCode::On));
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ExitStatus {
    Code(i32),
    Exited,
}

#[derive(Clone, PartialEq)]
pub struct StackListEntry {
    pub width: usize,
    pub label: String,
    pub is_selected: bool,
    pub is_emphasized: bool,
    pub stack_is_focused: bool,
    pub border_color: Option<PaletteColor>,
}

/// A sampled 450ms opening wave. Coordinates are the linked panel's, so its halves have
/// the same horizontal phase. Kept in `PaneFrame` so a changed sample invalidates its cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpeningRipple {
    elapsed_ms: u16,
    origin: usize,
    width: usize,
}

impl OpeningRipple {
    pub fn sample(
        start: std::time::Instant,
        now: std::time::Instant,
        origin: usize,
        width: usize,
    ) -> Option<Self> {
        let elapsed = now.saturating_duration_since(start).as_millis();
        (elapsed < 450).then_some(Self {
            elapsed_ms: elapsed as u16,
            origin,
            width,
        })
    }

    fn stroke(self, column: usize) -> &'static str {
        let t = f32::from(self.elapsed_ms) / 450.0;
        let x = column.saturating_sub(self.origin) as f32;
        let front = t * (self.width.max(1) as f32 + 12.0);
        let distance = front - x;
        if !(0.0..12.0).contains(&distance) {
            return boundary_type::HORIZONTAL;
        }
        let amplitude = (distance * std::f32::consts::PI / 12.0).sin() * (1.0 - t);
        if amplitude > 0.45 {
            "∿"
        } else if amplitude > 0.025 {
            "∽"
        } else {
            boundary_type::HORIZONTAL
        }
    }
}

pub struct FrameParams {
    pub focused_client: Option<ClientId>,
    pub is_main_client: bool, // more accurately: is_focused_for_main_client
    /// The color slot of each other client focused on this pane.
    pub other_focused_clients: Vec<Option<ColorSlot>>,
    pub style: Style,
    pub color: Option<PaletteColor>,
    pub other_cursors_exist_in_session: bool,
    pub pane_is_stacked_under: bool,
    pub pane_is_stacked_over: bool,
    pub pane_is_stacked: bool,
    pub should_draw_pane_frames: bool,
    pub pane_is_floating: bool,
    pub show_pinned_indication: bool,
    pub show_scroll_indication: bool,
    pub content_offset: Offset,
    pub mouse_is_hovering_over_pane: bool,
    pub pane_is_selectable: bool,
    pub show_help_text: bool,
    pub highlight_tooltip: Option<String>,
    pub omit_title: bool,
    pub frame_geom_override: Option<PaneGeom>,
    pub stack_list_entry: Option<StackListEntry>,
    pub blank_title: bool,
    pub same_worktree: bool,
    pub mouse_scroll_resize: bool,
    pub dimmed: bool,
    pub guest_choice_indicator: Option<GuestChoiceIndicator>,
    pub opening_ripple: Option<OpeningRipple>,
}

#[derive(Default, PartialEq)]
pub struct PaneFrame {
    pub geom: Viewport,
    pub title: String,
    /// Text and its screen row, after any stack headers.
    pub subtitle: Option<(String, usize)>,
    pub scroll_position: (usize, usize), // (position, length)
    pub style: Style,
    pub color: Option<PaletteColor>,
    pub focused_client: Option<ClientId>,
    pub is_main_client: bool,
    pub other_cursors_exist_in_session: bool,
    /// The color slot of each other client focused on this pane.
    pub other_focused_clients: Vec<Option<ColorSlot>>,
    exit_status: Option<ExitStatus>,
    is_first_run: bool,
    /// The pane's root process is stopped. Part of the struct rather than looked
    /// up at render time because `PaneFrame` is compared against the last frame
    /// to decide whether to repaint at all.
    is_suspended: bool,
    pane_is_stacked_over: bool,
    pane_is_stacked_under: bool,
    pane_is_stacked: bool,
    should_draw_pane_frames: bool,
    is_pinned: bool,
    is_floating: bool,
    show_pinned_indication: bool,
    show_scroll_indication: bool,
    content_offset: Offset,
    mouse_is_hovering_over_pane: bool,
    is_selectable: bool,
    show_help_text: bool,
    highlight_tooltip: Option<String>,
    omit_title: bool,
    stack_list_entry: Option<StackListEntry>,
    mouse_scroll_resize: bool,
    dimmed: bool,
    guest_choice_indicator: Option<GuestChoiceIndicator>,
    opening_ripple: Option<OpeningRipple>,
}

impl PaneFrame {
    pub fn new(
        geom: Viewport,
        scroll_position: (usize, usize),
        main_title: String,
        frame_params: FrameParams,
    ) -> Self {
        PaneFrame {
            geom,
            title: yinz_types::text::clean_string_from_control_and_linebreak(&main_title),
            subtitle: None,
            scroll_position,
            style: frame_params.style,
            color: frame_params.color,
            focused_client: frame_params.focused_client,
            is_main_client: frame_params.is_main_client,
            other_focused_clients: frame_params.other_focused_clients,
            other_cursors_exist_in_session: frame_params.other_cursors_exist_in_session,
            exit_status: None,
            is_first_run: false,
            is_suspended: false,
            pane_is_stacked_over: frame_params.pane_is_stacked_over,
            pane_is_stacked_under: frame_params.pane_is_stacked_under,
            pane_is_stacked: frame_params.pane_is_stacked,
            should_draw_pane_frames: frame_params.should_draw_pane_frames,
            is_pinned: false,
            is_floating: frame_params.pane_is_floating,
            show_pinned_indication: frame_params.show_pinned_indication,
            show_scroll_indication: frame_params.show_scroll_indication,
            content_offset: frame_params.content_offset,
            mouse_is_hovering_over_pane: frame_params.mouse_is_hovering_over_pane,
            is_selectable: frame_params.pane_is_selectable,
            show_help_text: frame_params.show_help_text,
            highlight_tooltip: frame_params.highlight_tooltip,
            omit_title: frame_params.omit_title,
            stack_list_entry: frame_params.stack_list_entry,
            mouse_scroll_resize: frame_params.mouse_scroll_resize,
            dimmed: frame_params.dimmed,
            guest_choice_indicator: frame_params.guest_choice_indicator,
            opening_ripple: frame_params.opening_ripple,
        }
    }
    fn horizontal(&self, column: usize) -> &'static str {
        self.opening_ripple
            .filter(|_| self.should_draw_pane_frames && self.geom.rows > 1 && self.geom.cols > 2)
            .map(|ripple| ripple.stroke(column))
            .unwrap_or(boundary_type::HORIZONTAL)
    }

    pub fn is_pinned(mut self, is_pinned: bool) -> Self {
        self.is_pinned = is_pinned;
        self
    }
    pub fn add_exit_status(&mut self, exit_status: Option<i32>) {
        self.exit_status = match exit_status {
            Some(exit_status) => Some(ExitStatus::Code(exit_status)),
            None => Some(ExitStatus::Exited),
        };
    }
    pub fn indicate_suspended(&mut self) {
        self.is_suspended = true;
    }
    pub fn indicate_first_run(&mut self) {
        self.is_first_run = true;
    }
    fn client_cursor(&self, slot: Option<ColorSlot>) -> Vec<TerminalCharacter> {
        let color = slot.map(|slot| slot.colors(self.style.colors.multiplayer_user_colors));
        background_color(" ", color.map(|c| c.0))
    }
    fn get_corner(&self, corner: &'static str) -> &'static str {
        let corner = if !self.should_draw_pane_frames
            && (corner == boundary_type::TOP_LEFT || corner == boundary_type::TOP_RIGHT)
        {
            boundary_type::HORIZONTAL
        } else if self.pane_is_stacked_under && corner == boundary_type::TOP_RIGHT {
            boundary_type::BOTTOM_RIGHT
        } else if self.pane_is_stacked_under && corner == boundary_type::TOP_LEFT {
            boundary_type::BOTTOM_LEFT
        } else {
            corner
        };
        if self.style.rounded_corners {
            match corner {
                boundary_type::TOP_RIGHT => boundary_type::TOP_RIGHT_ROUND,
                boundary_type::TOP_LEFT => boundary_type::TOP_LEFT_ROUND,
                boundary_type::BOTTOM_RIGHT => boundary_type::BOTTOM_RIGHT_ROUND,
                boundary_type::BOTTOM_LEFT => boundary_type::BOTTOM_LEFT_ROUND,
                _ => corner,
            }
        } else {
            corner
        }
    }
    fn render_title_right_side(
        &self,
        max_length: usize,
    ) -> Option<(Vec<TerminalCharacter>, usize)> {
        // string and length because of color
        let has_scroll = self.show_scroll_indication
            && (self.scroll_position.0 > 0 || self.scroll_position.1 > 0);
        if has_scroll && self.is_selectable {
            let pin_indication =
                if self.is_floating && self.is_selectable && self.show_pinned_indication {
                    self.render_pinned_indication(max_length)
                } else {
                    None
                }; // no pin indication for tiled panes
            let space_for_scroll_indication = pin_indication
                .as_ref()
                .map(|(_, length)| max_length.saturating_sub(*length + 1))
                .unwrap_or(max_length);
            let scroll_indication = self.render_scroll_indication(space_for_scroll_indication);
            match (pin_indication, scroll_indication) {
                (
                    Some((mut pin_indication, pin_indication_len)),
                    Some((mut scroll_indication, scroll_indication_len)),
                ) => {
                    let mut characters: Vec<_> = std::mem::take(&mut scroll_indication);
                    let mut separator = foreground_color("|", self.color);
                    characters.append(&mut separator);
                    characters.append(&mut pin_indication);
                    Some((characters, pin_indication_len + scroll_indication_len + 1))
                },
                (Some(pin_indication), None) => Some(pin_indication),
                (None, Some(scroll_indication)) => Some(scroll_indication),
                _ => None,
            }
        } else if self.is_floating && self.is_selectable && self.show_pinned_indication {
            self.render_pinned_indication(max_length)
        } else {
            None
        }
    }
    fn render_scroll_indication(
        &self,
        max_length: usize,
    ) -> Option<(Vec<TerminalCharacter>, usize)> {
        let prefix = " SCROLL: ";
        let full_indication = format!(" {}/{} ", self.scroll_position.0, self.scroll_position.1);
        let short_indication = format!(" {} ", self.scroll_position.0);
        let full_indication_len = full_indication.chars().count();
        let short_indication_len = short_indication.chars().count();
        let prefix_len = prefix.chars().count();
        if prefix_len + full_indication_len <= max_length {
            Some((
                foreground_color(&format!("{}{}", prefix, full_indication), self.color),
                prefix_len + full_indication_len,
            ))
        } else if full_indication_len <= max_length {
            Some((
                foreground_color(&full_indication, self.color),
                full_indication_len,
            ))
        } else if short_indication_len <= max_length {
            Some((
                foreground_color(&short_indication, self.color),
                short_indication_len,
            ))
        } else {
            None
        }
    }
    fn render_pinned_indication(
        &self,
        max_length: usize,
    ) -> Option<(Vec<TerminalCharacter>, usize)> {
        let is_checked = if self.is_pinned { '+' } else { ' ' };
        let full_indication = format!(" PIN [{}] ", is_checked);
        let full_indication_len = full_indication.chars().count();
        if full_indication_len <= max_length {
            Some((
                foreground_color(&full_indication, self.color),
                full_indication_len,
            ))
        } else {
            None
        }
    }
    fn render_my_focus(&self, max_length: usize) -> Option<(Vec<TerminalCharacter>, usize)> {
        let mut left_separator = foreground_color(boundary_type::VERTICAL_LEFT, self.color);
        let mut right_separator = foreground_color(boundary_type::VERTICAL_RIGHT, self.color);
        let full_indication_text = "MY FOCUS";
        let mut full_indication = vec![];
        full_indication.append(&mut left_separator);
        full_indication.push(EMPTY_TERMINAL_CHARACTER);
        full_indication.append(&mut foreground_color(full_indication_text, self.color));
        full_indication.push(EMPTY_TERMINAL_CHARACTER);
        full_indication.append(&mut right_separator);
        let full_indication_len = full_indication_text.width() + 4; // 2 for separators 2 for padding
        let short_indication_text = "ME";
        let mut short_indication = vec![];
        short_indication.append(&mut left_separator);
        short_indication.push(EMPTY_TERMINAL_CHARACTER);
        short_indication.append(&mut foreground_color(short_indication_text, self.color));
        short_indication.push(EMPTY_TERMINAL_CHARACTER);
        short_indication.append(&mut right_separator);
        let short_indication_len = short_indication_text.width() + 4; // 2 for separators 2 for padding
        if full_indication_len <= max_length {
            Some((full_indication, full_indication_len))
        } else if short_indication_len <= max_length {
            Some((short_indication, short_indication_len))
        } else {
            None
        }
    }
    fn render_my_and_others_focus(
        &self,
        max_length: usize,
    ) -> Option<(Vec<TerminalCharacter>, usize)> {
        let mut left_separator = foreground_color(boundary_type::VERTICAL_LEFT, self.color);
        let mut right_separator = foreground_color(boundary_type::VERTICAL_RIGHT, self.color);
        let full_indication_text = "MY FOCUS AND:";
        let short_indication_text = "+";
        let mut full_indication = foreground_color(full_indication_text, self.color);
        let mut full_indication_len = full_indication_text.width();
        let mut short_indication = foreground_color(short_indication_text, self.color);
        let mut short_indication_len = short_indication_text.width();
        for slot in &self.other_focused_clients {
            let mut text = self.client_cursor(*slot);
            full_indication_len += 2;
            full_indication.push(EMPTY_TERMINAL_CHARACTER);
            full_indication.append(&mut text.clone());
            short_indication_len += 2;
            short_indication.push(EMPTY_TERMINAL_CHARACTER);
            short_indication.append(&mut text);
        }
        if full_indication_len + 4 <= max_length {
            // 2 for separators, 2 for padding
            let mut ret = vec![];
            ret.append(&mut left_separator);
            ret.push(EMPTY_TERMINAL_CHARACTER);
            ret.append(&mut full_indication);
            ret.push(EMPTY_TERMINAL_CHARACTER);
            ret.append(&mut right_separator);
            Some((ret, full_indication_len + 4))
        } else if short_indication_len + 4 <= max_length {
            // 2 for separators, 2 for padding
            let mut ret = vec![];
            ret.append(&mut left_separator);
            ret.push(EMPTY_TERMINAL_CHARACTER);
            ret.append(&mut short_indication);
            ret.push(EMPTY_TERMINAL_CHARACTER);
            ret.append(&mut right_separator);
            Some((ret, short_indication_len + 4))
        } else {
            None
        }
    }
    fn render_other_focused_users(
        &self,
        max_length: usize,
    ) -> Option<(Vec<TerminalCharacter>, usize)> {
        let mut left_separator = foreground_color(boundary_type::VERTICAL_LEFT, self.color);
        let mut right_separator = foreground_color(boundary_type::VERTICAL_RIGHT, self.color);
        let full_indication_text = if self.other_focused_clients.len() == 1 {
            "FOCUSED USER:"
        } else {
            "FOCUSED USERS:"
        };
        let middle_indication_text = "U:";
        let mut full_indication = foreground_color(full_indication_text, self.color);
        let mut full_indication_len = full_indication_text.width();
        let mut middle_indication = foreground_color(middle_indication_text, self.color);
        let mut middle_indication_len = middle_indication_text.width();
        let mut short_indication = vec![];
        let mut short_indication_len = 0;
        for slot in &self.other_focused_clients {
            let mut text = self.client_cursor(*slot);
            full_indication_len += 2;
            full_indication.push(EMPTY_TERMINAL_CHARACTER);
            full_indication.append(&mut text.clone());
            middle_indication_len += 2;
            middle_indication.push(EMPTY_TERMINAL_CHARACTER);
            middle_indication.append(&mut text.clone());
            short_indication_len += 2;
            short_indication.push(EMPTY_TERMINAL_CHARACTER);
            short_indication.append(&mut text);
        }
        if full_indication_len + 4 <= max_length {
            // 2 for separators, 2 for padding
            let mut ret = vec![];
            ret.append(&mut left_separator);
            ret.push(EMPTY_TERMINAL_CHARACTER);
            ret.append(&mut full_indication);
            ret.push(EMPTY_TERMINAL_CHARACTER);
            ret.append(&mut right_separator);
            Some((ret, full_indication_len + 4))
        } else if middle_indication_len + 4 <= max_length {
            // 2 for separators, 2 for padding
            let mut ret = vec![];
            ret.append(&mut left_separator);
            ret.push(EMPTY_TERMINAL_CHARACTER);
            ret.append(&mut middle_indication);
            ret.push(EMPTY_TERMINAL_CHARACTER);
            ret.append(&mut right_separator);
            Some((ret, middle_indication_len + 4))
        } else if short_indication_len + 3 <= max_length {
            // 2 for separators, 1 for padding
            let mut ret = vec![];
            ret.append(&mut left_separator);
            ret.push(EMPTY_TERMINAL_CHARACTER);
            ret.append(&mut short_indication);
            ret.push(EMPTY_TERMINAL_CHARACTER);
            ret.append(&mut right_separator);
            Some((ret, short_indication_len + 3))
        } else {
            None
        }
    }
    fn render_title_middle(&self, max_length: usize) -> Option<(Vec<TerminalCharacter>, usize)> {
        // string and length because of color
        let renders_exit_status_in_title = self.pane_is_stacked_under
            || self.pane_is_stacked_over
            || !self.should_draw_pane_frames;
        if renders_exit_status_in_title && self.exit_status.is_some() {
            let (first_part, first_part_len) = self.first_exited_held_title_part_full();
            return if first_part_len <= max_length {
                Some((first_part, first_part_len))
            } else {
                None
            };
        }
        if self.is_main_client
            && self.other_focused_clients.is_empty()
            && !self.other_cursors_exist_in_session
        {
            None
        } else if self.is_main_client
            && self.other_focused_clients.is_empty()
            && self.other_cursors_exist_in_session
        {
            self.render_my_focus(max_length)
        } else if self.is_main_client && !self.other_focused_clients.is_empty() {
            self.render_my_and_others_focus(max_length)
        } else if !self.other_focused_clients.is_empty() {
            self.render_other_focused_users(max_length)
        } else {
            None
        }
    }
    fn render_title_left_side(&self, max_length: usize) -> Option<(Vec<TerminalCharacter>, usize)> {
        let middle_truncated_sign = "[..]";
        let middle_truncated_sign_long = "[...]";
        let full_text = format!(" {} ", self.title);
        if max_length <= 6 || self.title.is_empty() {
            None
        } else if full_text.width() <= max_length {
            Some((foreground_color(&full_text, self.color), full_text.width()))
        } else {
            let length_of_each_half = (max_length - middle_truncated_sign.width()) / 2;

            let mut first_part: String = String::new();
            for char in full_text.chars() {
                if first_part.width() + char.width().unwrap_or(0) > length_of_each_half {
                    break;
                } else {
                    first_part.push(char);
                }
            }

            let mut second_part: String = String::new();
            for char in full_text.chars().rev() {
                if second_part.width() + char.width().unwrap_or(0) > length_of_each_half {
                    break;
                } else {
                    second_part.insert(0, char);
                }
            }

            let (title_left_side, title_length) = if first_part.width()
                + middle_truncated_sign.width()
                + second_part.width()
                < max_length
            {
                // this means we lost 1 character when dividing the total length into halves
                (
                    format!(
                        "{}{}{}",
                        first_part, middle_truncated_sign_long, second_part
                    ),
                    first_part.width() + middle_truncated_sign_long.width() + second_part.width(),
                )
            } else {
                (
                    format!("{}{}{}", first_part, middle_truncated_sign, second_part),
                    first_part.width() + middle_truncated_sign.width() + second_part.width(),
                )
            };
            Some((foreground_color(&title_left_side, self.color), title_length))
        }
    }
    fn three_part_title_line(
        &self,
        mut left_side: Vec<TerminalCharacter>,
        left_side_len: &usize,
        mut middle: Vec<TerminalCharacter>,
        middle_len: &usize,
        mut right_side: Vec<TerminalCharacter>,
        right_side_len: &usize,
    ) -> Vec<TerminalCharacter> {
        let total_title_length = self.geom.cols.saturating_sub(2); // 2 for the left and right corners
        let mut title_line = vec![];
        let left_side_start_position = self.geom.x + 1;
        let middle_start_position = self.geom.x + (total_title_length / 2) - (middle_len / 2) + 1;
        let right_side_start_position =
            (self.geom.x + self.geom.cols - 1).saturating_sub(*right_side_len);

        let mut col = self.geom.x;
        loop {
            if col == self.geom.x {
                title_line.append(&mut foreground_color(
                    self.get_corner(boundary_type::TOP_LEFT),
                    self.color,
                ));
            } else if col == self.geom.x + self.geom.cols - 1 {
                title_line.append(&mut foreground_color(
                    self.get_corner(boundary_type::TOP_RIGHT),
                    self.color,
                ));
            } else if col == left_side_start_position {
                title_line.append(&mut left_side);
                col += left_side_len;
                continue;
            } else if col == middle_start_position {
                title_line.append(&mut middle);
                col += middle_len;
                continue;
            } else if col == right_side_start_position {
                title_line.append(&mut right_side);
                col += right_side_len;
                continue;
            } else {
                title_line.append(&mut foreground_color(self.horizontal(col), self.color));
            }
            if col == self.geom.x + self.geom.cols - 1 {
                break;
            }
            col += 1;
        }
        title_line
    }
    fn left_and_middle_title_line(
        &self,
        mut left_side: Vec<TerminalCharacter>,
        left_side_len: &usize,
        mut middle: Vec<TerminalCharacter>,
        middle_len: &usize,
    ) -> Vec<TerminalCharacter> {
        let total_title_length = self.geom.cols.saturating_sub(2); // 2 for the left and right corners
        let mut title_line = vec![];
        let left_side_start_position = self.geom.x + 1;
        let middle_start_position = self.geom.x + (total_title_length / 2) - (*middle_len / 2) + 1;

        let mut col = self.geom.x;
        loop {
            if col == self.geom.x {
                title_line.append(&mut foreground_color(
                    self.get_corner(boundary_type::TOP_LEFT),
                    self.color,
                ));
            } else if col == self.geom.x + self.geom.cols - 1 {
                title_line.append(&mut foreground_color(
                    self.get_corner(boundary_type::TOP_RIGHT),
                    self.color,
                ));
            } else if col == left_side_start_position {
                title_line.append(&mut left_side);
                col += *left_side_len;
                continue;
            } else if col == middle_start_position {
                title_line.append(&mut middle);
                col += *middle_len;
                continue;
            } else {
                title_line.append(&mut foreground_color(self.horizontal(col), self.color));
            }
            if col == self.geom.x + self.geom.cols - 1 {
                break;
            }
            col += 1;
        }
        title_line
    }
    fn middle_only_title_line(
        &self,
        mut middle: Vec<TerminalCharacter>,
        middle_len: &usize,
    ) -> Vec<TerminalCharacter> {
        let total_title_length = self.geom.cols.saturating_sub(2); // 2 for the left and right corners
        let mut title_line = vec![];
        let middle_start_position = self.geom.x + (total_title_length / 2) - (*middle_len / 2) + 1;

        let mut col = self.geom.x;
        loop {
            if col == self.geom.x {
                title_line.append(&mut foreground_color(
                    self.get_corner(boundary_type::TOP_LEFT),
                    self.color,
                ));
            } else if col == self.geom.x + self.geom.cols - 1 {
                title_line.append(&mut foreground_color(
                    self.get_corner(boundary_type::TOP_RIGHT),
                    self.color,
                ));
            } else if col == middle_start_position {
                title_line.append(&mut middle);
                col += *middle_len;
                continue;
            } else {
                title_line.append(&mut foreground_color(self.horizontal(col), self.color));
            }
            if col == self.geom.x + self.geom.cols - 1 {
                break;
            }
            col += 1;
        }
        title_line
    }
    fn two_part_title_line(
        &self,
        mut left_side: Vec<TerminalCharacter>,
        left_side_len: &usize,
        mut right_side: Vec<TerminalCharacter>,
        right_side_len: &usize,
    ) -> Vec<TerminalCharacter> {
        let mut left_boundary =
            foreground_color(self.get_corner(boundary_type::TOP_LEFT), self.color);
        let mut right_boundary =
            foreground_color(self.get_corner(boundary_type::TOP_RIGHT), self.color);
        let total_title_length = self.geom.cols.saturating_sub(2); // 2 for the left and right corners
        let mut middle = String::new();
        for col in *left_side_len..total_title_length.saturating_sub(*right_side_len) {
            middle.push_str(self.horizontal(self.geom.x + 1 + col));
        }
        let mut ret = vec![];
        ret.append(&mut left_boundary);
        ret.append(&mut left_side);
        ret.append(&mut foreground_color(&middle, self.color));
        ret.append(&mut right_side);
        ret.append(&mut right_boundary);
        ret
    }
    fn left_only_title_line(
        &self,
        mut left_side: Vec<TerminalCharacter>,
        left_side_len: &usize,
    ) -> Vec<TerminalCharacter> {
        let mut left_boundary =
            foreground_color(self.get_corner(boundary_type::TOP_LEFT), self.color);
        let mut right_boundary =
            foreground_color(self.get_corner(boundary_type::TOP_RIGHT), self.color);
        let total_title_length = self.geom.cols.saturating_sub(2); // 2 for the left and right corners
        let mut middle_padding = String::new();
        for col in *left_side_len..total_title_length {
            middle_padding.push_str(self.horizontal(self.geom.x + 1 + col));
        }
        let mut ret = vec![];
        ret.append(&mut left_boundary);
        ret.append(&mut left_side);
        ret.append(&mut foreground_color(&middle_padding, self.color));
        ret.append(&mut right_boundary);
        ret
    }
    fn empty_title_line(&self) -> Vec<TerminalCharacter> {
        let mut left_boundary =
            foreground_color(self.get_corner(boundary_type::TOP_LEFT), self.color);
        let mut right_boundary =
            foreground_color(self.get_corner(boundary_type::TOP_RIGHT), self.color);
        let total_title_length = self.geom.cols.saturating_sub(2); // 2 for the left and right corners
        let mut middle_padding = String::new();
        for col in 0..total_title_length {
            middle_padding.push_str(self.horizontal(self.geom.x + 1 + col));
        }
        let mut ret = vec![];
        ret.append(&mut left_boundary);
        ret.append(&mut foreground_color(&middle_padding, self.color));
        ret.append(&mut right_boundary);
        ret
    }
    fn title_line_with_middle(
        &self,
        middle: Vec<TerminalCharacter>,
        middle_len: &usize,
    ) -> Vec<TerminalCharacter> {
        let total_title_length = self.geom.cols.saturating_sub(2); // 2 for the left and right corners
        let length_of_each_side = total_title_length.saturating_sub(*middle_len + 2) / 2;
        let left_side = self.render_title_left_side(length_of_each_side);
        let right_side = self.render_title_right_side(length_of_each_side);

        match (left_side, right_side) {
            (Some((left_side, left_side_len)), Some((right_side, right_side_len))) => self
                .three_part_title_line(
                    left_side,
                    &left_side_len,
                    middle,
                    middle_len,
                    right_side,
                    &right_side_len,
                ),
            (Some((left_side, left_side_len)), None) => {
                self.left_and_middle_title_line(left_side, &left_side_len, middle, middle_len)
            },
            _ => self.middle_only_title_line(middle, middle_len),
        }
    }
    fn title_line_without_middle(&self) -> Vec<TerminalCharacter> {
        let total_title_length = self.geom.cols.saturating_sub(2); // 2 for the left and right corners
        let left_side = self.render_title_left_side(total_title_length);
        let right_side = left_side.as_ref().and_then(|(_left_side, left_side_len)| {
            let space_left = total_title_length.saturating_sub(*left_side_len + 1); // 1 for a middle separator
            self.render_title_right_side(space_left)
        });
        match (left_side, right_side) {
            (Some((left_side, left_side_len)), Some((right_side, right_side_len))) => {
                self.two_part_title_line(left_side, &left_side_len, right_side, &right_side_len)
            },
            (Some((left_side, left_side_len)), None) => {
                self.left_only_title_line(left_side, &left_side_len)
            },
            _ => self.empty_title_line(),
        }
    }
    fn render_title(&self) -> Result<Vec<TerminalCharacter>> {
        let total_title_length = self.geom.cols.saturating_sub(2); // 2 for the left and right corners

        self.render_title_middle(total_title_length)
            .map(|(middle, middle_length)| self.title_line_with_middle(middle, &middle_length))
            .or_else(|| Some(self.title_line_without_middle()))
            .with_context(|| format!("failed to render title '{}'", self.title))
    }
    fn render_stack_list_entry(&self, entry: &StackListEntry) -> Vec<TerminalCharacter> {
        let usable_cols = self.geom.cols;
        let selection_marker = "> ";
        let marker_width = selection_marker.width();
        let corner_padding = 1;
        let corner_overhead = 2 * (1 + corner_padding);
        let inner_width = entry
            .width
            .min(usable_cols.saturating_sub(marker_width + corner_overhead));
        let content = if entry.label.width() <= inner_width {
            entry.label.clone()
        } else {
            let truncation_budget = inner_width.saturating_sub(1);
            let mut truncated = String::new();
            for character in entry.label.chars() {
                if truncated.width() + character.width().unwrap_or(0) > truncation_budget {
                    break;
                }
                truncated.push(character);
            }
            truncated.push('…');
            truncated
        };
        let full_width_entry_length = marker_width + corner_overhead + inner_width;
        let entry_start = usable_cols.saturating_sub(full_width_entry_length) / 2;
        let left_budget = entry_start;
        let (mut focus_part, focus_length) = self
            .bracketed_focus_indicator(left_budget)
            .unwrap_or((vec![], 0));
        let mut line = Vec::with_capacity(usable_cols);
        line.append(&mut focus_part);
        line.extend(repeat_n(
            EMPTY_TERMINAL_CHARACTER,
            left_budget.saturating_sub(focus_length),
        ));
        let unfocused_color = self.style.colors.frame_unselected.map(|frame| frame.base);
        let style_entry_text = |text: &str| {
            if entry.is_selected || entry.is_emphasized {
                foreground_color(text, self.color)
            } else if entry.stack_is_focused {
                foreground_color(text, unfocused_color)
            } else {
                dimmed_foreground_color(text, unfocused_color)
            }
        };
        if entry.is_selected {
            line.append(&mut foreground_color(selection_marker, None));
        } else {
            line.extend(repeat_n(EMPTY_TERMINAL_CHARACTER, marker_width));
        }
        line.append(&mut foreground_color(
            boundary_type::VERTICAL,
            entry.border_color,
        ));
        line.extend(repeat_n(EMPTY_TERMINAL_CHARACTER, corner_padding));
        line.append(&mut style_entry_text(&content));
        let content_padding = inner_width.saturating_sub(content.width());
        line.extend(repeat_n(EMPTY_TERMINAL_CHARACTER, content_padding));
        line.extend(repeat_n(EMPTY_TERMINAL_CHARACTER, corner_padding));
        line.append(&mut foreground_color(
            boundary_type::VERTICAL,
            entry.border_color,
        ));
        let mut occupied_columns = entry_start + full_width_entry_length;
        let (mut scroll_part, scroll_length) = self
            .bracketed_scroll_indicator(usable_cols.saturating_sub(occupied_columns))
            .unwrap_or((vec![], 0));
        if self.exit_status.is_some() {
            let (mut exit_part, exit_length) = self.first_exited_held_title_part_full();
            if occupied_columns + 1 + exit_length + scroll_length <= usable_cols {
                line.push(EMPTY_TERMINAL_CHARACTER);
                line.append(&mut exit_part);
                occupied_columns += 1 + exit_length;
            }
        }
        while occupied_columns < usable_cols.saturating_sub(scroll_length) {
            line.push(EMPTY_TERMINAL_CHARACTER);
            occupied_columns += 1;
        }
        line.append(&mut scroll_part);
        line
    }
    fn render_one_line_title(&self) -> Result<Vec<TerminalCharacter>> {
        if self.should_draw_pane_frames {
            let total_title_length = self.geom.cols.saturating_sub(2);
            return self
                .render_title_middle(total_title_length)
                .map(|(middle, middle_length)| self.title_line_with_middle(middle, &middle_length))
                .or_else(|| Some(self.title_line_without_middle()))
                .with_context(|| format!("failed to render title '{}'", self.title));
        }

        let width = self.geom.cols;
        let title = self.bracketed_pane_title(width);
        let title_length = title.as_ref().map(|(_, length)| *length).unwrap_or(0);
        let side_budget = width.saturating_sub(title_length) / 2;
        let focus = self.bracketed_focus_indicator(side_budget);
        let focus_length = focus.as_ref().map(|(_, length)| *length).unwrap_or(0);
        let right_budget = width.saturating_sub(focus_length + title_length);
        let right = self.bracketed_scroll_indicator(right_budget);
        Ok(self.compose_bracketed_title(focus, title, right))
    }
    fn bracketed_title_part(&self, content: &str) -> (Vec<TerminalCharacter>, usize) {
        let text = format!(" [ {} ] ", content);
        (foreground_color(&text, self.color), text.width())
    }
    fn bracketed_title_part_from_characters(
        &self,
        mut content: Vec<TerminalCharacter>,
        content_length: usize,
    ) -> (Vec<TerminalCharacter>, usize) {
        let mut part = foreground_color(" [ ", self.color);
        part.append(&mut content);
        part.append(&mut foreground_color(" ] ", self.color));
        (part, content_length + 6)
    }
    fn plain_title_part(&self, content: &str) -> (Vec<TerminalCharacter>, usize) {
        let text = format!(" {} ", content);
        (foreground_color(&text, self.color), text.width())
    }
    fn bracketed_pane_title(&self, max_length: usize) -> Option<(Vec<TerminalCharacter>, usize)> {
        let title_padding = 2;
        if self.title.is_empty() || max_length <= title_padding {
            return None;
        }
        let exit_part = self
            .exit_status
            .is_some()
            .then(|| self.first_exited_held_title_part_full());
        let exit_length = exit_part.as_ref().map(|(_, length)| *length).unwrap_or(0);
        let content_max_length = max_length.saturating_sub(title_padding + exit_length);
        let content = if self.title.width() <= content_max_length {
            self.title.clone()
        } else {
            let truncation_budget = content_max_length.saturating_sub(1);
            let mut truncated = String::new();
            for character in self.title.chars() {
                if truncated.width() + character.width().unwrap_or(0) > truncation_budget {
                    break;
                }
                truncated.push(character);
            }
            truncated.push('…');
            truncated
        };
        let (mut part, mut length) = self.plain_title_part(&content);
        if let Some((mut exit, exit_length)) = exit_part {
            part.append(&mut exit);
            length += exit_length;
        }
        Some((part, length))
    }
    fn bracketed_scroll_indicator(
        &self,
        max_length: usize,
    ) -> Option<(Vec<TerminalCharacter>, usize)> {
        let has_scroll = self.show_scroll_indication
            && (self.scroll_position.0 > 0 || self.scroll_position.1 > 0);
        if !(has_scroll && self.is_selectable) {
            return None;
        }
        let full_indication = format!(
            "SCROLL: {}/{}",
            self.scroll_position.0, self.scroll_position.1
        );
        let (full_part, full_length) = self.bracketed_title_part(&full_indication);
        if full_length <= max_length {
            return Some((full_part, full_length));
        }
        let short_indication = format!("{}/{}", self.scroll_position.0, self.scroll_position.1);
        let (short_part, short_length) = self.bracketed_title_part(&short_indication);
        if short_length <= max_length {
            return Some((short_part, short_length));
        }
        None
    }
    fn bracketed_focus_indicator(
        &self,
        max_length: usize,
    ) -> Option<(Vec<TerminalCharacter>, usize)> {
        if self.is_main_client
            && self.other_focused_clients.is_empty()
            && !self.other_cursors_exist_in_session
        {
            None
        } else if self.is_main_client
            && self.other_focused_clients.is_empty()
            && self.other_cursors_exist_in_session
        {
            let (full_part, full_length) = self.bracketed_title_part("MY FOCUS");
            if full_length <= max_length {
                Some((full_part, full_length))
            } else {
                let (short_part, short_length) = self.bracketed_title_part("ME");
                (short_length <= max_length).then_some((short_part, short_length))
            }
        } else if self.is_main_client && !self.other_focused_clients.is_empty() {
            self.bracketed_focused_users("MY FOCUS AND:", "+", max_length)
        } else if !self.other_focused_clients.is_empty() {
            let full_label = if self.other_focused_clients.len() == 1 {
                "FOCUSED USER:"
            } else {
                "FOCUSED USERS:"
            };
            self.bracketed_focused_users(full_label, "U:", max_length)
        } else {
            None
        }
    }
    fn focused_users_part(&self, label: &str) -> (Vec<TerminalCharacter>, usize) {
        let mut content = foreground_color(label, self.color);
        let mut content_length = label.width();
        for slot in &self.other_focused_clients {
            content.push(EMPTY_TERMINAL_CHARACTER);
            content.append(&mut self.client_cursor(*slot));
            content_length += 2;
        }
        self.bracketed_title_part_from_characters(content, content_length)
    }
    fn focused_users_cursors_part(&self) -> (Vec<TerminalCharacter>, usize) {
        let mut content = vec![];
        let mut content_length = 0;
        for slot in &self.other_focused_clients {
            if content_length > 0 {
                content.push(EMPTY_TERMINAL_CHARACTER);
                content_length += 1;
            }
            content.append(&mut self.client_cursor(*slot));
            content_length += 1;
        }
        self.bracketed_title_part_from_characters(content, content_length)
    }
    fn bracketed_focused_users(
        &self,
        full_label: &str,
        short_label: &str,
        max_length: usize,
    ) -> Option<(Vec<TerminalCharacter>, usize)> {
        let (full_part, full_length) = self.focused_users_part(full_label);
        if full_length <= max_length {
            return Some((full_part, full_length));
        }
        let (short_part, short_length) = self.focused_users_part(short_label);
        if short_length <= max_length {
            return Some((short_part, short_length));
        }
        let (cursors_part, cursors_length) = self.focused_users_cursors_part();
        (cursors_length <= max_length).then_some((cursors_part, cursors_length))
    }
    fn compose_bracketed_title(
        &self,
        left: Option<(Vec<TerminalCharacter>, usize)>,
        middle: Option<(Vec<TerminalCharacter>, usize)>,
        right: Option<(Vec<TerminalCharacter>, usize)>,
    ) -> Vec<TerminalCharacter> {
        let width = self.geom.cols;
        let left_length = left.as_ref().map(|(_, length)| *length).unwrap_or(0);
        let middle_length = middle.as_ref().map(|(_, length)| *length).unwrap_or(0);
        let right_length = right.as_ref().map(|(_, length)| *length).unwrap_or(0);
        let centered_start = (width / 2)
            .saturating_sub(middle_length / 2)
            .max(left_length);
        let middle_start = if middle.is_some() {
            if right_length > 0 {
                let scroll_start = width.saturating_sub(right_length);
                let centered_end = centered_start + middle_length;
                if centered_end > scroll_start {
                    scroll_start.saturating_sub(middle_length).max(left_length)
                } else {
                    centered_start
                }
            } else {
                centered_start
            }
        } else {
            left_length
        };
        let middle_end = if middle.is_some() {
            middle_start + middle_length
        } else {
            left_length
        };
        let right_start = width.saturating_sub(right_length).max(middle_end);
        let fill_character = if self.pane_is_stacked {
            foreground_color(boundary_type::HORIZONTAL, self.color)
                .into_iter()
                .next()
                .unwrap_or(EMPTY_TERMINAL_CHARACTER)
        } else {
            EMPTY_TERMINAL_CHARACTER
        };
        let mut placements: Vec<(usize, Vec<TerminalCharacter>, usize)> = vec![];
        if let Some((characters, length)) = left {
            placements.push((0, characters, length));
        }
        if let Some((characters, length)) = middle {
            placements.push((middle_start, characters, length));
        }
        if let Some((characters, length)) = right {
            placements.push((right_start, characters, length));
        }
        let mut title_line = vec![];
        let mut col = 0;
        while col < width {
            if let Some(index) = placements.iter().position(|(start, _, _)| *start == col) {
                let (_, mut characters, length) = placements.remove(index);
                title_line.append(&mut characters);
                col += length;
            } else {
                title_line.push(fill_character.clone());
                col += 1;
            }
        }
        title_line
    }
    fn render_highlight_tooltip_undertitle(&self) -> Result<Vec<TerminalCharacter>> {
        let max_undertitle_length = self.geom.cols.saturating_sub(2);

        let mut left_boundary =
            foreground_color(self.get_corner(boundary_type::BOTTOM_LEFT), self.color);
        let mut right_boundary =
            foreground_color(self.get_corner(boundary_type::BOTTOM_RIGHT), self.color);

        let tooltip_text = self.highlight_tooltip.as_deref().unwrap_or("");
        let text = format!(" Alt <Click> - {} ", tooltip_text);
        let text_len = text.chars().count();
        if text_len > max_undertitle_length {
            return Ok(self.empty_undertitle(max_undertitle_length));
        }

        let mut text_characters = foreground_color(&text, self.color);

        let padding_len = max_undertitle_length.saturating_sub(text_len);
        let mut padding = String::new();
        for col in 0..padding_len {
            padding.push_str(self.horizontal(self.geom.x + 1 + text_len + col));
        }

        let mut ret = vec![];
        ret.append(&mut left_boundary);
        ret.append(&mut text_characters);
        ret.append(&mut foreground_color(&padding, self.color));
        ret.append(&mut right_boundary);
        Ok(ret)
    }
    fn render_help_text_undertitle(&self) -> Result<Vec<TerminalCharacter>> {
        let max_undertitle_length = self.geom.cols.saturating_sub(2);

        let mut left_boundary =
            foreground_color(self.get_corner(boundary_type::BOTTOM_LEFT), self.color);
        let mut right_boundary =
            foreground_color(self.get_corner(boundary_type::BOTTOM_RIGHT), self.color);

        // Try different versions of the help text from longest to shortest
        let (mut help_text_characters, help_text_len) = if let Some((chars, len)) =
            self.help_text_version_full(max_undertitle_length)
        {
            (chars, len)
        } else if let Some((chars, len)) = self.help_text_version_medium(max_undertitle_length) {
            (chars, len)
        } else if let Some((chars, len)) = self.help_text_version_short(max_undertitle_length) {
            (chars, len)
        } else {
            return Ok(self.empty_undertitle(max_undertitle_length));
        };

        let padding_len = max_undertitle_length.saturating_sub(help_text_len);
        let mut padding = String::new();
        for col in 0..padding_len {
            padding.push_str(self.horizontal(self.geom.x + 1 + help_text_len + col));
        }

        let mut ret = vec![];
        ret.append(&mut left_boundary);
        ret.append(&mut help_text_characters);
        ret.append(&mut foreground_color(&padding, self.color));
        ret.append(&mut right_boundary);
        Ok(ret)
    }

    fn help_text_version_full(&self, max_length: usize) -> Option<(Vec<TerminalCharacter>, usize)> {
        self.help_text_version(max_length, HintTier::Full)
    }

    fn help_text_version_medium(
        &self,
        max_length: usize,
    ) -> Option<(Vec<TerminalCharacter>, usize)> {
        self.help_text_version(max_length, HintTier::Medium)
    }

    fn help_text_version_short(
        &self,
        max_length: usize,
    ) -> Option<(Vec<TerminalCharacter>, usize)> {
        self.help_text_version(max_length, HintTier::Minimal)
    }

    fn help_text_version(
        &self,
        max_length: usize,
        tier: HintTier,
    ) -> Option<(Vec<TerminalCharacter>, usize)> {
        let segments = resize_segments(self.is_floating, self.mouse_scroll_resize, tier);
        let (characters, length) = self.render_hint_segments(&segments);
        (length <= max_length).then_some((characters, length))
    }

    fn render_held_undertitle(&self) -> Result<Vec<TerminalCharacter>> {
        let max_undertitle_length = self.geom.cols.saturating_sub(2); // 2 for the left and right corners
        let (mut first_part, first_part_len) = self.first_exited_held_title_part_full();
        let mut left_boundary =
            foreground_color(self.get_corner(boundary_type::BOTTOM_LEFT), self.color);
        let mut right_boundary =
            foreground_color(self.get_corner(boundary_type::BOTTOM_RIGHT), self.color);
        let res = if self.is_main_client {
            let (mut second_part, second_part_len) = self.second_held_title_part_full();
            let full_text_len = first_part_len + second_part_len;
            if full_text_len <= max_undertitle_length {
                // render exit status and tips
                let mut padding = String::new();
                for col in full_text_len..max_undertitle_length {
                    padding.push_str(self.horizontal(self.geom.x + 1 + col));
                }
                let mut ret = vec![];
                ret.append(&mut left_boundary);
                ret.append(&mut first_part);
                ret.append(&mut second_part);
                ret.append(&mut foreground_color(&padding, self.color));
                ret.append(&mut right_boundary);
                ret
            } else if first_part_len <= max_undertitle_length {
                // render only exit status
                let mut padding = String::new();
                for col in first_part_len..max_undertitle_length {
                    padding.push_str(self.horizontal(self.geom.x + 1 + col));
                }
                let mut ret = vec![];
                ret.append(&mut left_boundary);
                ret.append(&mut first_part);
                ret.append(&mut foreground_color(&padding, self.color));
                ret.append(&mut right_boundary);
                ret
            } else {
                self.empty_undertitle(max_undertitle_length)
            }
        } else {
            if first_part_len <= max_undertitle_length {
                // render first part
                let full_text_len = first_part_len;
                let mut padding = String::new();
                for col in full_text_len..max_undertitle_length {
                    padding.push_str(self.horizontal(self.geom.x + 1 + col));
                }
                let mut ret = vec![];
                ret.append(&mut left_boundary);
                ret.append(&mut first_part);
                ret.append(&mut foreground_color(&padding, self.color));
                ret.append(&mut right_boundary);
                ret
            } else {
                self.empty_undertitle(max_undertitle_length)
            }
        };
        Ok(res)
    }
    /// The bottom frame line of a pane whose root process is stopped.
    ///
    /// Degrades by tier rather than by dropping halves the way the held
    /// undertitle does: the marker on its own says the pane is stuck without
    /// saying what to do about it, so the key is the last thing to go — and only
    /// for a client that could not act on it anyway.
    fn render_suspended_undertitle(&self) -> Result<Vec<TerminalCharacter>> {
        let max_undertitle_length = self.geom.cols.saturating_sub(2); // 2 for the left and right corners
        let mut left_boundary =
            foreground_color(self.get_corner(boundary_type::BOTTOM_LEFT), self.color);
        let mut right_boundary =
            foreground_color(self.get_corner(boundary_type::BOTTOM_RIGHT), self.color);
        let mut rendered = None;
        if self.is_main_client {
            for tier in [HintTier::Full, HintTier::Medium, HintTier::Minimal] {
                let (characters, len) = self.render_hint_segments(&suspended_segments(tier));
                if len <= max_undertitle_length {
                    rendered = Some((characters, len));
                    break;
                }
            }
        }
        if rendered.is_none() {
            let (characters, len) = self.render_hint_segments(&suspended_marker_segments());
            if len <= max_undertitle_length {
                rendered = Some((characters, len));
            }
        }
        let Some((mut characters, len)) = rendered else {
            return Ok(self.empty_undertitle(max_undertitle_length));
        };
        let mut padding = String::new();
        for col in len..max_undertitle_length {
            padding.push_str(self.horizontal(self.geom.x + 1 + col));
        }
        let mut ret = vec![];
        ret.append(&mut left_boundary);
        ret.append(&mut characters);
        ret.append(&mut foreground_color(&padding, self.color));
        ret.append(&mut right_boundary);
        Ok(ret)
    }
    fn render_mouse_shortcuts_undertitle(&self) -> Result<Vec<TerminalCharacter>> {
        let max_undertitle_length = self.geom.cols.saturating_sub(2); // 2 for the left and right corners
        let mut left_boundary =
            foreground_color(self.get_corner(boundary_type::BOTTOM_LEFT), self.color);
        let mut right_boundary =
            foreground_color(self.get_corner(boundary_type::BOTTOM_RIGHT), self.color);
        let res = if self.is_main_client {
            self.empty_undertitle(max_undertitle_length)
        } else {
            let (mut hover_shortcuts, hover_shortcuts_len) = self.hover_shortcuts_part_full();
            if hover_shortcuts_len <= max_undertitle_length {
                // render exit status and tips
                let mut padding = String::new();
                for col in hover_shortcuts_len..max_undertitle_length {
                    padding.push_str(self.horizontal(self.geom.x + 1 + col));
                }
                let mut ret = vec![];
                ret.append(&mut left_boundary);
                ret.append(&mut hover_shortcuts);
                ret.append(&mut foreground_color(&padding, self.color));
                ret.append(&mut right_boundary);
                ret
            } else {
                self.empty_undertitle(max_undertitle_length)
            }
        };
        Ok(res)
    }
    pub fn clicked_on_pinned(&mut self, position: Position) -> bool {
        if !self.is_floating
            || !self.is_selectable
            || !self.show_pinned_indication
            || !self.should_draw_pane_frames
            || self.omit_title
            || self.stack_list_entry.is_some()
            || position.line()
                != -(self.content_offset.top as isize) - isize::from(self.subtitle.is_some())
        {
            return false;
        }
        // Match the space assigned to the right side by the title layout. A title
        // that consumes it all has no pin control to click.
        let total_title_length = self.geom.cols.saturating_sub(2);
        let right_side_space = match self.render_title_middle(total_title_length) {
            Some((_, middle_length)) => {
                let side_space = total_title_length.saturating_sub(middle_length + 2) / 2;
                self.render_title_left_side(side_space).map(|_| side_space)
            },
            None => self
                .render_title_left_side(total_title_length)
                .map(|(_, length)| total_title_length.saturating_sub(length + 1)),
        };
        let Some(right_side_space) = right_side_space else {
            return false;
        };
        if self.render_pinned_indication(right_side_space).is_none() {
            return false;
        }
        // The checkbox is followed by one space and the right border.
        // Mouse positions are relative to the pane's content.
        let start = self.geom.cols - 5 - self.content_offset.left;
        (start..start + 3).contains(&position.column())
    }
    pub fn render(&self) -> Result<(Vec<CharacterChunk>, Option<String>)> {
        let (mut chunks, output) = self
            .render_frame()
            .context("failed to render themed pane frame")?;
        if let Some((foreground, background)) = self.style.chrome_colors() {
            for chunk in &mut chunks {
                chunk.add_pane_defaults(Some(foreground.into()), Some(background.into()));
            }
        }
        Ok((chunks, output))
    }

    fn render_frame(&self) -> Result<(Vec<CharacterChunk>, Option<String>)> {
        let err_context = || "failed to render pane frame";
        let mut character_chunks = vec![];
        if self.omit_title {
            return Ok((character_chunks, None));
        }
        if let Some(entry) = &self.stack_list_entry {
            character_chunks.push(CharacterChunk::new(
                self.render_stack_list_entry(entry),
                self.geom.x,
                self.geom.y,
            ));
            return Ok((character_chunks, None));
        }
        if self.geom.rows == 1 || !self.should_draw_pane_frames {
            // we do this explicitly when not drawing pane frames because this should only happen
            // if this is a stacked pane with pane frames off (and it doesn't necessarily have only
            // 1 row because it could also be a flexible stacked pane)
            // in this case we should always draw the pane title line, and only the title line
            let mut one_line_title = self.render_one_line_title().with_context(err_context)?;

            if self.content_offset.right != 0 && !self.should_draw_pane_frames {
                // here what happens is that the title should be offset to the right
                // in order to give room to the boundaries between the panes to be drawn
                one_line_title.pop();
            }
            let y_coords_of_title = if self.pane_is_stacked_under && !self.should_draw_pane_frames {
                // we only want to use the bottom offset in this case because panes that are
                // stacked above the flexible pane should actually appear exactly where they are on
                // screen, the content offset being "absorbed" by the flexible pane below them
                self.geom.y.saturating_sub(self.content_offset.bottom)
            } else {
                self.geom.y
            };

            character_chunks.push(CharacterChunk::new(
                one_line_title,
                self.geom.x,
                y_coords_of_title,
            ));
        } else {
            for row in 0..self.geom.rows {
                if row == 0 {
                    // top row
                    let title = self.render_title().with_context(err_context)?;
                    let x = self.geom.x;
                    let y = self.geom.y + row;
                    character_chunks.push(CharacterChunk::new(title, x, y));
                } else if row == self.geom.rows - 1 {
                    // bottom row
                    if self.highlight_tooltip.is_some() && self.is_main_client {
                        let x = self.geom.x;
                        let y = self.geom.y + row;
                        character_chunks.push(CharacterChunk::new(
                            self.render_highlight_tooltip_undertitle()
                                .with_context(err_context)?,
                            x,
                            y,
                        ));
                    } else if self.show_help_text && self.is_main_client {
                        let x = self.geom.x;
                        let y = self.geom.y + row;
                        character_chunks.push(CharacterChunk::new(
                            self.render_help_text_undertitle()
                                .with_context(err_context)?,
                            x,
                            y,
                        ));
                    } else if self.mouse_is_hovering_over_pane && !self.is_main_client {
                        let x = self.geom.x;
                        let y = self.geom.y + row;
                        character_chunks.push(CharacterChunk::new(
                            self.render_mouse_shortcuts_undertitle()
                                .with_context(err_context)?,
                            x,
                            y,
                        ));
                    } else if self.is_suspended {
                        let x = self.geom.x;
                        let y = self.geom.y + row;
                        character_chunks.push(CharacterChunk::new(
                            self.render_suspended_undertitle()
                                .with_context(err_context)?,
                            x,
                            y,
                        ));
                    } else if self.exit_status.is_some() || self.is_first_run {
                        let x = self.geom.x;
                        let y = self.geom.y + row;
                        character_chunks.push(CharacterChunk::new(
                            self.render_held_undertitle().with_context(err_context)?,
                            x,
                            y,
                        ));
                    } else {
                        let mut bottom_row = vec![];
                        for col in 0..self.geom.cols {
                            let boundary = if col == 0 {
                                // bottom left corner
                                self.get_corner(boundary_type::BOTTOM_LEFT)
                            } else if col == self.geom.cols - 1 {
                                // bottom right corner
                                self.get_corner(boundary_type::BOTTOM_RIGHT)
                            } else {
                                self.horizontal(self.geom.x + col)
                            };

                            let mut boundary_character = foreground_color(boundary, self.color);
                            bottom_row.append(&mut boundary_character);
                        }
                        let x = self.geom.x;
                        let y = self.geom.y + row;
                        character_chunks.push(CharacterChunk::new(bottom_row, x, y));
                    }
                } else {
                    let boundary_character_left =
                        foreground_color(boundary_type::VERTICAL, self.color);
                    let boundary_character_right =
                        foreground_color(boundary_type::VERTICAL, self.color);

                    let x = self.geom.x;
                    let y = self.geom.y + row;
                    character_chunks.push(CharacterChunk::new(boundary_character_left, x, y));

                    let x = (self.geom.x + self.geom.cols).saturating_sub(1);
                    let y = self.geom.y + row;
                    character_chunks.push(CharacterChunk::new(boundary_character_right, x, y));
                }
            }
        }
        if let Some((subtitle, y)) = &self.subtitle {
            let width = self
                .geom
                .cols
                .saturating_sub(self.content_offset.left + self.content_offset.right);
            let mut characters = dimmed_foreground_color(subtitle, self.color);
            let mut used: usize = characters.iter().map(|c| c.width()).sum();
            if used > width {
                used = 0;
                characters = characters
                    .into_iter()
                    .take_while(|c| {
                        if used + c.width() > width.saturating_sub(1) {
                            return false;
                        }
                        used += c.width();
                        true
                    })
                    .collect();
                if width > 0 {
                    characters.extend(dimmed_foreground_color("…", self.color));
                    used += 1;
                }
            }
            characters.extend(std::iter::repeat_n(
                EMPTY_TERMINAL_CHARACTER,
                width.saturating_sub(used),
            ));
            character_chunks.push(CharacterChunk::new(
                characters,
                self.geom.x + self.content_offset.left,
                *y,
            ));
        }
        Ok((character_chunks, None))
    }
    fn render_hint_segments(&self, segments: &[HintSegment]) -> (Vec<TerminalCharacter>, usize) {
        let mut characters = vec![];
        let mut length = 0;
        for segment in segments {
            let color = match segment.level {
                HintLevel::Plain => self.color,
                HintLevel::Emphasis => Some(self.style.colors.text_unselected.emphasis_0),
                HintLevel::Emphasis2 => Some(self.style.colors.text_unselected.emphasis_2),
                HintLevel::Error => Some(self.style.colors.exit_code_error.base),
                HintLevel::Success => Some(self.style.colors.exit_code_success.base),
            };
            characters.append(&mut foreground_color(&segment.text, color));
            length += segment.text.width();
        }
        (characters, length)
    }
    fn first_exited_held_title_part_full(&self) -> (Vec<TerminalCharacter>, usize) {
        match self.exit_status {
            Some(ExitStatus::Code(exit_code)) => {
                self.render_hint_segments(&exit_code_segments(HintExitStatus::Code(exit_code)))
            },
            Some(ExitStatus::Exited) => {
                self.render_hint_segments(&exit_code_segments(HintExitStatus::Exited))
            },
            None => (foreground_color(boundary_type::HORIZONTAL, self.color), 1),
        }
    }
    fn second_held_title_part_full(&self) -> (Vec<TerminalCharacter>, usize) {
        self.render_hint_segments(&rerun_segments(self.is_first_run, HintTier::Full))
    }
    fn hover_shortcuts_part_full(&self) -> (Vec<TerminalCharacter>, usize) {
        self.render_hint_segments(&hover_segments(HintTier::Full))
    }
    fn empty_undertitle(&self, max_undertitle_length: usize) -> Vec<TerminalCharacter> {
        let mut left_boundary =
            foreground_color(self.get_corner(boundary_type::BOTTOM_LEFT), self.color);
        let mut right_boundary =
            foreground_color(self.get_corner(boundary_type::BOTTOM_RIGHT), self.color);
        let mut ret = vec![];
        let mut padding = String::new();
        for col in 0..max_undertitle_length {
            padding.push_str(self.horizontal(self.geom.x + 1 + col));
        }
        ret.append(&mut left_boundary);
        ret.append(&mut foreground_color(&padding, self.color));
        ret.append(&mut right_boundary);
        ret
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yinz_types::pane_size::{Offset, Viewport};
    use yinz_types::style::Style;

    fn pane_frame_with(mouse_scroll_resize: bool, is_floating: bool, cols: usize) -> PaneFrame {
        PaneFrame::new(
            Viewport {
                x: 0,
                y: 0,
                cols,
                rows: 10,
            },
            (0, 0),
            String::new(),
            FrameParams {
                focused_client: None,
                is_main_client: true,
                other_focused_clients: vec![],
                style: Style::default(),
                color: None,
                other_cursors_exist_in_session: false,
                pane_is_stacked_over: false,
                pane_is_stacked_under: false,
                pane_is_stacked: false,
                should_draw_pane_frames: true,
                pane_is_floating: is_floating,
                show_pinned_indication: true,
                show_scroll_indication: true,
                content_offset: Offset::default(),
                mouse_is_hovering_over_pane: false,
                pane_is_selectable: true,
                show_help_text: true,
                highlight_tooltip: None,
                omit_title: false,
                frame_geom_override: None,
                stack_list_entry: None,
                blank_title: false,
                same_worktree: false,
                mouse_scroll_resize,
                dimmed: false,
                guest_choice_indicator: None,
                opening_ripple: None,
            },
        )
    }

    #[test]
    fn subtitle_moves_the_floating_pin_hit_target_with_the_title() {
        let mut frame = pane_frame_with(false, true, 80);
        frame.title = "pane".into();
        frame.content_offset = Offset::frame(1);
        frame.subtitle = Some(("review · running".into(), 1));
        assert!(frame.clicked_on_pinned(Position::new(-2, 75)));
        assert!(!frame.clicked_on_pinned(Position::new(-1, 75)));
    }

    #[test]
    fn pin_hit_target_matches_the_visible_checkbox() {
        for cols in [10, 20, 40, 80] {
            for show_pin in [false, true] {
                for title in ["", "pane 界", "a title that takes all available space"] {
                    let mut frame = pane_frame_with(false, true, cols);
                    frame.title = title.into();
                    frame.content_offset = Offset::frame(1);
                    frame.scroll_position = (3, 20);
                    frame.show_pinned_indication = show_pin;
                    let rendered = characters_to_string(&frame.render_title().unwrap());
                    let checkbox = rendered.rfind(" PIN [").map(|start| {
                        let column = rendered[..start].width() + 5 - frame.content_offset.left;
                        column..column + 3
                    });
                    for column in 0..cols {
                        assert_eq!(
                            frame.clicked_on_pinned(Position::new(-1, column as u16)),
                            checkbox
                                .as_ref()
                                .is_some_and(|range| range.contains(&column)),
                            "column {column} of {rendered:?}"
                        );
                    }
                }
            }
        }
        let mut frame = pane_frame_with(false, true, 80);
        frame.title = "pane".into();
        frame.content_offset = Offset::frame(1);
        frame.is_selectable = false;
        assert!(!frame.clicked_on_pinned(Position::new(-1, 75)));
        frame.is_selectable = true;
        frame.omit_title = true;
        assert!(!frame.clicked_on_pinned(Position::new(-1, 75)));
    }

    #[test]
    fn subtitle_fits_unicode_columns_without_touching_content_or_borders() {
        for full in [false, true] {
            for floating in [false, true] {
                for width in 1..12 {
                    let mut frame = pane_frame_with(false, floating, width + 2);
                    frame.should_draw_pane_frames = full;
                    frame.content_offset = if full {
                        Offset::frame(1)
                    } else {
                        Offset::shift_right_top_and_bottom(0, 1, 0)
                    };
                    frame.subtitle = Some(("界a\u{301} · 手順/検証 · running".into(), 1));
                    let (chunks, _) = frame.render().unwrap();
                    let subtitle = chunks.last().unwrap();
                    assert_eq!(subtitle.x, frame.content_offset.left);
                    assert_eq!(subtitle.y, 1);
                    assert_eq!(
                        subtitle
                            .terminal_characters
                            .iter()
                            .map(|c| c.width())
                            .sum::<usize>(),
                        frame.geom.cols - frame.content_offset.left - frame.content_offset.right
                    );
                    frame.stack_list_entry = Some(StackListEntry {
                        width,
                        label: "collapsed".into(),
                        is_selected: false,
                        is_emphasized: false,
                        stack_is_focused: false,
                        border_color: None,
                    });
                    assert_eq!(
                        frame.render().unwrap().0.len(),
                        1,
                        "collapsed entries only render their title"
                    );
                }
            }
        }
    }

    fn characters_to_string(chars: &[TerminalCharacter]) -> String {
        chars.iter().map(|c| c.character).collect()
    }

    fn right_side_indicators(show_pin: bool, show_scroll: bool) -> String {
        let mut frame = pane_frame_with(false, true, 80);
        frame.scroll_position = (1, 2);
        frame.show_pinned_indication = show_pin;
        frame.show_scroll_indication = show_scroll;
        frame
            .render_title_right_side(80)
            .map(|(characters, _)| characters_to_string(&characters))
            .unwrap_or_default()
    }

    #[test]
    fn overlay_and_modal_frame_indicators() {
        let floating = right_side_indicators(true, true);
        assert!(floating.contains("PIN"));
        assert!(floating.contains("SCROLL"));

        let overlay = right_side_indicators(false, true);
        assert!(!overlay.contains("PIN"));
        assert!(overlay.contains("SCROLL"));

        assert!(right_side_indicators(false, false).is_empty());

        let mut modal_title = pane_frame_with(false, true, 80);
        modal_title.scroll_position = (1, 2);
        modal_title.show_scroll_indication = false;
        assert!(modal_title.bracketed_scroll_indicator(80).is_none());
    }

    #[test]
    fn a_pane_without_scroll_or_pin_indications_has_no_right_title() {
        let mut frame = pane_frame_with(false, false, 80);
        frame.show_scroll_indication = false;
        frame.show_pinned_indication = false;
        assert!(frame.render_title_right_side(80).is_none());
    }

    #[test]
    fn help_text_full_includes_ctrl_scroll_when_enabled_for_tiled_pane() {
        let frame = pane_frame_with(true, false, 80);
        let (chars, _) = frame.help_text_version_full(80).unwrap();
        let text = characters_to_string(&chars);
        assert!(text.contains("Ctrl <MouseScroll>"));
        assert!(text.contains("<drag borders>"));
    }

    #[test]
    fn help_text_full_omits_ctrl_scroll_when_disabled_for_tiled_pane() {
        let frame = pane_frame_with(false, false, 80);
        let (chars, _) = frame.help_text_version_full(80).unwrap();
        let text = characters_to_string(&chars);
        assert!(!text.contains("MouseScroll"));
        assert!(text.contains("<drag borders> to resize"));
    }

    #[test]
    fn help_text_full_includes_ctrl_scroll_when_enabled_for_floating_pane() {
        let frame = pane_frame_with(true, true, 80);
        let (chars, _) = frame.help_text_version_full(80).unwrap();
        let text = characters_to_string(&chars);
        assert!(text.contains("Ctrl <MouseScroll>"));
        assert!(text.contains("Ctrl <drag borders>"));
    }

    #[test]
    fn help_text_full_omits_ctrl_scroll_when_disabled_for_floating_pane() {
        let frame = pane_frame_with(false, true, 80);
        let (chars, _) = frame.help_text_version_full(80).unwrap();
        let text = characters_to_string(&chars);
        assert!(!text.contains("MouseScroll"));
        assert!(text.contains("Ctrl <drag borders> to resize"));
    }

    #[test]
    fn help_text_medium_omits_ctrl_scroll_when_disabled() {
        let frame = pane_frame_with(false, false, 40);
        let (chars, _) = frame.help_text_version_medium(40).unwrap();
        let text = characters_to_string(&chars);
        assert!(!text.contains("MouseScroll"));
        assert!(text.contains("<drag borders> resize"));
    }

    #[test]
    fn help_text_short_omits_ctrl_scroll_when_disabled() {
        let frame = pane_frame_with(false, false, 20);
        let (chars, _) = frame.help_text_version_short(20).unwrap();
        let text = characters_to_string(&chars);
        assert!(!text.contains("MouseScroll"));
        assert!(text.contains("<drag borders>"));
    }
    #[test]
    fn opening_ripple_only_changes_border_strokes_and_handles_narrow_frames() {
        let start = std::time::Instant::now();
        for cols in [2, 3, 5, 12, 80, 120] {
            let mut frame = pane_frame_with(true, true, cols);
            frame.title = "title─∿".into();
            frame.highlight_tooltip = Some("hint─text".into());
            let normal = frame.render().unwrap().0;
            for ms in [0, 80, 160, 320, 449, 450, 900] {
                frame.opening_ripple = OpeningRipple::sample(
                    start,
                    start + std::time::Duration::from_millis(ms),
                    0,
                    cols,
                );
                let animated = frame.render().unwrap().0;
                assert_eq!(normal.len(), animated.len());
                for (plain, wave) in normal.iter().zip(&animated) {
                    assert_eq!((plain.x, plain.y), (wave.x, wave.y));
                    assert_eq!(
                        plain.terminal_characters.len(),
                        wave.terminal_characters.len()
                    );
                    for (before, after) in plain
                        .terminal_characters
                        .iter()
                        .zip(&wave.terminal_characters)
                    {
                        assert_eq!(before.width(), after.width());
                        if before.character != '─'
                            || (plain.y != 0 && plain.y != 9)
                            || ms == 0
                            || ms >= 450
                        {
                            assert_eq!(before, after);
                        }
                    }
                }
                if cols >= 80 {
                    assert!(
                        characters_to_string(&animated[0].terminal_characters).contains("title─∿")
                    );
                    assert!(animated
                        .iter()
                        .any(|chunk| characters_to_string(&chunk.terminal_characters)
                            .contains("hint─text")));
                }
            }
        }
        for glyph in ['∿', '∽'] {
            assert_eq!(glyph.width(), Some(1));
        }
    }

    #[test]
    fn opening_ripple_halves_sample_the_same_travelling_wave() {
        let start = std::time::Instant::now();
        let sample = |ms| {
            OpeningRipple::sample(start, start + std::time::Duration::from_millis(ms), 3, 70)
                .unwrap()
        };
        let first = sample(80);
        let later = sample(300);
        let active_columns = |ripple: OpeningRipple| {
            (3..73)
                .filter(|x| ripple.stroke(*x) != "─")
                .collect::<Vec<_>>()
        };
        assert!(active_columns(first).last().unwrap() < active_columns(later).first().unwrap());
        let mut left = pane_frame_with(false, true, 35);
        left.geom.x = 3;
        let mut right = pane_frame_with(false, true, 35);
        right.geom.x = 38;
        left.opening_ripple = Some(later);
        right.opening_ripple = Some(later);
        for x in 3..73 {
            assert_eq!(left.horizontal(x), right.horizontal(x));
        }
        assert!(
            OpeningRipple::sample(start, start + std::time::Duration::from_millis(450), 3, 70)
                .is_none()
        );
        assert_eq!(
            OpeningRipple::sample(start, start, 0, 0).unwrap().stroke(0),
            "─"
        );
    }
}

#[cfg(test)]
#[path = "unit/frame_background_tests.rs"]
mod frame_background_tests;

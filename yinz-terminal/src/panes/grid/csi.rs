//! CSI terminal modes, cursor controls, and device reports.

use super::{AlternateScreenState, Grid, MouseMode, MouseTracking, Row};
use crate::panes::kitty_graphics::KittyGrid;
use crate::panes::sixel::SixelGrid;
use crate::panes::terminal_character::Cursor;
use crate::panes::terminal_character::CursorShape;
use crate::panes::terminal_character::EMPTY_TERMINAL_CHARACTER;
use std::collections::VecDeque;
use vte::Params;
use yinz_runtime::consts::VERSION;
use yinz_runtime::shared::version_number;

impl Grid {
    pub(super) fn dispatch_csi(
        &mut self,
        params: &Params,
        intermediates: &[u8],
        _ignore: bool,
        c: char,
    ) {
        let mut params_iter = params.iter();
        let mut next_param_or = |default: u16| {
            params_iter
                .next()
                .map(|param| param[0])
                .filter(|&param| param != 0)
                .unwrap_or(default) as usize
        };
        if c == 'm' {
            if intermediates.is_empty() {
                self.cursor
                    .pending_styles
                    .update(|styles| styles.add_style_from_ansi_params(&mut params_iter))
            }
        } else if c == 'C' || c == 'a' {
            // move cursor forward
            let move_by = next_param_or(1);
            self.move_cursor_forward_until_edge(move_by);
        } else if c == 'K' {
            // clear line (0 => right, 1 => left, 2 => all)
            if let Some(clear_type) = params_iter.next().map(|param| param[0]) {
                let mut char_to_replace = EMPTY_TERMINAL_CHARACTER;
                if let Some(background_color) = self.cursor.pending_styles.background {
                    char_to_replace
                        .styles
                        .update(|styles| styles.background = Some(background_color));
                }
                if clear_type == 0 {
                    self.replace_characters_in_line_after_cursor(char_to_replace);
                } else if clear_type == 1 {
                    self.replace_characters_in_line_before_cursor(char_to_replace);
                } else if clear_type == 2 {
                    self.clear_cursor_line();
                }
            };
        } else if c == 'J' {
            // clear all (0 => below, 1 => above, 2 => all, 3 => saved)
            let mut char_to_replace = EMPTY_TERMINAL_CHARACTER;
            if let Some(background_color) = self.cursor.pending_styles.background {
                char_to_replace
                    .styles
                    .update(|styles| styles.background = Some(background_color));
            }
            if let Some(clear_type) = params_iter.next().map(|param| param[0]) {
                if clear_type == 0 {
                    self.clear_all_after_cursor(char_to_replace);
                } else if clear_type == 1 {
                    self.clear_all_before_cursor(char_to_replace);
                } else if clear_type == 2 {
                    self.set_scroll_region_to_viewport_size();
                    self.fill_viewport(char_to_replace);
                    if let Some(images_to_reap) = self.sixel_grid.clear() {
                        self.sixel_grid.reap_images(images_to_reap);
                    }
                    self.kitty_grid
                        .clear_visible_placements(self.lines_above.len());
                } else if clear_type == 3 {
                    let dropped_rows = self.lines_above.len();
                    self.clear_lines_above();
                    if let Some(images_to_reap) = self.sixel_grid.clear() {
                        self.sixel_grid.reap_images(images_to_reap);
                    }
                    // Only the images that lived in the scrollback go with it - the ones on screen
                    // are untouched, per "the other commands to erase text must have no effect on
                    // graphics".
                    self.kitty_grid.drop_scrollback_rows(dropped_rows);
                    self.kitty_reanchor_all_from_pixels();
                }
            };
        } else if c == 'H' || c == 'f' {
            // goto row/col
            // we subtract 1 from the row/column because these are 1 indexed
            let row = next_param_or(1).saturating_sub(1);
            let col = next_param_or(1).saturating_sub(1);
            self.move_cursor_to(col, row, EMPTY_TERMINAL_CHARACTER);
        } else if c == 'A' {
            // move cursor up until edge of screen
            let move_up_count = next_param_or(1);
            self.move_cursor_up(move_up_count);
        } else if c == 'B' || c == 'e' {
            // move cursor down until edge of screen
            let move_down_count = next_param_or(1);
            let pad_character = EMPTY_TERMINAL_CHARACTER;
            self.move_cursor_down_until_edge_of_screen(move_down_count, pad_character);
        } else if c == 'D' {
            let move_back_count = next_param_or(1);
            self.move_cursor_back(move_back_count);
        } else if c == 'l' {
            let first_intermediate_is_questionmark = match intermediates.first() {
                Some(b'?') => true,
                None => false,
                _ => false,
            };
            if first_intermediate_is_questionmark {
                for param in params_iter.map(|param| param[0]) {
                    match param {
                        2026 => {
                            self.unlock_renders();
                        },
                        2004 => {
                            self.bracketed_paste_mode = false;
                        },
                        1049 => {
                            if let Some(mut alternate_screen_state) =
                                self.alternate_screen_state.take()
                            {
                                self.kitty_grid.teardown();
                                alternate_screen_state.apply_contents_to(
                                    &mut self.lines_above,
                                    &mut self.viewport,
                                    &mut self.cursor,
                                    &mut self.sixel_grid,
                                    &mut self.kitty_grid,
                                    &mut self.supports_kitty_keyboard_protocol,
                                );
                            }
                            self.alternate_screen_state = None;
                            self.clear_viewport_before_rendering = true;
                            self.force_change_size(self.height, self.width); // the alternative_viewport might have been of a different size...
                            self.mark_for_rerender();
                        },
                        25 => {
                            self.hide_cursor();
                            self.mark_for_rerender();
                        },
                        1 => {
                            self.cursor_key_mode = false;
                        },
                        3 => {
                            // DECCOLM - only side effects
                            self.set_scroll_region_to_viewport_size();
                            self.clear_all(EMPTY_TERMINAL_CHARACTER);
                            self.cursor.x = 0;
                            self.cursor.y = 0;
                        },
                        6 => {
                            self.erasure_mode = false;
                        },
                        7 => {
                            self.disable_linewrap = true;
                        },
                        80 => {
                            self.sixel_scrolling = false;
                        },
                        1000 => {
                            self.mouse_tracking = MouseTracking::Off;
                        },
                        1002 => {
                            self.mouse_tracking = MouseTracking::Off;
                        },
                        1003 => {
                            self.mouse_tracking = MouseTracking::Off;
                        },
                        1004 => {
                            self.focus_event_tracking = false;
                        },
                        1005 => {
                            self.mouse_mode = MouseMode::NoEncoding;
                        },
                        1006 => {
                            self.mouse_mode = MouseMode::NoEncoding;
                        },
                        2031 => {
                            self.color_palette_notification_enabled = false;
                        },
                        _ => {},
                    };
                }
            } else {
                for param in params_iter.map(|param| param[0]) {
                    match param {
                        4 => {
                            self.insert_mode = false;
                        },
                        20 => {
                            self.new_line_mode = false;
                        },
                        _ => {},
                    }
                }
            }
        } else if c == 'h' {
            let first_intermediate_is_questionmark = match intermediates.first() {
                Some(b'?') => true,
                None => false,
                _ => false,
            };
            if first_intermediate_is_questionmark {
                for param in params_iter.map(|param| param[0]) {
                    match param {
                        25 => {
                            self.show_cursor();
                            self.mark_for_rerender();
                        },
                        2026 => {
                            self.lock_renders();
                        },
                        2004 => {
                            self.bracketed_paste_mode = true;
                        },
                        1049 => {
                            // enter alternate buffer
                            if self.alternate_screen_state.is_some() {
                                continue;
                            }
                            let current_lines_above = std::mem::take(&mut self.lines_above);
                            let current_viewport = std::mem::replace(
                                &mut self.viewport,
                                VecDeque::from(vec![Row::new().canonical()]),
                            );
                            let current_cursor = std::mem::replace(
                                &mut self.cursor,
                                Cursor::new(0, 0, self.styled_underlines),
                            );
                            let current_supports_kitty_keyboard_protocol = std::mem::replace(
                                &mut self.supports_kitty_keyboard_protocol,
                                false,
                            );
                            let sixel_image_store = self.sixel_grid.sixel_image_store.clone();
                            let alternate_sixelgrid = std::mem::replace(
                                &mut self.sixel_grid,
                                SixelGrid::new(self.character_cell_size.clone(), sixel_image_store),
                            );
                            let kitty_image_store = self.kitty_grid.kitty_image_store.clone();
                            let alternate_kittygrid = std::mem::replace(
                                &mut self.kitty_grid,
                                KittyGrid::new(self.character_cell_size.clone(), kitty_image_store),
                            );
                            self.alternate_screen_state = Some(AlternateScreenState::new(
                                current_lines_above,
                                current_viewport,
                                current_cursor,
                                alternate_sixelgrid,
                                alternate_kittygrid,
                                current_supports_kitty_keyboard_protocol,
                            ));
                            self.clear_viewport_before_rendering = true;
                            self.scrollback_buffer_lines =
                                self.recalculate_scrollback_buffer_count();
                            self.output_buffer.update_all_lines(); // make sure the screen gets cleared in the next render
                        },
                        1 => {
                            self.cursor_key_mode = true;
                        },
                        3 => {
                            // DECCOLM - only side effects
                            self.set_scroll_region_to_viewport_size();
                            self.clear_all(EMPTY_TERMINAL_CHARACTER);
                            self.cursor.x = 0;
                            self.cursor.y = 0;
                        },
                        6 => {
                            self.erasure_mode = true;
                        },
                        7 => {
                            self.disable_linewrap = false;
                        },
                        80 => {
                            self.sixel_scrolling = true;
                        },
                        1000 => {
                            self.mouse_tracking = MouseTracking::Normal;
                        },
                        1002 => {
                            self.mouse_tracking = MouseTracking::ButtonEventTracking;
                        },
                        1003 => {
                            self.mouse_tracking = MouseTracking::AnyEventTracking;
                        },
                        1004 => {
                            self.focus_event_tracking = true;
                        },
                        1005 => {
                            self.mouse_mode = MouseMode::Utf8;
                        },
                        1006 => {
                            self.mouse_mode = MouseMode::Sgr;
                        },
                        2031 => {
                            self.color_palette_notification_enabled = true;
                        },
                        _ => {},
                    }
                }
            } else {
                for param in params_iter.map(|param| param[0]) {
                    match param {
                        4 => {
                            self.insert_mode = true;
                        },
                        20 => {
                            self.new_line_mode = true;
                        },
                        _ => {},
                    }
                }
            }
        } else if c == 'p' {
            let first_intermediate_is_questionmark = match intermediates.first() {
                Some(b'?') => true,
                None => false,
                _ => false,
            };
            if first_intermediate_is_questionmark {
                for param in params_iter.map(|param| param[0]) {
                    match param {
                        2026 => {
                            let response = "\u{1b}[?2026;2$y";
                            self.pending_messages_to_pty
                                .push(response.as_bytes().to_vec());
                        },
                        2031 => {
                            let value = if self.color_palette_notification_enabled {
                                1
                            } else {
                                2
                            };
                            let response = format!("\u{1b}[?2031;{}$y", value);
                            self.pending_messages_to_pty.push(response.into_bytes());
                        },
                        _ => {},
                    }
                }
            }
        } else if c == 'r' {
            if params.len() > 1 {
                let top = next_param_or(1).saturating_sub(1);
                let bottom = params_iter
                    .next()
                    .map(|param| param[0] as usize)
                    .filter(|&param| param != 0)
                    .map(|bottom| {
                        std::cmp::min(self.height.saturating_sub(1), bottom.saturating_sub(1))
                    });
                self.set_scroll_region(top, bottom);
            } else {
                self.set_scroll_region_to_viewport_size();
            }
        } else if c == 'M' {
            // delete lines if currently inside scroll region, or otherwise
            // delete lines in the entire viewport
            let line_count_to_delete = next_param_or(1);
            let mut pad_character = EMPTY_TERMINAL_CHARACTER;
            pad_character.styles = self.cursor.pending_styles.clone();
            self.delete_lines_in_scroll_region(line_count_to_delete, pad_character);
        } else if c == 'L' {
            // insert blank lines if inside scroll region, or otherwise insert
            // blank lines in the entire viewport
            let line_count_to_add = next_param_or(1);
            let mut pad_character = EMPTY_TERMINAL_CHARACTER;
            pad_character.styles = self.cursor.pending_styles.clone();
            self.add_empty_lines_in_scroll_region(line_count_to_add, pad_character);
        } else if c == 'G' || c == '`' {
            let column = next_param_or(1).saturating_sub(1);
            let column = std::cmp::min(column, self.width.saturating_sub(1));
            self.move_cursor_to_column(column);
        } else if c == 'g' {
            let clear_type = next_param_or(0);
            if clear_type == 0 {
                self.clear_tabstop(self.cursor.x);
            } else if clear_type == 3 {
                self.clear_all_tabstops();
            }
        } else if c == 'd' {
            // goto line
            let line = next_param_or(1).saturating_sub(1);
            let pad_character = EMPTY_TERMINAL_CHARACTER;
            self.move_cursor_to_line(line, pad_character);
        } else if c == 'P' {
            // erase characters
            let count = next_param_or(1);
            self.erase_characters(count, self.cursor.pending_styles.clone());
        } else if c == 'X' {
            // erase characters and replace with empty characters of current style
            let count = next_param_or(1);
            self.replace_with_empty_chars(count, self.cursor.pending_styles.clone());
        } else if c == 'T' {
            /*
             * 124  54  T   SD
             * Scroll down, new lines inserted at top of screen
             * [4T = Scroll down 4, bring previous lines back into view
             */
            let line_count = next_param_or(1);
            self.rotate_scroll_region_up(line_count);
        } else if c == 'S' {
            let first_intermediate_is_questionmark = match intermediates.first() {
                Some(b'?') => true,
                None => false,
                _ => false,
            };
            if first_intermediate_is_questionmark {
                let query_type = params_iter.next();
                let is_query = matches!(params_iter.next(), Some(&[1]));
                if is_query {
                    // XTSMGRAPHICS
                    match query_type {
                        Some(&[1]) => {
                            // number of color registers
                            let response = if self.sixel_host_support {
                                "\u{1b}[?1;0;65536S"
                            } else {
                                "\u{1b}[?1;3;0S"
                            };
                            self.pending_messages_to_pty
                                .push(response.as_bytes().to_vec());
                        },
                        Some(&[2]) => {
                            // Sixel graphics geometry in pixels
                            if !self.sixel_host_support {
                                let response = "\u{1b}[?2;3;0S";
                                self.pending_messages_to_pty
                                    .push(response.as_bytes().to_vec());
                            } else {
                                if let Some(character_cell_size) =
                                    *self.character_cell_size.borrow()
                                {
                                    let sixel_area_geometry = format!(
                                        "\u{1b}[?2;0;{};{}S",
                                        character_cell_size.width * self.width,
                                        character_cell_size.height * self.height,
                                    );
                                    self.pending_messages_to_pty
                                        .push(sixel_area_geometry.as_bytes().to_vec());
                                }
                            }
                        },
                        _ => {
                            // unsupported (eg. ReGIS graphics geometry)
                        },
                    }
                }
            } else {
                // move scroll up
                let count = next_param_or(1);
                self.rotate_scroll_region_down(count);
            }
        } else if c == 's' {
            self.save_cursor_position();
        } else if c == 'u' && intermediates == b">" {
            // Yinz only supports the first "progressive enhancement" layer of the kitty keyboard
            // protocol
            // 0 disables, everything else enables.
            let count = next_param_or(0);
            if !self.explicitly_disable_kitty_keyboard_protocol {
                self.supports_kitty_keyboard_protocol = count > 0;
            }
        } else if c == 'u' && intermediates == b"<" {
            // Yinz only supports the first "progressive enhancement" layer of the kitty keyboard
            // protocol
            if !self.explicitly_disable_kitty_keyboard_protocol {
                self.supports_kitty_keyboard_protocol = false;
            }
        } else if c == 'u' && intermediates == b"?" {
            // Yinz only supports the first "progressive enhancement" layer of the kitty keyboard
            // protocol
            let reply = if self.supports_kitty_keyboard_protocol {
                "\u{1b}[?1u"
            } else {
                "\u{1b}[?0u"
            };
            self.pending_messages_to_pty.push(reply.as_bytes().to_vec());
        } else if c == 'u' && intermediates == b"=" {
            // kitty keyboard protocol without the stack, just setting.
            // 0 disables, everything else enables.
            let count = next_param_or(0);
            if !self.explicitly_disable_kitty_keyboard_protocol {
                self.supports_kitty_keyboard_protocol = count > 0;
            }
        } else if c == 'u' {
            self.restore_cursor_position();
        } else if c == '@' {
            let count = next_param_or(1);
            for _ in 0..count {
                let mut pad_character = EMPTY_TERMINAL_CHARACTER;
                pad_character.styles = self.cursor.pending_styles.clone();
                self.add_character_at_cursor_position(pad_character, true);
            }
        } else if c == 'b' {
            if let Some(c) = self.preceding_char.clone() {
                for _ in 0..next_param_or(1) {
                    self.add_character(c.clone());
                }
            }
        } else if c == 'E' {
            // Moves cursor to beginning of the line n (default 1) lines down.
            let count = next_param_or(1);
            let pad_character = EMPTY_TERMINAL_CHARACTER;
            self.move_cursor_down_until_edge_of_screen(count, pad_character);
            self.move_cursor_to_beginning_of_line();
        } else if c == 'F' {
            // Moves cursor to beginning of the line n (default 1) lines up.
            let count = next_param_or(1);
            self.move_cursor_up(count);
            self.move_cursor_to_beginning_of_line();
        } else if c == 'I' {
            for _ in 0..next_param_or(1) {
                self.advance_to_next_tabstop(self.cursor.pending_styles.clone());
            }
        } else if c == 'q' {
            let first_intermediate_is_space = matches!(intermediates.first(), Some(b' '));
            if first_intermediate_is_space {
                // DECSCUSR (CSI Ps SP q) -- Set Cursor Style.
                let cursor_style_id = next_param_or(0);
                let shape = match cursor_style_id {
                    0 => Some(CursorShape::Initial),
                    2 => Some(CursorShape::Block),
                    1 => Some(CursorShape::BlinkingBlock),
                    3 => Some(CursorShape::BlinkingUnderline),
                    4 => Some(CursorShape::Underline),
                    5 => Some(CursorShape::BlinkingBeam),
                    6 => Some(CursorShape::Beam),
                    _ => None,
                };
                if let Some(cursor_shape) = shape {
                    self.cursor.change_shape(cursor_shape);
                }
            } else if matches!(intermediates.first(), Some(b'>')) {
                let version = version_number(VERSION);
                let xtversion = format!("\u{1b}P>|Yinz({})\u{1b}\\", version);
                self.pending_messages_to_pty
                    .push(xtversion.as_bytes().to_vec());
            }
        } else if c == 'Z' {
            for _ in 0..next_param_or(1) {
                self.move_to_previous_tabstop();
            }
        } else if c == 'c' {
            // identify terminal
            // https://vt100.net/docs/vt510-rm/DA1.html
            match intermediates.first() {
                None | Some(0) => {
                    // primary device attributes - VT220 with OSC 52 clipboard, advertising
                    // sixel (attribute 4) only if the attached terminal supports it
                    let terminal_capabilities = if self.sixel_host_support {
                        "\u{1b}[?62;4;52c"
                    } else {
                        "\u{1b}[?62;52c"
                    };
                    self.pending_messages_to_pty
                        .push(terminal_capabilities.as_bytes().to_vec());
                },
                Some(b'>') => {
                    // secondary device attributes
                    let version = version_number(VERSION);
                    let text = format!("\u{1b}[>0;{};1c", version);
                    self.pending_messages_to_pty.push(text.as_bytes().to_vec());
                },
                _ => {},
            }
        } else if c == 'n' {
            // DSR - device status report
            // https://vt100.net/docs/vt510-rm/DSR.html
            let first_intermediate_is_questionmark = match intermediates.first() {
                Some(b'?') => true,
                None => false,
                _ => false,
            };
            if first_intermediate_is_questionmark {
                // CSI ? 996 n — query host terminal color-palette mode
                // (Contour spec; see contour-terminal.org). Yinz
                // short-circuits this query: we know the host's mode
                // from our own startup `\e[?996n` plus unsolicited DSR
                // 997 updates while `\e[?2031h` is enabled, so the
                // pane gets answered locally without a host round-trip.
                // Pushing onto `pending_forwarded_queries` enrols the
                // query in the existing Grid → Tab → Screen pipeline;
                // Screen recognises the variant and writes the reply
                // straight to the pane's pty.
                for param in params_iter.map(|param| param[0]) {
                    if param == 996 {
                        self.pending_forwarded_queries
                            .push(crate::host_query::HostQuery::ColorPaletteMode);
                    }
                }
            } else {
                match next_param_or(0) {
                    5 => {
                        // report terminal status
                        let all_good = "\u{1b}[0n";
                        self.pending_messages_to_pty
                            .push(all_good.as_bytes().to_vec());
                    },
                    6 => {
                        // CPR - cursor position report

                        let offset = if self.erasure_mode {
                            self.scroll_region.0
                        } else {
                            0
                        };
                        let position_report = format!(
                            "\u{1b}[{};{}R",
                            self.cursor.y.saturating_sub(offset) + 1,
                            self.cursor.x + 1
                        );
                        self.pending_messages_to_pty
                            .push(position_report.as_bytes().to_vec());
                    },
                    _ => {},
                }
            }
        } else if c == 'x' {
            // DECREQTPARM - Request Terminal Parameters
            // https://vt100.net/docs/vt100-ug/chapter3.html#DECREQTPARM
            //
            // Respond with (same as xterm): Parity NONE, 8 bits,
            // xmitspeed 38400, recvspeed 38400.  (CLoCk MULtiplier =
            // 1, STP option flags = 0)
            //
            // (xterm used to respond to DECREQTPARM in all modes.
            // Now it seems to only do so when explicitly in VT100 mode.)
            let query = next_param_or(0);
            match query {
                0 | 1 => {
                    let response = format!("\u{1b}[{};1;1;128;128;1;0x", query + 2);
                    self.pending_messages_to_pty
                        .push(response.as_bytes().to_vec());
                },
                _ => {},
            }
        } else if c == 't' {
            match next_param_or(1) {
                14 => {
                    // Forward to host: apps asking for text-area pixels
                    // want the real window size, not Yinz's synthesised
                    // (cell_size * grid_size) value. The host's reply will
                    // be written back to this pane by the forwarding
                    // infrastructure on Screen.
                    self.pending_forwarded_queries
                        .push(crate::host_query::HostQuery::TextAreaPixelSize);
                },
                16 => {
                    // Forward to host: character-cell pixel size.
                    self.pending_forwarded_queries
                        .push(crate::host_query::HostQuery::CharacterCellPixelSize);
                },
                18 => {
                    // report text area
                    let text_area_report = format!("\x1b[8;{};{}t", self.height, self.width);
                    self.pending_messages_to_pty
                        .push(text_area_report.as_bytes().to_vec());
                },
                22 => {
                    self.push_current_title_to_stack();
                },
                23 => {
                    self.pop_title_from_stack();
                },
                _ => {},
            }
        } else {
            if self.debug {
                log::warn!("Unhandled csi: {}->{:?}", c, params);
            }
        }
    }
}

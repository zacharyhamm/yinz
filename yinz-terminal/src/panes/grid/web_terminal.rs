use super::{
    calculate_row_display_height, rgb_to_hex_string, CharsetIndex, Cursor, Grid, MouseMode,
    MouseTracking, Row, StandardCharset,
};
use std::{collections::VecDeque, fmt::Write};

fn restore_charsets(vt: &mut String, cursor: &Cursor, active: CharsetIndex) {
    use CharsetIndex::*;
    for (index, prefix) in [(G0, '('), (G1, ')'), (G2, '*'), (G3, '+')] {
        vt.push('\x1b');
        vt.push(prefix);
        vt.push(match cursor.charsets[index] {
            StandardCharset::Ascii => 'B',
            StandardCharset::UK => 'A',
            StandardCharset::SpecialCharacterAndLineDrawing => '0',
        });
    }
    vt.push_str(match active {
        G0 => "\x0f",
        G1 => "\x0e",
        G2 => "\x1bn",
        G3 => "\x1bo",
    });
}

impl Grid {
    /// Recreate the two VT buffers and the modes needed to continue parsing live output.
    pub fn web_terminal_snapshot(&self) -> Option<String> {
        let mut vt = format!(
            "\x1b]10;{}\x1b\\\x1b]11;{}\x1b\\",
            rgb_to_hex_string(self.application_foreground()),
            rgb_to_hex_string(self.application_background())
        );
        // The first sixteen palette slots are configurable even without an OSC override.
        vt.push_str("\x1b]104\x1b\\\x1b]4");
        for index in 0..=u8::MAX {
            if index < 16
                || self
                    .changed_colors
                    .as_ref()
                    .is_some_and(|colors| colors[index as usize].is_some())
            {
                write!(
                    vt,
                    ";{index};{}",
                    rgb_to_hex_string(self.application_register(index))
                )
                .ok()?;
            }
        }
        vt.push_str("\x1b\\");
        let buffer = |vt: &mut String,
                      history: &VecDeque<Row>,
                      viewport: &VecDeque<Row>,
                      below: Option<&VecDeque<Row>>|
         -> Option<()> {
            // A local client may be reading scrollback. Include rows below its window.
            let mut remaining = self.height + 200;
            let mut rows: Vec<_> = history
                .iter()
                .chain(viewport)
                .chain(below.into_iter().flatten())
                .rev()
                .take_while(|row| {
                    // History stores wrapped lines joined together. Limit physical rows.
                    let height = calculate_row_display_height(row.width(), self.width.max(1));
                    if height > remaining {
                        return false;
                    }
                    remaining -= height;
                    true
                })
                .cloned()
                .collect();
            rows.reverse();
            let text = self.output_buffer.serialize(&rows, false, None).ok()?;
            vt.push_str(text.strip_prefix("\n\r").unwrap_or(&text));
            Some(())
        };
        let restore_tabstops = |vt: &mut String| -> Option<()> {
            vt.push_str("\x1b[3g");
            for stop in self.horizontal_tabstops.range(..self.width) {
                write!(vt, "\x1b[{}G\x1bH", stop + 1).ok()?;
            }
            Some(())
        };
        if let Some(normal) = &self.alternate_screen_state {
            buffer(&mut vt, &normal.lines_above, &normal.viewport, None)?;
            restore_tabstops(&mut vt)?;
            restore_charsets(&mut vt, &normal.cursor, self.active_charset);
            write!(
                vt,
                "\x1b[{};{}H{}\x1b[?1049h\x1b(B\x0f\x1b[H\x1b[0m",
                normal.cursor.y + 1,
                normal.cursor.x + 1,
                normal.cursor.pending_styles
            )
            .ok()?;
        }
        buffer(
            &mut vt,
            &self.lines_above,
            &self.viewport,
            Some(&self.lines_below),
        )?;
        restore_tabstops(&mut vt)?;
        let (top, bottom) = self.scroll_region;
        write!(vt, "\x1b[{};{}r", top + 1, bottom + 1).ok()?;
        let origin = if self.erasure_mode {
            vt.push_str("\x1b[?6h");
            top
        } else {
            0
        };
        let position = |vt: &mut String, cursor: &Cursor| -> Option<()> {
            let y = cursor.y.saturating_sub(origin) + 1;
            if self.width > 0 && cursor.x == self.width {
                // CUP clears xterm's pending wrap. Reprint the complete margin glyph
                // at its leading cell to leave the next printable byte ready to wrap.
                let margin = self.width - 1;
                let row = self.viewport.get(cursor.y);
                if let Some(row) = row.filter(|row| row.width() > margin) {
                    let (index, offset) = row.absolute_character_index_and_position_in_char(margin);
                    let glyph = row.columns.get(index)?;
                    write!(
                        vt,
                        "\x1b[{y};{}H\x1b[0m{}{}",
                        self.width - offset,
                        glyph.styles,
                        glyph.character
                    )
                    .ok()?;
                } else {
                    write!(vt, "\x1b[{y};{}H\x1b[0m ", self.width).ok()?;
                }
            } else {
                write!(vt, "\x1b[{y};{}H", cursor.x + 1).ok()?;
            }
            write!(
                vt,
                "\x1b[0m{}{}",
                cursor.pending_styles,
                cursor.get_shape().get_csi_str()
            )
            .ok()
        };
        if let Some(saved) = &self.saved_cursor_position {
            position(&mut vt, saved)?;
            restore_charsets(&mut vt, saved, self.active_charset);
            // Select ASCII again before replaying a pending-wrap glyph.
            vt.push_str("\x1b7\x1b(B\x0f");
        }
        position(&mut vt, &self.cursor)?;
        restore_charsets(&mut vt, &self.cursor, self.active_charset);
        for (enabled, sequence) in [
            (self.cursor_key_mode, "\x1b[?1h"),
            (self.bracketed_paste_mode, "\x1b[?2004h"),
            (self.cursor_is_hidden, "\x1b[?25l"),
            (self.disable_linewrap, "\x1b[?7l"),
            (self.insert_mode, "\x1b[4h"),
            (self.new_line_mode, "\x1b[20h"),
            (matches!(self.mouse_mode, MouseMode::Sgr), "\x1b[?1006h"),
            (matches!(self.mouse_mode, MouseMode::Utf8), "\x1b[?1005h"),
            (self.mouse_tracking == MouseTracking::Normal, "\x1b[?1000h"),
            (
                self.mouse_tracking == MouseTracking::ButtonEventTracking,
                "\x1b[?1002h",
            ),
            (
                self.mouse_tracking == MouseTracking::AnyEventTracking,
                "\x1b[?1003h",
            ),
        ] {
            if enabled {
                vt.push_str(sequence);
            }
        }
        Some(vt)
    }
}

//! VTE callbacks for characters, controls, DCS, and escape sequences.

use super::{append_dcs_payload, Grid};
use crate::panes::kitty_graphics::PLACEHOLDER;
use crate::panes::terminal_character::CharsetIndex;
use crate::panes::terminal_character::StandardCharset;
use crate::panes::terminal_character::TerminalCharacter;
use crate::panes::terminal_character::EMPTY_TERMINAL_CHARACTER;
use crate::ui::components::UiComponentParser;
use vte::Params;
use vte::Perform;
use yinz_host_protocol::nested_session;
use yinz_runtime::errors::prelude::*;
use yinz_types::ui_components::UI_COMPONENT_DCS_FINAL;

impl Perform for Grid {
    fn print(&mut self, c: char) {
        let c = self.cursor.charsets[self.active_charset].map(c);

        // Printable ASCII is always one cell wide, and is the overwhelming majority of what a
        // program prints - take it without consulting the unicode-width tables.
        let terminal_character = if c.is_ascii_graphic() || c == ' ' {
            TerminalCharacter::new_singlewidth_styled(c, self.cursor.pending_styles.clone())
        } else {
            TerminalCharacter::new_styled(c, self.cursor.pending_styles.clone())
        };
        self.set_preceding_character(terminal_character.clone());
        self.add_character(terminal_character);
        if c == PLACEHOLDER {
            // The cell has been written and the cursor has moved past it; open its mark slot so
            // the diacritics that follow have somewhere to land. Opening it here rather than on
            // the first diacritic is what makes a placeholder with *no* marks - which inherits
            // everything from the cell to its left - distinguishable from an ordinary cell.
            let (x, y) = (self.cursor.x.saturating_sub(1), self.cursor.y);
            if let Some(row) = self.viewport.get_mut(y) {
                row.writing(&self.agent_theme);
                // Not unconditional: with linewrap disabled at the right edge `add_character`
                // drops the character on the floor, and there is no cell to annotate.
                if row.character_at(x).map(|character| character.character) == Some(PLACEHOLDER) {
                    row.begin_placeholder_marks(x);
                }
            }
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            7 => {
                self.ring_bell = true;
            },
            8 => {
                // backspace
                self.move_cursor_back(1);
            },
            9 => {
                // tab
                self.advance_to_next_tabstop(self.cursor.pending_styles.clone());
            },
            10..=12 => {
                // 0a, newline
                // 0b, vertical tabulation
                // 0c, form feed
                self.add_newline();
            },
            13 => {
                // 0d, carriage return
                self.move_cursor_to_beginning_of_line();
            },
            14 => {
                self.set_active_charset(CharsetIndex::G1);
            },
            15 => {
                self.set_active_charset(CharsetIndex::G0);
            },
            _ => {
                if self.debug {
                    log::warn!("Unhandled execute: {:?}", byte);
                }
            },
        }
    }

    fn hook(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, c: char) {
        if c == 'q' && intermediates.first() == Some(&b'+') {
            self.xtgettcap_bytes = Some(vec![]);
        } else if c == 'q' && intermediates.is_empty() {
            // we only process sixel images if we know the pixel size of each character cell,
            // otherwise we can't reliably display them
            if self.current_cursor_pixel_coordinates().is_some() {
                let max_sixel_height_in_pixels = if self.sixel_scrolling {
                    let character_cell_height = self.character_cell_size.borrow().unwrap().height; // unwrap here is safe because `current_cursor_pixel_coordinates` above is only Some if it exists
                    Some(self.height * character_cell_height)
                } else {
                    None
                };
                self.sixel_grid.start_image(
                    max_sixel_height_in_pixels,
                    intermediates.iter().collect(),
                    params.iter().collect(),
                );
            }
        } else if c == UI_COMPONENT_DCS_FINAL {
            // UI-component (Yinz internal)
            self.ui_component_bytes = Some(vec![]);
        } else if c == 'n'
            && intermediates.is_empty()
            && params.len() == 1
            && params.iter().next() == Some(&[nested_session::NESTED_DCS_PARAM][..])
        {
            self.nested_frame_bytes = Some(vec![]);
        }
    }

    fn put(&mut self, byte: u8) {
        if self.sixel_grid.is_parsing() {
            self.sixel_grid.handle_byte(byte);
            // we explicitly set this to false here because in the context of Sixel, we only render the
            // image when it's done, i.e. in the unhook method
            self.should_render = false;
        } else if self.ui_component_bytes.is_some() {
            append_dcs_payload(&mut self.ui_component_bytes, byte);
        } else if self.nested_frame_bytes.is_some() {
            append_dcs_payload(&mut self.nested_frame_bytes, byte);
        } else if self.xtgettcap_bytes.is_some() {
            append_dcs_payload(&mut self.xtgettcap_bytes, byte);
        }
    }

    fn unhook(&mut self) {
        if self.sixel_grid.is_parsing() {
            self.create_sixel_image();
        } else if let Some(mut ui_component_bytes) = self.ui_component_bytes.take() {
            let component_bytes = ui_component_bytes.drain(..);
            let style = self.style;
            let arrow_fonts = self.arrow_fonts;
            UiComponentParser::new(self, style, arrow_fonts)
                .parse(component_bytes.collect())
                .non_fatal();
        } else if let Some(nested_frame_bytes) = self.nested_frame_bytes.take() {
            if let Some(message) = nested_session::decode_base64(&nested_frame_bytes)
                .and_then(|payload_bytes| nested_session::decode_payload(&payload_bytes))
            {
                self.pending_nested_session_messages.push(message);
            }
        } else if let Some(xtgettcap_bytes) = self.xtgettcap_bytes.take() {
            self.answer_xtgettcap(&xtgettcap_bytes);
        }
        self.mark_for_rerender();
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        self.dispatch_osc(params, bell_terminated);
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, c: char) {
        self.dispatch_csi(params, intermediates, ignore, c);
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (byte, intermediates.first()) {
            (b'A', charset_index_symbol) => {
                let charset_index: CharsetIndex = match charset_index_symbol {
                    Some(b'(') => CharsetIndex::G0,
                    Some(b')') => CharsetIndex::G1,
                    Some(b'*') => CharsetIndex::G2,
                    Some(b'+') => CharsetIndex::G3,
                    _ => {
                        // invalid, silently do nothing
                        return;
                    },
                };
                self.configure_charset(StandardCharset::UK, charset_index);
            },
            (b'B', charset_index_symbol) => {
                let charset_index: CharsetIndex = match charset_index_symbol {
                    Some(b'(') => CharsetIndex::G0,
                    Some(b')') => CharsetIndex::G1,
                    Some(b'*') => CharsetIndex::G2,
                    Some(b'+') => CharsetIndex::G3,
                    _ => {
                        // invalid, silently do nothing
                        return;
                    },
                };
                self.configure_charset(StandardCharset::Ascii, charset_index);
            },
            (b'0', charset_index_symbol) => {
                let charset_index: CharsetIndex = match charset_index_symbol {
                    Some(b'(') => CharsetIndex::G0,
                    Some(b')') => CharsetIndex::G1,
                    Some(b'*') => CharsetIndex::G2,
                    Some(b'+') => CharsetIndex::G3,
                    _ => {
                        // invalid, silently do nothing
                        return;
                    },
                };
                self.configure_charset(
                    StandardCharset::SpecialCharacterAndLineDrawing,
                    charset_index,
                );
            },
            (b'D', None) => {
                self.add_newline();
            },
            (b'E', None) => {
                self.add_newline();
                self.move_cursor_to_beginning_of_line();
            },
            (b'M', None) => {
                self.move_cursor_up_with_scrolling(1);
            },
            (b'c', None) => {
                self.reset_terminal_state();
            },
            (b'H', None) => {
                self.set_horizontal_tabstop();
            },
            (b'7', None) => {
                self.save_cursor_position();
            },
            (b'Z', None) => {
                let terminal_capabilities = "\u{1b}[?6c";
                self.pending_messages_to_pty
                    .push(terminal_capabilities.as_bytes().to_vec());
            },
            (b'8', None) => {
                self.restore_cursor_position();
            },
            (b'8', Some(b'#')) => {
                let mut fill_character = EMPTY_TERMINAL_CHARACTER;
                fill_character.character = 'E';
                self.fill_viewport(fill_character);
            },
            _ => {
                if self.debug {
                    log::warn!("Unhandled esc_dispatch: {}->{:?}", byte, intermediates);
                }
            },
        }
    }

    fn terminated(&self) -> bool {
        !self.pending_forwarded_queries.is_empty()
    }
}

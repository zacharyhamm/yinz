//! Conservative boundary tracking for vte 0.15. The parser has no public state accessor.
//! A migration is refused while a sequence or UTF-8 character is incomplete.
#[derive(Default)]
pub struct ParserBoundary {
    state: State,
    utf8: Vec<u8>,
}

#[derive(Default)]
enum State {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    Osc,
    DcsEntry,
    DcsParam,
    DcsIntermediate,
    DcsIgnore,
    Dcs,
    String,
}

impl ParserBoundary {
    pub fn is_ground(&self) -> bool {
        matches!(self.state, State::Ground) && self.utf8.is_empty()
    }

    pub fn advance(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.byte(byte);
        }
    }

    fn byte(&mut self, byte: u8) {
        if !self.utf8.is_empty() {
            self.utf8.push(byte);
            match std::str::from_utf8(&self.utf8) {
                Ok(_) => self.utf8.clear(),
                Err(error) if error.error_len().is_none() => {},
                Err(error) => {
                    let remainder = self.utf8
                        [error.valid_up_to() + error.error_len().unwrap_or_default()..]
                        .to_vec();
                    self.utf8.clear();
                    self.advance(&remainder);
                },
            }
            return;
        }
        if byte == 0x1b {
            self.state = State::Escape;
            return;
        }
        if matches!(byte, 0x18 | 0x1a) {
            self.state = State::Ground;
            return;
        }
        self.state = match self.state {
            State::Ground => {
                if (0xc2..=0xf4).contains(&byte) {
                    self.utf8.push(byte);
                }
                State::Ground
            },
            State::Escape => match byte {
                b'[' => State::Csi,
                b']' => State::Osc,
                b'P' => State::DcsEntry,
                b'X' | b'^' | b'_' => State::String,
                0x20..=0x2f => State::EscapeIntermediate,
                0x30..=0x7e => State::Ground,
                _ => State::Escape,
            },
            State::EscapeIntermediate => match byte {
                0x30..=0x7e => State::Ground,
                _ => State::EscapeIntermediate,
            },
            State::Csi => match byte {
                0x40..=0x7e => State::Ground,
                _ => State::Csi,
            },
            State::Osc if byte == 7 => State::Ground,
            State::Osc => State::Osc,
            State::DcsEntry => match byte {
                0x20..=0x2f => State::DcsIntermediate,
                0x30..=0x3f => State::DcsParam,
                0x40..=0x7e => State::Dcs,
                _ => State::DcsEntry,
            },
            State::DcsParam => match byte {
                0x20..=0x2f => State::DcsIntermediate,
                0x3c..=0x3f => State::DcsIgnore,
                0x40..=0x7e => State::Dcs,
                _ => State::DcsParam,
            },
            State::DcsIntermediate => match byte {
                0x30..=0x3f => State::DcsIgnore,
                0x40..=0x7e => State::Dcs,
                _ => State::DcsIntermediate,
            },
            State::DcsIgnore => State::DcsIgnore,
            State::Dcs if byte == 0x9c => State::Ground,
            State::Dcs => State::Dcs,
            State::String => State::String,
        };
    }
}

#[cfg(test)]
#[path = "unit/parser_boundary.rs"]
mod tests;

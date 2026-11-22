//! Prefixes of the XParseColor forms used by terminal foreground, background
//! and palette replies. Track only the fields needed across an input boundary.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ColorPrefix {
    Register { has_digit: bool },
    Start,
    RgbTag(usize),
    Rgb { channel: u8, digits: u8 },
    Hex(u8),
    Unknown,
}

impl ColorPrefix {
    pub(super) fn from_report(bytes: &[u8], header: &[u8]) -> Self {
        let initial = if header == b"\x1b]4;" {
            Self::Register { has_digit: false }
        } else {
            Self::Start
        };
        let body = &bytes[header.len()..];
        let body = body.strip_suffix(b"\x1b").unwrap_or(body);
        body.iter()
            .try_fold(initial, |prefix, &byte| prefix.advance(byte))
            .unwrap_or(Self::Unknown)
    }

    pub(super) fn advance(self, byte: u8) -> Option<Self> {
        match self {
            Self::Register { .. } if byte.is_ascii_digit() => {
                Some(Self::Register { has_digit: true })
            },
            Self::Register { has_digit: true } if byte == b';' => Some(Self::Start),
            Self::Start if byte == b'#' => Some(Self::Hex(0)),
            Self::Start if byte == b'r' => Some(Self::RgbTag(1)),
            Self::RgbTag(i) if b"rgb:"[i] == byte => Some(if i == 3 {
                Self::Rgb {
                    channel: 0,
                    digits: 0,
                }
            } else {
                Self::RgbTag(i + 1)
            }),
            Self::Rgb { channel, digits } if digits < 4 && byte.is_ascii_hexdigit() => {
                Some(Self::Rgb {
                    channel,
                    digits: digits + 1,
                })
            },
            Self::Rgb { channel, digits } if digits > 0 && channel < 2 && byte == b'/' => {
                Some(Self::Rgb {
                    channel: channel + 1,
                    digits: 0,
                })
            },
            Self::Hex(n) if n < 12 && byte.is_ascii_hexdigit() => Some(Self::Hex(n + 1)),
            // The normal OSC decoder also accepts host-specific color strings.
            // Apply component limits only after a known color introducer.
            Self::Start | Self::RgbTag(_) | Self::Unknown if (0x20..=0x7e).contains(&byte) => {
                Some(Self::Unknown)
            },
            _ => None,
        }
    }

    pub(super) fn complete(self) -> bool {
        self.digits_after_complete_value() == Some(0)
    }

    /// Trailing digits beyond the longest legal value, if one has been read.
    pub(super) fn digits_after_complete_value(self) -> Option<usize> {
        match self {
            Self::Rgb {
                channel: 2,
                digits: 1..,
            } => Some(0),
            Self::Hex(n @ 3..) => Some(usize::from(n % 3)),
            _ => None,
        }
    }
}

//! Framed input received while waiting for a retained OSC to finish.
//! An interrupt owns only its own bytes and terminator.
use super::{HostReply, OscEscape, OscKind, PartialOsc, PASTE_END_MARKER, PASTE_START_MARKER};

pub(super) enum Interruption {
    None,
    Key(Vec<u8>),
    String {
        osc: bool,
        payload: Option<Vec<u8>>,
        escape: bool,
    },
    Paste {
        end_matched: usize,
        fresh: bool,
    },
}

impl Interruption {
    pub(super) fn mark_boundary(&mut self) {
        match self {
            Self::Key(_) => *self = Self::None,
            Self::Paste { fresh, .. } => *fresh = false,
            Self::String { payload, .. } => *payload = None,
            Self::None => {},
        }
    }
}

impl PartialOsc {
    pub(super) fn consume_interruption(
        &mut self,
        byte: u8,
        limit: usize,
        replies: &mut Vec<HostReply>,
        keyboard: &mut Vec<u8>,
    ) -> bool {
        match &mut self.interruption {
            Interruption::None => {
                if self.escape != OscEscape::Fresh {
                    return false;
                }
                self.interruption = match byte {
                    b'[' | b'O' => {
                        self.interrupt_color(keyboard);
                        Interruption::Key(vec![0x1b, byte])
                    },
                    b']' | b'P' | b'X' | b'^' | b'_' => Interruption::String {
                        osc: byte == b']',
                        payload: (byte == b']').then(Vec::new),
                        escape: false,
                    },
                    _ => return false,
                };
                self.escape = OscEscape::None;
            },
            Interruption::Key(bytes) => {
                let csi = bytes[1] == b'[';
                if csi && (0x20..=0x3f).contains(&byte) && bytes.len() < 256 {
                    bytes.push(byte);
                } else if (csi && (0x40..=0x7e).contains(&byte))
                    || (!csi && matches!(byte, b'A'..=b'D' | b'F' | b'H' | b'P'..=b'S'))
                {
                    bytes.push(byte);
                    keyboard.extend_from_slice(bytes);
                    self.interruption = if bytes == PASTE_START_MARKER {
                        Interruption::Paste {
                            end_matched: 0,
                            fresh: true,
                        }
                    } else {
                        Interruption::None
                    };
                } else {
                    // The Escape is identifiable. A malformed key's parameters
                    // (and SS3's O) can still be old clipboard data.
                    keyboard.extend_from_slice(if csi { b"\x1b[" } else { b"\x1b" });
                    self.interruption = Interruption::None;
                    return false;
                }
            },
            Interruption::String {
                osc,
                payload,
                escape,
            } => {
                if (*osc && byte == 7) || (*escape && byte == b'\\') {
                    if let Some(data) = payload
                        && let Some(reply) = HostReply::from_osc_payload(data)
                    {
                        replies.push(reply);
                    }
                    self.interruption = Interruption::None;
                } else {
                    if *escape || payload.as_ref().is_some_and(|data| data.len() >= limit) {
                        *payload = None;
                    }
                    *escape = byte == 0x1b;
                    if !*escape && let Some(data) = payload {
                        data.push(byte);
                    }
                }
            },
            Interruption::Paste { end_matched, fresh } => {
                if byte == PASTE_END_MARKER[*end_matched] {
                    *end_matched += 1;
                    if *end_matched == PASTE_END_MARKER.len() {
                        if *fresh {
                            keyboard.extend_from_slice(PASTE_END_MARKER);
                        }
                        self.interruption = Interruption::None;
                    }
                } else {
                    let retired = matches!(self.kind, OscKind::RetiredClipboard(_));
                    if *fresh && !retired {
                        keyboard.extend_from_slice(&PASTE_END_MARKER[..*end_matched]);
                    }
                    *end_matched = usize::from(byte == 0x1b);
                    // Inside a retired reply, unframed base64 can still be its
                    // body. Preserve identifiable paste bytes within the markers.
                    if *fresh
                        && *end_matched == 0
                        && !(retired && (byte.is_ascii_alphanumeric() || b"+/=?;".contains(&byte)))
                    {
                        keyboard.push(byte);
                    }
                }
            },
        }
        true
    }
}

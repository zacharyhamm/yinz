//! Framed input received while waiting for a retained OSC to finish.
//! An interrupt owns only its own bytes and terminator.
use super::{
    csi_status, HostReply, InputBoundary, InputOrigin, OscKind, PartialOsc, SeqStatus,
    StartupKittyProbe, PASTE_END_MARKER, PASTE_START_MARKER,
};

pub(super) enum Interruption {
    None,
    Key {
        bytes: Vec<u8>,
        origin: InputOrigin,
    },
    String {
        introducer: u8,
        payload: Option<Vec<u8>>,
        escape: bool,
    },
    Paste {
        end_matched: usize,
        fresh: bool,
    },
}

impl Interruption {
    pub(super) fn mark_boundary(&mut self, boundary: InputBoundary) {
        match self {
            Self::Key { origin, .. } => origin.mark_boundary(boundary),
            Self::Paste { fresh, .. } => *fresh = false,
            Self::String { payload, .. } if boundary == InputBoundary::Recovery => *payload = None,
            _ => {},
        }
    }
}

fn key_status(bytes: &[u8]) -> SeqStatus {
    // SS3 has a much smaller keyboard grammar than CSI. Arbitrary parameters
    // or finals here could release bytes from the retired clipboard.
    let complete = SeqStatus::Complete(bytes.len());
    match bytes {
        [0x1b, b'O', b'A'..=b'D' | b'F' | b'H' | b'P'..=b'S' | b'a'..=b'd']
        | [0x1b, b'O', b'1', b'A'..=b'D']
        | [0x1b, b'O', b'1', b';', b'1'..=b'8', b'A'..=b'D'] => complete,
        b"\x1bO1" | b"\x1bO1;" | [0x1b, b'O', b'1', b';', b'1'..=b'8'] => SeqStatus::NeedMore,
        [0x1b, b'[', .., 0x40..=0x7e] => complete,
        [0x1b, b'[', .., 0x20..=0x3f] if bytes.len() <= 256 => SeqStatus::NeedMore,
        _ => SeqStatus::Malformed,
    }
}

fn paste_byte(kind: &mut OscKind, byte: u8, fresh: bool, keyboard: &mut Vec<u8>) {
    if let OscKind::RetiredClipboard(fields) = kind
        && fields.consume(byte, true)
    {
        return;
    }
    if fresh {
        keyboard.push(byte);
    }
}

impl PartialOsc {
    fn release_incomplete_key(&mut self, prefix: &[u8], keyboard: &mut Vec<u8>) {
        if prefix[1] == b'[' {
            keyboard.extend_from_slice(b"\x1b[");
            return;
        }
        // The Escape is distinguishable. SS3's O and its incomplete
        // parameters could have come from the old clipboard.
        keyboard.push(0x1b);
        if let OscKind::RetiredClipboard(fields) = &mut self.kind {
            keyboard.extend(
                prefix[1..]
                    .iter()
                    .copied()
                    .filter(|&b| !fields.consume(b, false)),
            );
        } else {
            for &byte in &prefix[2..] {
                self.consume_color_byte(byte, keyboard);
            }
        }
    }

    pub(super) fn consume_interruption(
        &mut self,
        byte: u8,
        limit: usize,
        probe: &mut StartupKittyProbe,
        replies: &mut Vec<HostReply>,
        keyboard: &mut Vec<u8>,
    ) -> bool {
        match &mut self.interruption {
            Interruption::None => {
                let Some(origin) = self.escape else {
                    return false;
                };
                self.interruption = match byte {
                    b'[' | b'O' => {
                        if origin == InputOrigin::Fresh {
                            self.interrupt_color(keyboard);
                        }
                        Interruption::Key {
                            bytes: vec![0x1b, byte],
                            origin,
                        }
                    },
                    b']' | b'P' | b'X' | b'^' | b'_' => Interruption::String {
                        introducer: byte,
                        payload: (origin != InputOrigin::Stale && matches!(byte, b']' | b'_'))
                            .then(Vec::new),
                        escape: false,
                    },
                    _ => return false,
                };
                self.escape = None;
            },
            Interruption::Key { bytes, origin } => {
                bytes.push(byte);
                match key_status(bytes) {
                    SeqStatus::NeedMore => {},
                    SeqStatus::Complete(_) => {
                        let fresh = *origin == InputOrigin::Fresh;
                        if fresh
                            || (*origin == InputOrigin::HostOnly
                                && matches!(csi_status(bytes), SeqStatus::Complete(_)))
                        {
                            keyboard.extend_from_slice(bytes);
                        }
                        self.interruption = if bytes == PASTE_START_MARKER {
                            Interruption::Paste {
                                end_matched: 0,
                                fresh,
                            }
                        } else {
                            Interruption::None
                        };
                    },
                    SeqStatus::Malformed => {
                        let fresh = *origin == InputOrigin::Fresh;
                        let mut prefix = std::mem::take(bytes);
                        prefix.pop(); // the outer scanner handles the mismatching byte
                        self.interruption = Interruption::None;
                        if fresh {
                            self.release_incomplete_key(&prefix, keyboard);
                        }
                        return false;
                    },
                }
            },
            Interruption::String {
                introducer,
                payload,
                escape,
            } => {
                if (*introducer == b']' && byte == 7) || (*escape && byte == b'\\') {
                    if let Some(data) = payload {
                        let reply = match *introducer {
                            b']' => HostReply::from_osc_payload(data),
                            b'_' => probe.observe_reply(data),
                            _ => None,
                        };
                        replies.extend(reply);
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
                    for &prefix in &PASTE_END_MARKER[..*end_matched] {
                        paste_byte(&mut self.kind, prefix, *fresh, keyboard);
                    }
                    *end_matched = usize::from(byte == 0x1b);
                    if *end_matched == 0 {
                        paste_byte(&mut self.kind, byte, *fresh, keyboard);
                    }
                }
            },
        }
        true
    }
}

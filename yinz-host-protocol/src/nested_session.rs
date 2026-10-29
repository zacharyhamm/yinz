use crate::nested_session_contract::nested_session_contract as proto;
use base64::alphabet::STANDARD as BASE64_STANDARD_ALPHABET;
use base64::engine::general_purpose::{
    GeneralPurpose, GeneralPurposeConfig, STANDARD as BASE64_STANDARD,
};
use base64::engine::{DecodePaddingMode, Engine as _};
use prost::Message;
use std::str::FromStr;
use std::time::{Duration, Instant};
use yinz_types::geometry::Direction;
use yinz_types::internal_ids::SessionName;
use yinz_types::keys::KeyWithModifier;

const BASE64_DECODER: GeneralPurpose = GeneralPurpose::new(
    &BASE64_STANDARD_ALPHABET,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

pub const NESTED_DCS_PARAM: u16 = 26661;
pub const NESTED_FRAME_HEADER: &[u8] = b"\x1bP26661n";
pub const NESTED_FRAME_TERMINATOR: &[u8] = b"\x1b\\";

pub const REANNOUNCE_SILENCE_MS: u64 = 3000;
pub const REANNOUNCE_CHECK_INTERVAL_MS: u64 = 1000;
/// How many `Announce` frames a client sends (the one at startup included)
/// before it concludes there is no Yinz host on the other side of its
/// terminal and stops asking.
pub const MAX_UNACKED_ANNOUNCES: usize = 5;

pub fn reannounce_silence_ms() -> u64 {
    std::env::var("YINZ_NESTED_REANNOUNCE_SILENCE_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(REANNOUNCE_SILENCE_MS)
}

pub fn reannounce_check_interval_ms() -> u64 {
    std::env::var("YINZ_NESTED_REANNOUNCE_CHECK_INTERVAL_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(REANNOUNCE_CHECK_INTERVAL_MS)
}

pub fn max_unacked_announces() -> usize {
    std::env::var("YINZ_NESTED_MAX_UNACKED_ANNOUNCES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(MAX_UNACKED_ANNOUNCES)
}

/// Decides when a client should re-send its `Announce` frame.
///
/// Every client announces itself at startup, whether or not it is running
/// inside a Yinz pane: nested-ness is not read from the environment but
/// inferred from the host answering. Until the first host frame arrives the
/// scheduler spends a budget of `budget` announces (the startup one counts)
/// and then goes quiet, so a client in a plain terminal does not keep writing
/// control frames nobody reads. First host contact lifts the budget for good;
/// from then on a re-announce goes out whenever the host has been silent for
/// `silence`.
pub struct ReannounceScheduler {
    last_heard_from_host: Instant,
    host_contacted: bool,
    announces_without_host_contact: usize,
    silence: Duration,
    budget: usize,
}

impl ReannounceScheduler {
    pub fn new(now: Instant) -> Self {
        ReannounceScheduler::with_settings(
            now,
            Duration::from_millis(reannounce_silence_ms()),
            max_unacked_announces(),
        )
    }

    pub fn with_settings(now: Instant, silence: Duration, budget: usize) -> Self {
        ReannounceScheduler {
            last_heard_from_host: now,
            host_contacted: false,
            // the client announces once at startup, before the scheduler
            // sees its first tick
            announces_without_host_contact: 1,
            silence,
            budget,
        }
    }

    /// Record a frame from the host. Returns whether this was the first one.
    pub fn note_host_contact(&mut self, now: Instant) -> bool {
        self.last_heard_from_host = now;
        let first_contact = !self.host_contacted;
        self.host_contacted = true;
        first_contact
    }

    /// Called once per check interval. Returns whether to announce now.
    pub fn on_tick(&mut self, now: Instant) -> bool {
        if self.budget_exhausted() {
            return false;
        }
        if now.duration_since(self.last_heard_from_host) < self.silence {
            return false;
        }
        if !self.host_contacted {
            self.announces_without_host_contact += 1;
        }
        true
    }

    pub fn host_contacted(&self) -> bool {
        self.host_contacted
    }

    /// True once an unanswered client has used up its announces; only host
    /// contact clears it.
    pub fn budget_exhausted(&self) -> bool {
        !self.host_contacted && self.announces_without_host_contact >= self.budget
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NestedSessionCapability {
    NestedControl,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NestedSessionMessage {
    Announce {
        session_name: SessionName,
        capabilities: Vec<NestedSessionCapability>,
    },
    FocusHost {
        direction: Option<Direction>,
    },
    ToggleHostFullscreen {
        fullscreen: bool,
    },
    Pong,
    Bye,
    AnnounceAck {
        ancestry: Vec<SessionName>,
        capabilities: Vec<NestedSessionCapability>,
        descend_keys: Vec<KeyWithModifier>,
    },
    FocusGained {
        from_direction: Option<Direction>,
    },
    FocusLost,
    FullscreenState {
        fullscreen: bool,
    },
    AncestryUpdate {
        ancestry: Vec<SessionName>,
    },
    Ping,
    ShortcutUpdate {
        ascend_keys: Vec<KeyWithModifier>,
        descend_keys: Vec<KeyWithModifier>,
    },
}

fn keys_to_proto(keys: &[KeyWithModifier]) -> Vec<String> {
    keys.iter().map(|key| key.to_keybind_string()).collect()
}

fn keys_from_proto(keys: &[String]) -> Vec<KeyWithModifier> {
    let parsed: Vec<KeyWithModifier> = keys
        .iter()
        .filter_map(|key| KeyWithModifier::from_str(key).ok())
        .collect();
    if parsed.len() == keys.len() {
        parsed
    } else {
        vec![]
    }
}

fn capabilities_to_proto(capabilities: &[NestedSessionCapability]) -> Vec<i32> {
    capabilities
        .iter()
        .map(|capability| match capability {
            NestedSessionCapability::NestedControl => proto::NestedCapability::NestedControl as i32,
        })
        .collect()
}

fn capabilities_from_proto(capabilities: &[i32]) -> Vec<NestedSessionCapability> {
    capabilities
        .iter()
        .filter_map(
            |capability| match proto::NestedCapability::try_from(*capability) {
                Ok(proto::NestedCapability::NestedControl) => {
                    Some(NestedSessionCapability::NestedControl)
                },
                _ => None,
            },
        )
        .collect()
}

fn direction_to_proto(direction: Option<Direction>) -> i32 {
    match direction {
        Some(Direction::Left) => proto::NestedDirection::Left as i32,
        Some(Direction::Right) => proto::NestedDirection::Right as i32,
        Some(Direction::Up) => proto::NestedDirection::Up as i32,
        Some(Direction::Down) => proto::NestedDirection::Down as i32,
        None => proto::NestedDirection::Unspecified as i32,
    }
}

fn direction_from_proto(direction: i32) -> Option<Direction> {
    match proto::NestedDirection::try_from(direction) {
        Ok(proto::NestedDirection::Left) => Some(Direction::Left),
        Ok(proto::NestedDirection::Right) => Some(Direction::Right),
        Ok(proto::NestedDirection::Up) => Some(Direction::Up),
        Ok(proto::NestedDirection::Down) => Some(Direction::Down),
        _ => None,
    }
}

impl From<NestedSessionMessage> for proto::NestedSessionMessage {
    fn from(message: NestedSessionMessage) -> Self {
        use proto::nested_session_message::Payload;
        let payload = match message {
            NestedSessionMessage::Announce {
                session_name,
                capabilities,
            } => Payload::Announce(proto::Announce {
                session_name: session_name.to_string(),
                capabilities: capabilities_to_proto(&capabilities),
            }),
            NestedSessionMessage::FocusHost { direction } => Payload::FocusHost(proto::FocusHost {
                direction: direction_to_proto(direction),
            }),
            NestedSessionMessage::ToggleHostFullscreen { fullscreen } => {
                Payload::HostFullscreen(proto::ToggleHostFullscreen { fullscreen })
            },
            NestedSessionMessage::Pong => Payload::Pong(proto::Pong {}),
            NestedSessionMessage::Bye => Payload::Bye(proto::Bye {}),
            NestedSessionMessage::AnnounceAck {
                ancestry,
                capabilities,
                descend_keys,
            } => Payload::AnnounceAck(proto::AnnounceAck {
                ancestry: ancestry
                    .into_iter()
                    .map(|name| name.into_string())
                    .collect(),
                capabilities: capabilities_to_proto(&capabilities),
                descend_keys: keys_to_proto(&descend_keys),
            }),
            NestedSessionMessage::FocusGained { from_direction } => {
                Payload::FocusGained(proto::FocusGained {
                    from_direction: direction_to_proto(from_direction),
                })
            },
            NestedSessionMessage::FocusLost => Payload::FocusLost(proto::FocusLost {}),
            NestedSessionMessage::FullscreenState { fullscreen } => {
                Payload::FullscreenState(proto::FullscreenState { fullscreen })
            },
            NestedSessionMessage::AncestryUpdate { ancestry } => {
                Payload::AncestryUpdate(proto::AncestryUpdate {
                    ancestry: ancestry
                        .into_iter()
                        .map(|name| name.into_string())
                        .collect(),
                })
            },
            NestedSessionMessage::Ping => Payload::Ping(proto::Ping {}),
            NestedSessionMessage::ShortcutUpdate {
                ascend_keys,
                descend_keys,
            } => Payload::ShortcutUpdate(proto::ShortcutUpdate {
                ascend_keys: keys_to_proto(&ascend_keys),
                descend_keys: keys_to_proto(&descend_keys),
            }),
        };
        proto::NestedSessionMessage {
            payload: Some(payload),
        }
    }
}

impl TryFrom<proto::NestedSessionMessage> for NestedSessionMessage {
    type Error = ();
    fn try_from(message: proto::NestedSessionMessage) -> Result<Self, Self::Error> {
        use proto::nested_session_message::Payload;
        match message.payload {
            Some(Payload::Announce(announce)) => Ok(NestedSessionMessage::Announce {
                session_name: announce.session_name.into(),
                capabilities: capabilities_from_proto(&announce.capabilities),
            }),
            Some(Payload::FocusHost(focus_host)) => Ok(NestedSessionMessage::FocusHost {
                direction: direction_from_proto(focus_host.direction),
            }),
            Some(Payload::HostFullscreen(host_fullscreen)) => {
                Ok(NestedSessionMessage::ToggleHostFullscreen {
                    fullscreen: host_fullscreen.fullscreen,
                })
            },
            Some(Payload::Pong(_)) => Ok(NestedSessionMessage::Pong),
            Some(Payload::Bye(_)) => Ok(NestedSessionMessage::Bye),
            Some(Payload::AnnounceAck(announce_ack)) => Ok(NestedSessionMessage::AnnounceAck {
                ancestry: announce_ack.ancestry.into_iter().map(Into::into).collect(),
                capabilities: capabilities_from_proto(&announce_ack.capabilities),
                descend_keys: keys_from_proto(&announce_ack.descend_keys),
            }),
            Some(Payload::FocusGained(focus_gained)) => Ok(NestedSessionMessage::FocusGained {
                from_direction: direction_from_proto(focus_gained.from_direction),
            }),
            Some(Payload::FocusLost(_)) => Ok(NestedSessionMessage::FocusLost),
            Some(Payload::FullscreenState(fullscreen_state)) => {
                Ok(NestedSessionMessage::FullscreenState {
                    fullscreen: fullscreen_state.fullscreen,
                })
            },
            Some(Payload::AncestryUpdate(ancestry_update)) => {
                Ok(NestedSessionMessage::AncestryUpdate {
                    ancestry: ancestry_update
                        .ancestry
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                })
            },
            Some(Payload::Ping(_)) => Ok(NestedSessionMessage::Ping),
            Some(Payload::ShortcutUpdate(shortcut_update)) => {
                Ok(NestedSessionMessage::ShortcutUpdate {
                    ascend_keys: keys_from_proto(&shortcut_update.ascend_keys),
                    descend_keys: keys_from_proto(&shortcut_update.descend_keys),
                })
            },
            None => Err(()),
        }
    }
}

pub fn encode_payload(message: &NestedSessionMessage) -> Vec<u8> {
    let proto_message: proto::NestedSessionMessage = message.clone().into();
    proto_message.encode_to_vec()
}

pub fn decode_payload(payload_bytes: &[u8]) -> Option<NestedSessionMessage> {
    let proto_message = proto::NestedSessionMessage::decode(payload_bytes).ok()?;
    NestedSessionMessage::try_from(proto_message).ok()
}

pub fn encode_frame(message: &NestedSessionMessage) -> Vec<u8> {
    encode_frame_from_payload(&encode_payload(message))
}

pub fn encode_frame_from_payload(payload_bytes: &[u8]) -> Vec<u8> {
    let encoded = BASE64_STANDARD.encode(payload_bytes);
    let mut frame = Vec::with_capacity(
        NESTED_FRAME_HEADER.len() + encoded.len() + NESTED_FRAME_TERMINATOR.len(),
    );
    frame.extend_from_slice(NESTED_FRAME_HEADER);
    frame.extend_from_slice(encoded.as_bytes());
    frame.extend_from_slice(NESTED_FRAME_TERMINATOR);
    frame
}

pub fn decode_base64(encoded: &[u8]) -> Option<Vec<u8>> {
    BASE64_DECODER.decode(encoded).ok()
}

const MAX_PARTIAL_FRAME_BYTES: usize = 1024 * 1024;

enum FrameScanStatus {
    Complete(usize),
    NeedMore,
    Diverged,
}

fn frame_scan_status(buf: &[u8]) -> FrameScanStatus {
    if buf.len() < NESTED_FRAME_HEADER.len() {
        return if NESTED_FRAME_HEADER.starts_with(buf) {
            FrameScanStatus::NeedMore
        } else {
            FrameScanStatus::Diverged
        };
    }
    if &buf[..NESTED_FRAME_HEADER.len()] != NESTED_FRAME_HEADER {
        return FrameScanStatus::Diverged;
    }
    let mut i = NESTED_FRAME_HEADER.len();
    while i < buf.len() {
        match buf[i] {
            0x1b => match buf.get(i + 1) {
                Some(&b'\\') => return FrameScanStatus::Complete(i + 2),
                Some(_) => return FrameScanStatus::Diverged,
                None => return FrameScanStatus::NeedMore,
            },
            _ => i += 1,
        }
    }
    FrameScanStatus::NeedMore
}

#[derive(Debug, Default)]
pub struct NestedFrameExtractor {
    partial_frame: Vec<u8>,
}

impl NestedFrameExtractor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn extract(&mut self, bytes: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut decoded_payloads = Vec::new();
        if self.partial_frame.is_empty() && !bytes.contains(&0x1b) {
            return (bytes.to_vec(), decoded_payloads);
        }
        let mut working = std::mem::take(&mut self.partial_frame);
        working.extend_from_slice(bytes);
        let mut cleaned = Vec::with_capacity(working.len());
        let mut i = 0;
        while i < working.len() {
            let rest = &working[i..];
            if rest[0] == 0x1b {
                match frame_scan_status(rest) {
                    FrameScanStatus::Complete(len) => {
                        let encoded_payload =
                            &rest[NESTED_FRAME_HEADER.len()..len - NESTED_FRAME_TERMINATOR.len()];
                        if let Some(payload) = decode_base64(encoded_payload) {
                            decoded_payloads.push(payload);
                        }
                        i += len;
                        continue;
                    },
                    FrameScanStatus::NeedMore => {
                        let tail = rest.to_vec();
                        if tail.len() > MAX_PARTIAL_FRAME_BYTES {
                            cleaned.extend_from_slice(&tail);
                        } else {
                            self.partial_frame = tail;
                        }
                        return (cleaned, decoded_payloads);
                    },
                    FrameScanStatus::Diverged => {},
                }
            }
            cleaned.push(working[i]);
            i += 1;
        }
        (cleaned, decoded_payloads)
    }

    pub fn partial_bytes(&self) -> &[u8] {
        &self.partial_frame
    }

    pub fn take_partial(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.partial_frame)
    }
}

#[cfg(test)]
#[path = "unit/nested_session.rs"]
mod tests;

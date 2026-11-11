use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use base64::alphabet::STANDARD as STANDARD_ALPHABET;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::{DecodePaddingMode, Engine as _};

const BASE64: GeneralPurpose = GeneralPurpose::new(
    &STANDARD_ALPHABET,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

const MAX_CSI_PARAMS: usize = 32;
const MAX_FORWARDED: usize = 2;
const MAX_OSC_LEN: usize = 8192;
const MAX_CLIPBOARD_OSC_LEN: usize = yinz_ipc::MAX_IPC_FRAME_LEN;
const MAX_IMAGE_BYTES: usize = 104_857_600;
const SHM_DIR: &str = "/dev/shm";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterState {
    Ground,
    Esc,
    EscBracket,
    OscForward,
    OscForwardEsc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscSignal {
    Notification(NotificationChunk),
    Title(Option<String>),
    Clipboard(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub title: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationChunk {
    pub id: Option<String>,
    pub title: bool,
    pub text: String,
    pub done: bool,
}

fn notification_chunk(osc: &[u8]) -> Option<NotificationChunk> {
    if let Some(rest) = osc.strip_prefix(b"9;") {
        return Some(NotificationChunk {
            id: None,
            title: false,
            text: String::from_utf8(rest.to_vec()).ok()?,
            done: true,
        });
    }
    let rest = osc.strip_prefix(b"99;")?;
    let separator = rest.iter().position(|byte| *byte == b';')?;
    let metadata = std::str::from_utf8(&rest[..separator]).ok()?;
    let mut chunk = NotificationChunk {
        id: Some(String::new()),
        title: true,
        text: String::new(),
        done: true,
    };
    let mut encoded = false;
    for pair in metadata.split(':') {
        match pair.split_once('=') {
            Some(("i", value)) => chunk.id = Some(value.to_owned()),
            Some(("p", "title")) => chunk.title = true,
            Some(("p", "body")) => chunk.title = false,
            Some(("p", _)) => return None,
            Some(("d", value)) => chunk.done = value != "0",
            Some(("e", value)) => encoded = value == "1",
            _ => {},
        }
    }
    let payload = &rest[separator + 1..];
    let payload = match encoded {
        true => BASE64.decode(payload).ok()?,
        false => payload.to_vec(),
    };
    chunk.text = String::from_utf8(payload).ok()?;
    Some(chunk)
}

fn title_body(osc: &[u8]) -> Option<&[u8]> {
    for prefix in [b"0;".as_slice(), b"1;".as_slice(), b"2;".as_slice()] {
        if let Some(rest) = osc.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    None
}

fn clipboard_body(osc: &[u8]) -> Option<String> {
    let rest = osc.strip_prefix(b"52;")?;
    let separator = rest.iter().position(|byte| *byte == b';')?;
    let encoded = &rest[separator + 1..];
    if encoded == b"?" {
        return None;
    }
    let decoded = BASE64.decode(encoded).ok()?;
    String::from_utf8(decoded).ok()
}

/// Decode terminal clipboard instructions without applying unrelated side effects.
pub fn clipboard_update(content: &str) -> Option<String> {
    let mut parser = ApcFilter::new();
    let mut text = None;
    for byte in content.bytes() {
        if let Kind::Signal(OscSignal::Clipboard(value)) = parser.advance(byte).kind {
            text = Some(value);
        }
    }
    text
}

fn osc_signal(osc: &[u8]) -> Option<OscSignal> {
    if osc.starts_with(b"9;") || osc.starts_with(b"99;") {
        return notification_chunk(osc).map(OscSignal::Notification);
    }
    if let Some(body) = title_body(osc) {
        return match String::from_utf8(body.to_vec()) {
            Ok(title) if title.is_empty() => Some(OscSignal::Title(None)),
            Ok(title) => Some(OscSignal::Title(Some(title))),
            Err(_) => None,
        };
    }
    clipboard_body(osc).map(OscSignal::Clipboard)
}

pub struct Forwarded {
    bytes: [u8; MAX_FORWARDED],
    len: usize,
}

impl Forwarded {
    fn of(bytes: &[u8]) -> Self {
        let mut buffer = [0u8; MAX_FORWARDED];
        buffer[..bytes.len()].copy_from_slice(bytes);
        Self {
            bytes: buffer,
            len: bytes.len(),
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

pub struct Step {
    pub bytes: Forwarded,
    pub kind: Kind,
}

impl Step {
    fn forward(bytes: &[u8]) -> Self {
        Self {
            bytes: Forwarded::of(bytes),
            kind: Kind::Forward,
        }
    }

    fn boundary(bytes: &[u8]) -> Self {
        Self {
            bytes: Forwarded::of(bytes),
            kind: Kind::Boundary,
        }
    }

    fn cleared(bytes: &[u8]) -> Self {
        Self {
            bytes: Forwarded::of(bytes),
            kind: Kind::Cleared,
        }
    }

    fn signal(bytes: &[u8], signal: OscSignal) -> Self {
        Self {
            bytes: Forwarded::of(bytes),
            kind: Kind::Signal(signal),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Forward,
    Cleared,
    Boundary,
    Signal(OscSignal),
}

pub struct ApcFilter {
    state: FilterState,
    params: Vec<u8>,
    osc: Vec<u8>,
    osc_overflow: bool,
}

impl ApcFilter {
    pub fn new() -> Self {
        Self {
            state: FilterState::Ground,
            params: Vec::new(),
            osc: Vec::new(),
            osc_overflow: false,
        }
    }

    pub fn advance(&mut self, byte: u8) -> Step {
        match self.state {
            FilterState::Ground => match byte {
                0x1b => {
                    self.state = FilterState::Esc;
                    Step::forward(&[])
                },
                0x00..=0x1f | 0x7f => Step::boundary(&[byte]),
                other => Step::forward(&[other]),
            },
            FilterState::Esc => match byte {
                0x1b => Step::forward(&[0x1b]),
                b'[' => {
                    self.state = FilterState::EscBracket;
                    self.params.clear();
                    Step::forward(&[0x1b, b'['])
                },
                b']' => {
                    self.state = FilterState::OscForward;
                    self.osc.clear();
                    self.osc_overflow = false;
                    Step::forward(&[0x1b, b']'])
                },
                other => {
                    self.state = FilterState::Ground;
                    Step::boundary(&[0x1b, other])
                },
            },
            FilterState::EscBracket => self.control_sequence(byte),
            FilterState::OscForward => match byte {
                0x1b => {
                    self.state = FilterState::OscForwardEsc;
                    Step::forward(&[])
                },
                0x07 | 0x9c => {
                    self.state = FilterState::Ground;
                    self.finish_osc(&[byte])
                },
                other => {
                    self.record_osc(other);
                    Step::forward(&[other])
                },
            },
            FilterState::OscForwardEsc => match byte {
                0x1b => Step::forward(&[0x1b]),
                b'\\' => {
                    self.state = FilterState::Ground;
                    self.finish_osc(&[0x1b, b'\\'])
                },
                other => {
                    self.state = FilterState::OscForward;
                    self.record_osc(0x1b);
                    self.record_osc(other);
                    Step::forward(&[0x1b, other])
                },
            },
        }
    }

    fn record_osc(&mut self, byte: u8) {
        if self.osc_overflow {
            return;
        }
        let limit = if self.osc.starts_with(b"52;") {
            MAX_CLIPBOARD_OSC_LEN
        } else {
            MAX_OSC_LEN
        };
        if self.osc.len() < limit {
            self.osc.push(byte);
        } else {
            self.osc_overflow = true;
            self.osc.clear();
        }
    }

    fn finish_osc(&mut self, terminator: &[u8]) -> Step {
        let body = std::mem::take(&mut self.osc);
        if std::mem::take(&mut self.osc_overflow) {
            return Step::boundary(terminator);
        }
        match osc_signal(&body) {
            Some(signal) => Step::signal(terminator, signal),
            None => Step::boundary(terminator),
        }
    }

    fn control_sequence(&mut self, byte: u8) -> Step {
        match byte {
            0x1b => {
                self.state = FilterState::Esc;
                Step::forward(&[])
            },
            0x20..=0x3f => {
                if self.params.len() < MAX_CSI_PARAMS {
                    self.params.push(byte);
                }
                Step::forward(&[byte])
            },
            0x40..=0x7e => {
                self.state = FilterState::Ground;
                match byte {
                    b'J' if self.erases_the_display() => Step::cleared(&[byte]),
                    _ => Step::boundary(&[byte]),
                }
            },
            _ => Step::forward(&[byte]),
        }
    }

    fn erases_the_display(&self) -> bool {
        matches!(self.params.as_slice(), b"2" | b"3")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub image_id: u32,
    pub placement_id: u32,
    pub cell_x: usize,
    pub cell_y: usize,
    pub offset_x: u32,
    pub offset_y: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub z: i32,
}

struct Resident {
    image: Rc<Image>,
    revision: u64,
}

pub struct Graphics {
    images: BTreeMap<u32, Resident>,
    placements: BTreeMap<(u32, u32), Placement>,
    revisions: u64,
    stamp: u64,
}

impl Graphics {
    pub fn new() -> Self {
        Self {
            images: BTreeMap::new(),
            placements: BTreeMap::new(),
            revisions: 0,
            stamp: 0,
        }
    }

    pub fn stamp(&self) -> u64 {
        self.stamp
    }

    pub fn image(&self, id: u32) -> Option<&Rc<Image>> {
        self.images.get(&id).map(|resident| &resident.image)
    }

    pub fn revision(&self, id: u32) -> Option<u64> {
        self.images.get(&id).map(|resident| resident.revision)
    }

    pub fn resident_ids(&self) -> Vec<u32> {
        self.images.keys().copied().collect()
    }

    pub fn placements(&self) -> impl Iterator<Item = &Placement> {
        self.placements.values()
    }

    pub fn clear(&mut self) {
        self.stamp += 1;
        self.images.clear();
        self.placements.clear();
    }

    pub fn insert_image(&mut self, id: u32, image: Image) {
        self.revisions += 1;
        self.stamp += 1;
        self.images.insert(
            id,
            Resident {
                image: Rc::new(image),
                revision: self.revisions,
            },
        );
    }

    pub fn insert_placement(&mut self, placement: Placement) {
        if !self.images.contains_key(&placement.image_id) {
            return;
        }
        self.stamp += 1;
        self.placements
            .insert((placement.image_id, placement.placement_id), placement);
    }

    pub fn remove_image(&mut self, id: u32) {
        self.stamp += 1;
        self.images.remove(&id);
        self.placements.retain(|key, _| key.0 != id);
    }

    pub fn remove_placement(&mut self, image_id: u32, placement_id: u32) {
        self.stamp += 1;
        self.placements.remove(&(image_id, placement_id));
    }
}

pub fn read_shared_media(path: &str, expected: usize) -> Option<Vec<u8>> {
    if expected > MAX_IMAGE_BYTES {
        report!("refusing a {} byte image from {:?}", expected, path);
        return None;
    }
    let path = Path::new(path);
    let inside = path.starts_with(SHM_DIR) || path.starts_with(std::env::temp_dir());
    let climbs = path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir));
    if !inside || climbs {
        report!("refusing to read graphics media from {:?}", path);
        return None;
    }
    let file = match open_without_blocking(path) {
        Ok(file) => file,
        Err(e) => {
            report!("failed to read {:?}: {}", path, e);
            return None;
        },
    };
    match file.metadata() {
        Ok(metadata) if metadata.is_file() => {},
        Ok(_) => {
            report!(
                "refusing to read graphics media from {:?}: not a file",
                path
            );
            return None;
        },
        Err(e) => {
            report!("failed to read {:?}: {}", path, e);
            return None;
        },
    }
    let mut bytes = Vec::with_capacity(expected);
    if let Err(e) = file.take(expected as u64).read_to_end(&mut bytes) {
        report!("failed to read {:?}: {}", path, e);
        return None;
    }
    if bytes.len() < expected {
        report!(
            "{:?} carries {} bytes, not the {} its record named",
            path,
            bytes.len(),
            expected
        );
        return None;
    }
    Some(bytes)
}

#[cfg(unix)]
fn open_without_blocking(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_without_blocking(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

#[cfg(test)]
pub(crate) mod test_helpers;

#[cfg(test)]
#[path = "unit/kitty_tests.rs"]
mod tests;

impl Default for ApcFilter {
    fn default() -> Self {
        Self::new()
    }
}
impl Default for Graphics {
    fn default() -> Self {
        Self::new()
    }
}

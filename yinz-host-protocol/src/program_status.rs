//! OSC 7501 revision 0.4. Adapted from Zellij (MIT), commit c8585459.
use base64::alphabet::STANDARD as BASE64_STANDARD_ALPHABET;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::{DecodePaddingMode, Engine as _};
use std::collections::BTreeMap;

pub use yinz_types::program_status::{
    AttentionStatus, BlockedKind, ProgramState, ProgramStatusRecord,
};

pub const MAX_SEQUENCE_BYTES: usize = 4096;
pub const MAX_KEY_BYTES: usize = 16;
pub const MAX_MSG_ENCODED_BYTES: usize = 2732;
pub const MAX_MSG_DECODED_BYTES: usize = 2048;
pub const MAX_TITLE_ENCODED_BYTES: usize = 256;
pub const MAX_TITLE_DECODED_BYTES: usize = 192;
pub const MAX_APP_BYTES: usize = 32;
pub const MAX_ID_BYTES: usize = 128;
pub const MAX_ID_SEGMENT_BYTES: usize = 32;
pub const MAX_ID_DEPTH: usize = 8;
pub const MAX_RECORDS: usize = 256;

pub const QUERY_REPLY_BODY: &str =
    "?:states=idle,working,done,blocked,error:kinds=permission,question,auth";

const BASE64_DECODER: GeneralPurpose = GeneralPurpose::new(
    &BASE64_STANDARD_ALPHABET,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgramStatusMessage {
    Query,
    Set(ProgramStatusRecord),
    Clear { id: Option<String> },
}

pub fn query_reply(bell_terminated: bool) -> Vec<u8> {
    let terminator = if bell_terminated { "\x07" } else { "\x1b\\" };
    format!("\x1b]7501;{}{}", QUERY_REPLY_BODY, terminator).into_bytes()
}

fn sequence_length(params: &[&[u8]], bell_terminated: bool) -> usize {
    let separators = params.len().saturating_sub(1);
    let body: usize = params.iter().map(|p| p.len()).sum();
    let terminator = if bell_terminated { 1 } else { 2 };
    2 + body + separators + terminator
}

fn is_value_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b',' | b'+' | b'/' | b'=' | b'-')
}

fn is_app_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-')
}

pub fn valid_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        return false;
    }
    let segments: Vec<&str> = id.split('/').collect();
    if segments.len() > MAX_ID_DEPTH {
        return false;
    }
    segments.iter().all(|segment| {
        !segment.is_empty()
            && segment.len() <= MAX_ID_SEGMENT_BYTES
            && segment.bytes().all(is_app_byte)
    })
}

fn has_forbidden_char(text: &str) -> bool {
    text.chars().any(|c| {
        let code = c as u32;
        code <= 0x1f || code == 0x7f || (0x80..=0x9f).contains(&code)
    })
}

fn decode_text(value: &str, max_encoded: usize, max_decoded: usize) -> Result<Option<String>, ()> {
    if value.len() > max_encoded {
        return Err(());
    }
    if value.is_empty() {
        return Ok(None);
    }
    let bytes = BASE64_DECODER.decode(value.as_bytes()).map_err(|_| ())?;
    if bytes.len() > max_decoded {
        return Err(());
    }
    let text = String::from_utf8(bytes).map_err(|_| ())?;
    if has_forbidden_char(&text) {
        return Err(());
    }
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

pub fn parse(params: &[&[u8]], bell_terminated: bool) -> Option<ProgramStatusMessage> {
    if sequence_length(params, bell_terminated) > MAX_SEQUENCE_BYTES {
        return None;
    }
    let body_bytes = params.get(1..)?.join(&b';');
    let body = std::str::from_utf8(&body_bytes).ok()?;
    if body.trim() == "?" {
        return Some(ProgramStatusMessage::Query);
    }
    let mut pairs: BTreeMap<&str, &str> = BTreeMap::new();
    for pair in body.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_lowercase()) {
            continue;
        }
        if key.len() > MAX_KEY_BYTES {
            return None;
        }
        if !value.bytes().all(is_value_byte) {
            continue;
        }
        pairs.insert(key, value);
    }
    let state_value = *pairs.get("state")?;
    let state = if state_value == "clear" {
        None
    } else {
        Some(ProgramState::parse(state_value)?)
    };
    let id = match pairs.get("id") {
        Some(id) => {
            if !valid_id(id) {
                return None;
            }
            Some((*id).to_owned())
        },
        None => None,
    };
    let app = match pairs.get("app") {
        Some(app) if app.len() > MAX_APP_BYTES => return None,
        Some(app) if !app.is_empty() && app.bytes().all(is_app_byte) => Some((*app).to_owned()),
        _ => None,
    };
    let title = match pairs.get("title") {
        Some(title) => decode_text(title, MAX_TITLE_ENCODED_BYTES, MAX_TITLE_DECODED_BYTES).ok()?,
        None => None,
    };
    let msg = match pairs.get("msg") {
        Some(msg) => decode_text(msg, MAX_MSG_ENCODED_BYTES, MAX_MSG_DECODED_BYTES).ok()?,
        None => None,
    };
    let Some(state) = state else {
        return Some(ProgramStatusMessage::Clear { id });
    };
    let kind = if state == ProgramState::Blocked {
        pairs.get("kind").and_then(|k| BlockedKind::parse(k))
    } else {
        None
    };
    let progress = if matches!(state, ProgramState::Working | ProgramState::Blocked) {
        pairs
            .get("progress")
            .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|p| p.parse::<u32>().ok())
            .filter(|p| *p <= 100)
            .map(|p| p as u8)
    } else {
        None
    };
    Some(ProgramStatusMessage::Set(ProgramStatusRecord {
        id,
        state: Some(state),
        kind,
        progress,
        app,
        title,
        msg,
        updated_ms: 0,
    }))
}

#[derive(Debug, Clone, Default)]
pub struct ProgramStatusRecords {
    records: BTreeMap<String, (u64, ProgramStatusRecord)>,
    sequence: u64,
    working_since: BTreeMap<String, u64>,
    worked: BTreeMap<String, u64>,
}

fn record_key(id: &Option<String>) -> String {
    id.clone().unwrap_or_default()
}

impl ProgramStatusRecords {
    pub fn snapshot(&self) -> Vec<ProgramStatusRecord> {
        let mut records: Vec<_> = self.records.values().collect();
        records.sort_by_key(|(sequence, _)| *sequence);
        records
            .into_iter()
            .map(|(_, record)| record.clone())
            .collect()
    }

    pub fn restore(records: Vec<ProgramStatusRecord>) -> Result<Self, &'static str> {
        if records.len() > MAX_RECORDS {
            return Err("too many program status records");
        }
        let mut restored = Self::default();
        for record in records {
            let state = record.state.ok_or("missing program state")?;
            let mut body = format!("state={}", state.as_str());
            for (key, value) in [("id", &record.id), ("app", &record.app)] {
                if let Some(value) = value {
                    body.push_str(&format!(":{key}={value}"));
                }
            }
            for (key, value) in [("msg", &record.msg), ("title", &record.title)] {
                if let Some(value) = value {
                    body.push_str(&format!(":{key}={}", BASE64_DECODER.encode(value)));
                }
            }
            if let Some(kind) = record.kind {
                body.push_str(&format!(":kind={}", kind.as_str()));
            }
            if let Some(progress) = record.progress {
                body.push_str(&format!(":progress={progress}"));
            }
            let Some(ProgramStatusMessage::Set(parsed)) = parse(&[b"7501", body.as_bytes()], false)
            else {
                return Err("invalid program status record");
            };
            let mut expected = record.clone();
            expected.updated_ms = 0;
            if parsed != expected {
                return Err("invalid program status fields");
            }
            restored.apply(ProgramStatusMessage::Set(record.clone()), record.updated_ms);
        }
        Ok(restored)
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn root(&self) -> Option<&ProgramStatusRecord> {
        self.records.get("").map(|(_, r)| r)
    }
    pub fn get(&self, id: &str) -> Option<&ProgramStatusRecord> {
        self.records.get(id).map(|(_, r)| r)
    }
    pub fn iter(&self) -> impl Iterator<Item = &ProgramStatusRecord> {
        self.records.values().map(|(_, r)| r)
    }
    pub fn all(&self) -> Vec<ProgramStatusRecord> {
        self.records
            .values()
            .map(|(_, r)| {
                let mut record = r.clone();
                if record.app.is_none() {
                    record.app = self.inherited_app(&record.id);
                }
                record
            })
            .collect()
    }
    pub fn worked_ms(&self, id: &Option<String>) -> Option<u64> {
        self.worked.get(&record_key(id)).copied()
    }
    fn follow_work(&mut self, key: &str, state: ProgramState, now_ms: u64) {
        match state {
            ProgramState::Working | ProgramState::Blocked => {
                self.working_since.entry(key.to_owned()).or_insert(now_ms);
                self.worked.remove(key);
            },
            ProgramState::Done | ProgramState::Error => {
                if let Some(started) = self.working_since.remove(key) {
                    self.worked
                        .insert(key.to_owned(), now_ms.saturating_sub(started));
                }
            },
            ProgramState::Idle => {
                self.working_since.remove(key);
                self.worked.remove(key);
            },
        }
    }
    fn forget_missing(&mut self) {
        let records = &self.records;
        self.working_since
            .retain(|key, _| records.contains_key(key));
        self.worked.retain(|key, _| records.contains_key(key));
    }
    pub fn apply(&mut self, message: ProgramStatusMessage, now_ms: u64) -> bool {
        let changed = self.apply_message(message, now_ms);
        self.forget_missing();
        changed
    }
    fn apply_message(&mut self, message: ProgramStatusMessage, now_ms: u64) -> bool {
        match message {
            ProgramStatusMessage::Query => false,
            ProgramStatusMessage::Clear { id: None } => {
                let changed = !self.records.is_empty();
                self.records.clear();
                changed
            },
            ProgramStatusMessage::Clear { id: Some(id) } => {
                let prefix = format!("{}/", id);
                let before = self.records.len();
                self.records
                    .retain(|key, _| key != &id && !key.starts_with(&prefix));
                before != self.records.len()
            },
            ProgramStatusMessage::Set(mut record) => {
                record.updated_ms = now_ms;
                let key = record_key(&record.id);
                if !self.records.contains_key(&key)
                    && self.records.len() >= MAX_RECORDS
                    && let Some(oldest) = self
                        .records
                        .iter()
                        .min_by_key(|(_, (seq, _))| *seq)
                        .map(|(k, _)| k.clone())
                {
                    self.records.remove(&oldest);
                }
                self.sequence += 1;
                self.follow_work(&key, record.state(), now_ms);
                let changed = self
                    .records
                    .get(&key)
                    .map(|(_, existing)| {
                        let mut existing = existing.clone();
                        existing.updated_ms = now_ms;
                        existing != record
                    })
                    .unwrap_or(true);
                self.records.insert(key, (self.sequence, record));
                changed
            },
        }
    }
    pub fn clear_all(&mut self) -> bool {
        let changed = !self.records.is_empty();
        self.records.clear();
        self.forget_missing();
        changed
    }
    pub fn drop_transient(&mut self) -> bool {
        let before = self.records.len();
        self.records.retain(|_, (_, r)| !r.state().is_transient());
        self.forget_missing();
        before != self.records.len()
    }
    pub fn inherited_app(&self, id: &Option<String>) -> Option<String> {
        let parent = |path: &str| path.rsplit_once('/').map(|(parent, _)| parent.to_owned());
        let mut current = parent(id.as_ref()?);
        loop {
            let key = record_key(&current);
            if let Some(app) = self.records.get(&key).and_then(|(_, r)| r.app.clone()) {
                return Some(app);
            }
            current = parent(&current?);
        }
    }
    pub fn deciding_record(&self) -> Option<(AttentionStatus, ProgramStatusRecord)> {
        self.deciding_record_matching(|_| true)
    }
    /// Select among matching records after resolving their inherited app names.
    pub fn deciding_record_matching(
        &self,
        matches: impl Fn(&ProgramStatusRecord) -> bool,
    ) -> Option<(AttentionStatus, ProgramStatusRecord)> {
        let resolve_app = |record: &ProgramStatusRecord| {
            let mut record = record.clone();
            if record.app.is_none() {
                record.app = self.inherited_app(&record.id);
            }
            record
        };
        let root = self.root().map(resolve_app).filter(&matches);
        let mut best: Option<(AttentionStatus, ProgramStatusRecord)> =
            root.map(|r| (r.state().attention_status(), r));
        for (key, (_, record)) in self.records.iter() {
            if key.is_empty() {
                continue;
            }
            let record = resolve_app(record);
            if !matches(&record) {
                continue;
            }
            let child_status = match record.state() {
                ProgramState::Blocked => AttentionStatus::NeedsYou,
                ProgramState::Error => AttentionStatus::Failed,
                _ => continue,
            };
            let raises = match &best {
                None => true,
                Some((status, _)) => child_status > *status,
            };
            if raises {
                best = Some((child_status, record));
            }
        }
        best
    }
}

#[cfg(test)]
#[path = "unit/program_status.rs"]
mod tests;

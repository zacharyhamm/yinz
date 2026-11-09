//! Private snapshots and locked read/merge/write operations shared with password rotation.
use super::{password, SessionInfo};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};
use yinz_runtime::errors::prelude::*;

const MAX_BYTES: u64 = 256 * 1024;
pub(crate) const FLUSH_MS: i64 = 5 * 60 * 1000;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Row {
    pub id: String,
    pub label: String,
    pub token_hash: String,
    pub created_at_ms: i64,
    pub last_seen_ms: i64,
    pub expires_at_ms: i64,
    pub remember: bool,
}
impl Row {
    pub fn idle_expires_at_ms(&self) -> i64 {
        self.last_seen_ms
            .saturating_add(if self.remember {
                14 * 86400000
            } else {
                12 * 3600000
            })
            .min(self.expires_at_ms)
    }
    pub fn valid(&self, now: i64) -> bool {
        now < self.expires_at_ms && now < self.idle_expires_at_ms()
    }
    pub fn info(&self) -> SessionInfo {
        SessionInfo {
            id: self.id.clone(),
            label: self.label.clone(),
            created_at_ms: self.created_at_ms,
            last_seen_ms: self.last_seen_ms,
            expires_at_ms: self.expires_at_ms,
            idle_expires_at_ms: self.idle_expires_at_ms(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    version: u32,
    pub verifier_id: String,
    pub sessions: Vec<Row>,
}
impl Snapshot {
    fn empty(verifier: &str) -> Self {
        Self {
            version: 1,
            verifier_id: verifier.into(),
            sessions: vec![],
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
    mode: u32,
    uid: u32,
}
fn identity(m: &std::fs::Metadata) -> Identity {
    Identity {
        dev: m.dev(),
        ino: m.ino(),
        len: m.len(),
        mtime: (m.mtime(), m.mtime_nsec()),
        ctime: (m.ctime(), m.ctime_nsec()),
        mode: m.mode(),
        uid: m.uid(),
    }
}
fn hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn open(data: &Path) -> Result<Option<std::fs::File>> {
    // Validate the directory on reads too; provisioning must not silently repair unsafe state.
    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(data.join("web"))
        .context("open private web directory")?;
    let m = dir.metadata().context("inspect web directory")?;
    // SAFETY: geteuid has no preconditions.
    if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        bail!("unsafe web directory");
    }
    match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(data.join("web/sessions.json"))
    {
        Ok(file) => {
            password::validate_private(&file)?;
            Ok(Some(file))
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("open private sessions"),
    }
}
fn decode(file: std::fs::File) -> Result<Snapshot> {
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("read sessions")?;
    if bytes.len() as u64 > MAX_BYTES {
        bail!("sessions too large");
    }
    let s: Snapshot =
        serde_json::from_slice(&bytes).map_err(|_| anyhow!("invalid sessions JSON"))?;
    if s.version != 1 || !hex(&s.verifier_id, 32) || s.sessions.len() > 16 {
        bail!("invalid session store");
    }
    let mut ids = HashSet::new();
    let mut hashes = HashSet::new();
    for r in &s.sessions {
        if !hex(&r.id, 16)
            || !hex(&r.token_hash, 64)
            || !ids.insert(&r.id)
            || !hashes.insert(&r.token_hash)
            || r.label.len() > 128
            || r.label.chars().any(char::is_control)
            || r.created_at_ms < 0
            || r.last_seen_ms < r.created_at_ms
            || r.expires_at_ms <= r.created_at_ms
            || r.last_seen_ms >= r.expires_at_ms
        {
            bail!("invalid session row");
        }
    }
    Ok(s)
}
pub(crate) fn read(data: &Path, verifier: &str) -> Result<Snapshot> {
    let Some(file) = open(data)? else {
        return Ok(Snapshot::empty(verifier));
    };
    let s = decode(file)?;
    Ok(if s.verifier_id == verifier {
        s
    } else {
        Snapshot::empty(verifier)
    })
}
fn write(data: &Path, s: &Snapshot) -> Result<()> {
    let bytes = serde_json::to_vec(s).context("encode sessions")?;
    password::atomic_write(&data.join("web"), "sessions.json", &bytes)
}
pub(crate) fn hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn valid_token(token: &str) -> bool {
    token.strip_prefix("yzw1_").is_some_and(|s| {
        s.len() == 43
            && URL_SAFE_NO_PAD
                .decode(s)
                .is_ok_and(|b| b.len() == 32 && URL_SAFE_NO_PAD.encode(b) == s)
    })
}
#[derive(Default)]
pub(crate) struct Store {
    cache: Option<(Identity, Snapshot)>,
    dirty: HashMap<(String, String, String), i64>,
    last_flush: i64,
}
impl Store {
    pub fn snapshot(&mut self, data: &Path, verifier: &str) -> Result<Snapshot> {
        let result = (|| {
            let Some(file) = open(data)? else {
                self.cache = None;
                return Ok(Snapshot::empty(verifier));
            };
            let id = identity(&file.metadata().context("inspect sessions")?);
            if let Some((old, s)) = &self.cache
                && *old == id
                && s.verifier_id == verifier
            {
                return Ok(s.clone());
            }
            let s = decode(file)?;
            self.cache = Some((id, s.clone()));
            Ok(if s.verifier_id == verifier {
                s
            } else {
                Snapshot::empty(verifier)
            })
        })();
        if result.is_err() {
            self.cache = None;
        }
        result
    }
    fn merge(&mut self, s: &mut Snapshot, now: i64) {
        for r in &mut s.sessions {
            if let Some(t) =
                self.dirty
                    .get(&(s.verifier_id.clone(), r.id.clone(), r.token_hash.clone()))
            {
                r.last_seen_ms = r.last_seen_ms.max(*t);
            }
        }
        s.sessions.retain(|r| r.valid(now));
    }
    pub fn authenticate(
        &mut self,
        data: &Path,
        verifier: &str,
        token: &str,
        now: i64,
    ) -> Result<Option<SessionInfo>> {
        let mut s = self.snapshot(data, verifier)?;
        self.merge(&mut s, now);
        self.dirty.retain(|(v, id, h), _| {
            v == verifier && s.sessions.iter().any(|r| r.id == *id && r.token_hash == *h)
        });
        let h = hash(token);
        let Some(mut row) = s
            .sessions
            .into_iter()
            .find(|r| r.token_hash == h && r.valid(now))
        else {
            return Ok(None);
        };
        // Only touches of still-valid disk rows apply; dirty state never restores missing rows.
        let key = (verifier.into(), row.id.clone(), h);
        let seen = self.dirty.entry(key).or_insert(row.last_seen_ms);
        *seen = (*seen).max(now);
        row.last_seen_ms = *seen;
        if now.saturating_sub(self.last_flush) >= FLUSH_MS {
            self.flush(data, verifier, now)?;
        }
        Ok(Some(row.info()))
    }
    pub fn flush(&mut self, data: &Path, verifier: &str, now: i64) -> Result<()> {
        let _lock = password::lock(&data.join("web"))?;
        if !matches!(password::read_state(data),password::PasswordState::Ready(v) if v.verifier_id==verifier)
        {
            self.dirty.clear();
            self.cache = None;
            return Ok(());
        }
        let mut s = read(data, verifier)?;
        self.merge(&mut s, now);
        write(data, &s)?;
        self.dirty.clear();
        self.cache = None;
        self.last_flush = now;
        Ok(())
    }
    // Caller holds the persistent password lock and has checked the current verifier.
    pub fn issue(
        &mut self,
        data: &Path,
        verifier: &str,
        label: String,
        remember: bool,
        days: u32,
        now: i64,
    ) -> Result<(String, SessionInfo)> {
        if now < 0 {
            bail!("invalid session clock");
        }
        let mut s = read(data, verifier)?;
        self.merge(&mut s, now);
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(|_| anyhow!("secure randomness unavailable"))?;
        let token = format!("yzw1_{}", URL_SAFE_NO_PAD.encode(random));
        if s.sessions.len() >= 16 {
            s.sessions
                .sort_by(|a, b| (a.last_seen_ms, &a.id).cmp(&(b.last_seen_ms, &b.id)));
            s.sessions.remove(0);
        }
        let row = Row {
            id: password::random_id()?[..16].into(),
            label,
            token_hash: hash(&token),
            created_at_ms: now,
            last_seen_ms: now,
            expires_at_ms: now
                .checked_add(i64::from(days) * 86400000)
                .filter(|t| *t > now)
                .ok_or_else(|| anyhow!("invalid session lifetime"))?,
            remember,
        };
        let info = row.info();
        s.sessions.push(row);
        write(data, &s)?;
        self.dirty.clear();
        self.cache = None;
        self.last_flush = now;
        Ok((token, info))
    }
    // Caller still holds the password lock from issuance.
    pub fn discard_issued(&mut self, data: &Path, verifier: &str, id: &str) -> Result<()> {
        let mut s = read(data, verifier)?;
        s.sessions.retain(|r| r.id != id);
        write(data, &s)?;
        self.cache = None;
        Ok(())
    }
    pub fn logout_guarded(
        &mut self,
        data: &Path,
        id: &str,
        now: i64,
        live: impl FnOnce() -> bool,
    ) -> Result<()> {
        let _lock = password::lock(&data.join("web"))?;
        let password::PasswordState::Ready(v) = password::read_state(data) else {
            bail!("no password");
        };
        if !live() {
            bail!("request canceled");
        }
        let mut s = read(data, &v.verifier_id)?;
        self.merge(&mut s, now);
        s.sessions.retain(|r| r.id != id);
        write(data, &s)?;
        self.dirty.clear();
        self.cache = None;
        self.last_flush = now;
        Ok(())
    }
}
/// Secret-free, read-only active session summaries for the local CLI.
pub fn summaries(data: &Path, now: i64) -> Result<Vec<SessionInfo>> {
    let password::PasswordState::Ready(v) = password::read_state(data) else {
        return Ok(vec![]);
    };
    Ok(read(data, &v.verifier_id)?
        .sessions
        .into_iter()
        .filter(|r| r.valid(now))
        .map(|r| r.info())
        .collect())
}
#[cfg(test)]
#[path = "../unit/sessions.rs"]
mod tests;

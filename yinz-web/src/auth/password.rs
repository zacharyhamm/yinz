//! Private, locked, atomic password verifiers. No bearer sessions are enabled here.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use yinz_runtime::errors::prelude::*;

#[derive(Clone, PartialEq, Eq)]
pub struct Verifier {
    pub phc: String,
    pub verifier_id: String,
    pub updated_at_ms: i64,
}

impl std::fmt::Debug for Verifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verifier")
            .field("verifier_id", &self.verifier_id)
            .field("updated_at_ms", &self.updated_at_ms)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasswordState {
    Missing,
    Invalid(String),
    Ready(Verifier),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    phc: String,
    verifier_id: String,
    updated_at_ms: i64,
}

pub(crate) fn random_id() -> Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| anyhow!("cannot obtain secure randomness"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub(crate) fn private_dir(data_dir: &Path) -> Result<PathBuf> {
    let dir = data_dir.join("web");
    yinz_runtime::shared::ensure_secure_dir(&dir).context("secure web directory")?;
    Ok(dir)
}

pub(crate) fn validate_private(file: &File) -> Result<()> {
    let metadata = file.metadata().context("inspect private file")?;
    // SAFETY: geteuid has no preconditions.
    validate_metadata(&metadata, unsafe { libc::geteuid() })
}

fn validate_metadata(metadata: &std::fs::Metadata, uid: u32) -> Result<()> {
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        bail!("file must be regular, owned by the current user and private");
    }
    Ok(())
}

fn lock(dir: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(dir.join(".lock"))
        .context("open web lock")?;
    validate_private(&file)?;
    // SAFETY: file owns a valid descriptor. The persistent inode is never removed.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error()).context("lock web files");
    }
    Ok(file)
}

pub(crate) fn atomic_write(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut temp = tempfile::NamedTempFile::new_in(dir).context("create private temporary file")?;
    temp.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))
        .context("set private file permissions")?;
    temp.write_all(bytes).context("write private file")?;
    temp.as_file().sync_all().context("sync private file")?;
    temp.persist(dir.join(name))
        .map_err(|e| e.error)
        .context("publish private file")?;
    sync_dir(dir)
}

pub(crate) fn sync_dir(dir: &Path) -> Result<()> {
    File::open(dir)
        .context("open directory for sync")?
        .sync_all()
        .context("sync directory")
}

fn remove(dir: &Path, name: &str) -> Result<bool> {
    match std::fs::remove_file(dir.join(name)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).context("remove private web file"),
    }
}

fn publish(dir: &Path, v: Verifier) -> Result<()> {
    // Invalidate and sync BEFORE publishing a new verifier. A failed publication cannot
    // leave old bearer sessions alongside a newly published verifier, even after a crash.
    remove(dir, "sessions.json")?;
    sync_dir(dir)?;
    let stored = Stored {
        version: 1,
        phc: v.phc,
        verifier_id: v.verifier_id,
        updated_at_ms: v.updated_at_ms,
    };
    let bytes = serde_json::to_vec(&stored).context("encode password file")?;
    atomic_write(dir, "password", &bytes)
}

pub fn write(data_dir: &Path, password: &zeroize::Zeroizing<String>) -> Result<()> {
    if password.chars().count() < 12 || password.contains('\0') {
        bail!("password must contain at least 12 characters and no NUL");
    }
    let mut salt = [0; 16];
    getrandom::fill(&mut salt).map_err(|_| anyhow!("cannot obtain password salt"))?;
    let salt = SaltString::encode_b64(&salt).map_err(|_| anyhow!("cannot encode password salt"))?;
    let params =
        Params::new(65536, 3, 1, Some(32)).map_err(|_| anyhow!("invalid password parameters"))?;
    let phc = Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| anyhow!("cannot hash password"))?
        .to_string();
    let v = Verifier {
        phc,
        verifier_id: random_id()?,
        updated_at_ms: super::Clock::now_ms(&super::SystemClock),
    };
    let dir = private_dir(data_dir)?;
    let _lock = lock(&dir)?;
    publish(&dir, v)
}

pub fn revoke_sessions(data_dir: &Path) -> Result<()> {
    let dir = private_dir(data_dir)?;
    let _lock = lock(&dir)?;
    let mut v = match read_state(data_dir) {
        PasswordState::Ready(v) => v,
        _ => bail!("no usable web password; set a password first"),
    };
    v.verifier_id = random_id()?;
    v.updated_at_ms = super::Clock::now_ms(&super::SystemClock);
    publish(&dir, v)
}

pub fn clear(data_dir: &Path) -> Result<bool> {
    let dir = private_dir(data_dir)?;
    let _lock = lock(&dir)?;
    // Remove the verifier first so any partial failure is fail-closed.
    let password = remove(&dir, "password")?;
    sync_dir(&dir)?;
    let sessions = remove(&dir, "sessions.json")?;
    sync_dir(&dir)?;
    Ok(password || sessions)
}

fn parse(bytes: &[u8]) -> Result<Verifier> {
    let stored: Stored =
        serde_json::from_slice(bytes).map_err(|_| anyhow!("invalid password JSON"))?;
    if stored.version != 1
        || stored.verifier_id.len() != 32
        || !stored.verifier_id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        bail!("invalid password version or verifier id");
    }
    validate_phc(&stored.phc)?;
    Ok(Verifier {
        phc: stored.phc,
        verifier_id: stored.verifier_id,
        updated_at_ms: stored.updated_at_ms,
    })
}

fn validate_phc(phc: &str) -> Result<PasswordHash<'_>> {
    let hash = PasswordHash::new(phc).map_err(|_| anyhow!("invalid password encoding"))?;
    if hash.algorithm.as_str() != "argon2id" || hash.version != Some(19) {
        bail!("unsupported password algorithm");
    }
    let mut values = [None; 3];
    for (name, value) in hash.params.iter() {
        let index = match name.as_str() {
            "m" => 0,
            "t" => 1,
            "p" => 2,
            _ => bail!("unsupported password parameter"),
        };
        if values[index].is_some() {
            bail!("duplicate password parameter");
        }
        values[index] = Some(
            value
                .decimal()
                .map_err(|_| anyhow!("invalid password parameter"))?,
        );
    }
    if !values[0].is_some_and(|v| (19456..=262144).contains(&v))
        || !values[1].is_some_and(|v| (2..=10).contains(&v))
        || !values[2].is_some_and(|v| (1..=4).contains(&v))
    {
        bail!("password parameters outside safe bounds");
    }
    let salt = hash.salt.ok_or_else(|| anyhow!("missing password salt"))?;
    let mut decoded = [0; 64];
    let salt = salt
        .decode_b64(&mut decoded)
        .map_err(|_| anyhow!("invalid password salt"))?;
    if salt.len() < 8 || hash.hash.is_none_or(|h| h.len() < 16) {
        bail!("invalid password salt or hash length");
    }
    Ok(hash)
}

pub fn read_state(data_dir: &Path) -> PasswordState {
    fn read(data_dir: &Path) -> Result<Option<Verifier>> {
        let dir = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_DIRECTORY)
            .open(data_dir.join("web"))
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).context("open web directory"),
        };
        let m = dir.metadata().context("inspect web directory")?;
        // SAFETY: geteuid has no preconditions.
        if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            bail!("web directory must be private and owned");
        }
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(data_dir.join("web/password"))
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).context("open password file"),
        };
        validate_private(&file)?;
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .context("read password file")?;
        if bytes.len() > 4096 {
            bail!("password file exceeds 4096 bytes");
        }
        parse(&bytes).map(Some)
    }
    match read(data_dir) {
        Ok(Some(v)) => PasswordState::Ready(v),
        Ok(None) => PasswordState::Missing,
        Err(e) => PasswordState::Invalid(e.to_string()),
    }
}

pub fn verify(v: &Verifier, candidate: &[u8]) -> bool {
    validate_phc(&v.phc)
        .is_ok_and(|hash| Argon2::default().verify_password(candidate, &hash).is_ok())
}

#[cfg(test)]
#[path = "../unit/password.rs"]
pub(crate) mod tests;

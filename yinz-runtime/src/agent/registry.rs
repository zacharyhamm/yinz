//! Leased pane registrations shared by provider-specific conversation routers.
use crate::errors::prelude::*;
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::SystemTime;

pub struct Registration {
    pub dir: tempfile::TempDir,
    pub env: HashMap<String, String>,
    _lease: File,
}

impl Registration {
    pub fn new(root: &Path, prefix: &str, mut env: HashMap<String, String>) -> Result<Self> {
        let sockets = std::path::absolute(&*crate::consts::YINZ_SOCK_DIR)
            .context("resolve pane socket directory")?;
        // YINZ_SOCKET_DIR takes the base; YINZ_SOCK_DIR appends the contract version.
        env.insert(
            crate::envs::SOCKET_DIR_ENV_KEY.into(),
            sockets
                .parent()
                .and_then(Path::to_str)
                .context("encode pane socket directory")?
                .into(),
        );
        crate::shared::ensure_secure_dir(root).context("protect pane registry")?;
        let dir = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(root)
            .context("create pane registration")?;
        let lease = File::create(dir.path().join("lease")).context("create pane lease")?;
        lease.lock().context("hold pane lease")?;
        Ok(Self {
            dir,
            env,
            _lease: lease,
        })
    }
}

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().context("missing registry directory")?)
            .context("prepare registry file")?;
    serde_json::to_writer(&mut file, value).context("write registry file")?;
    file.persist(path).context("publish registry file")?;
    Ok(())
}

pub fn lease_held(dir: &Path) -> Result<bool> {
    let lease = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join("lease"))
        .context("open pane lease")?;
    // Shared, so concurrent probes of a dead lease cannot see each other as its holder.
    match lease.try_lock_shared() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(e)) => Err(e).context("check pane lease"),
    }
}

pub fn read_binding<T: DeserializeOwned>(dir: &Path) -> Result<Option<(T, SystemTime)>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join("binding.json"))
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("open pane binding"),
    };
    let metadata = file.metadata().context("inspect pane binding")?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 {
        return Ok(None);
    }
    let binding = serde_json::from_reader(file.take(16 * 1024)).context("read pane binding")?;
    let modified = metadata.modified().context("read pane binding time")?;
    Ok(Some((binding, modified)))
}

pub fn latest_binding<T: DeserializeOwned>(
    root: &Path,
    matches: impl Fn(&T) -> bool,
) -> Result<Option<T>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("read pane registry"),
    };
    // shortcut: scan open registrations, index by session if thousands of panes make this hot.
    let mut latest = None;
    for entry in entries {
        let entry = entry.context("read pane registration")?;
        let read = || -> Result<Option<_>> {
            if !lease_held(&entry.path()).context("check pane registration lease")? {
                return Ok(None);
            }
            let Some((binding, modified)) =
                read_binding(&entry.path()).context("read pane registration binding")?
            else {
                return Ok(None);
            };
            Ok(matches(&binding).then_some((modified, binding)))
        };
        // Registrations are created and removed concurrently with hooks.
        if let Ok(Some(candidate)) = read()
            && latest.as_ref().is_none_or(|(time, _)| candidate.0 > *time)
        {
            latest = Some(candidate);
        }
    }
    Ok(latest.map(|(_, binding)| binding))
}

#[cfg(test)]
#[path = "../unit/agent_registry_tests.rs"]
mod tests;

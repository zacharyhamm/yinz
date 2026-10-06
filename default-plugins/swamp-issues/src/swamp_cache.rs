//! Complete issue snapshots in the plugin's persistent /cache preopen, and the marker that
//! remembers the host granted this plugin its permissions.
use super::{Credentials, Search};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub(super) struct Cache {
    version: u32,
    pub catalogue: Option<Search>,
    pub text_search: Option<(String, Search)>,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            version: VERSION,
            catalogue: None,
            text_search: None,
        }
    }
}

impl Cache {
    pub fn search(&self, query: &str) -> Option<&Search> {
        if query.is_empty() {
            self.catalogue.as_ref()
        } else {
            self.text_search
                .as_ref()
                .filter(|(text, _)| text == query)
                .map(|(_, search)| search)
        }
    }

    pub fn read(path: &Path) -> Option<Self> {
        let cache: Self = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        (cache.version == VERSION).then_some(cache)
    }

    pub fn write(&self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("cache path has no parent"))?;
        fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec(self)?;
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        // create_new handles even a timestamp/sequence collision between WASI instances.
        let (temporary, mut file) = loop {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let temporary = parent.join(format!(".swamp-{stamp}-{sequence}.tmp"));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => break (temporary, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let result = file
            .write_all(&bytes)
            .and_then(|_| file.sync_all())
            .and_then(|_| fs::rename(&temporary, path));
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

/// Whether `marker` records that an earlier load was granted the permissions Swamp asks for.
///
/// The host answers a request it has already granted from its own cache, without a prompt; but
/// only a request says what the host has cached, and a first request with nothing cached puts
/// a prompt over the whole pane. The marker is what lets a returning user skip the key press
/// that a new one is asked for.
pub(super) fn remembers_permission(marker: &Path) -> bool {
    marker.is_file()
}

/// Record the host's answer: a grant as the marker, a denial as no marker, so the next load
/// waits to be asked again.
pub(super) fn remember_permission(marker: &Path, granted: bool) -> io::Result<()> {
    if granted {
        let parent = marker
            .parent()
            .ok_or_else(|| io::Error::other("permission marker has no parent"))?;
        fs::create_dir_all(parent)?;
        return fs::write(marker, b"");
    }
    match fs::remove_file(marker) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Credentials::resolve has already normalized the server URL. Hash both
/// partitions so neither a key nor a username is written into a filename.
pub(super) fn identity(credentials: &Credentials) -> String {
    let server = credentials.server_url.trim().trim_end_matches('/');
    let mut caller = Sha256::new();
    match &credentials.api_key {
        Some(key) => {
            caller.update(b"authenticated\0");
            caller.update((key.len() as u64).to_be_bytes());
            caller.update(key.as_bytes());
            if let Some(username) = &credentials.username {
                caller.update(b"username\0");
                caller.update(username.as_bytes());
            }
        },
        None => caller.update(b"anonymous"),
    }
    let hex = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    format!(
        "{}-{}",
        hex(&Sha256::digest(server.as_bytes())),
        hex(&caller.finalize())
    )
}

#[cfg(test)]
#[path = "unit/cache.rs"]
mod tests;

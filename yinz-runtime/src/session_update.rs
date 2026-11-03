//! Stable discovery paths for update coordinators shared by clients and servers.
use crate::errors::prelude::*;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub fn canonical_socket(socket: &Path) -> Result<PathBuf> {
    Ok(socket
        .parent()
        .context("session socket has no parent")?
        .canonicalize()
        .context("resolve session socket directory")?
        .join(socket.file_name().context("session socket has no name")?))
}

/// Serialize cancellation with the coordinator's transition into terminal
/// transfer. A successful cancellation can never race past that decision.
pub fn lock_phase(directory: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(directory.join("phase.lock"))
        .context("open update phase lock")?;
    lock.lock().context("lock update phase")?;
    Ok(lock)
}
pub fn record(data_dir: &Path, socket: &Path) -> Result<PathBuf> {
    let digest = Sha256::digest(canonical_socket(socket)?.as_os_str().as_encoded_bytes());
    let key: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(data_dir.join("updates").join(format!("session-{key}.json")))
}
pub fn read_active(data_dir: &Path, socket: &Path) -> Result<Option<serde_json::Value>> {
    let path = record(data_dir, socket)?;
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("read active update operation"),
    };
    let operation: String =
        serde_json::from_slice(&bytes).context("decode active update operation")?;
    uuid::Uuid::parse_str(&operation).context("invalid active update operation")?;
    let value = serde_json::from_slice(
        &std::fs::read(
            data_dir
                .join("updates")
                .join(operation)
                .join("journal.json"),
        )
        .context("read active update journal")?,
    )
    .context("decode active update journal")?;
    Ok(Some(value))
}

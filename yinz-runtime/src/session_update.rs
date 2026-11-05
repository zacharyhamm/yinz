//! Stable discovery paths for update coordinators shared by clients and servers.
use crate::errors::prelude::*;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Joining an operation must never silently change the user's artifact source.
pub fn ensure_matching_mode(
    source_build: bool,
    channel: yinz_types::session_update::ReleaseChannel,
    requested_source_build: bool,
    requested_channel: yinz_types::session_update::ReleaseChannel,
) -> Result<()> {
    use yinz_types::session_update::ReleaseChannel;
    let mode = match (source_build, channel) {
        (true, _) => "local source",
        (false, ReleaseChannel::Stable) => "stable",
        (false, ReleaseChannel::Prerelease) => "prerelease",
    };
    anyhow::ensure!(
        source_build == requested_source_build && channel == requested_channel,
        "a {mode} update is already in progress; cancel it first (or finish recovery if terminal transfer has begun)"
    );
    Ok(())
}

pub fn canonical_socket(socket: &Path) -> Result<PathBuf> {
    Ok(socket
        .parent()
        .context("session socket has no parent")?
        .canonicalize()
        .context("resolve session socket directory")?
        .join(socket.file_name().context("session socket has no name")?))
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
    read_journal(&data_dir.join("updates").join(operation)).map(Some)
}

pub fn read_journal(directory: &Path) -> Result<serde_json::Value> {
    let mut value: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.join("journal.json")).context("read update journal")?,
    )
    .context("decode update journal")?;
    // Cancellation is a historical failure outcome, including in status replies.
    if value["phase"] == "Cancelled" {
        value["phase"] = "Failed".into();
    }
    Ok(value)
}

/// Hash the image executing in this process, even after its installed path is replaced.
/// Call from an update worker, never from a client render callback.
pub fn running_executable_digest() -> Result<String> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        static DIGEST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        if let Some(digest) = DIGEST.get() {
            return Ok(digest.clone());
        }
        // current_exe() resolves the install path, which may already point to the
        // next generation. Linux's process-image link retains the actual inode.
        let mut file =
            std::fs::File::open("/proc/self/exe").context("open running executable image")?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let size = file
                .read(&mut buffer)
                .context("hash running executable image")?;
            if size == 0 {
                break;
            }
            hash.update(&buffer[..size]);
        }
        let digest: String = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let _ = DIGEST.set(digest.clone());
        Ok(digest)
    }
    #[cfg(not(target_os = "linux"))]
    {
        bail!("source-update image identity is supported only on Linux")
    }
}

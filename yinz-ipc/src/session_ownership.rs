//! Session names stay claimed from before bind until all server cleanup finishes.
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use yinz_runtime::errors::prelude::*;

pub struct SessionOwnership {
    state: Mutex<State>,
}

struct State {
    claim: File,
    path: PathBuf,
    bound: bool,
    shutting_down: bool,
}

/// The lock file for the session at `socket` in the sibling `directory`.
fn open_lock_file(socket: &Path, directory: &str) -> Result<File> {
    let directory = socket
        .parent()
        .context("session socket has no parent")?
        .join(directory);
    yinz_runtime::shared::ensure_secure_dir(&directory)
        .with_context(|| format!("secure session lock directory {directory:?}"))?;
    let name = socket.file_name().context("session socket has no name")?;
    // Never unlink: all contenders must lock the same inode, including after a crash.
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(name))
        .context("open session lock file")
}

fn claim(path: &Path) -> Result<Option<File>> {
    let file = open_lock_file(path, crate::consts::SESSION_OWNERSHIP_DIR)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error).context("lock session ownership file"),
    }
}

/// Serialize bootstraps that may create the session at `socket`, so each decides whether the
/// name is taken after the previous creator finished. Kept apart from ownership claims, which
/// the created server takes; dropping the file releases the lock.
pub fn lock_creation(socket: &Path, deadline: Instant) -> Result<File> {
    let file = open_lock_file(socket, crate::consts::SESSION_CREATION_LOCK_DIR)?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => {},
            Err(TryLockError::Error(error)) => return Err(error).context("lock session creation"),
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(
            !remaining.is_zero(),
            "another bootstrap is still creating session {:?}",
            socket.file_name().unwrap_or_default()
        );
        std::thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

impl SessionOwnership {
    /// Wait for teardown to release its claim, or yield to a reachable server.
    pub fn acquire_for_startup(path: &Path) -> Result<Option<Self>> {
        Self::acquire_until(
            path,
            Instant::now() + Duration::from_secs(25),
            std::thread::sleep,
        )
    }

    fn acquire_until(
        path: &Path,
        deadline: Instant,
        mut wait: impl FnMut(Duration),
    ) -> Result<Option<Self>> {
        loop {
            if let Some(owner) = Self::acquire(path)? {
                return Ok(Some(owner));
            }
            match crate::consts::ipc_connect(path) {
                Ok(_) => return Ok(None),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                    ) => {},
                Err(error) => return Err(error).context("check claimed session socket"),
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            anyhow::ensure!(
                !remaining.is_zero(),
                "timed out waiting for session ownership"
            );
            wait(remaining.min(Duration::from_millis(50)));
        }
    }

    pub fn acquire(path: &Path) -> Result<Option<Self>> {
        Ok(claim(path)?.map(|claim| Self {
            state: Mutex::new(State {
                claim,
                path: path.to_owned(),
                bound: false,
                shutting_down: false,
            }),
        }))
    }

    pub fn bind(&self) -> Result<interprocess::local_socket::Listener> {
        use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            !state.bound && !state.shutting_down,
            "session already bound or shutting down"
        );
        remove_stale_socket(&state.path)?;
        let listener = ListenerOptions::new()
            .name(
                state
                    .path
                    .as_path()
                    .to_fs_name::<GenericFilePath>()
                    .context("session socket name")?,
            )
            .reclaim_name(false)
            .create_sync()
            .context("bind session socket")?;
        state.bound = true;
        Ok(listener)
    }

    /// The transaction validates and renames disk state while both names are claimed.
    pub fn rename(
        &self,
        destination: &Path,
        transaction: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            state.bound && !state.shutting_down,
            "session is shutting down"
        );
        let destination_claim =
            claim(destination)?.context("destination session is already owned")?;
        remove_stale_socket(destination)?;
        transaction()?;
        state.path = destination.to_owned();
        state.claim = destination_claim;
        Ok(())
    }

    /// Unlink once, but retain the claim until workers and client writers have finished.
    pub fn shutdown(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.shutting_down = true;
        if state.bound {
            state.bound = false;
            match std::fs::remove_file(&state.path) {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(error).context("remove owned session socket"),
            }
        }
        Ok(())
    }
}

/// The caller must hold the name's claim before inspecting or removing its socket.
fn remove_stale_socket(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            crate::consts::is_ipc_socket(&metadata.file_type()),
            "session path is not a socket"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("inspect session socket"),
    }
    match crate::consts::ipc_connect(path) {
        Ok(_) => bail!("a live server already owns {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            std::fs::remove_file(path).context("remove stale session socket")
        },
        Err(error) => Err(error).context("check existing session socket"),
    }
}

impl Drop for SessionOwnership {
    fn drop(&mut self) {
        self.shutdown().non_fatal();
    }
}

#[cfg(test)]
#[path = "unit/session_ownership_tests.rs"]
mod tests;

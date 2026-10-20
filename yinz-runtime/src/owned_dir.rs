//! Owned scratch directories, shared with xtask without pulling in the host runtime.
//! A sibling lease is published before its directory, under a permanent registry lock.
//! Only an unlocked lease authorizes reclamation; names, ages and PIDs do not.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const TEST_CACHE_ROOT_ENV: &str = "YINZ_TEST_CACHE_ROOT";

#[derive(Debug)]
pub struct OwnedDir {
    path: PathBuf,
    lease: Option<File>,
    // Children keep the root and its lock alive through their final user's drop.
    _parent: Option<Arc<OwnedDir>>,
    _finished: Option<std::sync::mpsc::Sender<()>>,
}

impl OwnedDir {
    pub fn new(parent: &Path) -> io::Result<Arc<Self>> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        let metadata = fs::symlink_metadata(parent)?;
        // SAFETY: geteuid has no preconditions. The registry must not be replaceable by
        // another user while owners hold locks on files inside it.
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o022 != 0
        {
            return Err(io::Error::other("temporary directory registry must be an owned directory without group/other write access"));
        }
        let parent = parent.canonicalize()?;
        let registry = open_lock(&parent.join(".registry.lock"), true)?;
        registry.lock()?;
        reclaim(&parent)?;
        let lease = tempfile::Builder::new()
            .prefix("r")
            .suffix(".lease")
            .tempfile_in(&parent)?;
        lease.as_file().lock()?;
        let (lease, lease_path) = lease.keep()?;
        let path = lease_path.with_extension("");
        if let Err(error) = fs::DirBuilder::new().mode(0o700).create(&path) {
            // A random-name collision must not claim an existing, unmarked directory.
            remove_file(&lease_path)?;
            return Err(error);
        }
        Ok(Arc::new(Self {
            path,
            lease: Some(lease),
            _parent: None,
            _finished: None,
        }))
    }

    #[allow(dead_code)] // Only the plugin host creates individually owned children.
    pub fn child(self: &Arc<Self>) -> io::Result<Arc<Self>> {
        let path = tempfile::Builder::new()
            .prefix("p")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(&self.path)?
            .into_path();
        Ok(Arc::new(Self {
            path,
            lease: None,
            _parent: Some(self.clone()),
            _finished: None,
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Subscribe before sharing the root. Disconnection follows the final owner's cleanup,
    /// so a host can use its remaining shutdown budget without polling filesystem state.
    #[allow(dead_code)] // xtask waits for child processes instead.
    pub fn drop_notification(self: &mut Arc<Self>) -> io::Result<std::sync::mpsc::Receiver<()>> {
        let root = Arc::get_mut(self)
            .ok_or_else(|| io::Error::other("temporary directory is already shared"))?;
        let (send, receive) = std::sync::mpsc::channel();
        root._finished = Some(send);
        Ok(receive)
    }

    /// Test runners pass their lease to descendants so killing the runner cannot make
    /// a still-running nextest or test child look abandoned. Plugin hosts keep CLOEXEC.
    pub fn inherit_lock(&self) -> io::Result<()> {
        if let Some(lease) = &self.lease {
            // SAFETY: the owned file remains open; F_SETFD changes only its descriptor flags.
            if unsafe { libc::fcntl(lease.as_raw_fd(), libc::F_SETFD, 0) } == -1 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

fn open_lock(path: &Path, create: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other(
            "temporary directory lease is not a regular file",
        ));
    }
    Ok(file)
}

fn reclaim(parent: &Path) -> io::Result<()> {
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name
            .to_str()
            .filter(|name| name.starts_with('r') && name.ends_with(".lease"))
        else {
            continue;
        };
        match entry.file_type() {
            Ok(kind) if kind.is_file() => {},
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        }
        let lease = match open_lock(&entry.path(), false) {
            Ok(lease) => lease,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        match lease.try_lock() {
            Ok(()) => {},
            Err(TryLockError::WouldBlock) => continue,
            Err(TryLockError::Error(error)) => return Err(error),
        }
        let path = parent.join(name).with_extension("");
        // remove_dir_all does not follow symlinks, including symlinks within the tree.
        remove_dir(&path)?;
        remove_file(&entry.path())?;
    }
    Ok(())
}

fn remove_dir(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

impl Drop for OwnedDir {
    fn drop(&mut self) {
        if let Err(error) = remove_dir(&self.path) {
            eprintln!(
                "failed to remove temporary directory {}: {error}",
                self.path.display()
            );
            // Leave the marker so a later startup can retry this cleanup.
            return;
        }
        if self.lease.is_some()
            && let Err(error) = remove_file(&self.path.with_extension("lease"))
        {
            eprintln!(
                "failed to remove temporary directory lease for {}: {error}",
                self.path.display()
            );
        }
    }
}

#[cfg(test)]
#[path = "unit/owned_dir_tests.rs"]
mod tests;

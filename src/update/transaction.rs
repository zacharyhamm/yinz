//! A recoverable installation, independent of the session migration coordinator.
//!
//! Publication exchanges complete trees and keeps the displaced generation. The
//! journal records intent before each transition; inode identities distinguish a
//! completed rename from a crash just before it. Binary and data-directory claims
//! survive participant helper exits. Drop only releases advisory process locks.
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CString, OsString},
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::{ffi::OsStrExt, fs::MetadataExt, fs::OpenOptionsExt, fs::PermissionsExt},
    path::{Path, PathBuf},
};
use yinz_runtime::errors::prelude::*;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Staging,
    Prepared,
    Publishing,
    Published,
    RollingBack,
    RolledBack,
    Committed,
}

impl Status {
    fn terminal(self) -> bool {
        matches!(self, Self::RolledBack | Self::Committed)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
}

fn identity(path: &Path) -> Result<Option<Identity>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file() || metadata.is_dir(),
                "installation entry must be a regular file or directory: {}",
                path.display()
            );
            Ok(Some(Identity {
                device: metadata.dev(),
                inode: metadata.ino(),
            }))
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Journal {
    format: u32,
    operation: String,
    executable: PathBuf,
    plugins: PathBuf,
    status: Status,
    binary_root: PathBuf,
    binary_root_id: Identity,
    data_root: PathBuf,
    data_root_id: Identity,
    old_binary: Identity,
    old_builtins: Option<Identity>,
    new_binary: Option<Identity>,
    new_builtins: Option<Identity>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
struct Claim {
    operation: String,
    executable: PathBuf,
}

struct Location {
    executable: PathBuf,
    plugins: PathBuf,
    journal: PathBuf,
    claim: PathBuf,
    _locks: [File; 2],
}

fn sidecar(executable: &Path, suffix: &str) -> Result<PathBuf> {
    let mut name = OsString::from(".");
    name.push(
        executable
            .file_name()
            .context("executable has no filename")?,
    );
    name.push(suffix);
    Ok(executable.with_file_name(name))
}

/// Live servers hold the builtin lock shared while they read installed plugin
/// sources (`yinz_steel_builtins::read_guard`). Those readers finish, so wait
/// briefly for them instead of reporting a concurrent update.
const SHARED_READER_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

fn lock(path: &Path, wait: std::time::Duration) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("open installation lock {}", path.display()))?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::park_timeout(std::time::Duration::from_millis(50));
            },
            Err(error) => {
                return Err(anyhow!("{error}")).with_context(|| {
                    format!(
                        "installation is busy ({}); another update or plugin load is running",
                        path.display()
                    )
                })
            },
        }
    }
}

impl Location {
    fn lock(executable: &Path, data_dir: &Path) -> Result<Self> {
        let executable = executable
            .canonicalize()
            .context("resolve installed executable")?;
        anyhow::ensure!(executable.is_file(), "installed executable is not a file");
        for path in [
            data_dir.to_path_buf(),
            data_dir.join("plugins"),
            data_dir.join("plugins/builtins"),
        ] {
            anyhow::ensure!(
                !path.is_symlink(),
                "builtin installation directory is a symlink: {}",
                path.display()
            );
        }
        fs::create_dir_all(data_dir.join("plugins")).context("create builtin parent directory")?;
        let plugins = data_dir
            .join("plugins")
            .canonicalize()
            .context("resolve builtin parent directory")?;
        let binary_lock = lock(
            &sidecar(&executable, ".yinz-update.lock")?,
            std::time::Duration::ZERO,
        )?;
        let data_lock = lock(&plugins.join(".yinz-update.lock"), SHARED_READER_WAIT)?;
        Ok(Self {
            journal: sidecar(&executable, ".yinz-update.json")?,
            claim: plugins.join(".yinz-update-active.json"),
            executable,
            plugins,
            _locks: [binary_lock, data_lock],
        })
    }

    fn read_journal(&self) -> Result<Option<Journal>> {
        let Some(journal) = read_json::<Journal>(&self.journal)? else {
            return Ok(None);
        };
        anyhow::ensure!(
            journal.format == 1,
            "unsupported installation journal format"
        );
        anyhow::ensure!(
            journal.executable == self.executable,
            "unfinished update belongs to another binary or data directory: {}",
            self.journal.display()
        );
        if journal.plugins != self.plugins {
            let old_claim = read_json::<Claim>(&journal.plugins.join(".yinz-update-active.json"))?;
            anyhow::ensure!(journal.status.terminal()
                && !journal.binary_root.exists() && !journal.data_root.exists()
                && old_claim.is_none_or(|claim| claim.operation != journal.operation || claim.executable != journal.executable),
                "unfinished update belongs to data directory {}; recover it before using another data directory", journal.plugins.display());
        }
        for (root, parent) in [
            (
                &journal.binary_root,
                self.executable
                    .parent()
                    .context("executable has no parent")?,
            ),
            (&journal.data_root, journal.plugins.as_path()),
        ] {
            anyhow::ensure!(
                root.parent() == Some(parent)
                    && root
                        .file_name()
                        .is_some_and(|name| name.as_bytes().starts_with(b".yinz-update-")),
                "invalid artifact path in installation journal"
            );
        }
        Ok(Some(journal))
    }

    fn check_claim(&self, operation: &str) -> Result<()> {
        if let Some(claim) = read_json::<Claim>(&self.claim)? {
            anyhow::ensure!(
                claim.operation == operation && claim.executable == self.executable,
                "builtin installation belongs to unfinished update {} for {}",
                claim.operation,
                claim.executable.display()
            );
        }
        Ok(())
    }

    fn claim(&self, operation: &str) -> Result<()> {
        self.check_claim(operation)?;
        write_json(
            &self.claim,
            &Claim {
                operation: operation.to_owned(),
                executable: self.executable.clone(),
            },
        )
    }
}

/// Persistent installation state; dropping it deliberately does not roll back.
pub(crate) struct Transaction {
    location: Location,
    journal: Journal,
}

impl Transaction {
    pub(crate) fn prepare(
        source: &Path,
        executable: &Path,
        data_dir: &Path,
        operation: &str,
    ) -> Result<Self> {
        anyhow::ensure!(
            !operation.is_empty()
                && operation.len() <= 256
                && !operation.chars().any(char::is_control),
            "invalid update operation ID"
        );
        anyhow::ensure!(
            source.join("yinz").is_file()
                && !source.join("yinz").is_symlink()
                && source.join("plugins/builtins").is_dir(),
            "release is missing its executable or builtin plugins"
        );
        let location = Location::lock(executable, data_dir)?;
        location.check_claim(operation)?;
        if let Some(journal) = location.read_journal()? {
            if journal.operation == operation {
                anyhow::ensure!(matches!(journal.status, Status::Prepared | Status::Publishing | Status::Published | Status::Committed), "update preparation was interrupted or rolled back; recover operation {operation} before retrying");
                return Ok(Self { location, journal });
            }
            anyhow::ensure!(
                journal.status.terminal(),
                "installation belongs to unfinished update {}",
                journal.operation
            );
            let mut previous = Self { location, journal };
            previous.cleanup()?;
            return Self::prepare_after_lock(source, previous.location, operation);
        }
        Self::prepare_after_lock(source, location, operation)
    }

    fn prepare_after_lock(source: &Path, location: Location, operation: &str) -> Result<Self> {
        // Both parents are probed before recording any publication intent. In
        // particular, an unsupported atomic directory exchange cannot leave a
        // partially replaced builtin tree.
        let binary_parent = location
            .executable
            .parent()
            .context("executable has no parent")?;
        probe_exchange(binary_parent)?;
        probe_exchange(&location.plugins)?;
        let binary_root = tempfile::Builder::new()
            .prefix(".yinz-update-")
            .tempdir_in(binary_parent)
            .context("stage executable beside installation")?;
        let data_root = tempfile::Builder::new()
            .prefix(".yinz-update-")
            .tempdir_in(&location.plugins)
            .context("stage builtin generation beside installation")?;
        let journal = Journal {
            format: 1,
            operation: operation.to_owned(),
            executable: location.executable.clone(),
            plugins: location.plugins.clone(),
            status: Status::Staging,
            binary_root: binary_root.path().to_path_buf(),
            binary_root_id: identity(binary_root.path())?
                .context("missing binary staging directory")?,
            data_root: data_root.path().to_path_buf(),
            data_root_id: identity(data_root.path())?
                .context("missing builtin staging directory")?,
            old_binary: identity(&location.executable)?.context("missing installed executable")?,
            old_builtins: identity(&location.plugins.join("builtins"))?,
            new_binary: None,
            new_builtins: None,
        };
        // Keep these paths even when the initial journal rename succeeds but its
        // directory fsync fails. Dropping TempDir on that error would strand an
        // observable Staging journal without the roots needed for recovery.
        let _ = binary_root.into_path();
        let _ = data_root.into_path();
        let mut transaction = Self { location, journal };
        let result = write_json(&transaction.location.journal, &transaction.journal)
            .and_then(|()| transaction.location.claim(operation))
            .and_then(|()| transaction.stage(source));
        if let Err(error) = result {
            return match transaction.rollback() {
                Ok(()) => {
                    Err(error.context("prepare installation; active generation was not changed"))
                },
                Err(recovery) => Err(error.context(format!(
                    "prepare installation; cleanup requires recovery of {operation}: {recovery:#}"
                ))),
            };
        }
        Ok(transaction)
    }

    fn stage(&mut self, source: &Path) -> Result<()> {
        fs::copy(source.join("yinz"), self.binary()).context("stage executable")?;
        fs::set_permissions(self.binary(), fs::Permissions::from_mode(0o755))
            .context("set staged executable permissions")?;
        File::open(self.binary())
            .context("open staged executable")?
            .sync_all()
            .context("sync staged executable")?;
        let generation = self.generation();
        let mut directory_modes = std::collections::BTreeMap::new();
        if self.journal.old_builtins.is_some() {
            copy_tree(&self.location.plugins.join("builtins"), &generation)?;
            collect_directory_modes(
                &self.location.plugins.join("builtins"),
                Path::new(""),
                &mut directory_modes,
            )?;
        }
        copy_tree(&source.join("plugins/builtins"), &generation)?;
        collect_directory_modes(
            &source.join("plugins/builtins"),
            Path::new(""),
            &mut directory_modes,
        )?;
        // Preserve custom directory privacy as well as file contents. Apply modes
        // only after staging so read-only installed directories can be overlaid.
        for (relative, permissions) in directory_modes.iter().rev() {
            fs::set_permissions(generation.join(relative), permissions.clone())
                .context("preserve builtin directory permissions")?;
        }
        sync_tree(&generation)?;
        sync_dir(&self.journal.binary_root)?;
        sync_dir(&self.journal.data_root)?;
        self.journal.new_binary = identity(&self.binary())?;
        self.journal.new_builtins = identity(&generation)?;
        self.set_status(Status::Prepared)
    }

    pub(crate) fn open(executable: &Path, data_dir: &Path, operation: &str) -> Result<Self> {
        Self::lookup(executable, data_dir, operation)?
            .context("no installation update journal for this operation")
    }

    /// Absence means this operation never staged an installation. A different
    /// active operation is an error, never permission to roll its files back.
    pub(crate) fn lookup(
        executable: &Path,
        data_dir: &Path,
        operation: &str,
    ) -> Result<Option<Self>> {
        let location = Location::lock(executable, data_dir)?;
        let Some(journal) = location.read_journal()? else {
            location.check_claim(operation)?;
            return Ok(None);
        };
        if journal.operation != operation {
            anyhow::ensure!(
                journal.status.terminal(),
                "installation journal belongs to operation {}",
                journal.operation
            );
            location.check_claim(operation)?;
            return Ok(None);
        }
        if !journal.status.terminal() {
            location.claim(operation)?;
        }
        Ok(Some(Self { location, journal }))
    }

    pub(crate) fn status(&self) -> Status {
        self.journal.status
    }

    pub(crate) fn publish(&mut self) -> Result<()> {
        if matches!(self.journal.status, Status::Published | Status::Committed) {
            anyhow::ensure!(
                identity(&self.location.executable)? == self.journal.new_binary
                    && identity(&self.location.plugins.join("builtins"))?
                        == self.journal.new_builtins,
                "published installation was modified"
            );
            return Ok(());
        }
        anyhow::ensure!(
            matches!(self.journal.status, Status::Prepared | Status::Publishing),
            "cannot publish installation in {:?}",
            self.journal.status
        );
        self.location.check_claim(&self.journal.operation)?;
        self.verify_roots()?;
        self.set_status(Status::Publishing)?;
        self.publish_builtins()?;
        self.publish_binary()?;
        self.set_status(Status::Published)
    }

    fn publish_builtins(&self) -> Result<()> {
        transition(
            &self.location.plugins.join("builtins"),
            &self.generation(),
            self.journal.old_builtins,
            self.journal
                .new_builtins
                .context("no staged builtin identity")?,
            true,
        )
    }

    fn publish_binary(&self) -> Result<()> {
        transition(
            &self.location.executable,
            &self.binary(),
            Some(self.journal.old_binary),
            self.journal
                .new_binary
                .context("no staged executable identity")?,
            true,
        )
    }

    pub(crate) fn rollback(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.journal.status != Status::Committed,
            "installation is already committed"
        );
        if self.journal.status == Status::RolledBack {
            return self.cleanup();
        }
        self.location.check_claim(&self.journal.operation)?;
        self.verify_roots()?;
        let was_staging = self.journal.status == Status::Staging;
        self.set_status(Status::RollingBack)?;
        // Staging cannot have published either resource. The optional identities
        // also make an interrupted staging rollback safe to repeat.
        if !was_staging {
            if let Some(new) = self.journal.new_binary {
                transition(
                    &self.location.executable,
                    &self.binary(),
                    Some(self.journal.old_binary),
                    new,
                    false,
                )?;
            }
            if let Some(new) = self.journal.new_builtins {
                transition(
                    &self.location.plugins.join("builtins"),
                    &self.generation(),
                    self.journal.old_builtins,
                    new,
                    false,
                )?;
            }
        }
        self.set_status(Status::RolledBack)?;
        self.cleanup()
    }

    pub(crate) fn commit(&mut self) -> Result<()> {
        anyhow::ensure!(
            matches!(self.journal.status, Status::Published | Status::Committed),
            "cannot commit installation in {:?}",
            self.journal.status
        );
        if self.journal.status != Status::Committed {
            anyhow::ensure!(
                identity(&self.location.executable)? == self.journal.new_binary
                    && identity(&self.location.plugins.join("builtins"))?
                        == self.journal.new_builtins,
                "published installation was modified before commit"
            );
            self.set_status(Status::Committed)?;
        }
        self.cleanup()
    }

    #[cfg(test)]
    pub(super) fn mark_committed_for_test(&mut self) -> Result<()> {
        self.set_status(Status::Committed)
    }

    #[cfg(test)]
    pub(super) fn backup_roots_for_test(&self) -> (PathBuf, PathBuf) {
        (
            self.journal.binary_root.clone(),
            self.journal.data_root.clone(),
        )
    }

    fn binary(&self) -> PathBuf {
        self.journal.binary_root.join("binary")
    }
    fn generation(&self) -> PathBuf {
        self.journal.data_root.join("builtins")
    }

    fn set_status(&mut self, status: Status) -> Result<()> {
        let previous = self.journal.status;
        self.journal.status = status;
        if let Err(error) = write_json(&self.location.journal, &self.journal) {
            // Publication of the journal may itself have succeeded before a
            // directory-sync error. Never clean backups on the strength of an
            // in-memory terminal state that was not actually written.
            self.journal.status = previous;
            if let Ok(Some(observed)) = self.location.read_journal() {
                self.journal.status = observed.status;
            }
            return Err(error);
        }
        Ok(())
    }

    fn verify_roots(&self) -> Result<()> {
        for (path, expected) in [
            (&self.journal.binary_root, self.journal.binary_root_id),
            (&self.journal.data_root, self.journal.data_root_id),
        ] {
            anyhow::ensure!(
                identity(path)? == Some(expected),
                "staging directory changed: {}",
                path.display()
            );
        }
        Ok(())
    }

    fn cleanup(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.journal.status.terminal(),
            "cannot remove an active installation generation"
        );
        // A prior helper may have observed the new receipt after rename but
        // failed its directory fsync. Make the terminal decision durable before
        // removing the only rollback copy, including on retry.
        write_json(&self.location.journal, &self.journal)?;
        for (path, expected) in [
            (&self.journal.binary_root, self.journal.binary_root_id),
            (&self.journal.data_root, self.journal.data_root_id),
        ] {
            if let Some(actual) = identity(path)? {
                anyhow::ensure!(
                    actual == expected,
                    "refusing to remove replaced staging directory: {}",
                    path.display()
                );
                make_retired_directories_writable(path)?;
                fs::remove_dir_all(path).context("remove retired installation generation")?;
                sync_dir(path.parent().context("staging directory has no parent")?)?;
            }
        }
        if let Some(claim) = read_json::<Claim>(&self.location.claim)?
            && claim.operation == self.journal.operation
            && claim.executable == self.location.executable
        {
            fs::remove_file(&self.location.claim)
                .context("release persistent builtin installation claim")?;
            sync_dir(&self.location.plugins)?;
        }
        // Keep a small terminal receipt so retried Commit/Rollback/Status requests
        // remain idempotent even after all generation backups have been removed.
        Ok(())
    }
}

fn transition(
    active: &Path,
    staged: &Path,
    old: Option<Identity>,
    new: Identity,
    publish: bool,
) -> Result<()> {
    let actual = identity(active)?;
    let desired = if publish { Some(new) } else { old };
    if actual == desired {
        // A previous helper may have completed rename but failed either parent
        // fsync. Repeating the observed transition must make it durable before
        // its caller records Published/RolledBack and may delete the backup.
        sync_dir(active.parent().context("installation has no parent")?)?;
        sync_dir(staged.parent().context("staging has no parent")?)?;
        return Ok(());
    }
    let expected = if publish { old } else { Some(new) };
    anyhow::ensure!(
        actual == expected,
        "installation ownership changed at {}; refusing recovery",
        active.display()
    );
    if old.is_some() {
        let staged_expected = if publish { Some(new) } else { old };
        anyhow::ensure!(
            identity(staged)? == staged_expected,
            "saved installation generation changed at {}",
            staged.display()
        );
        exchange(active, staged)?;
    } else if publish {
        anyhow::ensure!(
            identity(staged)? == Some(new),
            "staged builtin generation changed"
        );
        fs::rename(staged, active).context("publish initial builtin generation")?;
    } else {
        anyhow::ensure!(
            identity(staged)?.is_none(),
            "builtin recovery destination is occupied"
        );
        fs::rename(active, staged).context("withdraw initial builtin generation")?;
    }
    sync_dir(active.parent().context("installation has no parent")?)?;
    sync_dir(staged.parent().context("staging has no parent")?)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("open update record {}", path.display()))
        },
    };
    anyhow::ensure!(
        file.metadata().context("inspect update record")?.is_file(),
        "update record is not a regular file"
    );
    serde_json::from_reader(file)
        .map(Some)
        .with_context(|| format!("read update record {}", path.display()))
}

/// Directory permissions belong to the active generation; retained copies may
/// become writable only after a durable terminal decision permits their removal.
fn make_retired_directories_writable(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("inspect retired generation directory")?;
    if !metadata.is_dir() {
        return Ok(());
    }
    fs::set_permissions(
        path,
        fs::Permissions::from_mode(metadata.permissions().mode() | 0o700),
    )
    .context("permit cleanup of retired generation directory")?;
    for entry in fs::read_dir(path).context("read retired generation directory")? {
        let entry = entry.context("read retired generation entry")?;
        if entry
            .file_type()
            .context("inspect retired generation entry")?
            .is_dir()
        {
            make_retired_directories_writable(&entry.path())?;
        }
    }
    Ok(())
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    anyhow::ensure!(
        !path.is_symlink(),
        "update record is a symlink: {}",
        path.display()
    );
    let parent = path.parent().context("update record has no parent")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).context("stage update record")?;
    serde_json::to_writer(&mut file, value).context("serialize update record")?;
    file.write_all(b"\n").context("finish update record")?;
    file.as_file().sync_all().context("sync update record")?;
    file.persist(path).context("publish update record")?;
    sync_dir(parent)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    anyhow::ensure!(
        !source.is_symlink() && source.is_dir(),
        "builtin source is not a regular directory: {}",
        source.display()
    );
    yinz_steel_builtins::copy_tree(source, destination).context("stage complete builtin generation")
}

fn collect_directory_modes(
    source: &Path,
    relative: &Path,
    modes: &mut std::collections::BTreeMap<PathBuf, fs::Permissions>,
) -> Result<()> {
    let metadata = fs::symlink_metadata(source).context("inspect builtin directory permissions")?;
    anyhow::ensure!(
        metadata.is_dir(),
        "builtin source is not a regular directory"
    );
    modes
        .entry(relative.to_owned())
        .or_insert_with(|| metadata.permissions());
    for entry in fs::read_dir(source).context("read builtin directory permissions")? {
        let entry = entry.context("read builtin permission entry")?;
        if entry
            .file_type()
            .context("inspect builtin permission entry")?
            .is_dir()
        {
            collect_directory_modes(&entry.path(), &relative.join(entry.file_name()), modes)?;
        }
    }
    Ok(())
}

fn sync_tree(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path).context("read staged builtin generation")? {
        let entry = entry.context("read staged builtin entry")?;
        if entry
            .file_type()
            .context("inspect staged builtin entry")?
            .is_dir()
        {
            sync_tree(&entry.path())?;
        } else {
            File::open(entry.path())
                .context("open staged builtin")?
                .sync_all()
                .context("sync staged builtin")?;
        }
    }
    sync_dir(path)
}

fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)
        .with_context(|| format!("open directory {}", path.display()))?
        .sync_all()
        .with_context(|| format!("sync directory {}", path.display()))
}

fn probe_exchange(parent: &Path) -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix(".yinz-update-probe-")
        .tempdir_in(parent)
        .with_context(|| {
            format!(
                "installation directory is not writable: {}",
                parent.display()
            )
        })?;
    let left = temp.path().join("left");
    let right = temp.path().join("right");
    fs::create_dir(&left).context("create atomic exchange probe")?;
    fs::create_dir(&right).context("create atomic exchange probe")?;
    exchange(&left, &right)
        .context("installation filesystem does not support atomic generation exchange")
}

fn exchange(left: &Path, right: &Path) -> Result<()> {
    let left = CString::new(left.as_os_str().as_bytes()).context("invalid exchange path")?;
    let right = CString::new(right.as_os_str().as_bytes()).context("invalid exchange path")?;
    #[cfg(target_os = "linux")]
    // SAFETY: the C strings remain alive through the call and contain no NULs.
    let result = unsafe {
        nix::libc::renameat2(
            nix::libc::AT_FDCWD,
            left.as_ptr(),
            nix::libc::AT_FDCWD,
            right.as_ptr(),
            nix::libc::RENAME_EXCHANGE,
        )
    };
    #[cfg(target_os = "macos")]
    // SAFETY: the C strings remain alive through the call and contain no NULs.
    let result =
        unsafe { nix::libc::renamex_np(left.as_ptr(), right.as_ptr(), nix::libc::RENAME_SWAP) };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let result = {
        let _ = (left, right);
        anyhow::bail!("atomic installation exchange is unsupported on this platform");
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()).context("atomically exchange installation generations")
    }
}

/// A standalone update owns no pane migration, so an interrupted installation
/// can be restored before trying again. Session-owned journals require their
/// coordinator to decide commit versus recovery from actual pane ownership.
pub(super) fn recover_standalone(executable: &Path, data_dir: &Path) -> Result<()> {
    let location = Location::lock(executable, data_dir)?;
    let Some(journal) = location.read_journal()? else {
        return Ok(());
    };
    anyhow::ensure!(
        journal.status.terminal() || journal.operation.starts_with("standalone-"),
        "unfinished session update {}; resume its coordinator before installing",
        journal.operation
    );
    let mut transaction = Transaction { location, journal };
    if transaction.status() == Status::Committed {
        transaction.commit()
    } else {
        transaction.rollback()
    }
}

#[cfg(test)]
#[path = "../unit/update_transaction.rs"]
mod tests;

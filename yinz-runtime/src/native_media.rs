//! Prove and retain a specific shared directory for one native attachment.
use crate::{errors::prelude::*, fs_beneath};
use anyhow::ensure;
use nix::{
    errno::Errno,
    fcntl::{openat, OFlag},
    sys::stat::Mode,
    unistd::{unlinkat, UnlinkatFlags},
};
use rand::distr::{Alphanumeric, SampleString};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};
use yinz_types::render_negotiation::MediaProof;

const PROBE_PREFIX: &str = "yinz-native-probe-";
pub const IMAGE_PREFIX: &str = "yinz-native-image-";
const TOKEN_LEN: usize = 16;

/// Smaller images travel inline; staging a file costs more than copying them.
pub const SHARED_MEDIA_MIN_BYTES: usize = 256 * 1024;
/// The largest decoded image either peer stages or reads, equal to the Kitty decode limit.
pub const MAX_SHARED_IMAGE_BYTES: usize = 104_857_600;
/// Entries one viewer may retain at once, across its active and retired streams.
pub const VIEWER_MEDIA_ENTRIES: usize = 32;
/// Bytes one viewer may retain at once, across its active and retired streams.
pub const VIEWER_MEDIA_BYTES: usize = 128 * 1024 * 1024;

// A viewer with nothing outstanding can always take any image the reader would accept.
const _: () = assert!(VIEWER_MEDIA_BYTES >= MAX_SHARED_IMAGE_BYTES && VIEWER_MEDIA_ENTRIES > 0);

/// Inline residency and transfer-chunk bytes one native frame may carry to a viewer that
/// agreed chunked media. Larger images continue in later frames, one un-ACKed chunk frame
/// at a time.
pub const PORTABLE_FRAME_MEDIA_BYTES: usize = 8 * 1024 * 1024;
/// Room left in a portable frame for cells, records, sideband and the IPC envelope.
pub const PORTABLE_FRAME_HEADROOM_BYTES: usize = 32 * 1024 * 1024;

// A frame carrying its full media allowance stays far below the IPC frame limit.
const _: () = assert!(
    PORTABLE_FRAME_MEDIA_BYTES + PORTABLE_FRAME_HEADROOM_BYTES <= crate::consts::MAX_IPC_FRAME_LEN
);

/// Retained media held against one viewer's budget.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaUsage {
    pub entries: usize,
    pub bytes: usize,
}

/// Why a budget declined a reservation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The viewer already retains its maximum number of entries.
    Entries,
    /// The reservation would exceed the viewer's remaining bytes.
    Bytes,
    /// The reservation can never fit, even with nothing retained.
    TooLarge,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Refusal::Entries => "entry limit reached",
            Refusal::Bytes => "byte limit reached",
            Refusal::TooLarge => "larger than the shared media limit",
        })
    }
}

#[derive(Debug)]
struct BudgetInner {
    limit: MediaUsage,
    used: Mutex<MediaUsage>,
}

/// One viewer's accounting for retained media: staged files, and the transfer buffers
/// and assembled images of chunked portable delivery. Reserve before allocating; the
/// returned lease gives its share back when dropped.
#[derive(Clone, Debug)]
pub struct MediaBudget(Arc<BudgetInner>);

impl MediaBudget {
    pub fn new(limit_entries: usize, limit_bytes: usize) -> Self {
        Self(Arc::new(BudgetInner {
            limit: MediaUsage {
                entries: limit_entries,
                bytes: limit_bytes,
            },
            used: Mutex::new(MediaUsage::default()),
        }))
    }

    /// The production per-viewer budget.
    pub fn viewer() -> Self {
        Self::new(VIEWER_MEDIA_ENTRIES, VIEWER_MEDIA_BYTES)
    }

    fn used(&self) -> MutexGuard<'_, MediaUsage> {
        // Usage is updated in single assignments, so a poisoned value is still consistent.
        self.0.used.lock().unwrap_or_else(|poisoned| {
            self.0.used.clear_poison();
            poisoned.into_inner()
        })
    }

    pub fn try_reserve(&self, bytes: usize) -> std::result::Result<MediaLease, Refusal> {
        let limit = self.0.limit;
        if bytes > limit.bytes.min(MAX_SHARED_IMAGE_BYTES) || limit.entries == 0 {
            return Err(Refusal::TooLarge);
        }
        let mut used = self.used();
        if used.entries >= limit.entries {
            return Err(Refusal::Entries);
        }
        if bytes > limit.bytes - used.bytes {
            return Err(Refusal::Bytes);
        }
        *used = MediaUsage {
            entries: used.entries + 1,
            bytes: used.bytes + bytes,
        };
        Ok(MediaLease {
            budget: self.clone(),
            bytes,
        })
    }

    pub fn usage(&self) -> MediaUsage {
        *self.used()
    }

    pub fn limit(&self) -> MediaUsage {
        self.0.limit
    }
}

/// One retained entry's share of a viewer budget, returned exactly once on drop.
#[derive(Debug)]
pub struct MediaLease {
    budget: MediaBudget,
    bytes: usize,
}

impl MediaLease {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for MediaLease {
    fn drop(&mut self) {
        let mut used = self.budget.used();
        let released = used
            .entries
            .checked_sub(1)
            .zip(used.bytes.checked_sub(self.bytes));
        debug_assert!(released.is_some(), "media lease released twice");
        let (entries, bytes) = released.unwrap_or_default();
        *used = MediaUsage { entries, bytes };
    }
}

pub fn directory() -> PathBuf {
    std::env::temp_dir()
}

#[derive(Clone, Debug)]
pub struct MediaRoot {
    path: PathBuf,
    fd: Arc<File>,
}

impl MediaRoot {
    pub fn open(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .context("resolve native media directory")?;
        let fd = fs_beneath::open_root(&path).context("pin native media directory")?;
        Ok(Self {
            path,
            fd: Arc::new(fd),
        })
    }

    fn read(&self, name: &str, prefix: &str, len: usize) -> Result<Vec<u8>> {
        let suffix = name
            .strip_prefix(prefix)
            .context("invalid native media basename")?;
        ensure!(
            suffix.len() == 6 && suffix.bytes().all(|b| b.is_ascii_alphanumeric()),
            "invalid native media basename"
        );
        let file =
            fs_beneath::open_beneath(&self.fd, Path::new(name)).context("open native media")?;
        let metadata = file.metadata().context("inspect native media")?;
        ensure!(
            metadata.is_file()
                && metadata.uid() == nix::unistd::geteuid().as_raw()
                && metadata.mode() & 0o777 == 0o600
                && metadata.len() == len as u64,
            "invalid native media owner, mode, type or length"
        );
        let mut bytes = Vec::new();
        file.take(len as u64 + 1)
            .read_to_end(&mut bytes)
            .context("copy native media")?;
        ensure!(bytes.len() == len, "native media changed length");
        Ok(bytes)
    }

    pub fn read_image(&self, path: &[u8], len: usize) -> Result<Vec<u8>> {
        ensure!(
            len <= MAX_SHARED_IMAGE_BYTES,
            "native image exceeds size limit"
        );
        let path = std::str::from_utf8(path).context("invalid native image path")?;
        let (parent, basename) = path
            .rsplit_once('/')
            .context("expected absolute native image path")?;
        ensure!(
            Some(parent) == self.path.to_str(),
            "native image outside proven directory"
        );
        self.read(basename, IMAGE_PREFIX, len)
    }

    pub fn answer(&self, proof: &MediaProof) -> Result<Vec<u8>> {
        ensure!(
            self.path.to_str() == Some(&proof.root),
            "different native media root"
        );
        let tokens = self.read(&proof.basename, PROBE_PREFIX, 2 * TOKEN_LEN)?;
        ensure!(
            tokens[..TOKEN_LEN] == proof.challenge,
            "native media challenge mismatch"
        );
        Ok(tokens[TOKEN_LEN..].to_vec())
    }

    fn create_file(&self, prefix: &str) -> Result<MediaFile> {
        for _ in 0..32 {
            let suffix = Alphanumeric.sample_string(&mut rand::rng(), 6);
            let basename = format!("{prefix}{suffix}");
            let flags = OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC;
            match openat(
                &*self.fd,
                basename.as_str(),
                flags,
                Mode::S_IRUSR | Mode::S_IWUSR,
            ) {
                Ok(fd) => {
                    return Ok(MediaFile {
                        root: self.clone(),
                        basename,
                        file: fd.into(),
                        _lease: None,
                    })
                },
                Err(Errno::EEXIST) => continue,
                Err(error) => {
                    return Err(error).context("create file in pinned native media directory")
                },
            }
        }
        bail!("unable to allocate a unique native media basename")
    }

    /// Stage `bytes` under a lease reserved for exactly that length. On failure the
    /// partial file is unlinked and the lease returned before the error is.
    pub fn write_image(&self, bytes: &[u8], lease: MediaLease) -> Result<MediaFile> {
        ensure!(
            lease.bytes() == bytes.len(),
            "native image lease covers {} bytes, not {}",
            lease.bytes(),
            bytes.len()
        );
        let mut image = self
            .create_file(IMAGE_PREFIX)
            .context("create native image")?;
        image._lease = Some(lease);
        // The server's umask can differ from the client that created the probe.
        let metadata = image
            .file
            .metadata()
            .context("inspect staged native image")?;
        ensure!(
            metadata.mode() & 0o777 == 0o600,
            "staged native image requires mode 0600"
        );
        image.file.write_all(bytes).context("stage native image")?;
        Ok(image)
    }
}

/// Owns a staging entry in the pinned directory until its probe or frame is released.
/// An image also owns its budget lease, released only after the file is unlinked.
pub struct MediaFile {
    root: MediaRoot,
    basename: String,
    file: File,
    // Fields drop after `Drop::drop` unlinks the file, so the budget never covers less
    // than what is on disk.
    _lease: Option<MediaLease>,
}

impl MediaFile {
    pub fn path(&self) -> PathBuf {
        self.root.path.join(&self.basename)
    }
}

impl Drop for MediaFile {
    fn drop(&mut self) {
        unlinkat(
            &*self.root.fd,
            self.basename.as_str(),
            UnlinkatFlags::NoRemoveDir,
        )
        .context("remove staged native media")
        .non_fatal();
    }
}

pub struct Probe {
    pub root: MediaRoot,
    pub offer: MediaProof,
    pub expected: [u8; TOKEN_LEN],
    _file: MediaFile,
}

impl Probe {
    pub fn create(path: &Path) -> Result<Self> {
        Self::create_in(MediaRoot::open(path).context("open native probe directory")?)
    }

    fn create_in(root: MediaRoot) -> Result<Self> {
        let mut probe = root
            .create_file(PROBE_PREFIX)
            .context("create native media probe")?;
        let challenge = uuid::Uuid::new_v4().into_bytes();
        let expected = uuid::Uuid::new_v4().into_bytes();
        probe
            .file
            .write_all(&challenge)
            .context("write native challenge")?;
        probe
            .file
            .write_all(&expected)
            .context("write native response")?;
        let offer = MediaProof {
            root: root
                .path
                .to_str()
                .context("invalid native root encoding")?
                .into(),
            basename: probe.basename.clone(),
            challenge: challenge.into(),
        };
        Ok(Self {
            root,
            offer,
            expected,
            _file: probe,
        })
    }
}

#[cfg(test)]
#[path = "unit/native_media_tests.rs"]
mod tests;

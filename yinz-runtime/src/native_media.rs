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
    sync::Arc,
};
use yinz_types::render_negotiation::MediaProof;

const PROBE_PREFIX: &str = "yinz-native-probe-";
pub const IMAGE_PREFIX: &str = "yinz-native-image-";
const TOKEN_LEN: usize = 16;

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
        ensure!(len <= 104_857_600, "native image exceeds size limit");
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

    pub fn write_image(&self, bytes: &[u8]) -> Result<MediaFile> {
        let mut image = self
            .create_file(IMAGE_PREFIX)
            .context("create native image")?;
        image.file.write_all(bytes).context("stage native image")?;
        Ok(image)
    }
}

/// Owns a staging entry in the pinned directory until its probe or frame is released.
pub struct MediaFile {
    root: MediaRoot,
    basename: String,
    file: File,
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

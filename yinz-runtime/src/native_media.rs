//! Prove and retain a specific shared directory for one native attachment.
use crate::{errors::prelude::*, fs_beneath};
use anyhow::ensure;
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use yinz_types::render_negotiation::MediaProof;

const PROBE_PREFIX: &str = "yinz-native-probe-";
pub const IMAGE_PREFIX: &str = "yinz-native-image-";
const TOKEN_LEN: usize = 16;

pub fn directory() -> PathBuf {
    std::env::temp_dir()
}

#[derive(Debug)]
pub struct MediaRoot {
    path: PathBuf,
    fd: File,
}

impl MediaRoot {
    pub fn open(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .context("resolve native media directory")?;
        let fd = fs_beneath::open_root(&path).context("pin native media directory")?;
        Ok(Self { path, fd })
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

    pub fn write_image(&self, bytes: &[u8]) -> Result<NamedTempFile> {
        let mut file = tempfile::Builder::new()
            .prefix(IMAGE_PREFIX)
            .tempfile_in(&self.path)
            .context("create native image")?;
        // Verify the new file is in the pinned directory even if its pathname was replaced.
        let pinned = fs_beneath::open_beneath(
            &self.fd,
            Path::new(file.path().file_name().context("missing image basename")?),
        )?
        .metadata()?;
        let actual = file.as_file().metadata()?;
        ensure!(
            (actual.dev(), actual.ino()) == (pinned.dev(), pinned.ino()),
            "native image root changed"
        );
        file.write_all(bytes).context("stage native image")?;
        Ok(file)
    }
}

pub struct Probe {
    pub root: MediaRoot,
    pub offer: MediaProof,
    pub expected: [u8; TOKEN_LEN],
    _file: NamedTempFile,
}

impl Probe {
    pub fn create(path: &Path) -> Result<Self> {
        let root = MediaRoot::open(path)?;
        let mut file = tempfile::Builder::new()
            .prefix(PROBE_PREFIX)
            .tempfile_in(&root.path)
            .context("create native media probe")?;
        let challenge = uuid::Uuid::new_v4().into_bytes();
        let expected = uuid::Uuid::new_v4().into_bytes();
        file.write_all(&challenge)
            .context("write native challenge")?;
        file.write_all(&expected).context("write native response")?;
        let offer = MediaProof {
            root: root
                .path
                .to_str()
                .context("invalid native root encoding")?
                .into(),
            basename: file
                .path()
                .file_name()
                .and_then(|n| n.to_str())
                .context("invalid native probe basename")?
                .into(),
            challenge: challenge.into(),
        };
        Ok(Self {
            root,
            offer,
            expected,
            _file: file,
        })
    }
}

#[cfg(test)]
#[path = "unit/native_media_tests.rs"]
mod tests;

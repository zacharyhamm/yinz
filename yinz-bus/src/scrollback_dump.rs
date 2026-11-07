//! Scrollback dumps that an in-place editor pane opens.
//!
//! A dump holds a pane's whole scrollback, so it lives in a folder only this user can read
//! (`0700`), is created fresh with mode `0600`, and exists only as long as the editor pane
//! that shows it: the pty thread deletes it when that pane closes or fails to start.

use std::path::{Path, PathBuf};

use yinz_runtime::consts::YINZ_TMP_DIR;
use yinz_runtime::errors::prelude::*;

/// The private folder that holds every scrollback dump.
pub fn scrollback_dump_dir() -> PathBuf {
    YINZ_TMP_DIR.join("scrollback-dumps")
}

/// A fresh, unique path for one dump inside [`scrollback_dump_dir`]. The folder itself is
/// secured when the dump is written.
pub fn new_scrollback_dump_path() -> PathBuf {
    scrollback_dump_dir().join(format!("{}.dump", uuid::Uuid::new_v4()))
}

/// Whether `path` names a dump this module handed out, and so one Yinz may delete.
pub fn is_scrollback_dump(path: &Path) -> bool {
    path.parent() == Some(scrollback_dump_dir().as_path())
        && path
            .extension()
            .is_some_and(|extension| extension == "dump")
}

/// Write `contents` to a new file at `path` that only this user can read.
///
/// The parent folder is created or checked as a private directory first. The file must not
/// exist yet: an existing file or symlink at `path` is an error rather than something to
/// overwrite or follow.
pub fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;

    let err_context = || format!("failed to write private file {}", path.display());
    let parent = path
        .parent()
        .with_context(|| format!("private file {} has no parent folder", path.display()))?;
    yinz_runtime::shared::ensure_secure_dir(parent).with_context(err_context)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path).with_context(err_context)?;
    file.write_all(contents).with_context(err_context)
}

/// Delete a dump. A path outside [`scrollback_dump_dir`] is left alone, and a dump that is
/// already gone (or that a test backend never wrote) is not an error.
pub fn remove_scrollback_dump(path: &Path) -> Result<()> {
    if !is_scrollback_dump(path) {
        log::warn!(
            "refusing to delete {}: not a scrollback dump",
            path.display()
        );
        return Ok(());
    }
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error)
            .with_context(|| format!("failed to delete scrollback dump {}", path.display())),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "./unit/scrollback_dump_tests.rs"]
mod scrollback_dump_tests;

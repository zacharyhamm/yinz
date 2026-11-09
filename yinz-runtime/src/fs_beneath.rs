//! Descriptor-relative reads that refuse symlinks and special-file blocking.
use crate::errors::prelude::*;
use std::{
    fs::File,
    path::{Component, Path},
};

pub fn open_root(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .context("open confined root")
}

// Pin each directory descriptor so a symlink swap after canonicalization cannot escape the root.
pub fn open_beneath(root: &File, path: &Path) -> Result<File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    };
    let mut directory = root.try_clone().context("retain plugin source root")?;
    let parts: Vec<_> = path.components().collect();
    for (index, component) in parts.iter().enumerate() {
        let Component::Normal(name) = component else {
            bail!("noncanonical module path")
        };
        let name = std::ffi::CString::new(name.as_bytes()).context("module path contains NUL")?;
        let mut flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        if index + 1 != parts.len() {
            flags |= libc::O_DIRECTORY;
        }
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error()).context("open module path component");
        }
        directory = unsafe { File::from_raw_fd(fd) };
    }
    Ok(directory)
}

#[cfg(test)]
#[path = "unit/fs_beneath.rs"]
mod tests;

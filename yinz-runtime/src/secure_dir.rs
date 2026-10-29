use std::collections::VecDeque;
use std::ffi::{CString, OsString};
use std::fs::{File, OpenOptions, Permissions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path};

#[cfg(target_os = "linux")]
const ANCESTOR_ACCESS: libc::c_int = libc::O_PATH;
#[cfg(target_os = "macos")]
const ANCESTOR_ACCESS: libc::c_int = libc::O_SEARCH;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const ANCESTOR_ACCESS: libc::c_int = libc::O_RDONLY;
const DIRECTORY_FLAGS: libc::c_int = libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

pub(super) fn ensure(path: &Path) -> io::Result<()> {
    let absolute = std::path::absolute(path)?;
    let mut pending: VecDeque<_> = absolute
        .components()
        .map(|part| part.as_os_str().to_owned())
        .collect();
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(ANCESTOR_ACCESS | DIRECTORY_FLAGS)
        .open("/")?;
    let mut directory = root.try_clone()?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    #[cfg(target_os = "linux")]
    let unmapped_uid = unmapped_ancestor_uid();
    #[cfg(not(target_os = "linux"))]
    let unmapped_uid = None;
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        if part == "/" {
            directory = root.try_clone()?;
            validate(&directory, uid, unmapped_uid, false)?;
            continue;
        }
        if part == "." {
            continue;
        }
        let last = pending.is_empty();
        if last && part == ".." {
            return Err(io::Error::other("private directory must have a final name"));
        }
        let name = CString::new(part.as_bytes())?;
        let mut opened = open_directory(&directory, &name, last);
        if opened
            .as_ref()
            .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
        {
            // SAFETY: the directory descriptor and NUL-terminated name are valid.
            if unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
            opened = open_directory(&directory, &name, last);
        }
        let child = match opened {
            Ok(child) => child,
            Err(error) if !last => {
                // Trusted aliases (including macOS /tmp and /var) still traverse checked targets.
                let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
                // SAFETY: fstatat initializes metadata on success; the other pointers are valid.
                if unsafe {
                    libc::fstatat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        metadata.as_mut_ptr(),
                        libc::AT_SYMLINK_NOFOLLOW,
                    )
                } != 0
                {
                    return Err(error);
                }
                // SAFETY: fstatat succeeded.
                let metadata = unsafe { metadata.assume_init() };
                if metadata.st_mode & libc::S_IFMT != libc::S_IFLNK
                    || !trusted_ancestor_owner(metadata.st_uid, uid, unmapped_uid)
                {
                    return Err(error);
                }
                links += 1;
                if links > 40 {
                    return Err(io::Error::other("too many directory symlinks"));
                }
                let mut target = vec![0; libc::PATH_MAX as usize];
                // SAFETY: the name, directory and writable target buffer remain valid.
                let length = unsafe {
                    libc::readlinkat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        target.as_mut_ptr().cast(),
                        target.len(),
                    )
                };
                if length < 0 {
                    return Err(io::Error::last_os_error());
                }
                if length as usize == target.len() {
                    return Err(io::Error::other("directory symlink target is too long"));
                }
                target.truncate(length as usize);
                let target = OsString::from_vec(target);
                for component in Path::new(&target).components().rev() {
                    if component != Component::CurDir {
                        pending.push_front(component.as_os_str().to_owned());
                    }
                }
                continue;
            },
            Err(error) => return Err(error),
        };
        validate(&child, uid, unmapped_uid, last)?;
        if last {
            if child.metadata()?.mode() & 0o7777 != 0o700 {
                child.set_permissions(Permissions::from_mode(0o700))?;
            }
            return Ok(());
        }
        directory = child;
    }
    Err(io::Error::other("private directory must have a final name"))
}

fn open_directory(parent: &File, name: &CString, last: bool) -> io::Result<File> {
    // Only the final directory needs a descriptor that supports fchmod.
    let access = if last {
        libc::O_RDONLY
    } else {
        ANCESTOR_ACCESS
    };
    // SAFETY: parent remains open and name is NUL-terminated. Success transfers a new descriptor.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), access | DIRECTORY_FLAGS) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(target_os = "linux")]
fn unmapped_ancestor_uid() -> Option<u32> {
    let uid_map = std::fs::read_to_string("/proc/self/uid_map").ok()?;
    let overflow_uid = std::fs::read_to_string("/proc/sys/kernel/overflowuid").ok()?;
    namespace_overflow_uid(&uid_map, &overflow_uid)
}

#[cfg(target_os = "linux")]
fn namespace_overflow_uid(uid_map: &str, overflow_uid: &str) -> Option<u32> {
    let overflow_uid = overflow_uid.trim().parse::<u32>().ok()?;
    let mut remapped = false;
    for line in uid_map.lines() {
        let mut fields = line.split_whitespace();
        let inside = fields.next()?.parse::<u32>().ok()?;
        let outside = fields.next()?.parse::<u32>().ok()?;
        let count = fields.next()?.parse::<u32>().ok()?;
        let end = inside.checked_add(count)?;
        outside.checked_add(count)?;
        if fields.next().is_some() || count == 0 || (inside..end).contains(&overflow_uid) {
            return None;
        }
        remapped |= inside != outside;
    }
    remapped.then_some(overflow_uid)
}

fn trusted_ancestor_owner(owner: u32, uid: u32, unmapped_uid: Option<u32>) -> bool {
    // Unmapped ancestors rely on the sandbox boundary: their host owners are indistinguishable.
    owner == uid || owner == 0 || Some(owner) == unmapped_uid
}

fn validate(directory: &File, uid: u32, unmapped_uid: Option<u32>, last: bool) -> io::Result<()> {
    let metadata = directory.metadata()?;
    let trusted_owner = metadata.uid() == uid
        || (!last && trusted_ancestor_owner(metadata.uid(), uid, unmapped_uid));
    let sticky_ancestor = !last && metadata.mode() & 0o1000 != 0;
    if !trusted_owner || (metadata.mode() & 0o022 != 0 && !sticky_ancestor) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "directory must have a trusted owner and no group/other write access",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/secure_dir_tests.rs"]
mod tests;

//! A private configuration tree with one live, writable host pathname.
use fuser::*;
use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use yinz_runtime::errors::prelude::*;

const ROOT: INodeNo = INodeNo(1);
const CREDENTIAL: INodeNo = INodeNo(2);
const NAME: &str = ".credentials.json";
const TTL: Duration = Duration::ZERO;
type FsResult<T> = std::result::Result<T, Errno>;

pub(super) fn mount_lock() -> io::Result<File> {
    let path = PathBuf::from(format!("/tmp/.yinz-claude-mounts-{}.lock", unsafe {
        libc::getuid()
    }));
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = match options.open(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match options.write(true).create_new(true).mode(0o600).open(&path) {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    return mount_lock();
                },
                result => result?,
            }
        },
        result => result?,
    };
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 || meta.uid() != unsafe { libc::getuid() } {
        return Err(io::Error::other("unsafe Claude mount lock"));
    }
    Ok(file)
}

struct MountMutation(File);

impl MountMutation {
    fn acquire() -> io::Result<Self> {
        let file = mount_lock()?;
        file.lock()?;
        Ok(Self(file))
    }
}

impl Drop for MountMutation {
    fn drop(&mut self) {
        // A concurrent fork can retain the open description until its own exec.
        self.0
            .unlock()
            .context("unlock Claude mount mutation")
            .non_fatal();
    }
}

pub(super) fn guard_setup(command: &mut Command) -> Result<()> {
    let lock = mount_lock().context("open Claude mount lock")?;
    let path = std::ffi::CString::new(fd_path(&lock).as_os_str().as_bytes())
        .context("encode Claude mount lock path")?;
    let parent = std::process::id() as libc::pid_t;
    command.arg("--block-fd").arg(lock.as_raw_fd().to_string());
    // Bubblewrap closes this descriptor after mount setup, before executing the agent.
    unsafe {
        command.pre_exec(move || {
            // Cover parent death while waiting for the lock, before Bubblewrap starts.
            if libc::getpid() != parent {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(io::Error::other("sandbox parent exited"));
                }
            }
            let fd = libc::open(path.as_ptr(), libc::O_RDONLY);
            if fd == -1 {
                return Err(io::Error::last_os_error());
            }
            let result =
                if libc::flock(fd, libc::LOCK_SH) == -1 || libc::dup2(fd, lock.as_raw_fd()) == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                };
            libc::close(fd);
            result
        });
    }
    Ok(())
}

enum Node {
    Directory {
        lower: Option<File>,
        children: BTreeMap<OsString, Option<INodeNo>>,
    },
    File {
        file: File,
        private: bool,
        shared: bool,
    },
    Symlink(PathBuf),
}

struct Handle {
    inode: INodeNo,
    // Credentials retain the opened host inode even after another writer replaces it.
    credential: Option<File>,
    flags: i32,
}

struct Tree {
    host: File,
    nodes: HashMap<INodeNo, Node>,
    handles: HashMap<FileHandle, Handle>,
    directories: HashMap<FileHandle, Vec<(INodeNo, FileType, OsString)>>,
    next: u64,
}

fn directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
}

fn fd_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

fn regular(file: &File) -> FsResult<()> {
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() > 1 {
        return Err(Errno::EPERM);
    }
    Ok(())
}

fn valid_name(name: &OsStr) -> FsResult<()> {
    let mut parts = Path::new(name).components();
    if !matches!(parts.next(), Some(Component::Normal(_)))
        || parts.next().is_some()
        || name.as_bytes().contains(&0)
        || name.as_bytes().contains(&b'/')
    {
        return Err(Errno::EINVAL);
    }
    Ok(())
}

impl Tree {
    fn new(host: &Path, settings: &[u8]) -> FsResult<Self> {
        let host = directory(host)?;
        let mut tree = Self {
            nodes: HashMap::from([(
                ROOT,
                Node::Directory {
                    lower: Some(host.try_clone()?),
                    children: BTreeMap::new(),
                },
            )]),
            host,
            handles: HashMap::new(),
            directories: HashMap::new(),
            next: 3,
        };
        for name in [".claude.json", ".config.json"] {
            if name == ".config.json" {
                match fs::symlink_metadata(fd_path(&tree.host).join(name)) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        tree.children(ROOT)?.insert(name.into(), None);
                        continue;
                    },
                    Err(error) => return Err(error.into()),
                    Ok(_) => {},
                }
            }
            let mut file = tempfile::tempfile()?;
            file.write_all(settings)?;
            tree.insert(
                ROOT,
                OsStr::new(name),
                Node::File {
                    file,
                    private: true,
                    shared: false,
                },
            )?;
        }
        tree.insert(
            ROOT,
            OsStr::new("projects"),
            Node::Directory {
                lower: None,
                children: BTreeMap::new(),
            },
        )?;
        // Missing credentials are valid; unsafe existing credentials are not.
        match tree.credential(libc::O_RDONLY) {
            Ok(_) | Err(Errno::ENOENT) => {},
            Err(error) => return Err(error),
        }
        Ok(tree)
    }

    fn id(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    fn children(&mut self, parent: INodeNo) -> FsResult<&mut BTreeMap<OsString, Option<INodeNo>>> {
        match self.nodes.get_mut(&parent) {
            Some(Node::Directory { children, .. }) => Ok(children),
            _ => Err(Errno::ENOTDIR),
        }
    }

    fn insert(&mut self, parent: INodeNo, name: &OsStr, node: Node) -> FsResult<INodeNo> {
        valid_name(name)?;
        let ino = INodeNo(self.id());
        self.children(parent)?.insert(name.to_owned(), Some(ino));
        self.nodes.insert(ino, node);
        Ok(ino)
    }

    fn credential(&self, flags: i32) -> FsResult<File> {
        let file = OpenOptions::new()
            .read(flags & libc::O_ACCMODE != libc::O_WRONLY)
            .write(flags & libc::O_ACCMODE != libc::O_RDONLY)
            .append(flags & libc::O_APPEND != 0)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(fd_path(&self.host).join(NAME))?;
        regular(&file)?;
        Ok(file)
    }

    fn lookup(&mut self, parent: INodeNo, name: &OsStr) -> FsResult<INodeNo> {
        valid_name(name)?;
        if parent == ROOT && name == NAME {
            self.credential(libc::O_RDONLY)?;
            return Ok(CREDENTIAL);
        }
        let Some(Node::Directory { lower, children }) = self.nodes.get(&parent) else {
            return Err(Errno::ENOTDIR);
        };
        if let Some(ino) = children.get(name) {
            return ino.ok_or(Errno::ENOENT);
        }
        let path = fd_path(lower.as_ref().ok_or(Errno::ENOENT)?).join(name);
        let meta = fs::symlink_metadata(&path)?;
        let node = if meta.is_symlink() {
            Node::Symlink(fs::read_link(&path)?)
        } else if meta.is_dir() {
            Node::Directory {
                lower: Some(directory(&path)?),
                children: BTreeMap::new(),
            }
        } else if meta.is_file() {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)?;
            if !file.metadata()?.is_file() {
                return Err(Errno::EPERM);
            }
            Node::File {
                file,
                private: false,
                shared: false,
            }
        } else {
            return Err(Errno::EPERM);
        };
        self.insert(parent, name, node)
    }

    fn private_file(&mut self, ino: INodeNo) -> FsResult<&mut File> {
        let Some(Node::File { file, private, .. }) = self.nodes.get_mut(&ino) else {
            return Err(Errno::EINVAL);
        };
        if !*private {
            let mut copy = tempfile::tempfile()?;
            // Reopen so copying does not change an existing descriptor's offset.
            io::copy(&mut File::open(fd_path(file))?, &mut copy)?;
            copy.set_permissions(fs::Permissions::from_mode(file.metadata()?.mode() & 0o777))?;
            *file = copy;
            *private = true;
        }
        Ok(file)
    }

    fn attr(&self, ino: INodeNo, fh: Option<FileHandle>) -> FsResult<FileAttr> {
        let credential;
        let (kind, meta, size) = if ino == CREDENTIAL {
            let file = if let Some(file) = fh
                .and_then(|fh| self.handles.get(&fh))
                .and_then(|h| h.credential.as_ref())
            {
                file
            } else {
                credential = self.credential(libc::O_RDONLY)?;
                &credential
            };
            (FileType::RegularFile, Some(file.metadata()?), 0)
        } else {
            match self.nodes.get(&ino).ok_or(Errno::ENOENT)? {
                Node::File { file, .. } => (FileType::RegularFile, Some(file.metadata()?), 0),
                Node::Directory { .. } => (FileType::Directory, None, 0),
                Node::Symlink(target) => (FileType::Symlink, None, target.as_os_str().len() as u64),
            }
        };
        Ok(FileAttr {
            ino,
            kind,
            size: meta.as_ref().map_or(size, |m| m.len()),
            blocks: meta.as_ref().map_or(0, |m| m.blocks()),
            atime: meta
                .as_ref()
                .and_then(|m| m.accessed().ok())
                .unwrap_or(SystemTime::UNIX_EPOCH),
            mtime: meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .unwrap_or(SystemTime::UNIX_EPOCH),
            ctime: SystemTime::UNIX_EPOCH,
            crtime: SystemTime::UNIX_EPOCH,
            perm: if ino == CREDENTIAL {
                0o600
            } else {
                meta.as_ref().map_or(0o700, |m| (m.mode() & 0o777) as u16)
            },
            nlink: 1,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 4096,
            flags: 0,
        })
    }

    fn open(&mut self, ino: INodeNo, flags: i32) -> FsResult<FileHandle> {
        let credential = if ino == CREDENTIAL {
            Some(self.credential(flags)?)
        } else {
            if flags & libc::O_ACCMODE != libc::O_RDONLY {
                self.private_file(ino)?;
            }
            if !matches!(self.nodes.get(&ino), Some(Node::File { .. })) {
                return Err(Errno::EINVAL);
            }
            None
        };
        let fh = FileHandle(self.id());
        self.handles.insert(
            fh,
            Handle {
                inode: ino,
                credential,
                flags,
            },
        );
        if flags & libc::O_TRUNC != 0 {
            self.file(fh, true)?.set_len(0)?;
        }
        Ok(fh)
    }

    fn file(&self, fh: FileHandle, writing: bool) -> FsResult<&File> {
        let handle = self.handles.get(&fh).ok_or(Errno::EBADF)?;
        if (writing && handle.flags & libc::O_ACCMODE == libc::O_RDONLY)
            || (!writing && handle.flags & libc::O_ACCMODE == libc::O_WRONLY)
        {
            return Err(Errno::EBADF);
        }
        let file = match &handle.credential {
            Some(file) => file,
            None => match self.nodes.get(&handle.inode) {
                Some(Node::File { file, .. }) => file,
                _ => return Err(Errno::EBADF),
            },
        };
        if writing {
            regular(file)?;
        }
        Ok(file)
    }

    fn create(
        &mut self,
        parent: INodeNo,
        name: &OsStr,
        flags: i32,
        mode: u32,
    ) -> FsResult<(INodeNo, FileHandle)> {
        valid_name(name)?;
        match self.lookup(parent, name) {
            Ok(_) => return Err(Errno::EEXIST),
            Err(Errno::ENOENT) => {},
            Err(error) => return Err(error),
        }
        let ino = if parent == ROOT && name == NAME {
            let stage = tempfile::NamedTempFile::new_in(fd_path(&self.host))?;
            stage
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
            stage
                .persist_noclobber(fd_path(&self.host).join(NAME))
                .map_err(|e| Errno::from(e.error))?;
            CREDENTIAL
        } else {
            let file = tempfile::tempfile()?;
            file.set_permissions(fs::Permissions::from_mode(mode & 0o777))?;
            self.insert(
                parent,
                name,
                Node::File {
                    file,
                    private: true,
                    shared: false,
                },
            )?
        };
        Ok((ino, self.open(ino, flags)?))
    }

    fn entries(&mut self, ino: INodeNo) -> FsResult<Vec<(INodeNo, FileType, OsString)>> {
        let Some(Node::Directory { lower, children }) = self.nodes.get(&ino) else {
            return Err(Errno::ENOTDIR);
        };
        let mut names: std::collections::BTreeSet<_> = children.keys().cloned().collect();
        if let Some(lower) = lower {
            for entry in fs::read_dir(fd_path(lower))? {
                names.insert(entry?.file_name());
            }
        }
        if ino == ROOT {
            names.insert(NAME.into());
        }
        let mut entries = vec![
            (ino, FileType::Directory, ".".into()),
            (ROOT, FileType::Directory, "..".into()),
        ];
        for name in names {
            match self.lookup(ino, &name) {
                Ok(child) => entries.push((child, self.attr(child, None)?.kind, name)),
                Err(Errno::ENOENT | Errno::EPERM | Errno::ELOOP) => {},
                Err(error) => return Err(error),
            }
        }
        Ok(entries)
    }

    fn remove(&mut self, parent: INodeNo, name: &OsStr, directory: bool) -> FsResult<()> {
        let ino = self.lookup(parent, name)?;
        let is_dir = matches!(self.nodes.get(&ino), Some(Node::Directory { .. }));
        if is_dir != directory {
            return Err(if is_dir {
                Errno::EISDIR
            } else {
                Errno::ENOTDIR
            });
        }
        if directory && self.entries(ino)?.len() > 2 {
            return Err(Errno::ENOTEMPTY);
        }
        if ino == CREDENTIAL {
            fs::remove_file(fd_path(&self.host).join(NAME))?;
        } else {
            self.children(parent)?.insert(name.to_owned(), None);
        }
        Ok(())
    }

    fn rename(
        &mut self,
        parent: INodeNo,
        name: &OsStr,
        dest: INodeNo,
        newname: &OsStr,
        flags: RenameFlags,
    ) -> FsResult<()> {
        valid_name(newname)?;
        if !flags.is_empty() {
            return Err(Errno::EINVAL);
        }
        let ino = self.lookup(parent, name)?;
        if parent == dest && name == newname {
            return Ok(());
        }
        if ino == CREDENTIAL {
            return Err(Errno::EPERM);
        }
        if dest == ROOT && newname == NAME {
            match self.credential(libc::O_RDONLY) {
                Ok(_) | Err(Errno::ENOENT) => {},
                Err(error) => return Err(error),
            }
            let source = self.private_file(ino)?;
            regular(source)?;
            let mut stage = tempfile::NamedTempFile::new_in(fd_path(&self.host))?;
            stage
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
            let source = match self.nodes.get(&ino) {
                Some(Node::File { file, .. }) => file,
                _ => return Err(Errno::EINVAL),
            };
            io::copy(&mut File::open(fd_path(source))?, stage.as_file_mut())?;
            stage.as_file().sync_all()?;
            let file = stage
                .persist(fd_path(&self.host).join(NAME))
                .map_err(|e| Errno::from(e.error))?;
            // Descriptors opened on the staging file now refer to the published host inode.
            self.nodes.insert(
                ino,
                Node::File {
                    file,
                    private: true,
                    shared: true,
                },
            );
        } else {
            let kind = self.attr(ino, None)?.kind;
            if kind == FileType::Directory && self.contains_directory(ino, dest) {
                return Err(Errno::EINVAL);
            }
            match self.lookup(dest, newname) {
                Ok(previous) => {
                    let previous_kind = self.attr(previous, None)?.kind;
                    if (kind == FileType::Directory) != (previous_kind == FileType::Directory) {
                        return Err(if previous_kind == FileType::Directory {
                            Errno::EISDIR
                        } else {
                            Errno::ENOTDIR
                        });
                    }
                    if kind == FileType::Directory && self.entries(previous)?.len() > 2 {
                        return Err(Errno::ENOTEMPTY);
                    }
                },
                Err(Errno::ENOENT) => {},
                Err(error) => return Err(error),
            }
            self.children(dest)?.insert(newname.to_owned(), Some(ino));
        }
        self.children(parent)?.insert(name.to_owned(), None);
        Ok(())
    }

    fn contains_directory(&self, parent: INodeNo, candidate: INodeNo) -> bool {
        parent == candidate
            || match self.nodes.get(&parent) {
                Some(Node::Directory { children, .. }) => children
                    .values()
                    .flatten()
                    .any(|child| self.contains_directory(*child, candidate)),
                _ => false,
            }
    }
}

struct ConfigFs(Mutex<Tree>);

impl ConfigFs {
    fn with<T>(&self, operation: impl FnOnce(&mut Tree) -> FsResult<T>) -> FsResult<T> {
        operation(&mut *self.0.lock().map_err(|_| Errno::EIO)?)
    }
}

impl Filesystem for ConfigFs {
    fn lookup(&self, _: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        match self.with(|tree| {
            let ino = tree.lookup(parent, name)?;
            tree.attr(ino, None)
        }) {
            Ok(attr) => reply.entry(&TTL, &attr, Generation(0)),
            Err(error) => reply.error(error),
        }
    }
    fn getattr(&self, _: &Request, ino: INodeNo, fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.with(|tree| tree.attr(ino, fh)) {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(error) => reply.error(error),
        }
    }
    fn readlink(&self, _: &Request, ino: INodeNo, reply: ReplyData) {
        match self.with(|tree| match tree.nodes.get(&ino) {
            Some(Node::Symlink(target)) => Ok(target.as_os_str().as_bytes().to_vec()),
            _ => Err(Errno::EINVAL),
        }) {
            Ok(data) => reply.data(&data),
            Err(error) => reply.error(error),
        }
    }
    fn open(&self, _: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        match self.with(|tree| tree.open(ino, flags.0)) {
            Ok(fh) => reply.opened(fh, FopenFlags::FOPEN_DIRECT_IO),
            Err(error) => reply.error(error),
        }
    }
    fn create(
        &self,
        _: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        match self.with(|tree| {
            let (ino, fh) = tree.create(parent, name, flags, mode & !umask)?;
            Ok((tree.attr(ino, Some(fh))?, fh))
        }) {
            Ok((attr, fh)) => {
                reply.created(&TTL, &attr, Generation(0), fh, FopenFlags::FOPEN_DIRECT_IO)
            },
            Err(error) => reply.error(error),
        }
    }
    fn read(
        &self,
        _: &Request,
        _: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _: OpenFlags,
        _: Option<LockOwner>,
        reply: ReplyData,
    ) {
        match self.with(|tree| {
            let mut data = vec![0; size as usize];
            let len = tree.file(fh, false)?.read_at(&mut data, offset)?;
            data.truncate(len);
            Ok(data)
        }) {
            Ok(data) => reply.data(&data),
            Err(error) => reply.error(error),
        }
    }
    fn write(
        &self,
        _: &Request,
        _: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _: WriteFlags,
        _: OpenFlags,
        _: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        match self.with(|tree| {
            let file = tree.file(fh, true)?;
            if tree.handles[&fh].flags & libc::O_APPEND != 0 {
                OpenOptions::new()
                    .append(true)
                    .open(fd_path(file))?
                    .write_all(data)?;
            } else {
                file.write_all_at(data, offset)?;
            }
            Ok(data.len() as u32)
        }) {
            Ok(len) => reply.written(len),
            Err(error) => reply.error(error),
        }
    }
    fn flush(&self, _: &Request, _: INodeNo, _: FileHandle, _: LockOwner, reply: ReplyEmpty) {
        reply.ok();
    }
    fn fsync(&self, _: &Request, _: INodeNo, fh: FileHandle, _: bool, reply: ReplyEmpty) {
        match self.with(|tree| {
            let handle = tree.handles.get(&fh).ok_or(Errno::EBADF)?;
            tree.file(fh, handle.flags & libc::O_ACCMODE != libc::O_RDONLY)?
                .sync_all()?;
            Ok(())
        }) {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }
    fn release(
        &self,
        _: &Request,
        _: INodeNo,
        fh: FileHandle,
        _: OpenFlags,
        _: Option<LockOwner>,
        _: bool,
        reply: ReplyEmpty,
    ) {
        match self.with(|tree| {
            tree.handles.remove(&fh);
            Ok(())
        }) {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }
    fn setattr(
        &self,
        _: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _: Option<SystemTime>,
        fh: Option<FileHandle>,
        _: Option<SystemTime>,
        _: Option<SystemTime>,
        _: Option<SystemTime>,
        flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        match self.with(|tree| {
            if uid.is_some_and(|uid| uid != unsafe { libc::getuid() })
                || gid.is_some_and(|gid| gid != unsafe { libc::getgid() })
                || flags.is_some()
            {
                return Err(Errno::EPERM);
            }
            let shared = ino == CREDENTIAL
                || matches!(tree.nodes.get(&ino), Some(Node::File { shared: true, .. }));
            let credential;
            let file = if let Some(fh) = fh {
                tree.file(fh, true)?
            } else if ino == CREDENTIAL {
                credential = tree.credential(libc::O_RDWR)?;
                &credential
            } else if matches!(tree.nodes.get(&ino), Some(Node::Directory { .. })) {
                return tree.attr(ino, None);
            } else {
                tree.private_file(ino)?
            };
            regular(file)?;
            if let Some(size) = size {
                file.set_len(size)?;
            }
            if let Some(mode) = mode {
                file.set_permissions(fs::Permissions::from_mode(if shared {
                    0o600
                } else {
                    mode & 0o777
                }))?;
            }
            let mut times = fs::FileTimes::new();
            let time = |t| match t {
                TimeOrNow::Now => SystemTime::now(),
                TimeOrNow::SpecificTime(time) => time,
            };
            if let Some(atime) = atime {
                times = times.set_accessed(time(atime));
            }
            if let Some(mtime) = mtime {
                times = times.set_modified(time(mtime));
            }
            file.set_times(times)?;
            tree.attr(ino, fh)
        }) {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(error) => reply.error(error),
        }
    }
    fn mkdir(&self, _: &Request, parent: INodeNo, name: &OsStr, _: u32, _: u32, reply: ReplyEntry) {
        match self.with(|tree| {
            if parent == ROOT && name == NAME {
                return Err(Errno::EPERM);
            }
            match tree.lookup(parent, name) {
                Err(Errno::ENOENT) => {},
                Ok(_) => return Err(Errno::EEXIST),
                Err(error) => return Err(error),
            }
            let ino = tree.insert(
                parent,
                name,
                Node::Directory {
                    lower: None,
                    children: BTreeMap::new(),
                },
            )?;
            tree.attr(ino, None)
        }) {
            Ok(attr) => reply.entry(&TTL, &attr, Generation(0)),
            Err(error) => reply.error(error),
        }
    }
    fn unlink(&self, _: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        match self.with(|tree| tree.remove(parent, name, false)) {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }
    fn rmdir(&self, _: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        match self.with(|tree| tree.remove(parent, name, true)) {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }
    fn rename(
        &self,
        _: &Request,
        parent: INodeNo,
        name: &OsStr,
        dest: INodeNo,
        newname: &OsStr,
        flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        match self.with(|tree| tree.rename(parent, name, dest, newname, flags)) {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }
    fn opendir(&self, _: &Request, ino: INodeNo, _: OpenFlags, reply: ReplyOpen) {
        match self.with(|tree| {
            let entries = tree.entries(ino)?;
            let fh = FileHandle(tree.id());
            tree.directories.insert(fh, entries);
            Ok(fh)
        }) {
            Ok(fh) => reply.opened(fh, FopenFlags::empty()),
            Err(error) => reply.error(error),
        }
    }
    fn readdir(
        &self,
        _: &Request,
        _: INodeNo,
        fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let result = self.with(|tree| {
            let entries = tree.directories.get(&fh).ok_or(Errno::EBADF)?;
            for (index, (ino, kind, name)) in entries.iter().enumerate().skip(offset as usize) {
                if reply.add(*ino, (index + 1) as u64, *kind, name) {
                    break;
                }
            }
            Ok(())
        });
        match result {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }
    fn releasedir(&self, _: &Request, _: INodeNo, fh: FileHandle, _: OpenFlags, reply: ReplyEmpty) {
        match self.with(|tree| {
            tree.directories.remove(&fh);
            Ok(())
        }) {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }
}

pub(super) struct View {
    // Unmount before removing the directory, including on a failed sandbox spawn.
    session: Option<BackgroundSession>,
    directory: tempfile::TempDir,
    cleanup: Child,
}

impl View {
    pub(super) fn mount(host: &Path, settings: &[u8]) -> Result<Self> {
        let tree = Tree::new(host, settings)
            .map_err(|error| io::Error::from_raw_os_error(error.into()))
            .context("prepare Claude configuration view")
            .context(super::MotherSetupError)?;
        if !Command::new("flock")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Claude mount cleanup requires flock from util-linux")?
            .success()
        {
            bail!("Claude mount cleanup requires flock from util-linux");
        }
        let lock = MountMutation::acquire().context("lock Claude mount creation")?;
        let directory = tempfile::Builder::new()
            .prefix("yinz-claude-")
            .tempdir()
            .context("create Claude mountpoint")?;
        let mut config = Config::default();
        config.mount_options = vec![
            MountOption::FSName("yinz-claude".into()),
            MountOption::NoSuid,
            MountOption::NoDev,
        ];
        let session = fuser::spawn_mount(ConfigFs(Mutex::new(tree)), directory.path(), &config)
            .context("mount Claude configuration view; /dev/fuse and fusermount3 are required")?;
        // A separate process group survives a killed launcher without granting allow_other.
        let cleanup = Command::new("/bin/sh")
            .args([
                "-c",
                "if ! IFS= read -r finished; then exec flock -x \"$2\" sh -c 'fusermount3 -u -z -- \"$1\"; rmdir -- \"$1\"' sh \"$1\"; fi",
                "sh",
            ])
            .arg(directory.path())
            .arg(fs::read_link(fd_path(&lock.0)).context("resolve Claude mount lock")?)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .context("watch Claude mount lifetime")?;
        Ok(Self {
            session: Some(session),
            directory,
            cleanup,
        })
    }
    pub(super) fn path(&self) -> &Path {
        self.directory.path()
    }
    pub(super) fn finished(&self) -> bool {
        self.session.as_ref().is_none_or(|s| s.guard.is_finished())
    }
}

impl Drop for View {
    fn drop(&mut self) {
        // Recursive root binds must not overlap a host mount disappearing.
        let lock = MountMutation::acquire();
        lock.as_ref()
            .map(|_| ())
            .map_err(|error| anyhow!("{error}"))
            .context("lock Claude mount removal")
            .non_fatal();
        if let Some(session) = self.session.take() {
            session
                .umount_and_join()
                .context("unmount Claude configuration view")
                .non_fatal();
        }
        if let Some(mut lease) = self.cleanup.stdin.take() {
            lease
                .write_all(b"finished\n")
                .context("release Claude mount watcher")
                .non_fatal();
        }
        self.cleanup
            .wait()
            .context("reap Claude mount watcher")
            .non_fatal();
    }
}

#[cfg(test)]
#[path = "unit/claude_view.rs"]
mod tests;

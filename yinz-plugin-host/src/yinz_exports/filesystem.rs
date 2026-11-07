//! Plugin filesystem mounts, path confinement, and file operations.

use super::permissions::has_permission;
use super::{write_response, PLUGINS_FS_ROOT};
use crate::plugin_map::PluginEnv;
use crate::PluginInstruction;
use anyhow::ensure;
use prost::Message;
use std::collections::{HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::{fs, thread};
use yinz_plugin_api::plugin_command::{
    fs_list_response, fs_metadata_response, fs_mutate_response, fs_read_response, ProtobufFsEntry,
    ProtobufFsEntryList, ProtobufFsListResponse, ProtobufFsMetadataResponse,
    ProtobufFsMutateResponse, ProtobufFsReadResponse,
};
use yinz_runtime::consts::{
    YINZ_CONFIG_DIR_ENV, YINZ_CONFIG_FILE_ENV, YINZ_PLUGIN_PERMISSIONS_CACHE, YINZ_PLUGIN_TMP_DIR,
    YINZ_TMP_LOG_DIR,
};
use yinz_runtime::errors::prelude::*;
use yinz_runtime::home::default_config_dirs;
use yinz_types::data::{Event, FileMetadata, PermissionType};

/// The maximum number of bytes an `FsRead` will return, whatever the plugin asks for.
///
/// These commands answer inline on the plugin thread, so a read is a pause for every plugin
/// pinned to that thread. Larger files are read in ranges so one call cannot buffer the entire
/// file or monopolize the thread with an unbounded read.
const MAX_FS_READ_BYTES: usize = 8 * 1024 * 1024;

/// Where a plugin-visible path actually lives, and whether it left the plugin's own folders.
struct ResolvedPath {
    real: PathBuf,
    /// True when the path named no virtual root and so points somewhere in the wider filesystem.
    /// Reaching one of these needs `FullHdAccess` on top of the read/write permission.
    escaped_roots: bool,
}

/// The folders a plugin may name, by their virtual roots. `/plugins` is the user's plugin folder
/// and is absent when there is none, so nothing can be written into a folder nobody chose.
fn fs_roots(env: &PluginEnv) -> Vec<(&'static str, PathBuf)> {
    let mut roots = vec![
        ("/host", env.plugin_cwd.clone()),
        ("/data", env.plugin_own_data_dir.clone()),
        ("/cache", env.plugin_own_cache_dir.clone()),
        ("/tmp", YINZ_PLUGIN_TMP_DIR.to_path_buf()),
    ];
    if let Some(user_plugin_dir) = env.user_plugin_dir.as_ref() {
        roots.push((PLUGINS_FS_ROOT, user_plugin_dir.clone()));
    }
    roots
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FsPathResolution {
    FollowSymlink,
    PreserveEntry,
}

/// Resolve existing ancestors, retaining missing components for paths that may be created later.
/// Roots need the same resolution as requested paths when a symlink precedes a missing folder.
pub(super) fn resolve_fs_ancestors(path: &Path) -> Result<PathBuf> {
    let mut existing = path;
    // Collected outermost-last, then re-attached in order. Joining onto an empty `PathBuf` as we
    // go would leave a trailing separator, which asks the OS for a directory.
    let mut tail = vec![];
    loop {
        match existing.canonicalize() {
            Ok(canonical) => {
                return Ok(tail
                    .iter()
                    .rev()
                    .fold(canonical, |path: PathBuf, name| path.join(name)));
            },
            Err(error) => {
                // A dangling link is an existing entry, not a missing path component.
                // Reattaching it unchecked would let a subsequent create follow it outside
                // the root. Entry operations resolve only the parent to preserve the link.
                ensure!(
                    !existing
                        .symlink_metadata()
                        .is_ok_and(|metadata| metadata.file_type().is_symlink()),
                    "'{}' contains an unresolved symlink; refusing to follow it",
                    path.display()
                );
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(error)
                        .with_context(|| format!("failed to resolve '{}'", path.display()));
                }
                match (existing.parent(), existing.file_name()) {
                    (Some(parent), Some(name)) => {
                        tail.push(name.to_os_string());
                        existing = parent;
                    },
                    _ => {
                        return Err(anyhow!(
                            "'{}' could not be resolved to a real path",
                            path.display()
                        ))
                    },
                }
            },
        }
    }
}

/// Resolve a plugin-visible path to a real one, refusing to leave the root it named.
///
/// This is deliberately *not* [`super::pane_creation::translate_plugin_path`], which the pane-opening commands use.
/// That one maps the four prefixes and stops: `/host/../../etc/passwd` resolves straight out of
/// `/host`, and any genuine absolute path beginning `/tmp` is silently rewritten into the
/// plugins' temp dir. Both are tolerable when the result is handed to an editor the user is
/// watching, and neither is tolerable when the result is read or written on the plugin's say-so.
///
/// So this follows [`scan_host_folder`] instead: canonicalize the root and the target (or its
/// parent for operations on the directory entry), and refuse anything that does not land inside.
/// Canonicalizing catches symlinks pointing out of the root along the path being accessed.
///
/// A path naming no virtual root is resolved against the real filesystem and marked
/// `escaped_roots`, so that the caller can ask for `FullHdAccess`.
fn resolve_plugin_fs_path(
    env: &PluginEnv,
    path: &Path,
    resolution: FsPathResolution,
) -> Result<ResolvedPath> {
    let roots = fs_roots(env);

    let (prefix, root, relative) = match roots.iter().find_map(|(prefix, root)| {
        path.strip_prefix(prefix)
            .ok()
            .map(|rest| (*prefix, root, rest))
    }) {
        Some((prefix, root, rest)) => (Some(prefix), root.clone(), rest.to_path_buf()),
        // A relative path is the plugin's cwd, the same reading `translate_plugin_path` gives it.
        None if path.is_relative() => (Some("/host"), env.plugin_cwd.clone(), path.to_path_buf()),
        // FullHdAccess paths still resolve links before O_NOFOLLOW opens their final entry.
        // They use the real filesystem root, with the permission check applied below.
        None => (None, PathBuf::from("/"), path.to_path_buf()),
    };

    let canonical_root = resolve_fs_ancestors(&root)?;
    let joined: PathBuf = canonical_root.join(&relative).components().collect();

    let resolved = match (resolution, relative.file_name(), joined.parent()) {
        // Remove, rename and symlink metadata operate on the entry, not its target. Resolve
        // its parent as usual. A virtual root (no relative file name) still resolves in full.
        (FsPathResolution::PreserveEntry, Some(name), Some(parent)) => {
            resolve_fs_ancestors(parent)?.join(name)
        },
        _ => resolve_fs_ancestors(&joined)?,
    };

    if !resolved.starts_with(&canonical_root) {
        // Name the link when there is one: a plugin that wrote `link.scm` and is refused
        // should hear that it is a link, not merely that it points somewhere.
        let through_a_symlink = joined
            .ancestors()
            .take_while(|ancestor| ancestor.starts_with(&canonical_root))
            .any(|ancestor| {
                ancestor
                    .symlink_metadata()
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
            });
        if through_a_symlink {
            return Err(anyhow!(
                "'{}' is a symlink pointing outside the plugin's own folders; refusing",
                path.display()
            ));
        }
        return Err(anyhow!(
            "'{}' points outside the plugin's own folders",
            path.display()
        ));
    }

    Ok(ResolvedPath {
        real: resolved,
        escaped_roots: prefix.is_none(),
    })
}

/// What an `Fs*` command is about to do with a path, for the rules that depend on it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FsIntent {
    Read,
    Write,
}

/// Resolve a path for one of the `Fs*` commands, applying the rules `command_permission`
/// cannot: `FullHdAccess` for a path outside every root, and `AuthorPlugins` for a write under
/// `/plugins`.
///
/// Checked here rather than in `command_permission` because that only sees the command, and
/// where a path lands is not knowable without resolving it first. The data store does the same
/// thing for `shared:` namespaces in `resolve_data_namespace`.
pub(super) fn resolve_fs_path_with_permission(
    env: &PluginEnv,
    path: &Path,
    intent: FsIntent,
    resolution: FsPathResolution,
) -> Result<PathBuf> {
    let resolved = resolve_plugin_fs_path(env, path, resolution)?;
    if resolved.escaped_roots && !has_permission(env, PermissionType::FullHdAccess) {
        return Err(anyhow!(
            "'{}' is outside the plugin's own folders (/host, /data, /cache, /tmp, /plugins), \
             which needs the 'FullHdAccess' permission",
            path.display()
        ));
    }
    if intent == FsIntent::Write
        && env.user_plugin_dir.is_some()
        && !has_permission(env, PermissionType::AuthorPlugins)
    {
        let plugin_dir = resolve_plugin_fs_path(
            env,
            Path::new(PLUGINS_FS_ROOT),
            FsPathResolution::FollowSymlink,
        )?
        .real;
        // Authority follows the physical directory, including aliases through /host and
        // absolute paths. Removing or renaming an ancestor also changes the plugin folder.
        if reaches_folder(&resolved.real, &plugin_dir, intent, resolution) {
            return Err(anyhow!(
                "'{}' changes the plugin folder, which needs the 'AuthorPlugins' permission",
                path.display()
            ));
        }
    }
    // The server log and `--debug` pane dumps are what `ReadPluginLog` guards with
    // `AuthorPlugins`. No root maps onto them, but `/host` or an absolute path can still reach.
    if !has_permission(env, PermissionType::AuthorPlugins) {
        let log_dir =
            resolve_fs_ancestors(&YINZ_TMP_LOG_DIR).unwrap_or_else(|_| YINZ_TMP_LOG_DIR.clone());
        if reaches_folder(&resolved.real, &log_dir, intent, resolution) {
            return Err(anyhow!(
                "'{}' reaches the yinz log folder, which needs the 'AuthorPlugins' permission",
                path.display()
            ));
        }
    }
    if intent == FsIntent::Write {
        refuse_protected_host_state(env, path, &resolved.real, resolution)?;
    }
    // Removing or renaming a root, or a folder holding one, would take every later command's
    // folder with it, and renaming onto an empty root would replace it.
    if intent == FsIntent::Write && resolution == FsPathResolution::PreserveEntry {
        for (prefix, root) in fs_roots(env) {
            if resolve_fs_ancestors(&root).is_ok_and(|root| root.starts_with(&resolved.real)) {
                return Err(anyhow!(
                    "'{}' is or holds the plugin's {prefix} folder, which cannot be removed or \
                     replaced",
                    path.display()
                ));
            }
        }
    }
    Ok(resolved.real)
}

/// Whether an access to `real` reaches into `folder`: a path inside it, or, for the entry
/// writes (remove, rename), an ancestor whose removal or move takes `folder` along.
fn reaches_folder(
    real: &Path,
    folder: &Path,
    intent: FsIntent,
    resolution: FsPathResolution,
) -> bool {
    real.starts_with(folder)
        || (intent == FsIntent::Write
            && resolution == FsPathResolution::PreserveEntry
            && folder.starts_with(real))
}

/// Refuse a write that reaches the host state through which a plugin could grant itself more
/// than it was given, whatever permissions it holds.
///
/// `/host` is often `$HOME`, and an absolute path is one `FullHdAccess` grant away, so both reach:
/// - the permission cache, its lock and the folder it is saved through. `request_permission`
///   grants whatever the cache lists without a prompt, so writing it turns `WriteFiles` into
///   every other permission.
/// - the session's config folder and file, and its layout folder. Keybindings and layouts run
///   commands, and editing them is what `Reconfigure` and the layout commands guard.
/// - the data folder: its installed builtins run as builtins, and `plugin-data.db` is the
///   shared plugin database that `PluginData` and `SharedPluginData` guard.
///
/// Refused outright rather than behind a stronger permission: no plugin edits these through
/// the `Fs*` commands, and each has its own guarded command. The plugin's own folders are
/// exempt when they sit inside a protected folder; the default user plugin folder is inside
/// the config folder, and writes there are already guarded by `AuthorPlugins`.
///
/// # Identity, not spelling
///
/// The decision compares *file identities* (device and inode), not path strings. A path
/// string is only one of many names the kernel accepts for a file: symlinked folders and
/// chains of them, case-insensitive lookups (the default on macOS APFS), Unicode
/// normalization (APFS, HFS+), hard links and bind mounts all reach the same file under a
/// spelling no list of protected spellings contains. So [`ProtectedState::gather`] walks
/// every protected path the way a reader opens it and records the identity of each entry it
/// meets, and the target's existing entries are compared against those identities:
/// - a write or create (`FollowSymlink`) is refused when the target, or any folder above it,
///   *is* a protected folder or file: it would land inside protected state.
/// - a remove or rename end (`PreserveEntry`) is refused when the entry itself is one met on
///   the way to protected state, link or folder: moving it swaps everything beneath it. Its
///   parent folder is checked as a write is, so entries inside protected folders stay put.
///
/// Spellings remain only for what does not exist yet, where there is no identity to compare:
/// a missing protected path is recorded as its deepest existing folder's identity plus the
/// names below it, and a target is matched by its own deepest existing folder's identity plus
/// its missing names, compared case-insensitively (see [`fold_name`]).
///
/// This is check-then-act, like the rest of [`resolve_fs_path_with_permission`]; the opens
/// that follow use `O_NOFOLLOW` on the final entry, and nothing here widens that window.
fn refuse_protected_host_state(
    env: &PluginEnv,
    path: &Path,
    real: &Path,
    resolution: FsPathResolution,
) -> Result<()> {
    let refusal = || {
        anyhow!(
            "'{}' is yinz's own configuration, permission or data state, which plugins cannot \
             change",
            path.display()
        )
    };
    let protected = ProtectedState::gather(env);

    // Where the target would land: its deepest existing folder, and the names below that
    // folder which do not exist yet. A remove or rename acts on the entry, so the place it
    // lands is its parent; the entry itself is compared by its own identity.
    let (landing, moves_onto_missing) = match (resolution, real.parent(), real.file_name()) {
        (FsPathResolution::PreserveEntry, Some(parent), Some(name)) => {
            let (folder, mut missing) = deepest_existing(parent);
            match fs::symlink_metadata(real) {
                Ok(metadata) => {
                    if protected.on_the_way.contains(&file_id(&metadata)) {
                        return Err(refusal());
                    }
                    ((folder, missing), false)
                },
                Err(_) => {
                    // On a case-insensitive folder a differently spelled entry would already
                    // exist and be caught by identity above. A missing entry spelled like one
                    // on the way to protected state is refused all the same, so a rename
                    // cannot put something where a reader looks.
                    if missing.is_empty()
                        && let Some(folder) = folder
                        && let Ok(metadata) = fs::metadata(folder)
                        && protected
                            .names
                            .contains(&(file_id(&metadata), fold_name(name)))
                    {
                        return Err(refusal());
                    }
                    missing.push(name);
                    ((folder, missing), true)
                },
            }
        },
        _ => (deepest_existing(real), false),
    };
    let (Some(folder), missing) = landing else {
        return Ok(());
    };

    // The plugin's own folders are exempt when one sits strictly inside a protected folder,
    // as the plugin's `/cache` sits inside the cache folder, and the target is inside it. Both
    // sides are canonical spellings from `resolve_fs_ancestors`, so a mismatch can
    // only withhold the exemption, never grant it to a folder outside the plugin's own.
    let own_folders: Vec<PathBuf> = fs_roots(env)
        .into_iter()
        .filter(|(prefix, _)| *prefix != "/host")
        .filter_map(|(_, root)| resolve_fs_ancestors(&root).ok())
        .collect();
    let in_own_folder = |protected_folder: &Path| {
        own_folders.iter().any(|own| {
            own != protected_folder && own.starts_with(protected_folder) && real.starts_with(own)
        })
    };

    // Existing folders, from the deepest up: any of them being protected state means the
    // target lands inside it.
    for ancestor in folder.ancestors() {
        let Ok(metadata) = fs::metadata(ancestor) else {
            continue;
        };
        if protected.locations.contains(&file_id(&metadata)) && !in_own_folder(ancestor) {
            return Err(refusal());
        }
    }

    // Protected paths that do not exist yet, matched below the same existing folder.
    let Ok(folder_id) = fs::metadata(folder).map(|metadata| file_id(&metadata)) else {
        return Ok(());
    };
    let folded: Vec<String> = missing.iter().map(|name| fold_name(name)).collect();
    for (protected_folder, names) in &protected.missing {
        if *protected_folder != folder_id {
            continue;
        }
        // At or below the missing protected path: writing there creates it.
        if folded.starts_with(names) {
            let location = missing[..names.len()]
                .iter()
                .fold(folder.to_path_buf(), |path, name| path.join(name));
            if !in_own_folder(&location) {
                return Err(refusal());
            }
        }
        // Moving an entry onto a missing folder above it installs the protected path whole.
        if moves_onto_missing && names.starts_with(&folded) {
            return Err(refusal());
        }
    }
    Ok(())
}

/// The folders and files of [`refuse_protected_host_state`] other than the permission cache.
///
/// Every conventional config folder is listed, not only the one that exists: creating a
/// missing one would make it the config folder of the next session. The session's own config
/// folder and file come from its environment, where the server records its `--config-dir` and
/// `--config` paths for plugins.
fn protected_host_folders(env: &PluginEnv) -> Vec<PathBuf> {
    let session_config = [YINZ_CONFIG_DIR_ENV, YINZ_CONFIG_FILE_ENV]
        .into_iter()
        .filter_map(|key| env.session_env_vars.get(key))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    session_config
        .chain(default_config_dirs().into_iter().flatten())
        .chain(env.layout_dir.clone())
        .chain(Some(env.yinz_data_dir.clone()))
        .filter(|folder| !folder.as_os_str().is_empty())
        .collect()
}

/// A file's identity: the device and inode the kernel reaches, however the path is spelled.
type FileId = (u64, u64);

fn file_id(metadata: &fs::Metadata) -> FileId {
    (metadata.dev(), metadata.ino())
}

/// A name folded for comparing entries that do not exist yet, where no identity is available.
///
/// Lowercasing approximates the case folding of case-insensitive file systems (APFS and HFS+
/// by default), so a missing protected name and a plugin's spelling of it match whichever
/// case either uses. It over-matches on case-sensitive file systems, which only refuses a
/// name that differs from a protected one by case. Unicode normalization is not folded; that
/// gap is limited to names that do not exist yet, since existing entries compare by identity.
fn fold_name(name: &OsStr) -> String {
    name.to_string_lossy().to_lowercase()
}

/// The deepest existing entry at or above `path`, and the names below it that do not exist,
/// outermost first. `path` is a canonical spelling, so its lexical parents are its real ones.
fn deepest_existing(path: &Path) -> (Option<&Path>, Vec<&OsStr>) {
    let mut missing = vec![];
    for ancestor in path.ancestors() {
        // Any failure, not only `NotFound`, leaves no identity to compare; the name is then
        // matched as a missing one, and the operation itself fails if it is unreachable.
        if fs::symlink_metadata(ancestor).is_ok() {
            missing.reverse();
            return (Some(ancestor), missing);
        }
        if let Some(name) = ancestor.file_name() {
            missing.push(name);
        }
    }
    missing.reverse();
    (None, missing)
}

/// The identities a plugin's target is compared against in [`refuse_protected_host_state`],
/// gathered afresh for each check so that a folder moved since the last one is seen where it
/// is now.
#[derive(Default)]
struct ProtectedState {
    /// The protected folders and files themselves, as reached through every link.
    locations: HashSet<FileId>,
    /// Every existing entry met on the way to a protected path, link entries and the folders
    /// they lead to included, and the protected entries themselves.
    on_the_way: HashSet<FileId>,
    /// Each entry of `on_the_way` by its folder's identity and its folded name, for a rename
    /// onto a missing name that a reader would look up.
    names: HashSet<(FileId, String)>,
    /// Protected paths that do not exist yet: the identity of the deepest folder that does,
    /// as a reader reaches it, and the folded names below it.
    missing: Vec<(FileId, Vec<String>)>,
}

impl ProtectedState {
    fn gather(env: &PluginEnv) -> Self {
        let mut state = Self::default();
        let cache = YINZ_PLUGIN_PERMISSIONS_CACHE.as_path();
        // The cache is saved under a lock beside its real path, which a symlink can move, and
        // staged in a temporary file in its real folder before being renamed over it, so the
        // whole folder is protected, both as configured and where the cache really is.
        let mut cache_paths = vec![cache.with_added_extension("lock")];
        cache_paths.extend(cache.parent().map(Path::to_path_buf));
        if let Some(real_cache) = state.record(cache) {
            cache_paths.push(real_cache.with_added_extension("lock"));
            cache_paths.extend(real_cache.parent().map(Path::to_path_buf));
        }
        for path in cache_paths.iter().chain(&protected_host_folders(env)) {
            state.record(path);
        }
        state
    }

    /// Walk `path` as the kernel does for a reader that opens it, following its symlinks one
    /// component at a time rather than canonicalizing it, so that a link inside another link's
    /// target is met, and a dangling link still says where it points. Records every existing
    /// entry met, and returns the path with every link followed. Past a missing folder or a
    /// dangling link it continues lexically, which is where a reader would find whatever is
    /// later created there. A relative path is taken from the server's working directory, as
    /// the server takes it.
    fn record(&mut self, path: &Path) -> Option<PathBuf> {
        let path = std::path::absolute(path).ok()?;
        let root_id = || fs::metadata("/").ok().map(|metadata| file_id(&metadata));
        let mut pending: VecDeque<PathStep> = path_steps(&path).collect();
        let mut real = PathBuf::from("/");
        let mut real_id = root_id();
        let mut missing: Vec<OsString> = vec![];
        let mut hops = 0;
        while let Some(step) = pending.pop_front() {
            match step {
                PathStep::Root => {
                    real = PathBuf::from("/");
                    real_id = root_id();
                    missing.clear();
                },
                // Every folder in `real` is already resolved, so `..` is its lexical parent.
                PathStep::Parent => {
                    if missing.pop().is_none() {
                        real.pop();
                        real_id = fs::symlink_metadata(&real)
                            .ok()
                            .map(|metadata| file_id(&metadata));
                    }
                },
                // Below a missing folder nothing exists to look up.
                PathStep::Name(name) if !missing.is_empty() => missing.push(name),
                PathStep::Name(name) => {
                    let entry = real.join(&name);
                    let Ok(metadata) = fs::symlink_metadata(&entry) else {
                        missing.push(name);
                        continue;
                    };
                    let id = file_id(&metadata);
                    self.on_the_way.insert(id);
                    if let Some(folder) = real_id {
                        self.names.insert((folder, fold_name(&name)));
                    }
                    let target = (metadata.file_type().is_symlink() && hops < MAX_SYMLINK_HOPS)
                        .then(|| fs::read_link(&entry).ok())
                        .flatten();
                    match target {
                        // A relative target continues from the link's folder, which `real` is.
                        Some(target) => {
                            hops += 1;
                            for step in path_steps(&target).rev() {
                                pending.push_front(step);
                            }
                        },
                        None => {
                            real = entry;
                            real_id = Some(id);
                        },
                    }
                },
            }
        }
        if missing.is_empty() {
            if let Ok(metadata) = fs::metadata(&real) {
                let id = file_id(&metadata);
                self.locations.insert(id);
                self.on_the_way.insert(id);
            }
        } else if let Some(folder) = real_id {
            self.missing.push((folder, missing.iter().map(|name| fold_name(name)).collect()));
        }
        Some(missing.iter().fold(real, |path, name| path.join(name)))
    }
}

/// The most symlinks [`ProtectedState::record`] follows for one path, as the kernel's own
/// limit stops a link loop.
const MAX_SYMLINK_HOPS: usize = 40;

/// One component of a path being walked.
enum PathStep {
    Root,
    Parent,
    Name(OsString),
}

fn path_steps(path: &Path) -> impl DoubleEndedIterator<Item = PathStep> + '_ {
    path.components().filter_map(|component| match component {
        std::path::Component::Prefix(_) | std::path::Component::RootDir => Some(PathStep::Root),
        std::path::Component::CurDir => None,
        std::path::Component::ParentDir => Some(PathStep::Parent),
        std::path::Component::Normal(name) => Some(PathStep::Name(name.to_os_string())),
    })
}


/// Re-spell a real path in the virtual root the plugin used, so that a listing can be fed back in.
fn virtual_path(env: &PluginEnv, real: &Path, requested: &Path) -> PathBuf {
    for (prefix, root) in fs_roots(env).iter() {
        if !requested.starts_with(prefix) && !(requested.is_relative() && *prefix == "/host") {
            continue;
        }
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        if let Ok(rest) = real.strip_prefix(&canonical_root) {
            return PathBuf::from(prefix).join(rest);
        }
    }
    real.to_path_buf()
}

pub(super) fn fs_read(
    env: &mut PluginEnv,
    path: PathBuf,
    max_bytes: Option<usize>,
    offset: Option<u64>,
) {
    let cap = max_bytes
        .unwrap_or(MAX_FS_READ_BYTES)
        .min(MAX_FS_READ_BYTES);
    let result = resolve_fs_path_with_permission(
        env,
        &path,
        FsIntent::Read,
        FsPathResolution::FollowSymlink,
    )
    .and_then(|real| {
        // A FIFO can replace a file between path resolution and open. Do not let its open
        // block this thread, and check the opened file rather than a stale path's metadata.
        let mut file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(&real)
            .with_context(|| format!("failed to open '{}'", path.display()))?;
        let metadata = file
            .metadata()
            .with_context(|| format!("failed to inspect '{}'", path.display()))?;
        ensure!(
            metadata.is_file(),
            "'{}' is not a regular file",
            path.display()
        );
        if offset.is_none() && metadata.len() > cap as u64 {
            return Err(anyhow!(
                "'{}' is {} bytes, more than the {} byte limit for a plugin read",
                path.display(),
                metadata.len(),
                cap
            ));
        }
        if let Some(offset) = offset {
            file.seek(SeekFrom::Start(offset))
                .with_context(|| format!("failed to seek '{}'", path.display()))?;
        }
        // Bound the actual read too: the file can grow after metadata was read. The extra
        // byte detects truncation for whole-file reads without buffering the entire file.
        let mut contents = Vec::new();
        file.take((cap + usize::from(offset.is_none())) as u64)
            .read_to_end(&mut contents)
            .with_context(|| format!("failed to read '{}'", path.display()))?;
        ensure!(
            contents.len() <= cap,
            "'{}' grew beyond the {} byte limit for a plugin read",
            path.display(),
            cap
        );
        Ok(contents)
    });

    let response = ProtobufFsReadResponse {
        result: Some(match result {
            Ok(contents) => fs_read_response::Result::Contents(contents),
            Err(e) => fs_read_response::Result::Error(format!("{:#}", e)),
        }),
    };
    write_response(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send a file to plugin {}", env.name()))
        .non_fatal();
}

pub(super) fn fs_list(env: &mut PluginEnv, path: PathBuf) {
    let result = resolve_fs_path_with_permission(
        env,
        &path,
        FsIntent::Read,
        FsPathResolution::FollowSymlink,
    )
    .and_then(|real| {
        let entries =
            fs::read_dir(&real).with_context(|| format!("failed to list '{}'", path.display()))?;
        let mut listed = vec![];
        for entry in entries.flatten() {
            let metadata: Option<FileMetadata> = entry.metadata().ok().map(|m| m.into());
            listed.push(ProtobufFsEntry {
                path: virtual_path(env, &entry.path(), &path)
                    .display()
                    .to_string(),
                // `ProtobufFileMetadata` converts from the `Option`, carrying "unread" in its
                // own `metadata_is_set` flag rather than in the absence of the message.
                metadata: Some(metadata.into()),
            });
        }
        listed.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(listed)
    });

    let response = ProtobufFsListResponse {
        result: Some(match result {
            Ok(entries) => fs_list_response::Result::Entries(ProtobufFsEntryList { entries }),
            Err(e) => fs_list_response::Result::Error(format!("{:#}", e)),
        }),
    };
    write_response(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send a listing to plugin {}", env.name()))
        .non_fatal();
}

pub(super) fn fs_metadata(env: &mut PluginEnv, path: PathBuf, follow_symlinks: bool) {
    let result = resolve_fs_path_with_permission(
        env,
        &path,
        FsIntent::Read,
        if follow_symlinks {
            FsPathResolution::FollowSymlink
        } else {
            FsPathResolution::PreserveEntry
        },
    )
    .map(|real| fs::symlink_metadata(&real).ok().map(FileMetadata::from));

    let response = ProtobufFsMetadataResponse {
        result: Some(match result {
            Ok(Some(metadata)) => fs_metadata_response::Result::Metadata(Some(metadata).into()),
            Ok(None) => fs_metadata_response::Result::NotFound(true),
            Err(e) => fs_metadata_response::Result::Error(format!("{:#}", e)),
        }),
    };
    write_response(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send metadata to plugin {}", env.name()))
        .non_fatal();
}

pub(super) fn fs_write(
    env: &mut PluginEnv,
    path: PathBuf,
    contents: Vec<u8>,
    append: bool,
    create_dirs: bool,
) {
    let result = resolve_fs_path_with_permission(
        env,
        &path,
        FsIntent::Write,
        FsPathResolution::FollowSymlink,
    )
    .and_then(|real| {
        if create_dirs && let Some(parent) = real.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create the folders above '{}'", path.display())
            })?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .append(append)
            // Resolve legitimate links before opening; refuse a replacement link or a FIFO
            // without waiting for another process. Validate before truncating or writing.
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(&real)
            .with_context(|| format!("failed to open '{}' for writing", path.display()))?;
        ensure!(
            file.metadata()
                .with_context(|| format!("failed to inspect '{}'", path.display()))?
                .is_file(),
            "'{}' is not a regular file",
            path.display()
        );
        if !append {
            file.set_len(0)
                .with_context(|| format!("failed to truncate '{}'", path.display()))?;
        }
        file.write_all(&contents)
            .with_context(|| format!("failed to write '{}'", path.display()))
    });
    write_fs_mutate_response(env, result);
}

pub(super) fn fs_remove(env: &mut PluginEnv, path: PathBuf, recursive: bool) {
    let result = resolve_fs_path_with_permission(
        env,
        &path,
        FsIntent::Write,
        FsPathResolution::PreserveEntry,
    )
    .and_then(|real| {
        let metadata = fs::symlink_metadata(&real)
            .with_context(|| format!("failed to remove '{}'", path.display()))?;
        if metadata.is_dir() {
            if recursive {
                fs::remove_dir_all(&real)
            } else {
                fs::remove_dir(&real)
            }
        } else {
            fs::remove_file(&real)
        }
        .with_context(|| format!("failed to remove '{}'", path.display()))
    });
    write_fs_mutate_response(env, result);
}

pub(super) fn fs_create_dir(env: &mut PluginEnv, path: PathBuf, all: bool) {
    let result = resolve_fs_path_with_permission(
        env,
        &path,
        FsIntent::Write,
        FsPathResolution::FollowSymlink,
    )
    .and_then(|real| {
        if all {
            fs::create_dir_all(&real)
        } else {
            fs::create_dir(&real)
        }
        .with_context(|| format!("failed to create '{}'", path.display()))
    });
    write_fs_mutate_response(env, result);
}

pub(super) fn fs_rename(env: &mut PluginEnv, from: PathBuf, to: PathBuf) {
    // Both endpoints are resolved independently: renaming *out* of the plugin's folders is as
    // much of an escape as reading outside them.
    let result = resolve_fs_path_with_permission(
        env,
        &from,
        FsIntent::Write,
        FsPathResolution::PreserveEntry,
    )
    .and_then(|real_from| {
        let real_to = resolve_fs_path_with_permission(
            env,
            &to,
            FsIntent::Write,
            FsPathResolution::PreserveEntry,
        )?;
        fs::rename(&real_from, &real_to).with_context(|| {
            format!(
                "failed to rename '{}' to '{}'",
                from.display(),
                to.display()
            )
        })
    });
    write_fs_mutate_response(env, result);
}

fn write_fs_mutate_response(env: &mut PluginEnv, result: Result<()>) {
    let response = ProtobufFsMutateResponse {
        result: Some(match result {
            Ok(()) => fs_mutate_response::Result::Success(true),
            Err(e) => fs_mutate_response::Result::Error(format!("{:#}", e)),
        }),
    };
    write_response(env, &response.encode_to_vec())
        .with_context(|| format!("failed to answer a filesystem command for {}", env.name()))
        .non_fatal();
}

pub(super) fn scan_host_folder(env: &PluginEnv, folder_to_scan: PathBuf) {
    if !folder_to_scan.starts_with("/host") {
        log::error!(
            "Can only scan files in the /host filesystem, found: {}",
            folder_to_scan.display()
        );
        return;
    }
    let plugin_host_folder = env
        .plugin_cwd
        .canonicalize()
        .unwrap_or_else(|_| env.plugin_cwd.clone());
    let folder_to_scan = plugin_host_folder.join(folder_to_scan.strip_prefix("/host").unwrap());
    match folder_to_scan.canonicalize() {
        Ok(folder_to_scan) => {
            if !folder_to_scan.starts_with(&plugin_host_folder) {
                log::error!(
                    "Can only scan files in the plugin filesystem: {}, found: {}",
                    plugin_host_folder.display(),
                    folder_to_scan.display()
                );
                return;
            }
            let reading_folder = std::fs::read_dir(&folder_to_scan);
            match reading_folder {
                Ok(reading_folder) => {
                    let send_plugin_instructions = env.senders.to_plugin.clone();
                    let update_target = env.plugin_id;
                    let generation = env.generation;
                    let client_id = env.client_id;
                    thread::spawn({
                        move || {
                            let mut paths_in_folder = vec![];
                            for entry in reading_folder.flatten() {
                                let entry_metadata = entry.metadata().ok().map(|m| m.into());
                                paths_in_folder.push((
                                    PathBuf::from("/host").join(
                                        entry.path().strip_prefix(&plugin_host_folder).unwrap(),
                                    ),
                                    entry_metadata,
                                ));
                            }
                            send_plugin_instructions
                                .ok_or(anyhow!("found no sender to send plugin instruction to"))
                                .map(|sender| {
                                    let _ = sender.send(PluginInstruction::for_plugin_event(
                                        update_target,
                                        client_id,
                                        generation,
                                        Event::FileSystemUpdate(paths_in_folder),
                                    ));
                                })
                                .non_fatal();
                        }
                    });
                },
                Err(e) => {
                    log::error!("Failed to read folder {}: {e}", folder_to_scan.display());
                },
            }
        },
        Err(e) => {
            log::error!(
                "Failed to canonicalize path {folder_to_scan:?} when scanning folder: {:?}",
                e
            );
        },
    }
}

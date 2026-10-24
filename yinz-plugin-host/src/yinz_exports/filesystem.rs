//! Plugin filesystem mounts, path confinement, and file operations.

use super::permissions::has_permission;
use super::{write_response, PLUGINS_FS_ROOT};
use crate::plugin_map::PluginEnv;
use crate::PluginInstruction;
use anyhow::ensure;
use prost::Message;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::{fs, thread};
use yinz_plugin_api::plugin_command::{
    fs_list_response, fs_metadata_response, fs_mutate_response, fs_read_response, ProtobufFsEntry,
    ProtobufFsEntryList, ProtobufFsListResponse, ProtobufFsMetadataResponse,
    ProtobufFsMutateResponse, ProtobufFsReadResponse,
};
use yinz_runtime::consts::{YINZ_PLUGIN_TMP_DIR, YINZ_TMP_LOG_DIR};
use yinz_runtime::errors::prelude::*;
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

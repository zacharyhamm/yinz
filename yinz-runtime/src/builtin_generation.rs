//! Keep old live servers on their builtin generation after installation exchange.
use crate::errors::prelude::*;
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, Weak},
};

/// Pin installation reads against the updater's directory exchange. The lock inode
/// lives beside `builtins`, so replacing that directory cannot replace the lock.
pub fn read_guard(data_dir: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = data_dir.join("plugins/.yinz-update.lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .with_context(|| format!("open builtin generation lock {}", path.display()))?;
    file.lock_shared()
        .context("pin builtin installation generation")?;
    Ok(file)
}

/// File URLs into the installed tree use the same generation protection as builtin tags.
pub fn source_read_guard(source: &Path) -> Result<Option<std::fs::File>> {
    let data_dir = source.ancestors().find_map(|ancestor| {
        (ancestor.file_name().is_some_and(|name| name == "builtins")
            && ancestor
                .parent()?
                .file_name()
                .is_some_and(|name| name == "plugins"))
        .then(|| ancestor.parent().and_then(Path::parent))
        .flatten()
    });
    data_dir.map(read_guard).transpose()
}

pub fn builtin_path(data_dir: &Path, tag: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        crate::consts::BUILTIN_PLUGIN_NAMES.contains(&tag),
        "unknown builtin plugin '{tag}'"
    );
    let path = source_path(&data_dir.join("plugins/builtins").join(format!("{tag}.scm")))?;
    anyhow::ensure!(
        path.exists(),
        "builtin plugin is missing at {}; reinstall Yinz with --data-dir {}",
        path.display(),
        data_dir.display()
    );
    Ok(path)
}

static GENERATIONS: LazyLock<Mutex<HashMap<PathBuf, Weak<Generation>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct Generation {
    identity: Option<(u64, u64)>,
    state: Mutex<State>,
}

struct State {
    snapshot: tempfile::TempDir,
    /// Source metadata when `snapshot` was taken; unchanged trees are not copied again.
    fingerprint: Option<Fingerprint>,
    switched: bool,
}

type Fingerprint = Vec<(PathBuf, u64, u64, i64, i64, i64, i64)>;

/// Held for the entire plugin-host lifetime, including worker cleanup.
pub struct SessionGeneration {
    _generation: Option<Arc<Generation>>,
}

pub fn pin_session_generation(data_dir: &Path) -> Result<SessionGeneration> {
    let root = data_dir.join("plugins/builtins");
    let _lock = read_guard(data_dir)?;
    let data_dir = data_dir
        .canonicalize()
        .context("resolve builtin generation data directory")?;
    let mut registry = GENERATIONS
        .lock()
        .map_err(|_| anyhow!("builtin generation registry is poisoned"))?;
    if let Some(generation) = registry.get(&data_dir).and_then(Weak::upgrade) {
        return Ok(SessionGeneration {
            _generation: Some(generation),
        });
    }
    let identity = match fs::symlink_metadata(&root) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_dir(),
                "builtin generation is not a regular directory"
            );
            Some((metadata.dev(), metadata.ino()))
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("inspect builtin generation"),
    };
    // Absence is also a generation: an already running binary must never begin
    // loading files first installed by a later release.
    let fingerprint = identity.map(|_| fingerprint(&root)).transpose()?;
    let snapshot = if identity.is_some() {
        snapshot(&root)?
    } else {
        tempfile::Builder::new()
            .prefix("yinz-builtin-generation-")
            .tempdir()
            .context("retain missing builtin generation")?
    };
    let generation = Arc::new(Generation {
        identity,
        state: Mutex::new(State {
            snapshot,
            fingerprint,
            switched: false,
        }),
    });
    registry.insert(data_dir, Arc::downgrade(&generation));
    Ok(SessionGeneration {
        _generation: Some(generation),
    })
}

/// Preserve normal disk-edit reloads until the installed directory is exchanged.
/// Subsequent loads by that old server use its private snapshot, including in
/// supervised compilation subprocesses which cannot share an in-memory registry.
pub fn source_path(source: &Path) -> Result<PathBuf> {
    let Some(root) = source.ancestors().find(|ancestor| {
        ancestor.file_name().is_some_and(|name| name == "builtins")
            && ancestor
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "plugins")
    }) else {
        return Ok(source.to_owned());
    };
    let data_dir = root
        .parent()
        .and_then(Path::parent)
        .context("builtin source has no data directory")?;
    let Ok(data_dir) = data_dir.canonicalize() else {
        return Ok(source.to_owned());
    };
    let generation = GENERATIONS
        .lock()
        .map_err(|_| anyhow!("builtin generation registry is poisoned"))?
        .get(&data_dir)
        .and_then(Weak::upgrade);
    let Some(generation) = generation else {
        return Ok(source.to_owned());
    };
    let relative = source
        .strip_prefix(root)
        .context("resolve builtin generation source")?;
    anyhow::ensure!(
        relative
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir)),
        "builtin source escapes its generation"
    );
    let _lock = read_guard(&data_dir)?;
    let mut state = generation
        .state
        .lock()
        .map_err(|_| anyhow!("builtin generation is poisoned"))?;
    let current = fs::symlink_metadata(root)
        .ok()
        .filter(|metadata| metadata.is_dir())
        .map(|metadata| (metadata.dev(), metadata.ino()));
    if !state.switched && current == generation.identity {
        // Refresh on source access so intentional edits reloaded before the
        // update are carried into the old session's retained generation.
        if current.is_some() {
            // Fingerprint before copying: an edit racing the copy changes the
            // next fingerprint, which then refreshes this snapshot again.
            let fingerprint = Some(fingerprint(root)?);
            if state.fingerprint != fingerprint {
                state.snapshot = snapshot(root)?;
                state.fingerprint = fingerprint;
            }
        }
        return Ok(source.to_owned());
    }
    state.switched = true;
    Ok(state
        .snapshot
        .path()
        .join("plugins/builtins")
        .join(relative))
}

/// Metadata of every source `copy_sources` would retain, in a stable order.
fn fingerprint(source: &Path) -> Result<Fingerprint> {
    let mut pending = vec![(source.to_owned(), 0usize)];
    let mut entries = Vec::new();
    let mut files = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        anyhow::ensure!(
            depth <= 64,
            "builtin source generation exceeds maximum depth"
        );
        for entry in fs::read_dir(&directory).context("read builtin source generation")? {
            let entry = entry.context("read builtin generation entry")?;
            files += 1;
            anyhow::ensure!(
                files <= 4096,
                "builtin source generation has too many entries"
            );
            let kind = entry
                .file_type()
                .context("inspect builtin generation entry")?;
            if kind.is_dir() {
                pending.push((entry.path(), depth + 1));
            } else if kind.is_file() {
                let metadata = entry
                    .metadata()
                    .context("inspect builtin source metadata")?;
                entries.push((
                    entry.path(),
                    metadata.ino(),
                    metadata.len(),
                    metadata.mtime(),
                    metadata.mtime_nsec(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                ));
            }
        }
    }
    entries.sort();
    Ok(entries)
}

fn snapshot(source: &Path) -> Result<tempfile::TempDir> {
    let snapshot = tempfile::Builder::new()
        .prefix("yinz-builtin-generation-")
        .tempdir()
        .context("create private builtin generation snapshot")?;
    copy_sources(source, &snapshot.path().join("plugins/builtins"))?;
    Ok(snapshot)
}

fn copy_sources(source: &Path, destination: &Path) -> Result<()> {
    let mut pending = vec![(source.to_owned(), destination.to_owned(), 0usize)];
    let mut bytes = 0u64;
    let mut files = 0usize;
    while let Some((source, destination, depth)) = pending.pop() {
        anyhow::ensure!(
            depth <= 64,
            "builtin source generation exceeds maximum depth"
        );
        fs::create_dir_all(&destination).context("create builtin generation directory")?;
        for entry in fs::read_dir(&source).context("read builtin source generation")? {
            let entry = entry.context("read builtin generation entry")?;
            files += 1;
            anyhow::ensure!(
                files <= 4096,
                "builtin source generation has too many entries"
            );
            let kind = entry
                .file_type()
                .context("inspect builtin generation entry")?;
            if kind.is_dir() {
                pending.push((entry.path(), destination.join(entry.file_name()), depth + 1));
            } else if kind.is_file()
                && matches!(
                    entry
                        .path()
                        .extension()
                        .and_then(|extension| extension.to_str()),
                    Some("scm" | "ss" | "sld")
                )
            {
                let size = entry
                    .metadata()
                    .context("inspect builtin source size")?
                    .len();
                bytes = bytes.saturating_add(size);
                anyhow::ensure!(
                    size <= 8 * 1024 * 1024 && bytes <= 64 * 1024 * 1024,
                    "builtin source generation exceeds source limits"
                );
                use std::{io::Read, os::unix::fs::OpenOptionsExt};
                let input = fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(entry.path())
                    .context("open builtin generation source")?;
                anyhow::ensure!(
                    input
                        .metadata()
                        .context("inspect opened builtin source")?
                        .is_file(),
                    "builtin source changed into a special file"
                );
                let mut output = fs::File::create(destination.join(entry.file_name()))
                    .context("create retained builtin source")?;
                let copied = std::io::copy(&mut input.take(8 * 1024 * 1024 + 1), &mut output)
                    .context("retain builtin source generation")?;
                bytes = bytes.saturating_sub(size).saturating_add(copied);
                anyhow::ensure!(
                    copied <= 8 * 1024 * 1024 && bytes <= 64 * 1024 * 1024,
                    "builtin source generation exceeds source limits"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/builtin_generation.rs"]
mod tests;

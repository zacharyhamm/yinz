//! A checkpoint is published by replacing one symlink, after all of its files are durable.
//! Readers keep the shared lock until layout parsing has loaded the pane contents too.

use crate::errors::prelude::*;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

pub const METADATA: &str = "session-metadata.kdl";
pub const LAYOUT: &str = "session-layout.kdl";
const CURRENT: &str = "current";
const LOCK: &str = ".checkpoint.lock";
const PREFIX: &str = ".checkpoint-";

pub struct SessionCheckpoint {
    directory: PathBuf,
    _lock: Option<File>,
}

impl SessionCheckpoint {
    pub fn read(session_dir: &Path) -> Result<Self> {
        let lock = match File::open(session_dir.join(LOCK)) {
            Ok(lock) => {
                lock.lock_shared()
                    .context("failed to lock session checkpoint for reading")?;
                Some(lock)
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // A legacy reader racing the first publication can safely finish reading
                // the flat files: migration never changes or removes those files.
                if fs::symlink_metadata(session_dir.join(CURRENT)).is_ok() {
                    bail!("session checkpoint has a current pointer but no lock file");
                }
                return Ok(Self {
                    directory: session_dir.to_owned(),
                    _lock: None,
                });
            },
            Err(e) => return Err(e).context("failed to open session checkpoint lock"),
        };
        let directory = current_directory(session_dir)?.unwrap_or_else(|| session_dir.to_owned());
        Ok(Self {
            directory,
            _lock: lock,
        })
    }

    pub fn layout_path(&self) -> PathBuf {
        self.directory.join(LAYOUT)
    }

    pub fn metadata_path(&self) -> PathBuf {
        self.directory.join(METADATA)
    }
}

fn single_component(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn current_directory(session_dir: &Path) -> Result<Option<PathBuf>> {
    let target = match fs::read_link(session_dir.join(CURRENT)) {
        Ok(target) => target,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("failed to read current session checkpoint"),
    };
    if !single_component(&target) || !target.to_string_lossy().starts_with(PREFIX) {
        bail!("invalid session checkpoint target: {}", target.display());
    }
    let directory = session_dir.join(target);
    if !fs::metadata(&directory)
        .context("current session checkpoint is missing")?
        .is_dir()
    {
        bail!("current session checkpoint is not a directory");
    }
    Ok(Some(directory))
}

pub fn write_checkpoint(
    session_dir: &Path,
    metadata: &str,
    layout: &(String, BTreeMap<String, String>),
) -> Result<()> {
    write_checkpoint_with_progress(session_dir, metadata, layout, |_| {})
}

pub fn write_checkpoint_with_progress(
    session_dir: &Path,
    metadata: &str,
    layout: &(String, BTreeMap<String, String>),
    progress: impl FnMut(&'static str),
) -> Result<()> {
    write_checkpoint_with(session_dir, metadata, layout, |_| Ok(()), progress).with_context(|| {
        format!(
            "failed to save session checkpoint in {}",
            session_dir.display()
        )
    })
}

// The hook lets tests fail a staged write, publication, or final sync without disk-full
// fixtures, permissions that root can bypass, or timing-dependent process kills.
fn write_checkpoint_with(
    session_dir: &Path,
    metadata: &str,
    layout: &(String, BTreeMap<String, String>),
    mut before_write: impl FnMut(&Path) -> io::Result<()>,
    mut progress: impl FnMut(&'static str),
) -> Result<()> {
    progress("create directory");
    fs::create_dir_all(session_dir).context("failed to create session checkpoint folder")?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(session_dir.join(LOCK))
        .context("failed to open session checkpoint lock")?;
    progress("file lock");
    lock.lock()
        .context("failed to lock session checkpoint for writing")?;
    progress("collect files");
    let previous = current_directory(session_dir)?;
    let source = previous.as_deref().unwrap_or(session_dir);
    let mut files = BTreeMap::new();
    if layout.0.is_empty() {
        // Startup metadata may arrive before the first serialized layout. Keep the
        // previous recovery data, including when upgrading a flat checkpoint.
        for entry in fs::read_dir(source).context("failed to read previous checkpoint")? {
            let entry = entry.context("failed to read checkpoint file entry")?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if (previous.is_some() && name != METADATA && name != CURRENT)
                || name == LAYOUT
                || name.starts_with("initial_contents_")
            {
                files.insert(
                    name,
                    fs::read(entry.path()).context("failed to preserve checkpoint file")?,
                );
            }
        }
    } else {
        for (name, contents) in &layout.1 {
            if !single_component(Path::new(name))
                || name.starts_with('.')
                || name == LAYOUT
                || name == METADATA
                || name == CURRENT
            {
                bail!("invalid checkpoint contents filename: {name}");
            }
            files.insert(name.clone(), contents.as_bytes().to_vec());
        }
        files.insert(LAYOUT.to_owned(), layout.0.as_bytes().to_vec());
    }
    files.insert(METADATA.to_owned(), metadata.as_bytes().to_vec());
    let mut unchanged = BTreeMap::new();
    for (name, contents) in &files {
        let same = match fs::read(source.join(name)) {
            Ok(old) => old == *contents,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(e).context("failed to compare checkpoint contents"),
        };
        unchanged.insert(name, same);
    }
    if previous.is_some()
        && unchanged.values().all(|same| *same)
        && fs::read_dir(source)
            .context("failed to count checkpoint files")?
            .count()
            == files.len()
    {
        // A preceding publication may have succeeded before its directory sync failed.
        // Do not report that retry as successful until its directory entries are durable.
        progress("unchanged directory sync");
        sync_directory(source)?;
        progress("unchanged parent sync");
        sync_directory(session_dir)?;
        return Ok(());
    }

    let staging = tempfile::Builder::new()
        .prefix(PREFIX)
        .tempdir_in(session_dir)
        .context("failed to stage session checkpoint")?;
    for (name, contents) in &files {
        let path = staging.path().join(name);
        progress("stage file");
        before_write(&path).context("failed to stage checkpoint file")?;
        if previous.is_some() && unchanged[name] {
            fs::hard_link(source.join(name), &path)
                .context("failed to reuse unchanged checkpoint file")?;
        } else {
            let mut file = File::create(&path).context("failed to create checkpoint file")?;
            file.write_all(contents)
                .context("failed to write checkpoint file")?;
            progress("file sync");
            file.sync_all().context("failed to sync checkpoint file")?;
        }
    }
    let target = staging
        .path()
        .file_name()
        .context("checkpoint has no directory name")?;
    let pointer = staging.path().join(CURRENT);
    std::os::unix::fs::symlink(target, &pointer).context("failed to stage checkpoint pointer")?;
    progress("staging directory sync");
    sync_directory(staging.path())?;
    let parent = File::open(session_dir).context("failed to open checkpoint folder")?;
    progress("parent directory sync");
    parent
        .sync_all()
        .context("failed to sync checkpoint folder")?;
    if let Some(enclosing_dir) = session_dir.parent() {
        progress("enclosing directory sync");
        sync_directory(enclosing_dir)?;
    }
    progress("publication");
    before_write(&session_dir.join(CURRENT)).context("failed to publish checkpoint")?;
    fs::rename(pointer, session_dir.join(CURRENT)).context("failed to publish checkpoint")?;
    // Publication succeeded: even a following sync failure must not drop the new directory.
    let published = staging.into_path();
    progress("published directory sync");
    before_write(&published).context("failed to sync published checkpoint")?;
    sync_directory(&published)?;
    progress("publication parent sync");
    parent
        .sync_all()
        .context("failed to sync published checkpoint")?;
    progress("cleanup");
    cleanup(session_dir, &published, previous.as_deref())
        .context("failed to clean old session checkpoints")
        .non_fatal();
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("failed to sync checkpoint directory {}", path.display()))
}

fn cleanup(session_dir: &Path, published: &Path, previous: Option<&Path>) -> Result<()> {
    for entry in fs::read_dir(session_dir).context("failed to list old checkpoints")? {
        let entry = entry.context("failed to read old checkpoint entry")?;
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with(PREFIX)
            && entry
                .file_type()
                .context("failed to inspect old checkpoint entry")?
                .is_dir()
            && path != published
            && Some(path.as_path()) != previous
        {
            fs::remove_dir_all(path).context("failed to remove old checkpoint")?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/session_checkpoint_tests.rs"]
mod tests;

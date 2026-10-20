//! Native Git installs. A selected worktree is never edited or removed.
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use yinz_runtime::errors::prelude::*;
use yinz_types::input::git_plugin::{GitPlugin, GitRevision};

const TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledPlugin {
    pub entrypoint: PathBuf,
    pub root: PathBuf,
    pub commit: Option<String>,
}

fn installation(data_dir: &Path, plugin: &GitPlugin) -> PathBuf {
    let selector = plugin.revision.as_ref().map(GitRevision::property);
    let key = format!("{}\0{selector:?}", plugin.repo);
    data_dir.join("plugin-repositories").join(
        Sha256::digest(key.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        bail!("Git plugin operation cancelled");
    }
    Ok(())
}

fn git(directory: Option<&Path>, args: &[&str], cancel: &AtomicBool) -> Result<String> {
    check_cancel(cancel)?;
    let mut command = Command::new("git");
    command
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "submodule.recurse=false",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "/usr/bin/false")
        .env("SSH_ASKPASS", "/usr/bin/false")
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("DISPLAY", "yinz-noninteractive")
        .args(args);
    // Repository environment overrides must not redirect our writes to a user's checkout.
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(variable);
    }
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let (status, output, errors) =
        crate::worker::supervise_cancel(&mut command, &[], TIMEOUT, Some(cancel))
            .context("supervise Git")?;
    if !status.success() {
        bail!(
            "Git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&errors)
        );
    }
    String::from_utf8(output)
        .context("decode Git output")
        .map(|value| value.trim().to_owned())
}

fn entrypoint(root: &Path, plugin: &GitPlugin, commit: Option<String>) -> Result<InstalledPlugin> {
    plugin.validate().map_err(|e| anyhow!(e))?;
    let root = root.canonicalize().context("resolve repository root")?;
    let entrypoint = root
        .join(&plugin.entrypoint)
        .canonicalize()
        .context("resolve repository entrypoint")?;
    if !entrypoint.starts_with(&root) || !entrypoint.is_file() {
        bail!(
            "plugin entrypoint escapes repository or is not a file: {}",
            entrypoint.display()
        );
    }
    if !yinz_plugin_api::authoring::is_script_plugin(&entrypoint) {
        bail!("repository entrypoint must resolve to a Scheme file");
    }
    Ok(InstalledPlugin {
        root,
        entrypoint,
        commit,
    })
}

/// Read the selected commit without Git, locking, fetching or creating directories.
pub fn status(data_dir: &Path, plugin: &GitPlugin) -> Result<Option<InstalledPlugin>> {
    plugin.validate().map_err(|e| anyhow!(e))?;
    if plugin.is_local() {
        return entrypoint(Path::new(&plugin.repo), plugin, None).map(Some);
    }
    let directory = installation(data_dir, plugin);
    let selected = match fs::read_link(directory.join("current")) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("read installed plugin selection"),
    };
    let commit = selected
        .file_name()
        .and_then(|value| value.to_str())
        .context("invalid installed revision")?;
    if selected != Path::new("revisions").join(commit)
        || !matches!(commit.len(), 40 | 64)
        || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        bail!("invalid installed plugin selection");
    }
    entrypoint(&directory.join(&selected), plugin, Some(commit.to_owned())).map(Some)
}

/// Install on first use. Existing installs work offline. Local repositories are read only.
pub fn resolve(
    data_dir: &Path,
    plugin: &GitPlugin,
    entries: &[GitPlugin],
    cancel: &AtomicBool,
) -> Result<InstalledPlugin> {
    check_cancel(cancel)?;
    if let Some(installed) = status(data_dir, plugin)? {
        return Ok(installed);
    }
    install(data_dir, plugin, entries, false, cancel)
}

/// Validate every configured entrypoint sharing this repository before publishing an update.
pub fn install(
    data_dir: &Path,
    plugin: &GitPlugin,
    entries: &[GitPlugin],
    update: bool,
    cancel: &AtomicBool,
) -> Result<InstalledPlugin> {
    plugin.validate().map_err(|e| anyhow!(e))?;
    check_cancel(cancel)?;
    if plugin.is_local() {
        return entrypoint(Path::new(&plugin.repo), plugin, None);
    }
    let directory = installation(data_dir, plugin);
    fs::create_dir_all(&directory).context("create plugin repository directory")?;
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("lock"))
        .context("open repository lock")?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        check_cancel(cancel)?;
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            },
            Err(error) => return Err(anyhow!("cannot lock plugin repository: {error}")),
        }
    }
    if !update && let Some(installed) = status(data_dir, plugin)? {
        for entry in entries {
            entrypoint(&installed.root, entry, installed.commit.clone())?;
        }
        return Ok(installed);
    }
    let repository = directory.join("repository.git");
    let retry = repository.exists() && !directory.join("current").exists();
    if !repository.exists() {
        let staging = tempfile::tempdir_in(&directory).context("stage repository clone")?;
        let clone = staging.path().join("repository.git");
        git(
            None,
            &[
                "clone",
                "--bare",
                "--",
                &plugin.repo,
                clone.to_str().context("repository path is not UTF-8")?,
            ],
            cancel,
        )?;
        check_cancel(cancel)?;
        fs::rename(&clone, &repository).context("publish repository clone")?;
    } else if retry
        || (update
            && !matches!(
                plugin.revision,
                Some(GitRevision::Tag(_) | GitRevision::Commit(_))
            ))
    {
        let mut args = vec![
            "fetch",
            "--prune",
            "--no-tags",
            "origin",
            "+refs/heads/*:refs/heads/*",
        ];
        if retry {
            args.push("+refs/tags/*:refs/tags/*");
        }
        git(Some(&repository), &args, cancel)?;
    }
    let revision = match &plugin.revision {
        Some(GitRevision::Branch(value)) => format!("refs/heads/{value}"),
        Some(GitRevision::Tag(value)) => format!("refs/tags/{value}"),
        Some(GitRevision::Commit(value)) => value.clone(),
        None if !update && !retry => git(Some(&repository), &["symbolic-ref", "HEAD"], cancel)?,
        None => {
            let value = git(
                Some(&repository),
                &["ls-remote", "--symref", "origin", "HEAD"],
                cancel,
            )?;
            value
                .lines()
                .find_map(|line| {
                    line.strip_prefix("ref: ")
                        .and_then(|line| line.strip_suffix("\tHEAD"))
                })
                .context("remote has no default branch")?
                .to_owned()
        },
    };
    let commit = git(
        Some(&repository),
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ],
        cancel,
    )?;
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("Git returned an invalid commit ID");
    }
    let revisions = directory.join("revisions");
    fs::create_dir_all(&revisions).context("create revisions directory")?;
    let root = revisions.join(&commit);
    if !root.exists() {
        let staging = tempfile::tempdir_in(&directory).context("stage detached revision")?;
        let checkout = staging.path().join("checkout");
        git(
            Some(&repository),
            &[
                "worktree",
                "add",
                "--detach",
                checkout.to_str().context("checkout path is not UTF-8")?,
                &commit,
            ],
            cancel,
        )?;
        for entry in entries {
            entrypoint(&checkout, entry, Some(commit.clone()))?;
        }
        entrypoint(&checkout, plugin, Some(commit.clone()))?;
        check_cancel(cancel)?;
        git(
            Some(&repository),
            &[
                "worktree",
                "move",
                checkout.to_str().context("checkout path is not UTF-8")?,
                root.to_str().context("revision path is not UTF-8")?,
            ],
            cancel,
        )?;
    }
    for entry in entries {
        entrypoint(&root, entry, Some(commit.clone()))?;
    }
    let installed = entrypoint(&root, plugin, Some(commit.clone()))?;
    let selection = tempfile::tempdir_in(&directory).context("stage repository selection")?;
    let selected = selection.path().join("current");
    std::os::unix::fs::symlink(Path::new("revisions").join(&commit), &selected)
        .context("stage selected revision")?;
    check_cancel(cancel)?;
    fs::rename(selected, directory.join("current")).context("publish selected revision")?;
    Ok(installed)
}

#[cfg(test)]
#[path = "unit/git.rs"]
mod tests;

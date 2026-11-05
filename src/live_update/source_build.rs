//! Pinned local-main source snapshots and a visible, supervised build pane.
use super::{journal, journal::Store};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
use yinz_runtime::errors::prelude::*;
use yinz_types::session_update::Inventory;

/// Only returned after a confirmed worker exit or pane closure and successful cleanup.
#[derive(Debug, thiserror::Error)]
pub(super) enum TerminalFailure {
    #[error("source build pane was closed; compilation stopped before installation")]
    PaneClosed,
    #[error("source build worker exited without a result; compilation stopped before installation; inspect the retained pane and log at {}", log.display())]
    WorkerExited { log: PathBuf },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct BuildRecord {
    pub operation: String,
    pub repository: PathBuf,
    pub commit: String,
    pub version: String,
    pub snapshot: PathBuf,
    pub targets: Vec<String>,
    pub log: PathBuf,
    pub artifacts: BTreeMap<String, PathBuf>,
    pub sha256: BTreeMap<String, String>,
    pub pane: Option<String>,
    #[serde(default)]
    snapshot_ready: bool,
    #[serde(default)]
    pane_closed: bool,
}

fn output(command: &mut Command) -> Result<Vec<u8>> {
    let output = yinz_process::ssh::bootstrap_output(command, Duration::from_secs(30), None)
        .context("run source-update inspection")?;
    anyhow::ensure!(
        output.status.success(),
        "source-update command failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

fn git(repository: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(output(
        Command::new("git").arg("-C").arg(repository).args(args),
    )?)
    .context("decode source repository metadata")
}

fn session_command(inventory: &Inventory) -> Command {
    let mut command = Command::new(&inventory.executable);
    command.args(["--session", &inventory.session]);
    if let Some(data) = &inventory.assets.data_dir {
        command.arg("--data-dir").arg(data);
    }
    command
        .env_remove("YINZ_PANE_REF")
        .env_remove("YINZ_PANE_ID");
    command
}

fn main_checkout(repository: &Path) -> Result<(PathBuf, String)> {
    let origin = git(repository, &["remote", "get-url", "origin"])?;
    anyhow::ensure!(
        project_data::projects::origin_identity(origin.trim())
            == "repo:github.com/zacharyhamm/yinz",
        "the registered yinz checkout no longer points to github.com/zacharyhamm/yinz"
    );
    let worktrees = git(repository, &["worktree", "list", "--porcelain", "-z"])?;
    let mut path = None;
    let mut main = None;
    for field in worktrees.split('\0') {
        if let Some(value) = field.strip_prefix("worktree ") {
            path = Some(PathBuf::from(value));
        } else if field == "branch refs/heads/main" {
            anyhow::ensure!(
                main.is_none(),
                "more than one checkout has local main checked out"
            );
            main = path.clone();
        }
    }
    let main = main
        .context("local main must be checked out in a Git worktree before a source update")?
        .canonicalize()
        .context("resolve local main checkout")?;
    let commit = git(&main, &["rev-parse", "HEAD"])?.trim().to_owned();
    anyhow::ensure!(
        git(
            &main,
            &["status", "--porcelain=v1", "--untracked-files=normal"]
        )?
        .is_empty(),
        "local main checkout is dirty; commit or remove its changes before a source update"
    );
    anyhow::ensure!(
        git(&main, &["symbolic-ref", "HEAD"])?.trim() == "refs/heads/main"
            && git(&main, &["rev-parse", "HEAD"])?.trim() == commit,
        "local main changed while it was being inspected; retry the update"
    );
    Ok((main, commit))
}

fn read_record(path: &Path, operation: &str) -> Result<Option<BuildRecord>> {
    match fs::read(path) {
        Ok(bytes) => {
            let record: BuildRecord =
                serde_json::from_slice(&bytes).context("decode pinned source build")?;
            anyhow::ensure!(
                record.operation == operation,
                "source build operation identity mismatch"
            );
            Ok(Some(record))
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("read pinned source build"),
    }
}

pub(super) fn pin(store: &Store, inventory: &Inventory) -> Result<BuildRecord> {
    anyhow::ensure!(
        std::env::consts::OS == "linux" && matches!(std::env::consts::ARCH, "x86_64" | "aarch64"),
        "source updates require a Linux x86_64 or aarch64 server"
    );
    let directory = store.directory.join("source");
    yinz_runtime::shared::ensure_secure_dir(&directory).context("secure source build directory")?;
    let record_path = directory.join("build.json");
    let mut record = if let Some(record) = read_record(&record_path, &store.journal.operation)? {
        record
    } else {
        let registry: serde_json::Value = serde_json::from_slice(&output(
            session_command(inventory).args(["action", "project-list"]),
        )?)
        .context("read registered source project")?;
        let projects: Vec<project_data::projects::Project> =
            serde_json::from_value(registry["projects"].clone())
                .context("decode registered projects")?;
        let project = projects.iter().find(|project| project_data::projects::source_update_project(project))
            .context("register a project named yinz pointing to github.com/zacharyhamm/yinz before a source update")?;
        let (repository, commit) = main_checkout(Path::new(&project.root))?;
        let record = BuildRecord {
            operation: store.journal.operation.clone(),
            repository,
            commit,
            version: String::new(),
            snapshot: directory.join("checkout"),
            targets: Vec::new(),
            log: directory.join("build.log"),
            artifacts: BTreeMap::new(),
            sha256: BTreeMap::new(),
            pane: None,
            snapshot_ready: false,
            pane_closed: false,
        };
        // Pin before extraction: interruption must never select a newer main.
        journal::atomic_json(&record_path, &record)?;
        record
    };
    if !record.snapshot_ready && record.snapshot.exists() {
        let marker: serde_json::Value = serde_json::from_slice(
            &fs::read(record.snapshot.join(".yinz-source-snapshot.json"))
                .context("inspect interrupted pinned snapshot")?,
        )
        .context("decode pinned snapshot identity")?;
        anyhow::ensure!(
            marker["commit"] == record.commit && marker["operation"] == record.operation,
            "interrupted snapshot does not match this operation's pinned commit"
        );
        record.snapshot_ready = true;
        journal::atomic_json(&record_path, &record)?;
    }
    if !record.snapshot_ready {
        let stage = tempfile::tempdir_in(&directory).context("create isolated source snapshot")?;
        let archive = stage.path().join("source.tar");
        output(
            Command::new("git")
                .arg("-C")
                .arg(&record.repository)
                .args(["archive", "--format=tar", "--output"])
                .arg(&archive)
                .arg(&record.commit),
        )?;
        let checkout = stage.path().join("checkout");
        fs::create_dir(&checkout).context("create snapshot checkout")?;
        tar::Archive::new(fs::File::open(&archive).context("open pinned source archive")?)
            .unpack(&checkout)
            .context("extract pinned source snapshot")?;
        anyhow::ensure!(
            !record.snapshot.exists(),
            "an interrupted snapshot already exists; inspect the operation before retrying"
        );
        journal::atomic_json(
            &checkout.join(".yinz-source-snapshot.json"),
            &serde_json::json!({"commit":record.commit,"operation":record.operation}),
        )?;
        fs::rename(checkout, &record.snapshot).context("publish pinned source snapshot")?;
        record.snapshot_ready = true;
        journal::atomic_json(&record_path, &record)?;
    }
    if let Ok(metadata) = fs::symlink_metadata(record.snapshot.join("target")) {
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "source snapshot must have a real checkout-local target directory"
        );
    }
    if record.version.is_empty() {
        let metadata: serde_json::Value = serde_json::from_slice(&output(
            Command::new("cargo").current_dir(&record.snapshot).args([
                "metadata",
                "--no-deps",
                "--locked",
                "--format-version",
                "1",
            ]),
        )?)
        .context("read pinned source package metadata")?;
        anyhow::ensure!(
            metadata["target_directory"].as_str().map(Path::new)
                == Some(record.snapshot.join("target").as_path()),
            "pinned source must use its own checkout-local target directory"
        );
        record.version = metadata["packages"]
            .as_array()
            .context("source metadata has no packages")?
            .iter()
            .find(|package| package["name"] == "yinz")
            .and_then(|package| package["version"].as_str())
            .context("pinned source has no yinz package version")?
            .to_owned();
        super::update::version(&record.version)?;
        journal::atomic_json(&record_path, &record)?;
    }
    Ok(record)
}

fn targets(targets: &[String]) -> Result<Vec<String>> {
    let mut targets = targets.to_vec();
    targets.sort();
    targets.dedup();
    anyhow::ensure!(
        !targets.is_empty(),
        "source build requires at least one target"
    );
    for target in &targets {
        anyhow::ensure!(
            matches!(
                target.as_str(),
                "x86_64-unknown-linux-gnu" | "aarch64-unknown-linux-gnu"
            ),
            "source updates do not support target {target}"
        );
    }
    Ok(targets)
}

fn retain_pinned_targets(pinned: &[String], requested: &[String]) -> Result<Vec<String>> {
    if pinned.is_empty() {
        return Ok(requested.to_vec());
    }
    // A disconnected client may remove a required architecture. Keep the
    // original build and its artifacts; only an added target requires a new op.
    anyhow::ensure!(requested.iter().all(|target| pinned.contains(target)),
        "a participant requires an unbuilt source target; start a new update instead of changing a pinned build");
    Ok(pinned.to_vec())
}

fn digest(path: &Path) -> Result<String> {
    let mut input = fs::File::open(path).context("open built source artifact")?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = input
            .read(&mut buffer)
            .context("hash built source artifact")?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(super::update::hex(&hash.finalize()))
}

fn panes(inventory: &Inventory) -> Result<Vec<serde_json::Value>> {
    serde_json::from_slice(&output(session_command(inventory).args([
        "action",
        "list-panes",
        "--json",
    ]))?)
    .context("inspect source build pane")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaneState {
    Missing,
    Running,
    Exited,
}

fn pane_state(inventory: &Inventory, pane: &str) -> Result<PaneState> {
    let id: u64 = pane
        .strip_prefix("terminal_")
        .context("invalid build pane identity")?
        .parse()
        .context("parse build pane id")?;
    Ok(
        match panes(inventory)?
            .iter()
            .find(|value| value["id"].as_u64() == Some(id) && value["is_plugin"] != true)
        {
            None => PaneState::Missing,
            Some(value) if value["exited"] == true => PaneState::Exited,
            Some(_) => PaneState::Running,
        },
    )
}

fn pane_present(inventory: &Inventory, pane: &str) -> Result<bool> {
    Ok(pane_state(inventory, pane)? != PaneState::Missing)
}

pub(super) fn build(
    store: &Store,
    inventory: &Inventory,
    requested: &[String],
) -> Result<BuildRecord> {
    let requested = targets(requested)?;
    let mut record = pin(store, inventory)?;
    let directory = store.directory.join("source");
    let record_path = directory.join("build.json");
    record.targets = retain_pinned_targets(&record.targets, &requested)?;
    if record.artifacts.len() == record.targets.len() {
        for (target, path) in &record.artifacts {
            anyhow::ensure!(
                record.sha256.get(target) == Some(&digest(path)?),
                "pinned source artifact changed: {target}"
            );
        }
        if !record.pane_closed {
            if let Some(pane) = &record.pane {
                close_successful_pane(inventory, pane)?;
            }
            record.pane_closed = true;
            journal::atomic_json(&record_path, &record)?;
        }
        return Ok(record);
    }
    anyhow::ensure!(
        std::env::var_os("CARGO_TARGET_DIR").is_none(),
        "source builds do not accept CARGO_TARGET_DIR"
    );
    let script = directory.join("build.py");
    fs::write(&script, include_str!("source_build.py")).context("write source build worker")?;
    journal::atomic_json(&record_path, &record)?;
    if record.pane.is_none() {
        // Save launch intent first: a lost CLI acknowledgement must never duplicate compilation.
        let intent = directory.join("launch.json");
        anyhow::ensure!(!intent.exists(), "source build pane launch was interrupted; inspect its pane and build log before retrying");
        journal::atomic_json(&intent, &serde_json::json!({"operation":record.operation}))?;
        let reply = output(
            session_command(inventory)
                .args([
                    "action",
                    "new-pane",
                    "--name",
                    "Yinz source update",
                    "--cwd",
                ])
                .arg(&record.snapshot)
                .args(["--", "python3"])
                .arg(&script)
                .arg(&record_path),
        )?;
        let pane = String::from_utf8(reply)
            .context("decode source build pane")?
            .trim()
            .to_owned();
        anyhow::ensure!(
            pane.strip_prefix("terminal_")
                .is_some_and(|id| id.parse::<u32>().is_ok()),
            "source build pane launch returned an invalid identity"
        );
        record.pane = Some(pane);
        journal::atomic_json(&record_path, &record)?;
    }
    let pane = record
        .pane
        .as_deref()
        .context("source build pane is missing")?;
    let result_path = directory.join("result.json");
    let mut next_probe = Instant::now();
    loop {
        match fs::read(&result_path) {
            Ok(bytes) => {
                let result: serde_json::Value =
                    serde_json::from_slice(&bytes).context("decode source build result")?;
                // Closing the pane can make its SIGHUP failure receipt arrive
                // before our next periodic pane probe. Inspect ownership first
                // so neither a failure nor a successful receipt masks closure.
                let present = pane_present(inventory, pane)?;
                if !present {
                    stop(&directory)?;
                }
                validate_result(&result, present, &record.log)?;
                break;
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error).context("read source build result"),
        }
        if Instant::now() >= next_probe {
            match pane_state(inventory, pane)? {
                PaneState::Missing => {
                    stop(&directory)?;
                    return Err(TerminalFailure::PaneClosed.into());
                },
                PaneState::Exited => {
                    // The worker can publish its receipt between our read and
                    // this probe. Re-read after observing exit before failing.
                    match fs::read(&result_path) {
                        Ok(_) => continue,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                        Err(error) => return Err(error).context("recheck source build result"),
                    }
                    stop(&directory)?;
                    return Err(TerminalFailure::WorkerExited { log: record.log }.into());
                },
                PaneState::Running => {},
            }
            next_probe = Instant::now() + Duration::from_secs(1);
        }
        std::thread::park_timeout(Duration::from_millis(100));
    }
    for target in &record.targets {
        let artifact = record
            .snapshot
            .join("dist")
            .join(format!("yinz-{target}.tar.gz"));
        record.sha256.insert(target.clone(), digest(&artifact)?);
        record.artifacts.insert(target.clone(), artifact);
    }
    journal::atomic_json(&record_path, &record)?;
    close_successful_pane(inventory, pane)?;
    record.pane_closed = true;
    journal::atomic_json(&record_path, &record)?;
    Ok(record)
}

fn validate_result(result: &serde_json::Value, pane_present: bool, log: &Path) -> Result<()> {
    if !pane_present {
        // The caller has successfully stopped compilation before validating a
        // receipt whose pane disappeared.
        return Err(TerminalFailure::PaneClosed.into());
    }
    anyhow::ensure!(
        result["success"] == true,
        "source build failed: {}; pane and log retained at {}",
        result["error"].as_str().unwrap_or("see build output"),
        log.display()
    );
    Ok(())
}

fn close_successful_pane(inventory: &Inventory, pane: &str) -> Result<()> {
    if !pane_present(inventory, pane)? {
        return Ok(());
    }
    output(session_command(inventory).args(["action", "close-pane", "--pane-id", pane]))?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while pane_present(inventory, pane)? {
        anyhow::ensure!(
            Instant::now() < deadline,
            "successful source build pane did not close; refusing to migrate it"
        );
        std::thread::park_timeout(Duration::from_millis(25));
    }
    Ok(())
}

pub(super) fn cancel(store: &Store) -> Result<()> {
    let directory = store.directory.join("source");
    if directory.join("launch.json").exists() {
        // Launch intent precedes every worker. Before that point cancellation
        // needs no tools, even if preflight failed because Python is missing.
        fs::write(directory.join("build.py"), include_str!("source_build.py"))
            .context("write source build cancellation worker")?;
        stop(&directory)?;
    }
    Ok(())
}

fn stop(directory: &Path) -> Result<()> {
    // The worker validates the saved Linux process incarnation before signalling
    // its build group; it never guesses ownership from a recycled PID.
    output(
        Command::new("python3")
            .arg(directory.join("build.py"))
            .arg("--stop")
            .arg(directory.join("process.json")),
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "../unit/source_build.rs"]
mod tests;

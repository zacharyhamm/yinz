use super::*;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Discover,
    Prepare,
    Quiesce,
    Transfer,
    Reconnect,
    Commit,
    Complete,
    Recovering,
    RecoveryRequired,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Route {
    Server,
    Client(yinz_types::data::ClientId),
    Remote {
        host: String,
        helper: PathBuf,
        directory: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Participant {
    pub installation: Installation,
    pub route: Route,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteHelper {
    pub host: String,
    pub directory: PathBuf,
    pub cleaned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    pub format: u32,
    #[serde(default)]
    pub source_build: bool,
    pub operation: String,
    pub phase: Phase,
    pub source_socket: PathBuf,
    pub replacement_socket: PathBuf,
    pub source: Option<yinz_types::session_update::Inventory>,
    #[serde(default)]
    pub replacement: Option<yinz_types::session_update::Inventory>,
    #[serde(default)]
    pub replacement_identity: Option<yinz_types::session_update::ServerIdentity>,
    /// A restarted old-generation server recovering a pre-commit source crash.
    #[serde(default)]
    pub recovery_source_identity: Option<yinz_types::session_update::ServerIdentity>,
    #[serde(default)]
    pub source_restored: bool,
    /// Durable forward-only decision written before readers/input are enabled.
    #[serde(default)]
    pub handoff_committed: bool,
    pub migration: Option<yinz_types::session_migration::SessionMigrationInventory>,
    pub release: Option<serde_json::Value>,
    pub participants: Vec<Participant>,
    #[serde(default)]
    pub remote_helpers: Vec<RemoteHelper>,
    pub snapshot: PathBuf,
    pub mapping: Option<yinz_types::session_migration::SessionMigrationMap>,
    /// Written before asking a client to exec. Disappearance after this point
    /// is ambiguous until that original client acknowledges replacement readiness.
    #[serde(default)]
    pub restart_attempted: Vec<yinz_types::data::ClientId>,
    pub restarted_clients: Vec<yinz_types::data::ClientId>,
    pub disconnected_clients: Vec<yinz_types::data::ClientId>,
    pub restart_required: Vec<yinz_types::data::ClientId>,
    pub errors: Vec<String>,
}

pub struct Store {
    pub directory: PathBuf,
    pub journal: Journal,
    _lock: File,
}

impl Store {
    pub fn open(socket: &Path, data_dir: &Path, operation: &str) -> Result<Self> {
        Self::open_with_source(socket, data_dir, operation, false)
    }

    pub fn open_with_source(
        socket: &Path,
        data_dir: &Path,
        operation: &str,
        source_build: bool,
    ) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        uuid::Uuid::parse_str(operation).context("invalid update operation id")?;
        let root = data_dir.join("updates");
        yinz_runtime::shared::ensure_secure_dir(&root)
            .context("secure update journal directory")?;
        let socket = yinz_runtime::session_update::canonical_socket(socket)?;
        let active_path = yinz_runtime::session_update::record(data_dir, &socket)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(active_path.with_extension("lock"))
            .context("open session update lock")?;
        lock.try_lock()
            .context("an update coordinator is already running for this session")?;
        let actual_operation = match fs::read(&active_path) {
            Ok(bytes) => {
                let active: String =
                    serde_json::from_slice(&bytes).context("read active update operation")?;
                uuid::Uuid::parse_str(&active).context("invalid active update operation")?;
                let old: Journal = serde_json::from_slice(
                    &fs::read(root.join(&active).join("journal.json"))
                        .context("read previous update journal")?,
                )
                .context("decode previous update journal")?;
                if matches!(
                    old.phase,
                    Phase::Complete | Phase::Failed | Phase::Cancelled
                ) {
                    operation.to_owned()
                } else {
                    active
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => operation.to_owned(),
            Err(e) => return Err(e).context("inspect active update operation"),
        };
        let directory = root.join(&actual_operation);
        yinz_runtime::shared::ensure_secure_dir(&directory)
            .context("secure operation directory")?;
        let directory = directory
            .canonicalize()
            .context("resolve operation directory")?;
        let path = directory.join("journal.json");
        let journal = match fs::read(&path) {
            Ok(bytes) => {
                let mut journal: Journal =
                    serde_json::from_slice(&bytes).context("decode interrupted update journal")?;
                anyhow::ensure!(
                    journal.format == 1
                        && journal.operation == actual_operation
                        && journal.source_socket == socket,
                    "update journal identity or format mismatch"
                );
                // The snapshot always lives in this operation directory. A
                // relative path recorded from a relative data directory must
                // not depend on whichever directory this process started in.
                if journal.snapshot.is_relative() {
                    journal.snapshot = directory.join(
                        journal
                            .snapshot
                            .file_name()
                            .context("interrupted snapshot path has no file name")?,
                    );
                }
                journal
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Journal {
                format: 1,
                source_build,
                operation: actual_operation.clone(),
                phase: Phase::Discover,
                source_socket: socket.clone(),
                replacement_socket: socket.with_file_name(format!(".update-{}", actual_operation)),
                source: None,
                replacement: None,
                replacement_identity: None,
                recovery_source_identity: None,
                source_restored: false,
                handoff_committed: false,
                migration: None,
                release: None,
                participants: Vec::new(),
                remote_helpers: Vec::new(),
                snapshot: directory.join("snapshot.json"),
                mapping: None,
                restart_attempted: Vec::new(),
                restarted_clients: Vec::new(),
                disconnected_clients: Vec::new(),
                restart_required: Vec::new(),
                errors: Vec::new(),
            },
            Err(e) => return Err(e).context("read update journal"),
        };
        let store = Self {
            directory,
            journal,
            _lock: lock,
        };
        store.save()?;
        atomic_json(&active_path, &actual_operation)?;
        Ok(store)
    }
    pub fn save(&self) -> Result<()> {
        atomic_json(&self.directory.join("journal.json"), &self.journal)
    }
    pub fn phase(&mut self, phase: Phase) -> Result<()> {
        let _guard = yinz_runtime::session_update::lock_phase(&self.directory)?;
        anyhow::ensure!(
            phase != Phase::Transfer || !self.cancel_requested(),
            "update cancelled before terminal transfer"
        );
        self.journal.phase = phase;
        self.save()?;
        log::info!(
            "session update operation={} phase={phase:?}",
            self.journal.operation
        );
        Ok(())
    }
    pub fn cancel_requested(&self) -> bool {
        self.directory.join("cancel").exists()
    }
}

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("journal has no parent")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).context("stage update journal")?;
    serde_json::to_writer(&mut file, value).context("write update journal")?;
    file.flush().context("flush update journal")?;
    file.as_file().sync_all().context("sync update journal")?;
    file.persist(path).context("publish update journal")?;
    File::open(parent)
        .context("open update journal directory")?
        .sync_all()
        .context("sync update journal directory")
}

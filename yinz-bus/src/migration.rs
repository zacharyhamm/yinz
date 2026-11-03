//! Whole-session transfer uses a private snapshot file rather than one bounded
//! IPC message. Callers must negotiate the format before sending these requests.
use crate::{screen::ScreenInstruction, thread_bus::ThreadSenders};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use yinz_runtime::errors::prelude::*;
use yinz_types::{
    data::HeldTerminal,
    internal_ids::{ClientId, TerminalId},
    session_migration::*,
};

pub type Reply<T> = crossbeam::channel::Sender<Result<T, String>>;

#[derive(Clone, Debug)]
pub enum ScreenRequest {
    Prepare {
        operation: String,
        client: ClientId,
        reply: Reply<SessionMigrationInventory>,
    },
    Release {
        operation: String,
        snapshot: PathBuf,
        client: ClientId,
        deadline: Instant,
        reply: Reply<SessionMigrationSnapshot>,
    },
    ReadersPaused {
        operation: String,
        result: Result<Vec<(TerminalId, crate::os_input_output::ReleasedPane)>, String>,
    },
    InputDrained {
        operation: String,
        result: Result<Vec<(TerminalId, crate::os_input_output::ReleasedPane)>, String>,
    },
    Released {
        operation: String,
        results: Vec<(TerminalId, Result<crate::warp::ReleasedPaneForWarp, String>)>,
    },
    Adopt {
        operation: String,
        snapshot: PathBuf,
        client: ClientId,
        reply: Reply<SessionMigrationMap>,
    },
    Adopted {
        snapshot: Box<SessionMigrationSnapshot>,
        result: Result<Vec<crate::pty::AdoptedTabPanes>, String>,
        client: ClientId,
        path: PathBuf,
        reply: Reply<SessionMigrationMap>,
    },
    Abort {
        operation: String,
        reply: Reply<()>,
    },
    Aborted {
        operation: String,
        result: Result<(), String>,
        reply: Reply<()>,
    },
    Finish {
        operation: String,
        reply: Reply<()>,
    },
    RestoreClient {
        operation: String,
        source: ClientId,
        destination: ClientId,
        reply: Reply<()>,
    },
    Rename {
        operation: String,
        name: yinz_types::internal_ids::SessionName,
        client: ClientId,
        reply: Reply<()>,
    },
    PluginsPaused {
        operation: String,
        result: Result<(), String>,
    },
    PluginsResumed {
        operation: String,
        result: Result<(), String>,
    },
    TabReplayed {
        operation: String,
        tab: yinz_types::internal_ids::TabId,
    },
    SourceReconciled {
        operation: String,
        path: PathBuf,
        client: ClientId,
        reply: Reply<SessionMigrationMap>,
        result: Result<Box<SessionMigrationSnapshot>, String>,
    },
}

#[derive(Clone, Debug)]
pub enum PtyRequest {
    Prepare {
        inventory: SessionMigrationInventory,
        terminals: Vec<TerminalId>,
        held: HashMap<TerminalId, HeldTerminal>,
        reply: Reply<SessionMigrationInventory>,
    },
    Pause {
        operation: String,
        terminals: Vec<TerminalId>,
        held: HashMap<TerminalId, HeldTerminal>,
        deadline: Instant,
    },
    Release {
        operation: String,
        terminals: Vec<TerminalId>,
        held: HashMap<TerminalId, HeldTerminal>,
        deadline: Instant,
        path: PathBuf,
    },
    ReconcileSource {
        operation: String,
        path: PathBuf,
        client: ClientId,
        reply: Reply<SessionMigrationMap>,
    },
    Resume,
    Abort {
        operation: String,
        reply: Reply<()>,
    },
    Adopt {
        snapshot: Box<SessionMigrationSnapshot>,
        client: ClientId,
        path: PathBuf,
        reply: Reply<SessionMigrationMap>,
    },
}

fn request<T>(
    senders: &ThreadSenders,
    timeout: Duration,
    build: impl FnOnce(Reply<T>) -> ScreenRequest,
) -> Result<T> {
    let (tx, rx) = crossbeam::channel::bounded(1);
    senders
        .send_to_screen(ScreenInstruction::SessionMigration(build(tx)))
        .context("send session migration request")?;
    rx.recv_timeout(timeout)
        .context("wait for session migration reply; inspect the operation journal before retrying")?
        .map_err(|e| anyhow!("{e}"))
}
pub fn prepare_session(
    senders: &ThreadSenders,
    operation: &str,
    client: ClientId,
    timeout: Duration,
) -> Result<SessionMigrationInventory> {
    validate_operation(operation)?;
    request(senders, timeout, |reply| ScreenRequest::Prepare {
        operation: operation.into(),
        client,
        reply,
    })
}
pub fn release_session(
    senders: &ThreadSenders,
    operation: &str,
    snapshot: &Path,
    client: ClientId,
    timeout: Duration,
) -> Result<SessionMigrationSnapshot> {
    validate_operation(operation)?;
    request(senders, timeout, |reply| ScreenRequest::Release {
        operation: operation.into(),
        snapshot: snapshot.into(),
        client,
        deadline: Instant::now() + timeout,
        reply,
    })
}
pub fn adopt_session(
    senders: &ThreadSenders,
    operation: &str,
    snapshot: &Path,
    client: ClientId,
    timeout: Duration,
) -> Result<SessionMigrationMap> {
    validate_operation(operation)?;
    request(senders, timeout, |reply| ScreenRequest::Adopt {
        operation: operation.into(),
        snapshot: snapshot.into(),
        client,
        reply,
    })
}
pub fn abort_session(senders: &ThreadSenders, operation: &str, timeout: Duration) -> Result<()> {
    request(senders, timeout, |reply| ScreenRequest::Abort {
        operation: operation.into(),
        reply,
    })
}
pub fn finish_session(senders: &ThreadSenders, operation: &str, timeout: Duration) -> Result<()> {
    request(senders, timeout, |reply| ScreenRequest::Finish {
        operation: operation.into(),
        reply,
    })
}
pub fn restore_client(
    senders: &ThreadSenders,
    operation: &str,
    source: ClientId,
    destination: ClientId,
    timeout: Duration,
) -> Result<()> {
    request(senders, timeout, |reply| ScreenRequest::RestoreClient {
        operation: operation.into(),
        source,
        destination,
        reply,
    })
}
pub fn rename_session(
    senders: &ThreadSenders,
    operation: &str,
    name: yinz_types::internal_ids::SessionName,
    client: ClientId,
    timeout: Duration,
) -> Result<()> {
    request(senders, timeout, |reply| ScreenRequest::Rename {
        operation: operation.into(),
        name,
        client,
        reply,
    })
}
pub fn validate_operation(operation: &str) -> Result<()> {
    uuid::Uuid::parse_str(operation).context("invalid session update operation ID")?;
    Ok(())
}

/// Only files created privately for this operation are replaced. A mismatch is
/// an error, not permission to destroy another operation's recovery snapshot.
pub fn save_snapshot(path: &Path, snapshot: &SessionMigrationSnapshot) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    validate_operation(&snapshot.operation)?;
    anyhow::ensure!(
        path.is_absolute(),
        "migration snapshot path must be absolute"
    );
    if path.exists() {
        // Saves repeat per terminal during release and adoption. Check only the
        // identity fields instead of rebuilding every serialized terminal grid.
        let previous = load_snapshot_identity(path)?;
        anyhow::ensure!(
            previous.operation == snapshot.operation,
            "migration snapshot belongs to another operation"
        );
        anyhow::ensure!(
            previous.format == SESSION_MIGRATION_FORMAT,
            "unsupported migration snapshot format {}",
            previous.format
        );
        anyhow::ensure!(
            previous.inventory.instance_id == snapshot.inventory.instance_id,
            "snapshot belongs to a different source server"
        );
    }
    let parent = path.parent().context("missing snapshot parent")?;
    let temporary = parent.join(format!(".migration-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .context("create private migration snapshot")?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, snapshot)
            .context("serialize complete session migration snapshot")?;
        writer.flush().context("flush migration snapshot")?;
        writer
            .get_ref()
            .sync_all()
            .context("sync migration snapshot")?;
        fs::rename(&temporary, path).context("publish migration snapshot")?;
        File::open(parent)
            .context("open snapshot directory")?
            .sync_all()
            .context("sync snapshot directory")
    })();
    if result.is_err() {
        fs::remove_file(&temporary)
            .context("remove unpublished snapshot")
            .non_fatal();
    }
    result
}
#[derive(serde::Deserialize)]
struct SnapshotIdentity {
    format: u32,
    operation: String,
    inventory: InventoryIdentity,
}

#[derive(serde::Deserialize)]
struct InventoryIdentity {
    instance_id: String,
}

fn open_private_snapshot(path: &Path) -> Result<BufReader<File>> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).context("inspect migration snapshot")?;
    anyhow::ensure!(
        metadata.is_file() && metadata.mode() & 0o077 == 0,
        "migration snapshot must be a private regular file"
    );
    Ok(BufReader::new(
        File::open(path).context("open migration snapshot")?,
    ))
}

fn load_snapshot_identity(path: &Path) -> Result<SnapshotIdentity> {
    serde_json::from_reader(open_private_snapshot(path)?)
        .context("read migration snapshot identity")
}

pub fn load_snapshot(path: &Path, operation: &str) -> Result<SessionMigrationSnapshot> {
    let snapshot: SessionMigrationSnapshot =
        serde_json::from_reader(open_private_snapshot(path)?).context("read migration snapshot")?;
    anyhow::ensure!(
        snapshot.operation == operation,
        "migration snapshot belongs to another operation"
    );
    anyhow::ensure!(
        snapshot.format == SESSION_MIGRATION_FORMAT,
        "unsupported migration snapshot format {}",
        snapshot.format
    );
    Ok(snapshot)
}

#[cfg(test)]
#[path = "unit/migration_tests.rs"]
mod tests;

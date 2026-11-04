//! Host-local update participation. Helpers outlive the client and server that started them.
use crate::update;
use isahc::{config::RedirectPolicy, prelude::*, HttpClient};
use std::{
    io::{Read, Write},
    path::Path,
    time::Duration,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::{
    session_update::{Capabilities, Installation},
    update_client::{ParticipantInvocation, ParticipantRequest},
};
mod artifacts;
mod coordinator;
mod journal;
mod source_build;
mod transport;
pub(crate) use coordinator::run_with_channel as coordinator_main;

pub(crate) fn resume_client_update(
    session: &str,
    data_dir: Option<&Path>,
    original: u32,
    current: u32,
) -> Result<()> {
    yinz_client::remote_sessions::validate_name(session)?;
    let data_dir = data_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(yinz_runtime::home::get_default_data_dir);
    coordinator::resume_client(
        &yinz_runtime::consts::YINZ_SOCK_DIR.join(session),
        &data_dir,
        yinz_types::data::ClientId::new(original),
        yinz_types::data::ClientId::new(current),
    )?;
    println!("Client replacement confirmed; session update recovery completed.");
    Ok(())
}

pub(crate) fn capabilities() -> Capabilities {
    Capabilities {
        prerelease_update: true,
        source_binary_sha256: yinz_runtime::session_update::running_executable_digest().ok(),
        source_update: true,
        build_version: yinz_runtime::consts::VERSION.into(),
        client_server: yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION,
        ssh_bootstrap: yinz_runtime::consts::REMOTE_BOOTSTRAP_VERSION,
        update: yinz_types::session_update::PROTOCOL_VERSION,
        migration: yinz_types::session_migration::SESSION_MIGRATION_FORMAT,
    }
}

fn http_client() -> Result<HttpClient> {
    HttpClient::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(600))
        .redirect_policy(RedirectPolicy::Limit(10))
        .build()
        .context("initialize participant release downloader")
}

fn decode_configuration(value: &serde_json::Value) -> Result<yinz_config::config::Config> {
    yinz_config::config::Config::from_kdl(
        value
            .as_str()
            .context("update configuration is not KDL text")?,
        None,
    )
    .map_err(|error| anyhow!("{error}"))
    .context("decode effective update configuration")
}

fn executable_version(executable: &Path) -> Result<String> {
    let output = yinz_process::ssh::bootstrap_output(
        std::process::Command::new(executable).arg("--version"),
        Duration::from_secs(20),
        None,
    )
    .context("read installed executable version")?;
    anyhow::ensure!(
        output.status.success(),
        "installed executable version query failed"
    );
    let stdout = std::str::from_utf8(&output.stdout).context("decode executable version")?;
    let version = stdout
        .trim()
        .strip_prefix("yinz ")
        .context("invalid executable version response")?;
    update::version(version)?;
    Ok(version.to_owned())
}

fn validate_capabilities(executable: &Path) -> Result<Capabilities> {
    let output = yinz_process::ssh::bootstrap_output(
        std::process::Command::new(executable).arg(yinz_types::session_update::CAPABILITIES_ARG),
        Duration::from_secs(20),
        None,
    )
    .context("query staged update capabilities")?;
    anyhow::ensure!(
        output.status.success(),
        "staged executable does not support session updates"
    );
    let peer: Capabilities =
        serde_json::from_slice(&output.stdout).context("decode staged update capabilities")?;
    let ours = capabilities();
    anyhow::ensure!(
        peer.client_server == ours.client_server
            && peer.ssh_bootstrap == ours.ssh_bootstrap
            && peer.update == ours.update
            && peer.migration == ours.migration,
        "staged executable has incompatible update or migration contracts"
    );
    Ok(peer)
}

fn inventory(executable: &Path, data_dir: &Path) -> Result<Installation> {
    // Update participants sharing this data directory retain the same identity.
    // Do not use a process-global override: SSH aliases must resolve identically.
    std::fs::create_dir_all(data_dir).context("prepare participant data directory")?;
    let data_dir = data_dir
        .canonicalize()
        .context("resolve participant data directory")?;
    let identity_path = data_dir.join(".update-host-id");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(data_dir.join(".update-host-id.lock"))
        .context("open participant identity lock")?;
    lock.lock().context("lock participant identity")?;
    let host = match std::fs::read_to_string(&identity_path) {
        Ok(id) => {
            anyhow::ensure!(
                uuid::Uuid::parse_str(id.trim()).is_ok(),
                "invalid participant host identity"
            );
            id.trim().to_owned()
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = uuid::Uuid::new_v4().to_string();
            let mut temp =
                tempfile::NamedTempFile::new_in(&data_dir).context("stage participant identity")?;
            temp.write_all(id.as_bytes())
                .context("write participant identity")?;
            temp.as_file()
                .sync_all()
                .context("sync participant identity")?;
            temp.persist(&identity_path)
                .context("publish participant identity")?;
            std::fs::File::open(&data_dir)
                .context("open participant identity directory")?
                .sync_all()
                .context("persist participant identity directory")?;
            id
        },
        Err(e) => return Err(e).context("read participant host identity"),
    };
    Ok(Installation {
        host,
        executable: executable
            .canonicalize()
            .context("resolve participant executable")?,
        data_dir,
        running_version: yinz_runtime::consts::VERSION.into(),
        installed_version: executable_version(executable)?,
        target: update::target(std::env::consts::OS, std::env::consts::ARCH)?.into(),
        capabilities: capabilities(),
    })
}

pub(crate) fn participate(invocation: ParticipantInvocation) -> Result<serde_json::Value> {
    let data_dir = invocation
        .data_dir
        .unwrap_or_else(yinz_runtime::home::get_default_data_dir);
    let executable = invocation.executable;
    match invocation.request {
        ParticipantRequest::Inventory => serde_json::to_value(inventory(&executable, &data_dir)?)
            .context("encode installation inventory"),
        ParticipantRequest::SourceChunk {
            operation,
            artifact,
            offset,
            bytes,
        } => artifacts::receive(&data_dir, &operation, &artifact, offset, &bytes),
        ParticipantRequest::Prepare { operation, release } => {
            uuid::Uuid::parse_str(&operation).context("invalid update operation id")?;
            transport::preflight_local_sessions()?;
            let canonical_executable = executable
                .canonicalize()
                .context("resolve prepared installation")?;
            let receipt_dir = data_dir
                .join("updates")
                .join(&operation)
                .join("participants");
            yinz_runtime::shared::ensure_secure_dir(&receipt_dir)
                .context("secure participant receipts")?;
            use sha2::{Digest, Sha256};
            let key = update::hex(&Sha256::digest(
                canonical_executable.as_os_str().as_encoded_bytes(),
            ));
            let receipt_path = receipt_dir.join(format!("{key}.json"));
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(receipt_dir.join(format!("{key}.lock")))
                .context("open participant preparation lock")?;
            lock.try_lock()
                .context("this participant preparation is already running")?;
            match std::fs::read(&receipt_path) {
                Ok(bytes) => {
                    let pinned: serde_json::Value = serde_json::from_slice(&bytes)
                        .context("decode pinned participant release")?;
                    anyhow::ensure!(
                        pinned == release,
                        "operation id reused with different release metadata"
                    );
                    if let Some(transaction) = update::transaction::Transaction::lookup(
                        &executable,
                        &data_dir,
                        &operation,
                    )? {
                        anyhow::ensure!(
                            transaction.status() != update::transaction::Status::RolledBack,
                            "this operation was rolled back; begin a new update"
                        );
                        // Transaction::prepare refuses an interrupted Staging journal,
                        // so fail before downloading the release again.
                        anyhow::ensure!(
                            transaction.status() != update::transaction::Status::Staging,
                            "update preparation was interrupted; recover operation {operation} before retrying"
                        );
                        return serde_json::to_value(transaction.status())
                            .context("encode existing participant status");
                    }
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    journal::atomic_json(&receipt_path, &release)?
                },
                Err(error) => return Err(error).context("read pinned participant release"),
            }
            let release: update::Release =
                serde_json::from_value(release).context("decode pinned release")?;
            let identity = inventory(&executable, &data_dir)?;
            let target = update::version(&release.tag_name)?;
            anyhow::ensure!(
                release.source.is_some()
                    || (update::version(&identity.installed_version)? <= target
                        && update::version(&identity.running_version)? <= target),
                "refusing to downgrade a newer update participant"
            );
            let prepared = if release.source.is_some() {
                artifacts::prepare(&release, &identity.target, &data_dir, &operation)?
            } else {
                let client = http_client()?;
                update::prepare(
                    &release,
                    &identity.target,
                    &mut |url, output| update::download(&client, url, output),
                    &mut |_| {},
                )?
            };
            let staged = validate_capabilities(&prepared.source.join("yinz"))?;
            if let Some(source) = &release.source {
                let artifact = source
                    .artifacts
                    .iter()
                    .find(|artifact| artifact.target == identity.target)
                    .context("source artifact missing for staged participant")?;
                anyhow::ensure!(staged.source_update && staged.source_binary_sha256.as_deref() == Some(&artifact.binary_sha256),
                    "staged executable lacks source update support or reports the wrong source identity");
            }
            let mut transaction = update::transaction::Transaction::prepare(
                &prepared.source,
                &executable,
                &data_dir,
                &operation,
            )?;
            // Downloads happen without installation locks. A different operation
            // may have finished meanwhile; check again while both locks are held.
            if release.source.is_none()
                && update::version(&executable_version(&executable)?)? > target
            {
                transaction.rollback().context("discard staged downgrade")?;
                bail!("installation became newer during preparation; refusing downgrade");
            }
            serde_json::to_value(transaction.status())
                .context("encode prepared installation status")
        },
        request => {
            let operation = request
                .operation()
                .context("participant operation is missing")?;
            let mut transaction = if matches!(request, ParticipantRequest::Rollback { .. }) {
                let Some(transaction) =
                    update::transaction::Transaction::lookup(&executable, &data_dir, operation)?
                else {
                    return serde_json::to_value(update::transaction::Status::RolledBack)
                        .context("encode untouched installation status");
                };
                transaction
            } else {
                update::transaction::Transaction::open(&executable, &data_dir, operation)?
            };
            match request {
                ParticipantRequest::Publish { .. } => transaction.publish()?,
                ParticipantRequest::Rollback { .. } => transaction.rollback()?,
                ParticipantRequest::Commit { .. } => transaction.commit()?,
                ParticipantRequest::Status { .. } => {},
                ParticipantRequest::Inventory
                | ParticipantRequest::SourceChunk { .. }
                | ParticipantRequest::Prepare { .. } => {
                    unreachable!()
                },
            }
            serde_json::to_value(transaction.status()).context("encode installation status")
        },
    }
}

pub(crate) fn participant_main() -> Result<()> {
    const REQUEST_LIMIT: u64 = 2 * 1024 * 1024;
    let mut input = Vec::new();
    std::io::stdin()
        .take(REQUEST_LIMIT + 1)
        .read_to_end(&mut input)
        .context("read update participant request")?;
    anyhow::ensure!(
        input.len() as u64 <= REQUEST_LIMIT,
        "update participant request is too large"
    );
    let invocation = serde_json::from_slice(&input).context("decode update participant request")?;
    let result = participate(invocation).map_err(|error| format!("{error:#}"));
    serde_json::to_writer(std::io::stdout().lock(), &result)
        .context("write update participant reply")?;
    Ok(())
}

#[cfg(test)]
#[path = "unit/live_update.rs"]
mod tests;

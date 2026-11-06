use super::*;
use super::{
    journal::{Journal, Participant, Phase, Route, Store},
    transport::Connection,
};
use std::{process::Command as Process, time::Instant};
use yinz_types::{
    data::{ClientId, SessionConnection},
    session_update::{Command, Inventory, ReleaseChannel},
    update_client::{ClientReply, ClientRequest, Request},
};

#[cfg(test)]
std::thread_local! {
    /// Subprocess regressions exercise failed readiness without waiting a minute.
    pub(super) static CLIENT_READY_TIMEOUT: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::from_secs(60)) };
    pub(super) static PREPARATION_TIMEOUT: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::from_secs(60)) };
}

/// An explicit user selection binds a current attachment to one missing original.
/// The binding is proved by a real exec and restored-frame receipt, never by a
/// matching version or installation alone.
pub(super) fn resume_client(
    socket: &Path,
    data_dir: &Path,
    original: ClientId,
    current: ClientId,
) -> Result<()> {
    let active = yinz_runtime::session_update::read_active(data_dir, socket)?
        .context("this session has no recorded update to recover")?;
    let operation = active["operation"]
        .as_str()
        .context("update journal has no operation")?;
    let mut store = Store::open(socket, data_dir, operation)?;
    anyhow::ensure!(
        store.journal.operation == operation,
        "the active update changed; inspect its status before retrying"
    );
    anyhow::ensure!(
        store.journal.handoff_committed,
        "this update has not committed a replacement session"
    );
    anyhow::ensure!(
        store.journal.restart_attempted.contains(&original),
        "original client {original} has no recorded replacement attempt"
    );
    if store.journal.phase == Phase::Complete && store.journal.restarted_clients.contains(&original)
    {
        return Ok(());
    }
    anyhow::ensure!(
        matches!(
            store.journal.phase,
            Phase::Reconnect | Phase::Commit | Phase::RecoveryRequired
        ),
        "this update is not awaiting client recovery"
    );
    let mut replacement =
        locate_server(&store, true)?.context("the recorded replacement server is unavailable")?;
    let inventory: Inventory =
        serde_json::from_value(replacement.request(operation, Command::Inventory)?)
            .context("inspect replacement clients")?;
    let original_inventory = store
        .journal
        .source
        .as_ref()
        .context("missing original session")?;
    anyhow::ensure!(
        inventory.session == original_inventory.session
            && inventory.assets.update_target.as_deref() == Some(operation),
        "replacement no longer owns this update's session"
    );
    let already_ready = inventory.clients.iter().any(|client| {
        client
            .resumed
            .as_ref()
            .is_some_and(|(op, id)| op == operation && *id == original)
    });
    let result = (|| -> Result<()> {
        if !already_ready {
            let attached = inventory
                .clients
                .iter()
                .find(|client| client.id == current)
                .context("selected replacement client is no longer attached; list clients again")?;
            anyhow::ensure!(attached.supports_update, "selected client does not support in-place replacement; update it manually on its host first");
            anyhow::ensure!(
                attached.resumed.is_none(),
                "selected client already acknowledged another original attachment"
            );
            let expected = store
                .journal
                .participants
                .iter()
                .find(
                    |participant| matches!(participant.route, Route::Client(id) if id == original),
                )
                .context("original client has no prepared installation")?
                .installation
                .clone();
            let actual: Installation = serde_json::from_value(client_participant(
                &mut replacement,
                operation,
                current,
                ParticipantRequest::Inventory,
            )?)
            .context("identify selected recovery client installation")?;
            anyhow::ensure!(same_installation(&expected, &actual), "selected client belongs to a different host or installation; choose the original client's replacement attachment");
            let release: update::Release = serde_json::from_value(
                store
                    .journal
                    .release
                    .clone()
                    .context("missing pinned update release")?,
            )
            .context("decode pinned recovery release")?;
            anyhow::ensure!(update::version(&actual.installed_version)? == update::version(&release.tag_name)?, "selected client's installed executable is not the pinned release; repair that installation before retrying");
            anyhow::ensure!(release.source.is_some() || update::version(&actual.running_version)? <= update::version(&release.tag_name)?, "selected client is running a newer release; refusing to downgrade it during recovery");
            anyhow::ensure!(
                release.source.is_none() || attached.supports_source_update,
                "selected recovery client does not support source updates"
            );
            let migration = store
                .journal
                .migration
                .as_ref()
                .context("missing original client focus")?;
            let mapping = store
                .journal
                .mapping
                .as_ref()
                .context("missing restored pane mapping")?;
            let focus = migration
                .clients
                .iter()
                .find(|focus| focus.client_id == original);
            let target = SessionConnection {
                name: Some(inventory.session.clone().into()),
                tab_position: focus
                    .and_then(|focus| focus.tab_id)
                    .and_then(|id| migration.tabs.iter().position(|tab| tab.tab_id == id))
                    .map(|index| yinz_types::internal_ids::TabNumber::new(index as u32 + 1)),
                pane_id: focus.and_then(|focus| focus.pane_id).and_then(|id| {
                    mapping
                        .panes
                        .iter()
                        .find(|(old, _)| *old == id)
                        .map(|(_, new)| *new)
                }),
                ..Default::default()
            };
            client_participant(
                &mut replacement,
                operation,
                current,
                ParticipantRequest::Publish {
                    operation: operation.into(),
                },
            )?;
            if !store.journal.restart_required.contains(&original) {
                store.journal.restart_required.push(original);
            }
            store.save()?;
            let acknowledgement: ClientReply = serde_json::from_value(replacement.request(
                operation,
                Command::Client {
                    client: current,
                    request: Request {
                        token: uuid::Uuid::new_v4().to_string(),
                        request: ClientRequest::Restart {
                            operation: operation.into(),
                            source_client: original,
                            executable: expected.executable,
                            target,
                        },
                    },
                },
            )?)
            .context("decode explicit client replacement acknowledgement")?;
            anyhow::ensure!(
                matches!(acknowledgement, ClientReply::Restarting),
                "selected client did not acknowledge restart"
            );
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                let inventory: Inventory =
                    serde_json::from_value(replacement.request(operation, Command::Inventory)?)
                        .context("wait for explicitly recovered client readiness")?;
                if inventory.clients.iter().any(|client| {
                    client
                        .resumed
                        .as_ref()
                        .is_some_and(|(op, id)| op == operation && *id == original)
                }) {
                    break;
                }
                anyhow::ensure!(
                    Instant::now() < deadline,
                    "selected client did not acknowledge restored readiness; backups retained"
                );
                std::thread::park_timeout(Duration::from_millis(25));
            }
        }
        if !store.journal.restarted_clients.contains(&original) {
            store.journal.restarted_clients.push(original);
        }
        store.journal.restart_required.retain(|id| *id != original);
        store
            .journal
            .disconnected_clients
            .retain(|id| *id != original);
        store.save()?;
        let mut source = locate_server(&store, false)?;
        finish_forward(&mut store, source.as_mut(), Some(&mut replacement))
    })();
    if let Err(error) = &result {
        store
            .journal
            .errors
            .push(format!("Explicit client recovery failed: {error:#}"));
        if store.journal.phase == Phase::Complete {
            store.save()?;
        } else {
            store.phase(Phase::RecoveryRequired)?;
        }
    }
    result
}

#[cfg(test)]
pub fn run(socket: &Path, data_dir: &Path, operation: &str) -> Result<()> {
    run_store(Store::open(socket, data_dir, operation)?)
}

pub fn run_with_channel(
    socket: &Path,
    data_dir: &Path,
    operation: &str,
    source_build: bool,
    channel: ReleaseChannel,
) -> Result<()> {
    run_store(Store::open_with_channel(
        socket,
        data_dir,
        operation,
        source_build,
        channel,
    )?)
}

fn run_store(mut store: Store) -> Result<()> {
    if matches!(store.journal.phase, Phase::Complete | Phase::Failed) {
        return Ok(());
    }
    let mut source = None;
    let result = (|| {
        // A published session name now reaches the replacement. Reconnect by the
        // journal's process incarnation, never by an assumed socket/name association.
        source = if store.journal.source.is_some() {
            locate_server(&store, false)?
        } else {
            Some(Connection::connect(&store.journal.source_socket)?)
        };
        anyhow::ensure!(
            source.is_some() || store.journal.phase != Phase::Discover,
            "the source server is unavailable; no installation was changed"
        );
        let mut recreated = false;
        if !store.journal.handoff_committed && source.is_none() && store.journal.source.is_some() {
            match recreate_source(&mut store) {
                Ok(recovery) => {
                    // recreate_source has already initialized and restored clients.
                    recreated = true;
                    source = Some(recovery);
                },
                Err(error) => {
                    store
                        .journal
                        .errors
                        .push(format!("Cold recovery incomplete: {error:#}"));
                    store.phase(Phase::RecoveryRequired)?;
                    return Err(error);
                },
            }
        }
        if !recreated
            && store.journal.recovery_source_identity.is_some()
            && let Some(source) = source.as_mut()
        {
            initialize_recovery_source(&store, source)?;
        }
        if store.journal.handoff_committed {
            finish_forward(&mut store, source.as_mut(), None)
        } else if let Some(source) = source.as_mut() {
            if store.journal.phase == Phase::Discover {
                execute(&mut store, source)
            } else {
                recover(&mut store, source, None)
            }
        } else {
            store.phase(Phase::RecoveryRequired)?;
            Err(anyhow!("the recorded source server is unavailable; terminal ownership must be recovered before installation rollback"))
        }
    })();
    if let Err(error) = &result {
        store.journal.errors.push(format!("{error:#}"));
        store.save()?;
        record_discovery_failure(&mut store, error)?;
        if let Some(source) = source.as_mut() {
            let _ = source.request(
                &store.journal.operation,
                Command::Notify {
                    text: format!(
                        "Yinz update stopped: {error:#}. Recovery details: {}",
                        store.directory.display()
                    ),
                },
            );
        }
    }
    result
}

fn record_discovery_failure(store: &mut Store, error: &anyhow::Error) -> Result<()> {
    if store.journal.phase != Phase::Discover {
        return Ok(());
    }
    if store.journal.source_build {
        // A launched source worker may still own a build. Keep that operation
        // resumable until its result or cleanup is known, avoiding a second build.
        record_source_build_failure(store, error)
    } else {
        // Even transient discovery failures are terminal: nothing was prepared,
        // so a fresh request can safely rediscover on the user's chosen channel.
        store.phase(Phase::Failed)
    }
}

fn execute(store: &mut Store, source: &mut Connection) -> Result<()> {
    let operation = store.journal.operation.clone();
    let inventory: Inventory =
        serde_json::from_value(source.request(&operation, Command::Inventory)?)
            .context("decode source session inventory")?;
    store.journal.source = Some(inventory.clone());
    store.save()?;
    source.request(
        &operation,
        Command::Notify {
            text: if store.journal.source_build {
                "Preparing a pinned build of local main…".into()
            } else {
                format!(
                    "Checking the latest Yinz release ({})…",
                    store.journal.channel.label()
                )
            },
        },
    )?;
    if store.journal.source_build {
        let release = build_source_release(store, source, &inventory)?;
        let inventory: Inventory =
            serde_json::from_value(source.request(&operation, Command::Inventory)?)
                .context("refresh participants after source compilation")?;
        return execute_release(store, source, inventory, release);
    }
    let http = http_client()?;
    let release = resolve_release(store, &mut |url, out| update::download(&http, url, out))?;
    execute_release(store, source, inventory, release)
}

/// A crash after pinning but before preparation must not rediscover a newer tag.
fn resolve_release(
    store: &mut Store,
    download: &mut impl FnMut(&str, &mut dyn std::io::Write) -> Result<()>,
) -> Result<update::Release> {
    if let Some(pinned) = &store.journal.release {
        return serde_json::from_value(pinned.clone()).context("decode pinned update release");
    }
    let release = update::discover(store.journal.channel, download)?;
    store.journal.release = Some(serde_json::to_value(&release).context("pin selected release")?);
    store.save()?;
    Ok(release)
}

fn record_source_build_failure(store: &mut Store, error: &anyhow::Error) -> Result<()> {
    let build = store.directory.join("source");
    let finished = std::fs::read(build.join("result.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|result| result["success"].is_boolean());
    if error.is::<source_build::TerminalFailure>()
        || finished
        || !build.join("launch.json").exists()
    {
        store.phase(Phase::Failed)?;
    }
    Ok(())
}

fn build_source_release(
    store: &mut Store,
    source: &mut Connection,
    inventory: &Inventory,
) -> Result<update::Release> {
    use yinz_types::update_client::SourceArtifact;
    let operation = store.journal.operation.clone();
    let mut targets = std::collections::BTreeSet::new();
    let server = self::inventory(
        &inventory.executable,
        inventory
            .assets
            .data_dir
            .as_deref()
            .unwrap_or(&yinz_runtime::home::get_default_data_dir()),
    )?;
    targets.insert(server.target);
    for client in &inventory.clients {
        anyhow::ensure!(
            client.supports_update && client.supports_source_update,
            "client {} must be upgraded before a source update",
            client.id
        );
        let installation: Installation = serde_json::from_value(client_participant(
            source,
            &operation,
            client.id,
            ParticipantRequest::Inventory,
        )?)
        .context("inventory source-update client")?;
        anyhow::ensure!(
            installation.capabilities.source_update,
            "client {} does not support source artifacts; upgrade it first",
            client.id
        );
        targets.insert(installation.target);
    }
    let migration: yinz_types::session_migration::SessionMigrationInventory =
        serde_json::from_value(source.request(&operation, Command::PrepareMigration)?)
            .context("preflight source update terminals")?;
    let config = decode_configuration(&inventory.configuration)?;
    for host in &migration.remote_hosts {
        targets.insert(transport::remote_target(host, &config.env)?);
    }
    anyhow::ensure!(
        targets.iter().all(|target| matches!(
            target.as_str(),
            "x86_64-unknown-linux-gnu" | "aarch64-unknown-linux-gnu"
        )),
        "source updates require Linux x86_64 or aarch64 on every participant"
    );
    let build = source_build::build(store, inventory, &targets.into_iter().collect::<Vec<_>>())?;
    let data_dir = store
        .directory
        .parent()
        .and_then(Path::parent)
        .context("update data directory")?;
    let mut artifacts = Vec::new();
    for (target, path) in &build.artifacts {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
            std::fs::File::open(path).context("open source package")?,
        ));
        let mut binary_hash = None;
        for entry in archive.entries().context("read source package")? {
            let mut entry = entry.context("read source package entry")?;
            if entry.path().context("read source package path")? == Path::new("yinz") {
                use sha2::{Digest, Sha256};
                let mut hash = Sha256::new();
                let mut bytes = [0; 65536];
                loop {
                    let count = entry.read(&mut bytes).context("hash source executable")?;
                    if count == 0 {
                        break;
                    }
                    hash.update(&bytes[..count]);
                }
                anyhow::ensure!(binary_hash.is_none(), "duplicate source executable");
                binary_hash = Some(update::hex(&hash.finalize()));
            }
        }
        let archive_hash = artifacts::digest_file(path)?;
        anyhow::ensure!(
            build.sha256.get(target) == Some(&archive_hash),
            "pinned source archive changed after build completion"
        );
        let artifact = SourceArtifact {
            target: target.clone(),
            sha256: archive_hash,
            size: std::fs::metadata(path)
                .context("measure source archive")?
                .len(),
            binary_sha256: binary_hash.context("source archive has no executable")?,
        };
        let mut input = std::fs::File::open(path).context("open pinned source archive")?;
        artifacts::stage_local(data_dir, &operation, &artifact, &mut input)?;
        artifacts.push(artifact);
    }
    Ok(update::Release {
        channel: Default::default(),
        tag_name: build.version,
        assets: Vec::new(),
        source: Some(update::SourceRelease {
            commit: build.commit,
            artifacts,
        }),
    })
}

fn transfer_source_artifact(
    store: &Store,
    source: &mut Connection,
    index: usize,
    artifact: &yinz_types::update_client::SourceArtifact,
) -> Result<()> {
    let path = store
        .directory
        .join("source-artifacts")
        .join(format!("{}.tar.gz", artifact.target));
    anyhow::ensure!(
        artifacts::digest_file(&path)? == artifact.sha256,
        "pinned source artifact changed"
    );
    let mut file = std::fs::File::open(path).context("open source artifact for transfer")?;
    let mut offset = 0;
    let mut buffer = [0; 65536];
    loop {
        let count = file
            .read(&mut buffer)
            .context("read source artifact chunk")?;
        if count == 0 {
            break;
        }
        let reply = participant(
            source,
            &store.journal,
            index,
            ParticipantRequest::SourceChunk {
                operation: store.journal.operation.clone(),
                artifact: artifact.clone(),
                offset,
                bytes: buffer[..count].to_vec(),
            },
        )?;
        let acknowledged = reply["offset"]
            .as_u64()
            .context("missing source transfer offset")?;
        anyhow::ensure!(
            acknowledged >= offset + count as u64 && acknowledged <= artifact.size,
            "source artifact acknowledgement offset mismatch"
        );
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(acknowledged))
            .context("resume source transfer")?;
        offset = acknowledged;
    }
    anyhow::ensure!(offset == artifact.size, "source artifact length changed");
    Ok(())
}

fn require_prerelease_support(
    release: &update::Release,
    installation: &Installation,
) -> Result<()> {
    anyhow::ensure!(
        release.source.is_some()
            || update::version(&release.tag_name)?.pre.is_empty()
            || installation.capabilities.prerelease_update,
        "participant {} does not support prerelease updates; update it manually first",
        installation.executable.display()
    );
    Ok(())
}

fn wait_for_update_preparation(
    store: &Store,
    source: &mut Connection,
) -> Result<yinz_types::session_migration::SessionMigrationInventory> {
    #[cfg(test)]
    let timeout = PREPARATION_TIMEOUT.get();
    #[cfg(not(test))]
    let timeout = Duration::from_secs(60);
    wait_for_update_preparation_until(store, source, Instant::now() + timeout)
}

fn wait_for_update_preparation_until(
    store: &Store,
    source: &mut Connection,
    deadline: Instant,
) -> Result<yinz_types::session_migration::SessionMigrationInventory> {
    use yinz_types::session_migration::MigrationPreparation;
    let operation = &store.journal.operation;
    let mut reported = None;
    let mut last_report = Instant::now();
    let mut pending = "preparation progress from the session".to_owned();
    let result = (|| loop {
        anyhow::ensure!(Instant::now() < deadline, "preparation deadline reached");
        let progress: MigrationPreparation = serde_json::from_value(source.request_until(
            operation,
            Command::PrepareUpdate,
            deadline,
        )?)
        .context("decode update preparation progress")?;
        match progress {
            MigrationPreparation::Ready(inventory) => return Ok(inventory),
            MigrationPreparation::Waiting(work) => {
                pending = work
                    .iter()
                    .map(|work| work.description())
                    .collect::<Vec<_>>()
                    .join(", ");
                if reported.as_ref() != Some(&work)
                    || last_report.elapsed() >= Duration::from_secs(5)
                {
                    source.request_until(
                        operation,
                        Command::Notify {
                            text: format!("Preparing session update: waiting for {pending}…"),
                        },
                        deadline,
                    )?;
                    reported = Some(work);
                    last_report = Instant::now();
                }
                // Polling releases both server threads to consume completion
                // events, bounded by this preparation attempt's deadline.
                std::thread::park_timeout(
                    Duration::from_millis(100)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            },
        }
    })();
    if Instant::now() >= deadline {
        result.with_context(|| {
            format!("session update preparation timed out while waiting for {pending}")
        })
    } else {
        result
    }
}

fn inventory_update_clients(
    store: &mut Store,
    source: &mut Connection,
    inventory: &Inventory,
    release: &update::Release,
) -> Result<()> {
    let operation = store.journal.operation.clone();
    for client in &inventory.clients {
        if store
            .journal
            .participants
            .iter()
            .any(|participant| matches!(participant.route, Route::Client(id) if id == client.id))
        {
            continue;
        }
        if !client.supports_update || (release.source.is_some() && !client.supports_source_update) {
            source.request(
                &operation,
                Command::DisconnectUpdateClient { client: client.id },
            )?;
            if !store.journal.disconnected_clients.contains(&client.id) {
                store.journal.disconnected_clients.push(client.id);
            }
            store.save()?;
            continue;
        }
        match client_participant(source, &operation, client.id, ParticipantRequest::Inventory) {
            Ok(value) => {
                let installation: Installation =
                    serde_json::from_value(value).context("decode client installation")?;
                require_prerelease_support(release, &installation)?;
                store.journal.participants.push(Participant {
                    installation,
                    route: Route::Client(client.id),
                    state: "inventoried".into(),
                });
            },
            Err(error) => {
                let current: Inventory =
                    serde_json::from_value(source.request(&operation, Command::Inventory)?)
                        .context("check disconnected client")?;
                if current.clients.iter().any(|c| c.id == client.id) {
                    return Err(error);
                }
                store.journal.disconnected_clients.push(client.id);
            },
        }
        store.save()?;
    }
    Ok(())
}

fn inventory_update_hosts(
    store: &mut Store,
    release: &update::Release,
    hosts: Vec<String>,
    config: &yinz_config::config::Config,
) -> Result<()> {
    let operation = store.journal.operation.clone();
    for host in hosts {
        let already_staged = store.journal.participants.iter().any(|participant| {
            matches!(&participant.route, Route::Remote { host: staged, .. } if staged == &host)
        });
        if already_staged {
            continue;
        }
        let (executable, directory, helper) = transport::bootstrap_remote(
            &host,
            release,
            &config.env,
            &operation,
            Some(&store.directory.clone()),
            |directory| {
                store.journal.remote_helpers.push(journal::RemoteHelper {
                    host: host.clone(),
                    directory: directory.to_owned(),
                    cleaned: false,
                });
                store.save()
            },
        )?;
        let value = transport::remote_participant(
            &host,
            &helper,
            &ParticipantInvocation {
                executable,
                data_dir: None,
                request: ParticipantRequest::Inventory,
            },
            &config.env,
        )?;
        let installation = serde_json::from_value(value).context("decode remote installation")?;
        store.journal.participants.push(Participant {
            installation,
            route: Route::Remote {
                host,
                helper,
                directory,
            },
            state: "inventoried".into(),
        });
        store.save()?;
    }
    Ok(())
}

fn prepare_update_participants(
    store: &mut Store,
    source: &mut Connection,
    release: &update::Release,
) -> Result<()> {
    let operation = store.journal.operation.clone();
    for index in unique_participants(&store.journal) {
        if store.journal.participants[index].state == "prepared" {
            continue;
        }
        require_prerelease_support(release, &store.journal.participants[index].installation)?;
        if let Some(source_release) = &release.source {
            anyhow::ensure!(
                store.journal.participants[index]
                    .installation
                    .capabilities
                    .source_update,
                "participant does not support source artifacts; update it first"
            );
            let artifact = source_release
                .artifacts
                .iter()
                .find(|artifact| {
                    artifact.target == store.journal.participants[index].installation.target
                })
                .context("a participant requires an unbuilt source target; retry a new update")?;
            transfer_source_artifact(store, source, index, artifact)?;
        }
        set_installation_state(&mut store.journal, index, "preparing");
        store.save()?;
        participant(
            source,
            &store.journal,
            index,
            ParticipantRequest::Prepare {
                operation: operation.clone(),
                release: store
                    .journal
                    .release
                    .clone()
                    .context("missing pinned release")?,
            },
        )?;
        set_installation_state(&mut store.journal, index, "prepared");
        store.save()?;
    }
    Ok(())
}

fn reconcile_update_participants(
    store: &mut Store,
    source: &mut Connection,
    release: &update::Release,
    migration: &yinz_types::session_migration::SessionMigrationInventory,
) -> Result<()> {
    let current: Inventory =
        serde_json::from_value(source.request(&store.journal.operation, Command::Inventory)?)
            .context("refresh update participants")?;
    inventory_update_clients(store, source, &current, release)?;
    let config = decode_configuration(&current.configuration)?;
    inventory_update_hosts(store, release, migration.remote_hosts.clone(), &config)?;
    prepare_update_participants(store, source, release)
}

/// The release is resolved exactly once, before any participant is prepared.
pub(super) fn execute_release(
    store: &mut Store,
    source: &mut Connection,
    inventory: Inventory,
    release: update::Release,
) -> Result<()> {
    let operation = store.journal.operation.clone();
    store.journal.source = Some(inventory.clone());
    store.save()?;
    let target = update::version(&release.tag_name)?;
    anyhow::ensure!(
        release.source.is_some() || update::version(&inventory.running_version)? <= target,
        "source server is newer than the selected release; refusing downgrade"
    );
    for client in &inventory.clients {
        anyhow::ensure!(
            target.pre.is_empty() || release.source.is_some() || client.supports_update,
            "client {} does not support prerelease updates; update it manually first",
            client.id
        );
        anyhow::ensure!(
            release.source.is_none() || client.supports_source_update,
            "client {} does not support source updates; no installation was published",
            client.id
        );
    }
    store.journal.release = Some(serde_json::to_value(&release).context("record pinned release")?);
    store.phase(Phase::Prepare)?;
    let prepared = (|| {
        let migration = wait_for_update_preparation(store, source)?;
        store.journal.migration = Some(migration);
        store.save()?;
        let mut installation: Installation =
            serde_json::from_value(participate(ParticipantInvocation {
                data_dir: inventory.assets.data_dir.clone(),
                executable: inventory.executable.clone(),
                request: ParticipantRequest::Inventory,
            })?)
            .context("decode server installation")?;
        installation.running_version = inventory.running_version.clone();
        store.journal.participants.push(Participant {
            installation,
            route: Route::Server,
            state: "inventoried".into(),
        });
        store.save()?;
        inventory_update_clients(store, source, &inventory, &release)?;
        let config: yinz_config::config::Config = decode_configuration(&inventory.configuration)?;
        let hosts = store
            .journal
            .migration
            .as_ref()
            .context("missing migration inventory")?
            .remote_hosts
            .clone();
        let mut already_current = release.source.is_none()
            && store.journal.participants.iter().all(|participant| {
                update::version(&participant.installation.running_version)
                    .is_ok_and(|running| running >= target)
                    && update::version(&participant.installation.installed_version)
                        .is_ok_and(|installed| installed >= target)
            });
        if already_current {
            for host in &hosts {
                let mut command = yinz_client::remote_sessions::ssh_command(
                    host,
                    &["yinz", "--version"],
                    &config.env,
                )?;
                let output = yinz_process::ssh::bootstrap_output(
                    &mut command,
                    Duration::from_secs(20),
                    None,
                )?;
                anyhow::ensure!(
                    output.status.success(),
                    "could not check the installed release on {host}"
                );
                let text = std::str::from_utf8(&output.stdout)
                    .context("decode remote installed version")?;
                let remote = text
                    .trim()
                    .strip_prefix("yinz ")
                    .context("unexpected remote version response")?;
                already_current &= update::version(remote)? >= target;
            }
        }
        if already_current {
            source.request(&operation, Command::AbortMigration)?;
            store.phase(Phase::Complete)?;
            source.request(&operation,Command::Notify{text:format!("Yinz {target} ({}) is up to date. {} legacy clients need a manual update/restart on their host to verify their running version.",release.channel.label(),store.journal.restart_required.len())})?;
            return Ok(());
        }
        inventory_update_hosts(store, &release, hosts, &config)?;
        source.request(
            &operation,
            Command::Notify {
                text: format!(
                    "Preparing Yinz {target} ({}) on {} participating installations…",
                    if release.source.is_some() {
                        "local source"
                    } else {
                        release.channel.label()
                    },
                    unique_participants(&store.journal).len()
                ),
            },
        )?;
        prepare_update_participants(store, source, &release)?;
        Ok(())
    })();
    if let Err(error) = prepared {
        store
            .journal
            .errors
            .push(format!("Preparation failed: {error:#}"));
        recover(store, source, None)?;
        return Err(error);
    }
    if store.journal.phase == Phase::Complete {
        return Ok(());
    }
    // Pause old plugin workers before replacing their installed sources. Every
    // installation has already staged and validated its generation at this point.
    let publication = (|| {
        store.phase(Phase::Quiesce)?;
        source.request(
            &operation,
            Command::Notify {
                text: format!("Migrating this session to Yinz {target}…"),
            },
        )?;
        let migration = wait_for_update_preparation(store, source)?;
        reconcile_update_participants(store, source, &release, &migration)?;
        store.journal.migration = Some(migration);
        store.save()?;
        store.phase(Phase::Transfer)?;
        let released = source.request(
            &operation,
            Command::ReleaseMigration {
                snapshot: store.journal.snapshot.clone(),
            },
        )?;
        let actual: yinz_types::session_migration::SessionMigrationInventory =
            serde_json::from_value(released["inventory"].clone())
                .context("decode frozen migration inventory")?;
        // Reconcile the authoritative frozen inventory as well: completions
        // admitted during preparation may reveal another host or attachment.
        reconcile_update_participants(store, source, &release, &actual)?;
        store.journal.migration = Some(actual);
        let frozen: Inventory =
            serde_json::from_value(source.request(&operation, Command::Inventory)?)
                .context("capture quiesced client configuration")?;

        for client in &inventory.clients {
            if !frozen
                .clients
                .iter()
                .any(|attached| attached.id == client.id)
                && !store.journal.disconnected_clients.contains(&client.id)
            {
                store.journal.disconnected_clients.push(client.id);
            }
        }
        store
            .journal
            .restart_required
            .retain(|id| frozen.clients.iter().any(|client| client.id == *id));
        let original = store
            .journal
            .source
            .as_mut()
            .context("missing source inventory")?;
        original.clients = frozen.clients;
        original.configuration = frozen.configuration;
        store.save()?;
        publish_participants(store, source)?;
        start_replacement(store)
    })();
    let mut replacement = match publication {
        Ok(connection) => connection,
        Err(error) => {
            store
                .journal
                .errors
                .push(format!("Publication failed: {error:#}"));
            recover(store, source, None)?;
            return Err(error);
        },
    };
    let transfer = (|| {
        record_listener(store, &mut replacement)?;
        initialize_replacement(store, &mut replacement)?;
        record_replacement(store, &mut replacement)?;
        replacement.request(
            &operation,
            Command::RestoreClients {
                clients: store
                    .journal
                    .source
                    .as_ref()
                    .context("missing frozen clients")?
                    .clients
                    .clone(),
            },
        )?;
        let mapping = serde_json::from_value(replacement.request(
            &operation,
            Command::AdoptMigration {
                snapshot: store.journal.snapshot.clone(),
            },
        )?)
        .context("decode acknowledged pane adoption")?;
        store.journal.mapping = Some(mapping);
        store.save()?;
        let recovery_name = format!(".retired-{}", operation);
        source.rename(&operation, &recovery_name)?;
        replacement.rename(&operation, &inventory.session)?;
        store.phase(Phase::Reconnect)?;
        store.journal.handoff_committed = true;
        store.save()?;
        let plugins = replacement.request(&operation, Command::FinishMigration)
            .context("migrated plugins require repair; the destination remains active and installation backups are retained");
        // Finish can expose a permission prompt or plugin load error after the
        // irreversible handoff. Reconnect clients so they can repair that live
        // destination, while retaining the failed readiness obligation.
        let clients = reconnect_clients(store, source, &mut replacement);
        notify_plugin_repair(&mut replacement, &operation, &plugins);
        plugins?;
        clients?;
        Ok(())
    })();
    if let Err(error) = transfer {
        store
            .journal
            .errors
            .push(format!("Handoff failed: {error:#}"));
        store.save()?;
        if store.journal.phase == Phase::Reconnect {
            // Once clients are rendering the replacement, reversing ownership could
            // discard fresh input. Keep the working owner and backups for recovery.
            store.phase(Phase::RecoveryRequired)?;
        } else {
            recover(store, source, Some(&mut replacement))?;
        }
        return Err(error);
    }
    commit(store, Some(source), &mut replacement)
}

fn cleanup_remote_helpers(store: &mut Store) -> Result<()> {
    let config = decode_configuration(
        &store
            .journal
            .source
            .as_ref()
            .context("missing source configuration")?
            .configuration,
    )?;
    for index in 0..store.journal.remote_helpers.len() {
        let helper = &store.journal.remote_helpers[index];
        if helper.cleaned {
            continue;
        }
        transport::remove_remote_helper(
            &helper.host,
            &helper.directory,
            &store.journal.operation,
            &config.env,
        )?;
        store.journal.remote_helpers[index].cleaned = true;
        store.save()?;
    }
    Ok(())
}

pub(super) fn publish_participants(store: &mut Store, source: &mut Connection) -> Result<()> {
    let operation = store.journal.operation.clone();
    let unique = unique_participants(&store.journal);
    for &index in &unique {
        set_installation_state(&mut store.journal, index, "publishing");
        store.save()?;
        participant(
            source,
            &store.journal,
            index,
            ParticipantRequest::Publish {
                operation: operation.clone(),
            },
        )?;
        set_installation_state(&mut store.journal, index, "published");
        store.save()?;
    }
    // Shared installations prepare once, but each client must acknowledge its
    // own published operation before it accepts a restart request. A client
    // that published its own installation above has already acknowledged it.
    for index in 0..store.journal.participants.len() {
        if !unique.contains(&index)
            && matches!(store.journal.participants[index].route, Route::Client(id)
            if !store.journal.disconnected_clients.contains(&id))
        {
            participant(
                source,
                &store.journal,
                index,
                ParticipantRequest::Publish {
                    operation: operation.clone(),
                },
            )?;
            set_installation_state(&mut store.journal, index, "published");
            store.save()?;
        }
    }
    Ok(())
}

fn unique_participants(journal: &Journal) -> Vec<usize> {
    let mut seen = std::collections::HashSet::new();
    journal
        .participants
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            seen.insert((
                p.installation.host.clone(),
                p.installation.executable.clone(),
                p.installation.data_dir.clone(),
            ))
            .then_some(i)
        })
        .collect()
}

fn participant(
    source: &mut Connection,
    journal: &Journal,
    index: usize,
    request: ParticipantRequest,
) -> Result<serde_json::Value> {
    let p = &journal.participants[index];
    match &p.route {
        Route::Server => participate(ParticipantInvocation {
            data_dir: Some(p.installation.data_dir.clone()),
            executable: p.installation.executable.clone(),
            request,
        }),
        Route::Client(id) => client_participant(source, &journal.operation, *id, request),
        Route::Remote { host, helper, .. } => {
            let config: yinz_config::config::Config = decode_configuration(
                &journal
                    .source
                    .as_ref()
                    .context("missing source configuration")?
                    .configuration,
            )?;
            transport::remote_participant(
                host,
                helper,
                &ParticipantInvocation {
                    data_dir: Some(p.installation.data_dir.clone()),
                    executable: p.installation.executable.clone(),
                    request,
                },
                &config.env,
            )
        },
    }
}

fn rollback_participant(
    source: &mut Connection,
    store: &Store,
    index: usize,
) -> Result<serde_json::Value> {
    let participant = &store.journal.participants[index];
    let request = ParticipantRequest::Rollback {
        operation: store.journal.operation.clone(),
    };
    if !matches!(participant.route, Route::Client(_)) {
        return self::participant(source, &store.journal, index, request);
    }
    if store.journal.participants.iter().any(|candidate| {
        matches!(candidate.route, Route::Server)
            && candidate.installation.host == participant.installation.host
    }) {
        return participate(ParticipantInvocation {
            executable: participant.installation.executable.clone(),
            data_dir: Some(participant.installation.data_dir.clone()),
            request,
        });
    }
    // A restarted source assigns fresh client IDs. An attachment ID or a resume
    // claim alone is not proof that it can recover this installation.
    let current: Inventory =
        serde_json::from_value(source.request(&store.journal.operation, Command::Inventory)?)
            .context("locate connected rollback participants")?;
    let mut errors = Vec::new();
    for client in current
        .clients
        .iter()
        .filter(|client| client.supports_update)
    {
        let identity = client_participant(
            source,
            &store.journal.operation,
            client.id,
            ParticipantRequest::Inventory,
        )
        .and_then(|value| {
            serde_json::from_value::<Installation>(value)
                .context("decode rollback participant identity")
        });
        match identity {
            Ok(identity) if same_installation(&identity, &participant.installation) => {
                match client_participant(
                    source,
                    &store.journal.operation,
                    client.id,
                    request.clone(),
                ) {
                    Ok(value) => return Ok(value),
                    Err(error) => errors.push(format!("{error:#}")),
                }
            },
            Ok(_) => {},
            Err(error) => errors.push(format!("{error:#}")),
        }
    }
    bail!(
        "installation {} is disconnected; reconnect its client and retry recovery{}",
        participant.installation.executable.display(),
        if errors.is_empty() {
            String::new()
        } else {
            format!(": {}", errors.join("; "))
        }
    )
}

fn client_participant(
    source: &mut Connection,
    operation: &str,
    client: ClientId,
    request: ParticipantRequest,
) -> Result<serde_json::Value> {
    let token = uuid::Uuid::new_v4().to_string();
    let mut request = Request {
        token: token.clone(),
        request: ClientRequest::Participant(request),
    };
    let deadline = Instant::now() + Duration::from_secs(15 * 60);
    loop {
        let result: ClientReply =
            serde_json::from_value(source.request(operation, Command::Client { client, request })?)
                .context("decode client participant result")?;
        match result {
            ClientReply::Participant(value) => return Ok(value),
            ClientReply::Pending => {},
            _ => bail!("unexpected client participant response"),
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "client preparation timed out; inspect participant journal"
        );
        std::thread::park_timeout(Duration::from_millis(50));
        request = Request {
            token: uuid::Uuid::new_v4().to_string(),
            request: ClientRequest::Poll {
                token: token.clone(),
            },
        };
    }
}

fn start_replacement(store: &Store) -> Result<Connection> {
    let source = store
        .journal
        .source
        .as_ref()
        .context("missing source inventory")?;
    start_server_at(&source.executable, &store.journal.replacement_socket)
}

pub(super) fn start_server_at(executable: &Path, socket: &Path) -> Result<Connection> {
    use std::os::unix::process::CommandExt;
    // Capability negotiation is not an occupancy check: a legacy server or
    // unrelated listener still owns its endpoint even if it cannot answer us.
    match yinz_ipc::consts::ipc_connect(socket) {
        Ok(_) => bail!(
            "private update endpoint is already occupied: {}",
            socket.display()
        ),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) => {},
        Err(error) => return Err(error).context("inspect private update endpoint ownership"),
    }
    let mut child = Process::new(executable)
        .arg("--server")
        .arg(socket)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .context("start private replacement server")?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let connection = loop {
        if let Ok(connection) = Connection::connect(socket) {
            break connection;
        }
        if let Some(status) = child.try_wait().context("check replacement launcher")? {
            anyhow::ensure!(status.success(), "replacement server launcher failed");
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "replacement server did not become ready"
        );
        std::thread::park_timeout(Duration::from_millis(20));
    };
    Ok(connection)
}

/// Startup assets for a server that adopts this operation's frozen session, whether the
/// replacement or a recreated source rolling back.
///
/// The window size comes from the frozen migration inventory. The source's own assets hold the
/// size its first client had at startup, which no later resize updates; a server bootstrapped
/// at that size rebuilt every tab into the wrong screen. Older sources do not record a size,
/// and fall back to it.
pub(super) fn replacement_assets(
    journal: &Journal,
) -> Result<yinz_types::input::cli_assets::CliAssets> {
    let source = journal
        .source
        .as_ref()
        .context("missing source inventory")?;
    let mut assets = source.assets.clone();
    assets.update_configuration = Some(
        source
            .configuration
            .as_str()
            .context("missing effective configuration")?
            .to_owned(),
    );
    assets.configuration_options = None;
    assets.layout = None;
    assets.update_target = Some(journal.operation.clone());
    assets.update_source_session = Some(source.session.clone().into());
    assets.supports_in_place_update = false;
    assets.supports_source_update = false;
    if let Some(size) = journal
        .migration
        .as_ref()
        .and_then(|migration| migration.display_size)
        .filter(|size| size.rows > 0 && size.cols > 0)
    {
        assets.terminal_window_size = size;
    }
    Ok(assets)
}

fn initialize_replacement(store: &Store, connection: &mut Connection) -> Result<()> {
    connection.initialize(replacement_assets(&store.journal)?)?;
    let ready: Inventory =
        serde_json::from_value(connection.request(&store.journal.operation, Command::Inventory)?)
            .context("check private replacement initialization")?;
    anyhow::ensure!(
        ready.running_version
            == update::version(
                &serde_json::from_value::<update::Release>(
                    store.journal.release.clone().context("missing release")?
                )
                .context("decode pinned release")?
                .tag_name
            )?
            .to_string(),
        "replacement server is not running the staged release"
    );
    Ok(())
}

fn reconnect_clients(
    store: &mut Store,
    source: &mut Connection,
    replacement: &mut Connection,
) -> Result<()> {
    let operation = store.journal.operation.clone();
    let inventory = store
        .journal
        .source
        .clone()
        .context("missing source inventory")?;
    let migration = store
        .journal
        .migration
        .as_ref()
        .context("missing migration inventory")?;
    let mapping = store
        .journal
        .mapping
        .as_ref()
        .context("missing adoption mapping")?;
    let current: Inventory =
        serde_json::from_value(replacement.request(&operation, Command::Inventory)?)
            .context("read replacement clients")?;
    let mut expected = Vec::new();
    for client in &inventory.clients {
        if current.clients.iter().any(|c| {
            c.resumed
                .as_ref()
                .is_some_and(|(op, old)| op == &operation && *old == client.id)
        }) {
            if !store.journal.restarted_clients.contains(&client.id) {
                store.journal.restarted_clients.push(client.id);
            }
            store.journal.restart_required.retain(|id| *id != client.id);
            continue;
        }
        let attempted = store.journal.restart_attempted.contains(&client.id);
        if !attempted && store.journal.disconnected_clients.contains(&client.id) {
            continue;
        }
        let focus = migration
            .clients
            .iter()
            .find(|focus| focus.client_id == client.id);
        let target = SessionConnection {
            name: Some(inventory.session.clone().into()),
            tab_position: focus
                .and_then(|f| f.tab_id)
                .and_then(|id| migration.tabs.iter().position(|tab| tab.tab_id == id))
                .map(|i| yinz_types::internal_ids::TabNumber::new(i as u32 + 1)),
            pane_id: focus.and_then(|f| f.pane_id).and_then(|id| {
                mapping
                    .panes
                    .iter()
                    .find(|(old, _)| *old == id)
                    .map(|(_, new)| *new)
            }),
            ..Default::default()
        };
        if !client.supports_update {
            source.request(
                &operation,
                Command::LegacyReconnect {
                    client: client.id,
                    target,
                },
            )?;
            continue;
        }
        let attached: Inventory =
            serde_json::from_value(source.request(&operation, Command::Inventory)?)
                .context("inspect client before restart")?;
        if !attached
            .clients
            .iter()
            .any(|current| current.id == client.id)
        {
            if attempted {
                // Exec can disconnect before its acknowledgement reaches the
                // coordinator. The old process may also be attached to the new
                // server after exec failed. Neither proves readiness.
                expected.push(client.id);
                store
                    .journal
                    .disconnected_clients
                    .retain(|id| *id != client.id);
            } else {
                store.journal.disconnected_clients.push(client.id);
            }
            continue;
        }
        let participant = store
            .journal
            .participants
            .iter()
            .find(|p| matches!(p.route,Route::Client(id) if id==client.id))
            .context("missing prepared client")?;
        let executable = participant.installation.executable.clone();
        if !attempted {
            store.journal.restart_attempted.push(client.id);
        }
        if !store.journal.restart_required.contains(&client.id) {
            store.journal.restart_required.push(client.id);
        }
        // Persist uncertainty before sending the request: a lost reply must
        // never turn this client into an ordinary disconnected participant.
        store.save()?;
        let result = source.request(
            &operation,
            Command::Client {
                client: client.id,
                request: Request {
                    token: uuid::Uuid::new_v4().to_string(),
                    request: ClientRequest::Restart {
                        operation: operation.clone(),
                        source_client: client.id,
                        executable,
                        target,
                    },
                },
            },
        );
        match result {
            Ok(value) => {
                anyhow::ensure!(
                    matches!(
                        serde_json::from_value::<ClientReply>(value)
                            .context("decode restart acknowledgement")?,
                        ClientReply::Restarting
                    ),
                    "unexpected restart acknowledgement"
                );
                expected.push(client.id);
            },
            Err(error) => {
                let still_attached: Inventory =
                    serde_json::from_value(source.request(&operation, Command::Inventory)?)
                        .context("check failed restart client")?;
                if still_attached.clients.iter().any(|c| c.id == client.id) {
                    return Err(error
                        .context("capable client refused restart; installation backup retained"));
                }
                expected.push(client.id);
            },
        }
    }
    store.save()?;
    #[cfg(not(test))]
    let ready_timeout = Duration::from_secs(60);
    #[cfg(test)]
    let ready_timeout = CLIENT_READY_TIMEOUT.get();
    let deadline = Instant::now() + ready_timeout;
    while !expected.is_empty() {
        let current: Inventory =
            serde_json::from_value(replacement.request(&operation, Command::Inventory)?)
                .context("read replacement readiness")?;
        expected.retain(|id| {
            let ready = current.clients.iter().any(|client| {
                client
                    .resumed
                    .as_ref()
                    .is_some_and(|(op, source)| op == &operation && source == id)
            });
            if ready {
                if !store.journal.restarted_clients.contains(id) {
                    store.journal.restarted_clients.push(*id);
                }
                store
                    .journal
                    .restart_required
                    .retain(|pending| pending != id);
            }
            !ready
        });
        store.save()?;
        if Instant::now() >= deadline {
            for id in &expected {
                if !store.journal.restart_required.contains(id) {
                    store.journal.restart_required.push(*id);
                }
            }
            store.save()?;
            bail!("{} capable clients did not acknowledge replacement readiness (original IDs: {:?}); repair/reconnect those clients, then use `yinz update --resume-client ORIGINAL_ID --client CURRENT_ID --session SESSION` on the server host with its original --data-dir; backups retained",expected.len(),expected.iter().map(|id| id.get()).collect::<Vec<_>>());
        }
        std::thread::park_timeout(Duration::from_millis(50));
    }
    Ok(())
}

fn server_identity(inventory: &Inventory) -> yinz_types::session_update::ServerIdentity {
    yinz_types::session_update::ServerIdentity {
        server_pid: inventory.server_pid,
        server_instance: inventory.server_instance.clone(),
    }
}

fn source_identity(store: &Store) -> Result<yinz_types::session_update::ServerIdentity> {
    store
        .journal
        .recovery_source_identity
        .clone()
        .or_else(|| store.journal.source.as_ref().map(server_identity))
        .context("missing source process incarnation")
}

/// A missing or exited process cannot retain ownership. A live PID may have
/// been reused and is never authority to signal or steal its terminals.
fn ensure_process_dead(identity: &yinz_types::session_update::ServerIdentity) -> Result<()> {
    anyhow::ensure!(yinz_process::liveness::has_exited(identity.server_pid)?,
        "recorded server pid {} is still present or reused; its incarnation must be inspected before recovery",identity.server_pid);
    Ok(())
}

fn initialize_recovery_source(store: &Store, source: &mut Connection) -> Result<()> {
    let operation = &store.journal.operation;
    verify_identity(source, operation, &source_identity(store)?)?;
    let identity = source.request(operation, Command::Identity)?;
    let original = store
        .journal
        .source
        .as_ref()
        .context("missing original source configuration")?;
    if identity
        .get("initialized")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        source.initialize(replacement_assets(&store.journal)?)?;
    }
    let initialized: Inventory =
        serde_json::from_value(source.request(operation, Command::Inventory)?)
            .context("inspect recovered source initialization")?;
    anyhow::ensure!(
        initialized.running_version == original.running_version,
        "recovery server is not the original release"
    );
    anyhow::ensure!(
        initialized.assets.update_target.as_deref() == Some(operation),
        "recovery source belongs to another operation"
    );
    if !store.journal.source_restored {
        source.request(
            operation,
            Command::RestoreClients {
                clients: original.clients.clone(),
            },
        )?;
    }
    Ok(())
}

/// Recover a source which died after its full frozen snapshot became durable,
/// while the coordinator still has authority to roll the installation back.
fn recreate_source(store: &mut Store) -> Result<Connection> {
    anyhow::ensure!(
        !store.journal.handoff_committed,
        "cannot cold-roll back a committed handoff"
    );
    anyhow::ensure!(!store.journal.source_restored,"the restored source already resumed input; use normal session recovery instead of replaying its stale update snapshot");
    ensure_process_dead(&source_identity(store)?)?;
    store.phase(Phase::Recovering)?;
    let operation = store.journal.operation.clone();
    let mut destination = locate_server(store, true)?;
    if let Some(destination) = destination.as_mut() {
        let identity = destination.request(&operation, Command::Identity)?;
        if identity
            .get("initialized")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            anyhow::ensure!(
                identity
                    .get("update_operation")
                    .and_then(serde_json::Value::as_str)
                    == Some(&operation),
                "replacement belongs to another update"
            );
            let public_owner = Connection::connect(&store.journal.source_socket)
                .ok()
                .and_then(|mut connection| query_identity(&mut connection, &operation).ok());
            if public_owner.as_ref() == store.journal.replacement_identity.as_ref() {
                destination.rename(&operation, &format!(".update-{operation}"))?;
            }
            destination.request(&operation, Command::AbortMigration)?;
        }
    } else if let Some(identity) = &store.journal.replacement_identity {
        ensure_process_dead(identity)?;
    }
    release_cold_snapshot(store)?;
    let server_host = store
        .journal
        .participants
        .iter()
        .find(|participant| matches!(participant.route, Route::Server))
        .map(|participant| participant.installation.host.clone())
        .context("missing source installation")?;
    let config = decode_configuration(
        &store
            .journal
            .source
            .as_ref()
            .context("missing source configuration")?
            .configuration,
    )?;
    let mut failures = Vec::new();
    for index in unique_participants(&store.journal).into_iter().rev() {
        let participant = &store.journal.participants[index];
        if matches!(participant.state.as_str(), "inventoried" | "rolled_back") {
            continue;
        }
        let request = ParticipantRequest::Rollback {
            operation: operation.clone(),
        };
        let invocation = ParticipantInvocation {
            executable: participant.installation.executable.clone(),
            data_dir: Some(participant.installation.data_dir.clone()),
            request,
        };
        let result=match &participant.route {
            Route::Remote{host,helper,..}=>transport::remote_participant(host,helper,&invocation,&config.env),
            Route::Server=>participate(invocation),
            Route::Client(_) if participant.installation.host==server_host=>participate(invocation),
            Route::Client(_)=>Err(anyhow!("remote client installation {} is disconnected; its rollback receipt remains pending",participant.installation.executable.display())),
        };
        match result {
            Ok(_) => set_installation_state(&mut store.journal, index, "rolled_back"),
            Err(error) => failures.push(format!("{error:#}")),
        }
        store.save()?;
    }
    store.journal.errors.extend(failures);
    anyhow::ensure!(
        store
            .journal
            .participants
            .iter()
            .filter(|participant| matches!(participant.route, Route::Server))
            .all(|participant| matches!(participant.state.as_str(), "inventoried" | "rolled_back")),
        "source installation could not be restored; terminal snapshot retained"
    );
    let socket = store
        .journal
        .source_socket
        .with_file_name(format!(".rollback-{operation}"));
    anyhow::ensure!(
        Connection::connect(&socket).is_err(),
        "a different server occupies the recovery endpoint"
    );
    let executable = store
        .journal
        .source
        .as_ref()
        .context("missing source executable")?
        .executable
        .clone();
    let mut recovery = start_server_at(&executable, &socket)?;
    store.journal.recovery_source_identity = Some(query_identity(&mut recovery, &operation)?);
    for client in &store
        .journal
        .source
        .as_ref()
        .context("missing original clients")?
        .clients
    {
        if !store.journal.disconnected_clients.contains(&client.id) {
            store.journal.disconnected_clients.push(client.id);
        }
    }
    store.save()?;
    initialize_recovery_source(store, &mut recovery)?;
    Ok(recovery)
}

fn release_cold_snapshot(store: &Store) -> Result<()> {
    let mut snapshot = read_recovery_snapshot(&store.journal.snapshot)?;
    use yinz_types::session_migration::{MigrationOwnership, MigrationPhase};
    anyhow::ensure!(
        snapshot.operation == store.journal.operation
            && snapshot.format == yinz_types::session_migration::SESSION_MIGRATION_FORMAT,
        "cold recovery snapshot identity or format mismatch"
    );
    anyhow::ensure!(snapshot.phase!=MigrationPhase::Prepared
        && snapshot.tabs.len()==snapshot.inventory.tabs.len(),
        "source exited before a full terminal snapshot was durable; use normal session recovery, retaining the update journal");
    anyhow::ensure!(!snapshot.ownership.iter().any(|(_,owner)|matches!(owner,MigrationOwnership::Adopting)),
        "destination exited during an unacknowledged adoption; consumed terminal output may not be durable, so automatic cold restoration is unsafe");
    snapshot.phase = MigrationPhase::Released;
    snapshot.mapping = Default::default();
    snapshot.errors.clear();
    for (_, owner) in &mut snapshot.ownership {
        *owner = MigrationOwnership::Released;
    }
    journal::atomic_json(&store.journal.snapshot, &snapshot)
}

fn query_identity(
    connection: &mut Connection,
    operation: &str,
) -> Result<yinz_types::session_update::ServerIdentity> {
    serde_json::from_value(connection.request(operation, Command::Identity)?)
        .context("decode server process incarnation")
}

fn verify_identity(
    connection: &mut Connection,
    operation: &str,
    expected: &yinz_types::session_update::ServerIdentity,
) -> Result<()> {
    let actual = query_identity(connection, operation)?;
    anyhow::ensure!(
        &actual == expected,
        "session endpoint belongs to a different server incarnation; refusing update recovery"
    );
    Ok(())
}

/// Record the private listener before it can own any terminals or load plugins.
fn record_listener(store: &mut Store, replacement: &mut Connection) -> Result<()> {
    let status = replacement.request(&store.journal.operation, Command::Identity)?;
    let identity =
        serde_json::from_value(status.clone()).context("decode private listener identity")?;
    if let Some(expected) = &store.journal.replacement_identity {
        anyhow::ensure!(
            expected == &identity,
            "private replacement server incarnation changed"
        );
    } else {
        anyhow::ensure!(status.get("initialized").and_then(serde_json::Value::as_bool) == Some(false),
            "private update endpoint already contains an initialized session; refusing to take ownership");
    }
    if let Some(source) = &store.journal.source {
        anyhow::ensure!(
            server_identity(source) != identity,
            "source and replacement refer to the same server"
        );
    }
    store.journal.replacement_identity = Some(identity);
    store.save()
}

fn record_replacement(store: &mut Store, replacement: &mut Connection) -> Result<()> {
    record_listener(store, replacement)?;
    let inventory: Inventory =
        serde_json::from_value(replacement.request(&store.journal.operation, Command::Inventory)?)
            .context("inspect initialized replacement server")?;
    anyhow::ensure!(
        inventory.assets.update_target.as_deref() == Some(&store.journal.operation),
        "replacement does not belong to this update operation"
    );
    store.journal.replacement = Some(inventory);
    store.save()
}

fn locate_server(store: &Store, replacement: bool) -> Result<Option<Connection>> {
    let expected = if replacement {
        store.journal.replacement_identity.clone()
    } else {
        store
            .journal
            .recovery_source_identity
            .clone()
            .or_else(|| store.journal.source.as_ref().map(server_identity))
    };
    let Some(expected) = expected else {
        return Ok(None);
    };
    let retired = store
        .journal
        .source_socket
        .with_file_name(format!(".retired-{}", store.journal.operation));
    let recovery = store
        .journal
        .source_socket
        .with_file_name(format!(".rollback-{}", store.journal.operation));
    for path in [
        &store.journal.source_socket,
        &store.journal.replacement_socket,
        &retired,
        &recovery,
    ] {
        if !path.exists() {
            continue;
        }
        let Ok(mut connection) = Connection::connect(path) else {
            continue;
        };
        match query_identity(&mut connection, &store.journal.operation) {
            Ok(identity) if identity == expected => return Ok(Some(connection)),
            // Reading a name is never authority to mutate the unrelated session
            // that may now own it, including after a PID has been reused.
            _ => continue,
        }
    }
    Ok(None)
}

fn set_installation_state(journal: &mut Journal, index: usize, state: &str) {
    let installation = journal.participants[index].installation.clone();
    for participant in &mut journal.participants {
        if same_installation(&participant.installation, &installation) {
            participant.state = state.to_owned();
        }
    }
}

fn same_installation(a: &Installation, b: &Installation) -> bool {
    a.host == b.host && a.executable == b.executable && a.data_dir == b.data_dir
}

pub(super) fn manual_commit_matches(
    release: &update::Release,
    expected: &Installation,
    attached: &Installation,
    supports_source_update: bool,
) -> Result<bool> {
    if !same_installation(expected, attached) {
        return Ok(false);
    }
    if let Some(source) = &release.source {
        anyhow::ensure!(
            supports_source_update && attached.capabilities.source_update,
            "an attached client of {} cannot attest its source identity; its installation backup is retained",
            attached.executable.display()
        );
        let artifact = source
            .artifacts
            .iter()
            .find(|artifact| artifact.target == attached.target)
            .context("manually attached client has no pinned source artifact")?;
        anyhow::ensure!(
            attached.capabilities.source_binary_sha256.as_deref() == Some(&artifact.binary_sha256),
            "an attached client of {} is not running the pinned source executable; its installation backup is retained",
            attached.executable.display()
        );
        return Ok(true);
    }
    let target = update::version(&release.tag_name)?;
    anyhow::ensure!(
        update::version(&attached.running_version)? >= target
            && update::version(&attached.installed_version)? >= target,
        "an attached client of {} still runs an older release; its installation backup is retained",
        attached.executable.display()
    );
    Ok(true)
}

pub(super) fn manual_commit_candidates(
    release: &update::Release,
    expected: &Installation,
    clients: &[yinz_types::session_update::Client],
    mut inspect: impl FnMut(ClientId) -> Result<Installation>,
) -> Result<Vec<ClientId>> {
    let mut candidates = Vec::new();
    // Validate every matching attachment before issuing any cleanup request.
    for client in clients.iter().filter(|client| client.supports_update) {
        let identity = inspect(client.id)?;
        if manual_commit_matches(release, expected, &identity, client.supports_source_update)? {
            candidates.push(client.id);
        }
    }
    Ok(candidates)
}

fn commit_participant(
    store: &Store,
    index: usize,
    mut source: Option<&mut Connection>,
    replacement: &mut Connection,
) -> Result<serde_json::Value> {
    let participant_record = &store.journal.participants[index];
    let operation = &store.journal.operation;
    let request = ParticipantRequest::Commit {
        operation: operation.clone(),
    };
    match &participant_record.route {
        Route::Server => participate(ParticipantInvocation {
            executable: participant_record.installation.executable.clone(),
            data_dir: Some(participant_record.installation.data_dir.clone()),
            request,
        }),
        Route::Remote { host, helper, .. } => {
            let configuration: yinz_config::config::Config = decode_configuration(
                &store
                    .journal
                    .source
                    .as_ref()
                    .context("missing source configuration")?
                    .configuration,
            )?;
            transport::remote_participant(
                host,
                helper,
                &ParticipantInvocation {
                    executable: participant_record.installation.executable.clone(),
                    data_dir: Some(participant_record.installation.data_dir.clone()),
                    request,
                },
                &configuration.env,
            )
        },
        Route::Client(_) => {
            let originals: Vec<_> = store
                .journal
                .participants
                .iter()
                .filter_map(|candidate| {
                    if same_installation(&candidate.installation, &participant_record.installation)
                        && let Route::Client(id) = candidate.route
                    {
                        return Some(id);
                    }
                    None
                })
                .collect();
            let current: Inventory =
                serde_json::from_value(replacement.request(operation, Command::Inventory)?)
                    .context("locate reconnected installation participant")?;
            let mut errors = Vec::new();
            for client in &current.clients {
                if source.is_some()
                    && client.resumed.as_ref().is_some_and(|(op, original)| {
                        op == operation && originals.contains(original)
                    })
                {
                    match client_participant(replacement, operation, client.id, request.clone()) {
                        Ok(value) => return Ok(value),
                        Err(error) => errors.push(format!("{error:#}")),
                    }
                }
            }
            if source.is_none() {
                ensure_process_dead(&source_identity(store)?)?;
                let release: update::Release = serde_json::from_value(
                    store
                        .journal
                        .release
                        .clone()
                        .context("missing selected release")?,
                )
                .context("decode selected release for manual client commitment")?;
                // Manual attachments can service installation cleanup without
                // claiming their original client/configuration was replaced.
                let candidates = manual_commit_candidates(
                    &release,
                    &participant_record.installation,
                    &current.clients,
                    |id| {
                        serde_json::from_value(client_participant(
                            replacement,
                            operation,
                            id,
                            ParticipantRequest::Inventory,
                        )?)
                        .context("inspect manually attached installation")
                    },
                )?;
                for client in candidates {
                    match client_participant(replacement, operation, client, request.clone()) {
                        Ok(value) => return Ok(value),
                        Err(error) => errors.push(format!("{error:#}")),
                    }
                }
            }
            // A client whose re-exec failed can still own its original source
            // connection. Keep this fallback bounded and verify it is attached.
            if let Some(source) = source.as_mut() {
                let current: Inventory =
                    serde_json::from_value(source.request(operation, Command::Inventory)?)
                        .context("locate remaining source participant")?;
                for id in originals {
                    if current.clients.iter().any(|client| client.id == id) {
                        match client_participant(source, operation, id, request.clone()) {
                            Ok(value) => return Ok(value),
                            Err(error) => errors.push(format!("{error:#}")),
                        }
                    }
                }
            }
            bail!(
                "client installation {} is unavailable for commitment; its backup is retained{}",
                participant_record.installation.executable.display(),
                if errors.is_empty() {
                    String::new()
                } else {
                    format!(": {}", errors.join("; "))
                }
            )
        },
    }
}

fn verify_source_readiness(store: &Store, replacement: &mut Connection) -> Result<()> {
    let release: update::Release = serde_json::from_value(
        store
            .journal
            .release
            .clone()
            .context("missing pinned release")?,
    )
    .context("decode pinned source identity")?;
    let Some(source) = release.source else {
        return Ok(());
    };
    let expected = |target: &str| -> Result<&str> {
        Ok(source
            .artifacts
            .iter()
            .find(|artifact| artifact.target == target)
            .context("participant has no pinned source artifact")?
            .binary_sha256
            .as_str())
    };
    let inventory: Inventory =
        serde_json::from_value(replacement.request(&store.journal.operation, Command::Inventory)?)
            .context("verify running source server")?;
    let target = update::target(std::env::consts::OS, std::env::consts::ARCH)?;
    anyhow::ensure!(
        inventory.running_binary_sha256.as_deref() == Some(expected(target)?),
        "replacement server is not running the pinned source executable; backups retained"
    );
    for client in &inventory.clients {
        if !client
            .resumed
            .as_ref()
            .is_some_and(|(operation, _)| operation == &store.journal.operation)
        {
            continue;
        }
        anyhow::ensure!(
            client.supports_source_update,
            "replacement client cannot attest its source identity; backups retained"
        );
        let installed: Installation = serde_json::from_value(client_participant(
            replacement,
            &store.journal.operation,
            client.id,
            ParticipantRequest::Inventory,
        )?)
        .context("verify running source client")?;
        anyhow::ensure!(
            installed.capabilities.source_binary_sha256.as_deref()
                == Some(expected(&installed.target)?),
            "replacement client {} is not running the pinned source executable; backups retained",
            client.id
        );
    }
    Ok(())
}

fn commit(
    store: &mut Store,
    mut source: Option<&mut Connection>,
    replacement: &mut Connection,
) -> Result<()> {
    anyhow::ensure!(
        store.journal.handoff_committed,
        "cannot commit before the session handoff decision"
    );
    let operation = store.journal.operation.clone();
    verify_identity(
        replacement,
        &operation,
        store
            .journal
            .replacement_identity
            .as_ref()
            .context("replacement incarnation was not recorded")?,
    )?;
    verify_source_readiness(store, replacement)?;
    store.phase(Phase::Commit)?;
    let mut failures = Vec::new();
    for index in unique_participants(&store.journal) {
        if store.journal.participants[index].state == "committed" {
            continue;
        }
        match commit_participant(store, index, source.as_deref_mut(), replacement) {
            Ok(_) => set_installation_state(&mut store.journal, index, "committed"),
            Err(error) => failures.push(format!("{error:#}")),
        }
        store.save()?;
    }
    if !failures.is_empty() {
        store.journal.errors.extend(failures);
        store.phase(Phase::RecoveryRequired)?;
        bail!("the replacement session is active; some installations still require commitment (backups retained)");
    }
    if let Some(source) = source {
        verify_identity(source, &operation, &source_identity(store)?)?;
        source.kill()?;
    }
    cleanup_remote_helpers(store)?;
    if store.journal.source_build {
        for path in [
            store.directory.join("source").join("checkout"),
            store.directory.join("source-artifacts"),
        ] {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(error).context("clean committed source build artifacts"),
            }
        }
    }
    store.phase(Phase::Complete)?;
    let release: update::Release =
        serde_json::from_value(store.journal.release.clone().context("missing release")?)
            .context("decode selected release")?;
    replacement.request(&operation, Command::Notify { text: format!(
        "Updated to {}. Replaced {} clients; {} disconnected; {} legacy clients need a manual update/restart on their host.",
        release.tag_name, store.journal.restarted_clients.len(), store.journal.disconnected_clients.len(), store.journal.restart_required.len(),
    ) }).context("show completed update status")?;
    Ok(())
}

fn notify_plugin_repair(
    replacement: &mut Connection,
    operation: &str,
    readiness: &Result<serde_json::Value>,
) {
    if let Err(error) = readiness {
        replacement.request(operation, Command::Notify {
            text: format!("The updated session is active; installation backups are retained. Grant requested plugin permissions or reload the failed plugin, then invoke Update Yinz again to finish. Plugin readiness: {error:#}"),
        }).context("notify active replacement about plugin repair").non_fatal();
    }
}

fn finish_forward(
    store: &mut Store,
    mut source: Option<&mut Connection>,
    replacement: Option<&mut Connection>,
) -> Result<()> {
    let mut found = if replacement.is_none() {
        locate_server(store, true)?
    } else {
        None
    };
    let replacement = replacement.or(found.as_mut()).context("the committed replacement server is unavailable; do not roll back its installation or terminal ownership")?;
    let operation = store.journal.operation.clone();
    verify_identity(
        replacement,
        &operation,
        store
            .journal
            .replacement_identity
            .as_ref()
            .context("missing replacement incarnation")?,
    )?;
    let inventory: Inventory =
        serde_json::from_value(replacement.request(&operation, Command::Inventory)?)
            .context("inspect committed replacement session")?;
    let original = store
        .journal
        .source
        .as_ref()
        .context("missing original session identity")?
        .clone();
    anyhow::ensure!(
        inventory.session == original.session
            && inventory.assets.update_target.as_deref() == Some(&operation),
        "replacement no longer owns the committed logical session"
    );
    // The decision was persisted before FinishMigration. Replaying it is safe
    // both before and after its original acknowledgement was lost.
    let plugins = replacement.request(&operation, Command::FinishMigration)
        .context("migrated plugins require repair; the destination remains active and installation backups are retained");
    let clients = (|| -> Result<()> {
        if let Some(source) = source.as_deref_mut() {
            reconnect_clients(store, source, replacement)?;
        } else {
            ensure_process_dead(&source_identity(store)?)
                .context("source attachment ownership remains uncertain")?;
            let ready: Vec<_> = inventory
                .clients
                .iter()
                .filter_map(|client| {
                    client
                        .resumed
                        .as_ref()
                        .filter(|(op, _)| op == &operation)
                        .map(|(_, id)| *id)
                })
                .collect();
            for id in ready {
                if !store.journal.restarted_clients.contains(&id) {
                    store.journal.restarted_clients.push(id);
                }
                store
                    .journal
                    .restart_required
                    .retain(|pending| *pending != id);
            }
            for client in &original.clients {
                if !store.journal.restart_attempted.contains(&client.id)
                    && !store.journal.restarted_clients.contains(&client.id)
                {
                    // Proven source death disconnected attachments which were
                    // never asked to exec. Attempted restarts remain ambiguous.
                    if !store.journal.disconnected_clients.contains(&client.id) {
                        store.journal.disconnected_clients.push(client.id);
                    }
                    store.journal.restart_required.retain(|id| *id != client.id);
                }
            }
            store.save()?;
            let missing: Vec<_> = original
                .clients
                .iter()
                .filter(|client| {
                    client.supports_update
                        && !store.journal.restarted_clients.contains(&client.id)
                        && (store.journal.restart_attempted.contains(&client.id)
                            || !store.journal.disconnected_clients.contains(&client.id))
                })
                .map(|client| client.id)
                .collect();
            anyhow::ensure!(missing.is_empty(), "source server exited before {} clients acknowledged restart (original IDs: {:?}); replacement is active and backups are retained; recover a reconnected client with `yinz update --resume-client ORIGINAL_ID --client CURRENT_ID --session SESSION` on the server host with its original --data-dir", missing.len(), missing.iter().map(|id| id.get()).collect::<Vec<_>>());
        }
        Ok(())
    })();
    notify_plugin_repair(replacement, &operation, &plugins);
    plugins?;
    clients?;
    commit(store, source, replacement)
}

fn recover(
    store: &mut Store,
    source: &mut Connection,
    replacement: Option<&mut Connection>,
) -> Result<()> {
    if store.journal.handoff_committed {
        return finish_forward(store, Some(source), replacement);
    }
    verify_identity(source, &store.journal.operation, &source_identity(store)?)?;
    store.phase(Phase::Recovering)?;
    let operation = store.journal.operation.clone();
    let result = (|| {
        let mut found = if replacement.is_none() {
            locate_server(store, true)?
        } else {
            None
        };
        let mut replacement = replacement.or(found.as_mut());
        let original = store
            .journal
            .source
            .as_ref()
            .context("missing original session")?
            .session
            .clone();
        if let Some(destination) = replacement.as_deref_mut() {
            verify_identity(
                destination,
                &operation,
                store
                    .journal
                    .replacement_identity
                    .as_ref()
                    .context("missing replacement incarnation")?,
            )?;
            // Identity also reports initialization without waiting for session
            // startup, so a failed release cannot deadlock on an empty listener.
            let status = destination.request(&operation, Command::Identity)?;
            if status
                .get("initialized")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                anyhow::ensure!(
                    status
                        .get("update_operation")
                        .and_then(serde_json::Value::as_str)
                        == Some(&operation),
                    "replacement initialization belongs to another update"
                );
                let current: Inventory =
                    serde_json::from_value(destination.request(&operation, Command::Inventory)?)
                        .context("inspect recovery destination endpoint")?;
                let public_owner = Connection::connect(&store.journal.source_socket)
                    .ok()
                    .and_then(|mut connection| query_identity(&mut connection, &operation).ok());
                if current.session == original
                    && public_owner.as_ref() == store.journal.replacement_identity.as_ref()
                {
                    // Release the public endpoint while migration's authority is
                    // still active; AbortMigration retires that authority.
                    destination.rename(&operation, &format!(".update-{}", operation))?;
                }
                destination
                    .request(&operation, Command::AbortMigration)
                    .context("release destination-owned terminals before restoring source")?;
            }
        } else {
            if let Some(identity) = &store.journal.replacement_identity {
                ensure_process_dead(identity).context("replacement ownership is uncertain")?;
                if store.journal.snapshot.exists() && !store.journal.source_restored {
                    release_cold_snapshot(store)?;
                }
            } else {
                anyhow::ensure!(store.journal.replacement.is_none(), "replacement ownership is uncertain; cannot restore source until it is inspected");
            }
        }
        // The source process is the old generation. Restore its builtin sources
        // before Adopt queues any plugin initialization, and before Finish lets
        // those plugins write persisted state again.
        let mut failures = Vec::new();
        for index in unique_participants(&store.journal).into_iter().rev() {
            if matches!(
                store.journal.participants[index].state.as_str(),
                "inventoried" | "rolled_back"
            ) {
                continue;
            }
            match rollback_participant(source, store, index) {
                Ok(_) => set_installation_state(&mut store.journal, index, "rolled_back"),
                Err(error) => failures.push(format!(
                    "Installation {} recovery: {error:#}",
                    store.journal.participants[index]
                        .installation
                        .executable
                        .display()
                )),
            }
            store.save()?;
        }
        // Reopen the public source once its own code and builtin pair is old
        // again. Disconnected client-only installations keep their backups, but
        // must be able to reconnect before their rollback can be serviced.
        anyhow::ensure!(
            store
                .journal
                .participants
                .iter()
                .filter(|participant| matches!(participant.route, Route::Server))
                .all(|participant| matches!(
                    participant.state.as_str(),
                    "inventoried" | "rolled_back"
                )),
            "source installation still requires recovery: {}",
            failures.join("; ")
        );
        let snapshot = if store.journal.snapshot.exists() {
            Some(read_recovery_snapshot(&store.journal.snapshot)?)
        } else {
            None
        };
        let released = snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.phase != yinz_types::session_migration::MigrationPhase::Prepared
        });
        if released && !store.journal.source_restored {
            source.request(
                &operation,
                Command::AdoptMigration {
                    snapshot: store.journal.snapshot.clone(),
                },
            )?;
        }
        let current: Inventory =
            serde_json::from_value(source.request(&operation, Command::Inventory)?)
                .context("inspect source recovery endpoint")?;
        let public_owner = Connection::connect(&store.journal.source_socket)
            .ok()
            .and_then(|mut connection| query_identity(&mut connection, &operation).ok());
        if current.session != original || public_owner.as_ref() != Some(&source_identity(store)?) {
            source.rename(&operation, &original)?;
        }
        store.journal.source_restored = true;
        store.save()?;
        if released {
            source.request(&operation, Command::FinishMigration)?;
        } else {
            source.request(&operation, Command::AbortMigration)?;
        }
        if let Some(destination) = replacement {
            destination.kill()?;
        }
        anyhow::ensure!(
            failures.is_empty(),
            "source session is restored; some update participants still require recovery: {}",
            failures.join("; ")
        );
        cleanup_remote_helpers(store)?;
        store.phase(Phase::Failed)?;
        Ok(())
    })();
    if let Err(error) = &result {
        store
            .journal
            .errors
            .push(format!("Recovery incomplete: {error:#}"));
        store.phase(Phase::RecoveryRequired)?;
    }
    result
}

fn read_recovery_snapshot(
    path: &Path,
) -> Result<yinz_types::session_migration::SessionMigrationSnapshot> {
    serde_json::from_reader(std::io::BufReader::new(
        std::fs::File::open(path).context("open recovery snapshot")?,
    ))
    .context("read recovery snapshot")
}

#[cfg(test)]
#[path = "../unit/live_update_cold_recovery.rs"]
mod cold_recovery_tests;

#[cfg(all(
    test,
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[path = "../unit/source_build_coordinator.rs"]
mod source_build_tests;

#[cfg(test)]
#[path = "../unit/prerelease_coordinator.rs"]
mod prerelease_tests;

#[cfg(test)]
#[path = "../unit/live_update_discovery.rs"]
mod discovery_tests;

#[cfg(test)]
#[path = "../unit/update_preparation.rs"]
mod preparation_tests;

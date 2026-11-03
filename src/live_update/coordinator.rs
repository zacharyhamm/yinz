use super::*;
use super::{
    journal::{Journal, Participant, Phase, Route, Store},
    transport::Connection,
};
use std::{process::Command as Process, time::Instant};
use yinz_types::{
    data::{ClientId, SessionConnection},
    session_update::{Command, Inventory},
    update_client::{ClientReply, ClientRequest, Request},
};

#[cfg(test)]
std::thread_local! {
    /// Subprocess regressions exercise failed readiness without waiting a minute.
    pub(super) static CLIENT_READY_TIMEOUT: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::from_secs(60)) };
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
            anyhow::ensure!(update::version(&actual.running_version)? <= update::version(&release.tag_name)?, "selected client is running a newer release; refusing to downgrade it during recovery");
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

pub fn run(socket: &Path, data_dir: &Path, operation: &str) -> Result<()> {
    let mut store = Store::open(socket, data_dir, operation)?;
    if matches!(
        store.journal.phase,
        Phase::Complete | Phase::Failed | Phase::Cancelled
    ) {
        return Ok(());
    }
    // A published session name now reaches the replacement. Reconnect by the
    // journal's process incarnation, never by an assumed socket/name association.
    let mut source = if store.journal.source.is_some() {
        locate_server(&store, false)?
    } else {
        Some(Connection::connect(&store.journal.source_socket)?)
    };
    if !store.journal.handoff_committed && source.is_none() && store.journal.source.is_some() {
        match recreate_source(&mut store) {
            Ok(recovery) => source = Some(recovery),
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
    if store.journal.recovery_source_identity.is_some()
        && let Some(source) = source.as_mut()
    {
        initialize_recovery_source(&store, source)?;
    }
    let result = if store.journal.handoff_committed {
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
    };
    if let Err(error) = &result {
        store.journal.errors.push(format!("{error:#}"));
        store.save()?;
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
            text: "Checking the latest stable Yinz release…".into(),
        },
    )?;
    let http = http_client()?;
    let release = update::discover(&mut |url, out| update::download(&http, url, out))?;
    execute_release(store, source, inventory, release)
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
        update::version(&inventory.running_version)? <= target,
        "source server is newer than the selected stable release; refusing downgrade"
    );
    store.journal.release = Some(serde_json::to_value(&release).context("record pinned release")?);
    store.phase(Phase::Prepare)?;
    let prepared = (|| {
        check_cancel(store)?;
        let migration =
            serde_json::from_value(source.request(&operation, Command::PrepareMigration)?)
                .context("decode migration preflight")?;
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
        for client in &inventory.clients {
            if !client.supports_update {
                store.journal.restart_required.push(client.id);
                store.save()?;
                continue;
            }
            match client_participant(source, &operation, client.id, ParticipantRequest::Inventory) {
                Ok(value) => {
                    let installation =
                        serde_json::from_value(value).context("decode client installation")?;
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
        let config: yinz_config::config::Config = decode_configuration(&inventory.configuration)?;
        let hosts = store
            .journal
            .migration
            .as_ref()
            .context("missing migration inventory")?
            .remote_hosts
            .clone();
        let mut already_current = store.journal.participants.iter().all(|participant| {
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
            store.phase(Phase::Complete)?;
            source.request(&operation,Command::Notify{text:format!("Yinz {target} is up to date. {} legacy clients need a manual update/restart on their host to verify their running version.",store.journal.restart_required.len())})?;
            return Ok(());
        }
        for host in hosts {
            check_cancel(store)?;
            let (executable, directory, helper) = transport::bootstrap_remote(
                &host,
                &release,
                &config.env,
                &operation,
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
            let installation =
                serde_json::from_value(value).context("decode remote installation")?;
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
        source.request(
            &operation,
            Command::Notify {
                text: format!(
                    "Preparing Yinz {target} on {} participating installations…",
                    unique_participants(&store.journal).len()
                ),
            },
        )?;
        for index in unique_participants(&store.journal) {
            check_cancel(store)?;
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
    if let Err(error) = check_cancel(store) {
        recover(store, source, None)?;
        return Err(error);
    }
    // Pause old plugin workers before replacing their installed sources. Every
    // installation has already staged and validated its generation at this point.
    let publication = (|| {
        check_cancel(store)?;
        store.phase(Phase::Quiesce)?;
        source.request(
            &operation,
            Command::Notify {
                text: format!("Migrating this session to Yinz {target}…"),
            },
        )?;
        // Refresh preflight just before pausing: panes may have changed during download.
        let migration =
            serde_json::from_value(source.request(&operation, Command::PrepareMigration)?)
                .context("refresh migration preflight")?;
        let migration: yinz_types::session_migration::SessionMigrationInventory = migration;
        let staged_hosts = store
            .journal
            .migration
            .as_ref()
            .context("missing original preflight")?
            .remote_hosts
            .clone();
        anyhow::ensure!(
            migration
                .remote_hosts
                .iter()
                .all(|host| staged_hosts.contains(host)),
            "a pane on an unstaged remote host appeared during preparation; retry the update"
        );
        let current: Inventory =
            serde_json::from_value(source.request(&operation, Command::Inventory)?)
                .context("refresh attached clients")?;
        anyhow::ensure!(
            current.clients.iter().all(|client| inventory
                .clients
                .iter()
                .any(|prepared| prepared.id == client.id)),
            "a client attached during preparation; retry the update to prepare its installation"
        );
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
        anyhow::ensure!(actual.remote_hosts.iter().all(|host|staged_hosts.contains(host)), "a pane on an unstaged remote host appeared before quiescence; restoring source to prepare that host on retry");
        store.journal.migration = Some(actual);
        let frozen: Inventory =
            serde_json::from_value(source.request(&operation, Command::Inventory)?)
                .context("capture quiesced client configuration")?;
        anyhow::ensure!(frozen.clients.iter().all(|client|inventory.clients.iter().any(|prepared|prepared.id==client.id)), "a client attached before quiescence; restoring source so its installation can be prepared on retry");
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
        for index in unique_participants(&store.journal) {
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
        // own published operation before it accepts a restart request.
        for index in 0..store.journal.participants.len() {
            if matches!(store.journal.participants[index].route, Route::Client(_)) {
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

fn check_cancel(store: &Store) -> Result<()> {
    anyhow::ensure!(
        !store.cancel_requested(),
        "update cancelled before terminal transfer"
    );
    Ok(())
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

fn initialize_replacement(store: &Store, connection: &mut Connection) -> Result<()> {
    let source = store
        .journal
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
    assets.update_target = Some(store.journal.operation.clone());
    assets.update_source_session = Some(source.session.clone().into());
    assets.supports_in_place_update = false;
    connection.initialize(assets)?;
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
        let mut assets = original.assets.clone();
        assets.update_configuration = Some(
            original
                .configuration
                .as_str()
                .context("missing original effective configuration")?
                .to_owned(),
        );
        assets.configuration_options = None;
        assets.layout = None;
        assets.update_target = Some(operation.clone());
        assets.update_source_session = Some(original.session.clone().into());
        assets.supports_in_place_update = false;
        source.initialize(assets)?;
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
    target: &semver::Version,
    expected: &Installation,
    attached: &Installation,
) -> Result<bool> {
    if !same_installation(expected, attached) {
        return Ok(false);
    }
    anyhow::ensure!(
        update::version(&attached.running_version)? >= *target
            && update::version(&attached.installed_version)? >= *target,
        "an attached client of {} still runs an older release; its installation backup is retained",
        attached.executable.display()
    );
    Ok(true)
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
                let target = update::version(&release.tag_name)?;
                let mut candidates = Vec::new();
                // A manual attachment may service installation cleanup without
                // claiming that its original client/configuration was replaced.
                // Inspect every capable attachment before selecting a helper so
                // an old exec-failure fallback cannot be hidden by a newer peer.
                for client in current
                    .clients
                    .iter()
                    .filter(|client| client.supports_update)
                {
                    let identity: Installation = serde_json::from_value(client_participant(
                        replacement,
                        operation,
                        client.id,
                        ParticipantRequest::Inventory,
                    )?)
                    .context("inspect manually attached installation")?;
                    if manual_commit_matches(&target, &participant_record.installation, &identity)?
                    {
                        candidates.push(client.id);
                    }
                }
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
        store.phase(if store.cancel_requested() {
            Phase::Cancelled
        } else {
            Phase::Failed
        })?;
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

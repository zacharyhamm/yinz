//! Capability checks and correlated routing for the external session-update coordinator.
use super::*;
use std::{collections::HashMap, sync::mpsc, time::Instant};
use yinz_types::{session_update, update_client};

type Answer = mpsc::Sender<Result<update_client::ClientReply, String>>;

fn identity() -> session_update::ServerIdentity {
    static INSTANCE: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| uuid::Uuid::new_v4().to_string());
    session_update::ServerIdentity {
        server_pid: std::process::id(),
        server_instance: INSTANCE.clone(),
    }
}
#[derive(Default)]
pub(crate) struct PendingRequests {
    requests: HashMap<(ClientId, String), (Answer, Instant)>,
}

impl PendingRequests {
    pub fn answer(&mut self, client: ClientId, reply: update_client::Reply) {
        if let Some((sender, _)) = self.requests.remove(&(client, reply.token)) {
            let _ = sender.send(reply.result);
        }
    }
}

pub(crate) fn route_client_request(
    client: ClientId,
    request: update_client::Request,
    reply: Answer,
    pending: &mut PendingRequests,
    os_input: &mut Box<dyn ServerOsApi>,
    state: &Arc<RwLock<SessionState>>,
    data: &Arc<RwLock<Option<SessionMetaData>>>,
) {
    pending
        .requests
        .retain(|_, (_, at)| at.elapsed() < Duration::from_secs(30));
    let refusal = match state.read() {
        Ok(state) if !state.is_interactive(&client) => Some("Update client disconnected"),
        Ok(state)
            if !state
                .update_client_assets
                .get(&client)
                .is_some_and(|a| a.supports_in_place_update) =>
        {
            Some(update_client::UNSUPPORTED_CLIENT)
        },
        Ok(_) => None,
        Err(_) => Some("Cannot inspect update client capabilities"),
    };
    if let Some(reason) = refusal {
        let _ = reply.send(Err(reason.into()));
        return;
    }
    let key = (client, request.token.clone());
    if pending.requests.contains_key(&key) {
        let _ = reply.send(Err(
            "An update request with this token is already pending".into()
        ));
        return;
    }
    pending.requests.insert(key, (reply, Instant::now()));
    if let Err(error) = os_input.send_to_client(client, ServerToClientMsg::UpdateRequest(request)) {
        Err::<(), _>(error)
            .context("deliver client update request")
            .non_fatal();
        remove_client_and_flush_forwards(client, os_input, state, data).non_fatal();
    }
}

fn inventory(
    state: &Arc<RwLock<SessionState>>,
    data: &Arc<RwLock<Option<SessionMetaData>>>,
    client: ClientId,
) -> Result<session_update::Inventory> {
    let (mut assets, executable, mut clients, effective_client) = {
        let state = state
            .read()
            .to_anyhow()
            .context("read update participant state")?;
        let assets = state
            .update_startup_assets
            .clone()
            .context("session has not initialized")?;
        let executable = state
            .update_executable
            .clone()
            .context("server executable cannot be resolved")?;
        let mut clients: Vec<_> = state
            .interactive_client_ids()
            .into_iter()
            .map(|id| session_update::Client {
                id,
                supports_update: state
                    .update_client_assets
                    .get(&id)
                    .is_some_and(|a| a.supports_in_place_update),
                resumed: state.resumed_updates.get(&id).cloned(),
                configuration: String::new(),
                mode: Default::default(),
            })
            .collect();
        clients.sort_by_key(|client| client.id);
        (
            assets,
            executable,
            clients,
            state.cli_action_target(client, None),
        )
    };
    let data = data
        .read()
        .to_anyhow()
        .context("read session update configuration")?;
    let session = data.as_ref().context("session has not initialized")?;
    assets.update_layout = Some(
        serde_json::to_string(&MigrationLayout::from(session.effective_layout.clone()))
            .context("encode effective session layout")?,
    );
    for client in &mut clients {
        client.configuration = session
            .session_configuration
            .get_client_configuration(&client.id)
            .to_string(false)
            .map_err(|error| anyhow!("{error}"))
            .context("encode client update configuration")?;
        client.mode = session
            .current_input_modes
            .get(&client.id)
            .copied()
            .unwrap_or_default();
    }
    Ok(session_update::Inventory {
        session: session.senders.session.current().to_string(),
        server_pid: std::process::id(),
        server_instance: identity().server_instance,
        executable,
        running_version: yinz_runtime::consts::VERSION.into(),
        assets,
        clients,
        configuration: serde_json::Value::String(
            session
                .session_configuration
                .get_client_configuration(&effective_client)
                .to_string(false)
                .map_err(|error| anyhow!("{error}"))
                .context("encode effective session configuration")?,
        ),
    })
}

pub(crate) fn handle(
    request: session_update::Request,
    client: ClientId,
    state: &Arc<RwLock<SessionState>>,
    data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> session_update::Reply {
    let result = dispatch(&request, client, state, data).map_err(|error| format!("{error:#}"));
    session_update::Reply {
        token: request.token,
        operation: request.operation,
        result,
    }
}

fn dispatch(
    request: &session_update::Request,
    client: ClientId,
    state: &Arc<RwLock<SessionState>>,
    data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<serde_json::Value> {
    use session_update::Command;
    anyhow::ensure!(
        !request.token.is_empty() && request.token.len() <= 256,
        "invalid update request token"
    );
    anyhow::ensure!(
        uuid::Uuid::parse_str(&request.operation).is_ok(),
        "invalid update operation id"
    );
    if let Command::Identity = request.command {
        let mut value = serde_json::to_value(identity()).context("encode server incarnation")?;
        value["initialized"] = serde_json::Value::Bool(
            data.read()
                .to_anyhow()
                .context("read server initialization state")?
                .is_some(),
        );
        value["update_operation"] = serde_json::to_value(
            state
                .read()
                .to_anyhow()
                .context("read server update operation")?
                .update_startup_assets
                .as_ref()
                .and_then(|assets| assets.update_target.clone()),
        )
        .context("encode server update operation")?;
        return Ok(value);
    }
    if let Command::Inventory = request.command {
        return serde_json::to_value(inventory(state, data, client)?)
            .context("encode session inventory");
    }
    if let Command::Start = request.command {
        return start(request, client, state, data);
    }
    let senders = data
        .read()
        .to_anyhow()
        .context("read update session")?
        .as_ref()
        .context("session is unavailable")?
        .senders
        .clone();
    match &request.command {
        Command::RestoreClients { clients } => {
            let mut saved = Vec::new();
            for client in clients {
                let config = Config::from_kdl(&client.configuration, None)
                    .map_err(|error| anyhow!("{error}"))
                    .context("decode restored client configuration")?;
                saved.push((client.id, config, client.mode));
            }
            let mut state = state
                .write()
                .to_anyhow()
                .context("record restored update clients")?;
            anyhow::ensure!(
                state
                    .update_startup_assets
                    .as_ref()
                    .and_then(|a| a.update_target.as_deref())
                    == Some(request.operation.as_str()),
                "client restoration belongs to a different update target"
            );
            for (client, configuration, mode) in saved {
                state
                    .pending_update_clients
                    .insert((request.operation.clone(), client), (configuration, mode));
            }
            Ok(serde_json::Value::Null)
        },
        Command::RenameSession { name } => {
            yinz_bus::migration::rename_session(
                &senders,
                &request.operation,
                name.clone().into(),
                client,
                Duration::from_secs(30),
            )?;
            Ok(serde_json::Value::Null)
        },
        Command::Notify { text } => {
            anyhow::ensure!(text.len() <= 8192, "update notification is too large");
            senders
                .send_to_screen(ScreenInstruction::Notify(text.clone(), None, None, None))
                .context("show update progress")?;
            Ok(serde_json::Value::Null)
        },
        Command::LegacyReconnect { client, target } => {
            senders
                .send_to_server(ServerInstruction::SwitchSession(
                    target.clone(),
                    *client,
                    None,
                ))
                .context("reconnect legacy client")?;
            Ok(serde_json::Value::Null)
        },
        Command::Client { client, request } => {
            let (reply, receive) = mpsc::channel();
            senders
                .send_to_server(ServerInstruction::UpdateRequest {
                    client: *client,
                    request: request.clone(),
                    reply,
                })
                .context("route participant request")?;
            let result = receive
                .recv_timeout(Duration::from_secs(30))
                .context("client update request timed out; inspect its journal before retrying")?
                .map_err(anyhow::Error::msg)?;
            serde_json::to_value(result).context("encode participant reply")
        },
        Command::PrepareMigration => {
            let value = yinz_bus::migration::prepare_session(
                &senders,
                &request.operation,
                client,
                Duration::from_secs(60),
            )?;
            serde_json::to_value(value).context("encode session migration inventory")
        },
        Command::ReleaseMigration { snapshot } => {
            if !senders
                .session_update_quiesced
                .load(std::sync::atomic::Ordering::Acquire)
            {
                state
                    .write()
                    .to_anyhow()
                    .context("remember update control endpoint")?
                    .update_control_socket = Some(
                    yinz_runtime::consts::YINZ_SOCK_DIR.join(senders.session.current().as_str()),
                );
            }
            let value = yinz_bus::migration::release_session(
                &senders,
                &request.operation,
                snapshot,
                client,
                Duration::from_secs(120),
            )?;
            // The complete terminal state stays in the private durable snapshot;
            // echoing it here would reintroduce the IPC frame limit we avoided.
            Ok(
                serde_json::json!({"format":value.format,"operation":value.operation,"phase":value.phase,"inventory":value.inventory}),
            )
        },
        Command::AdoptMigration { snapshot } => {
            let value = yinz_bus::migration::adopt_session(
                &senders,
                &request.operation,
                snapshot,
                client,
                Duration::from_secs(120),
            )?;
            serde_json::to_value(value).context("encode session adoption result")
        },
        Command::AbortMigration => {
            yinz_bus::migration::abort_session(
                &senders,
                &request.operation,
                Duration::from_secs(60),
            )?;
            Ok(serde_json::Value::Null)
        },
        Command::FinishMigration => {
            yinz_bus::migration::finish_session(
                &senders,
                &request.operation,
                Duration::from_secs(90),
            )?;
            Ok(serde_json::Value::Null)
        },
        Command::Identity | Command::Inventory | Command::Start => unreachable!(),
        Command::Status | Command::Cancel => {
            let inventory = inventory(state, data, client)?;
            let directory = inventory
                .assets
                .data_dir
                .unwrap_or_else(get_default_data_dir)
                .join("updates")
                .join(&request.operation);
            let _guard = if matches!(request.command, Command::Cancel) {
                Some(yinz_runtime::session_update::lock_phase(&directory)?)
            } else {
                None
            };
            let value: serde_json::Value = serde_json::from_slice(
                &std::fs::read(directory.join("journal.json")).context("read update journal")?,
            )
            .context("decode update journal")?;
            if matches!(request.command, Command::Cancel) {
                let phase = value
                    .get("phase")
                    .and_then(serde_json::Value::as_str)
                    .context("journal has no phase")?;
                anyhow::ensure!(
                    matches!(phase, "Discover" | "Prepare" | "Quiesce"),
                    "terminal transfer has begun; the update must commit or recover"
                );
                use std::os::unix::fs::OpenOptionsExt;
                let marker = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .mode(0o600)
                    .open(directory.join("cancel"))
                    .context("request update cancellation")?;
                marker.sync_all().context("persist cancellation request")?;
                std::fs::File::open(&directory)
                    .context("open cancellation directory")?
                    .sync_all()
                    .context("persist cancellation directory")?;
            }
            Ok(value)
        },
    }
}

fn start(
    request: &session_update::Request,
    client: ClientId,
    state: &Arc<RwLock<SessionState>>,
    data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<serde_json::Value> {
    use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
    let inventory = inventory(state, data, client)?;
    let data_dir = inventory
        .assets
        .data_dir
        .clone()
        .unwrap_or_else(get_default_data_dir);
    let socket = control_socket(&inventory.session, state, data)?;
    yinz_runtime::shared::ensure_secure_dir(&data_dir.join("updates"))
        .context("secure session update directory")?;
    let record = yinz_runtime::session_update::record(&data_dir, &socket)?;
    let launch = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(record.with_extension("launch"))
        .context("open update launch lock")?;
    match launch.try_lock() {
        Ok(()) => {},
        Err(std::fs::TryLockError::WouldBlock) => {
            return Ok(
                yinz_runtime::session_update::read_active(&data_dir, &socket)?
                    .filter(|active| {
                        !matches!(
                            active["phase"].as_str(),
                            Some("Complete" | "Failed" | "Cancelled")
                        )
                    })
                    .unwrap_or_else(
                        || serde_json::json!({"phase":"Starting","session":inventory.session}),
                    ),
            )
        },
        Err(std::fs::TryLockError::Error(error)) => {
            return Err(error).context("lock update coordinator startup")
        },
    }
    let active = yinz_runtime::session_update::read_active(&data_dir, &socket)?;
    let mut operation = request.operation.clone();
    if let Some(active) = active {
        let phase = active["phase"]
            .as_str()
            .context("update journal has no phase")?;
        if !matches!(phase, "Complete" | "Failed" | "Cancelled") {
            operation = active["operation"]
                .as_str()
                .context("update journal has no operation")?
                .to_owned();
            let running = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .mode(0o600)
                .open(record.with_extension("lock"))
                .context("open update coordinator lock")?;
            match running.try_lock() {
                Ok(()) => {},
                Err(std::fs::TryLockError::WouldBlock) => return Ok(active),
                Err(std::fs::TryLockError::Error(error)) => {
                    return Err(error).context("inspect update coordinator lock")
                },
            }
        }
    }
    let mut child = std::process::Command::new(&inventory.executable)
        .arg(session_update::COORDINATOR_ARG)
        .arg(&socket)
        .arg(&data_dir)
        .arg(&operation)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .context("launch independent session update coordinator")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let result = loop {
        if let Some(active) = yinz_runtime::session_update::read_active(&data_dir, &socket)?
            && active["operation"].as_str() == Some(operation.as_str())
        {
            break Ok(active);
        }
        if let Some(status) = child
            .try_wait()
            .context("inspect update coordinator startup")?
        {
            break Err(anyhow!(
                "update coordinator exited before creating its journal: {status}"
            ));
        }
        if Instant::now() >= deadline {
            break Err(anyhow!(
                "update coordinator has not published its journal yet; check session notifications"
            ));
        }
        std::thread::park_timeout(Duration::from_millis(10));
    };
    thread::Builder::new()
        .name("update-coordinator-wait".into())
        .spawn(move || {
            child.wait().context("reap update coordinator").non_fatal();
        })
        .context("watch update coordinator")?;
    result
}

/// A retired source still serves its attached clients while the coordinator
/// replaces them. Its temporary socket name must not create a second operation
/// or hide the journal which was keyed by the original public endpoint.
fn control_socket(
    session: &str,
    state: &Arc<RwLock<SessionState>>,
    data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<PathBuf> {
    let quiesced = data
        .read()
        .to_anyhow()
        .context("inspect update action gate")?
        .as_ref()
        .is_some_and(|session| {
            session
                .senders
                .session_update_quiesced
                .load(std::sync::atomic::Ordering::Acquire)
        });
    if quiesced
        && let Some(socket) = state
            .read()
            .to_anyhow()
            .context("read update control endpoint")?
            .update_control_socket
            .clone()
    {
        return Ok(socket);
    }
    Ok(yinz_runtime::consts::YINZ_SOCK_DIR.join(session))
}

pub(crate) fn active_control(
    client: ClientId,
    cancel: bool,
    state: &Arc<RwLock<SessionState>>,
    data: &Arc<RwLock<Option<SessionMetaData>>>,
) -> Result<String> {
    let inventory = inventory(state, data, client)?;
    let data_dir = inventory
        .assets
        .data_dir
        .unwrap_or_else(get_default_data_dir);
    let socket = control_socket(&inventory.session, state, data)?;
    let Some(active) = yinz_runtime::session_update::read_active(&data_dir, &socket)? else {
        return Ok("No Yinz update has been started for this session".into());
    };
    let operation = active["operation"]
        .as_str()
        .context("update journal has no operation")?;
    if cancel {
        dispatch(
            &session_update::Request {
                token: uuid::Uuid::new_v4().to_string(),
                operation: operation.into(),
                command: session_update::Command::Cancel,
            },
            client,
            state,
            data,
        )?;
        return Ok("Update cancellation requested; preparation will finish cleanup".into());
    }
    let attempted = active["restart_attempted"].as_array();
    let pending = active["restart_required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|id| attempted.is_some_and(|attempted| attempted.contains(id)))
        .filter_map(serde_json::Value::as_u64)
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    Ok(format!(
        "Yinz update {}: {}{}{}",
        operation,
        active["phase"].as_str().unwrap_or("unknown"),
        active["errors"]
            .as_array()
            .and_then(|errors| errors.last())
            .and_then(serde_json::Value::as_str)
            .map(|error| format!(" — {error}"))
            .unwrap_or_default(),
        if pending.is_empty() {
            String::new()
        } else {
            format!(
                ". Original client IDs awaiting explicit retry: {}",
                pending.join(", ")
            )
        }
    ))
}

/// Restore templates from the source generation, independent of files that may have changed.
pub(crate) fn replacement_layout(
    assets: &yinz_types::input::cli_assets::CliAssets,
) -> Result<yinz_types::input::layout::Layout> {
    let saved: MigrationLayout = serde_json::from_str(
        assets
            .update_layout
            .as_deref()
            .context("replacement server is missing its effective layout")?,
    )
    .context("restore replacement layout")?;
    Ok(saved.restore())
}

#[cfg(test)]
#[path = "unit/update_layout.rs"]
mod layout_tests;

// JSON object keys cannot represent LayoutConstraint's data-bearing variants.
// Keep constrained swap layouts as entry sequences in this private migration format.
type MigrationSwap<T> = Vec<(
    Vec<(yinz_types::input::layout::LayoutConstraint, T)>,
    Option<String>,
)>;

#[derive(serde::Serialize, serde::Deserialize)]
struct MigrationLayout {
    layout: yinz_types::input::layout::Layout,
    tiled: MigrationSwap<yinz_types::input::layout::TiledPaneLayout>,
    floating: MigrationSwap<Vec<yinz_types::input::layout::FloatingPaneLayout>>,
}

impl From<yinz_types::input::layout::Layout> for MigrationLayout {
    fn from(mut layout: yinz_types::input::layout::Layout) -> Self {
        let tiled = std::mem::take(&mut layout.swap_tiled_layouts)
            .into_iter()
            .map(|(entries, name)| (entries.into_iter().collect(), name))
            .collect();
        let floating = std::mem::take(&mut layout.swap_floating_layouts)
            .into_iter()
            .map(|(entries, name)| (entries.into_iter().collect(), name))
            .collect();
        Self {
            layout,
            tiled,
            floating,
        }
    }
}

impl MigrationLayout {
    fn restore(mut self) -> yinz_types::input::layout::Layout {
        self.layout.swap_tiled_layouts = self
            .tiled
            .into_iter()
            .map(|(entries, name)| (entries.into_iter().collect(), name))
            .collect();
        self.layout.swap_floating_layouts = self
            .floating
            .into_iter()
            .map(|(entries, name)| (entries.into_iter().collect(), name))
            .collect();
        self.layout
    }
}

mod lifecycle;
#[cfg(test)]
use lifecycle::{arm_unload_watchdog, handle_plugin_loading_failure, PaneTicket};
pub use lifecycle::{handle_plugin_crash, LoadingContext};

use super::{PinnedExecutor, PluginId, PluginInstruction};
use crate::pipes::{
    apply_pipe_message_to_plugin, pipes_to_block_or_unblock, report_pipe_changes, PendingPipes,
    PipeOutcomes,
};
use crate::plugin_loader::{script_source, PluginLoader};
use crate::plugin_map::{
    AtomicEvent, PluginConfiguration, PluginEnv, PluginMap, RunningPlugin, RunningPluginEntry,
    Subscriptions,
};
use crate::plugin_panes::{
    send_effects, Failure, PaneEffect, PaneRequest, PluginPanes, Prepared, Surface,
};
use crate::{PipeStateChange, PluginRenderAsset};
use yinz_bus::plugins::{PipeCompletion, PipeRequestId, PluginInstanceId};
use yinz_types::internal_ids::TabId;

use log::info;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard, PoisonError, TryLockError, Weak,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc::Sender;
use url::Url;
use yinz_config::permission::PermissionCache;
use yinz_runtime::consts::YINZ_CACHE_DIR;
use yinz_runtime::owned_dir::OwnedDir;
use yinz_storage::data_access::DataAccess;
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::input::keybinds::Keybinds;
use yinz_types::style::HostTerminalThemeMode;
use {
    yinz_types::data::InputMode, yinz_types::data::KeybindsVec, yinz_types::data::LayoutInfo,
    yinz_types::data::LayoutWithError, yinz_types::data::PaneContents,
    yinz_types::data::PaneRenderReport, yinz_types::data::PermissionStatus,
    yinz_types::data::PermissionType, yinz_types::data::PipeMessage, yinz_types::data::PipeSource,
};

use crate::ClientId;
use crate::PaneId;
use yinz_bus::{
    background_jobs::BackgroundJob, loading_indication::LoadingIndication, route::NotificationEnd,
    screen::ScreenInstruction, thread_bus::ThreadSenders, ServerInstruction,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::pane_size::Size;
use {
    yinz_config::plugins::PluginConfig, yinz_types::data::Event, yinz_types::data::EventType,
    yinz_types::input::command::TerminalAction, yinz_types::input::layout::PluginUserConfiguration,
    yinz_types::input::layout::RunPlugin, yinz_types::input::layout::RunPluginLocation,
    yinz_types::input::layout::RunPluginOrAlias, yinz_types::input::plugins::PluginAliases,
};

#[derive(Debug, Clone)]
pub enum EventOrPipeMessage {
    Event(Option<ClientId>, Event),
    PluginEvent(
        ClientId,
        u64,
        Event,
        Option<yinz_bus::thread_bus::PluginReplyId>,
    ),
    // The cached delivery owns the pipe until replay registers the actual instances.
    PipeMessage(Option<ClientId>, PipeMessage, Option<PipeRequestId>, u64),
}

struct PipeDelivery {
    plugin: PluginId,
    instance_id: PluginInstanceId,
    instance: Arc<Mutex<RunningPlugin>>,
    client: ClientId,
    render_clients: Vec<ClientId>,
    message: PipeMessage,
    request: Option<PipeRequestId>,
}

impl EventOrPipeMessage {
    fn client_for_replay(
        &self,
        owner: ClientId,
        generation: Option<u64>,
        shared: bool,
    ) -> Option<ClientId> {
        match self {
            Self::PluginEvent(client, expected, _, _)
                if generation == Some(*expected) && (shared || *client == owner) =>
            {
                Some(*client)
            },
            Self::Event(Some(client), _) => (shared || *client == owner).then_some(*client),
            Self::PluginEvent(..) => None,
            _ => Some(owner),
        }
    }
}

#[path = "background_plugins.rs"]
mod background_plugins;
#[path = "permission_requests.rs"]
mod permission_requests;
#[path = "picker.rs"]
pub(crate) mod picker;

pub struct PluginBridge {
    pub(crate) pane_hooks: HashMap<(yinz_types::input::hooks::HookPoint, PaneId), PluginId>,
    picks: HashMap<PluginId, picker::PendingPick>,
    permission_requests: BTreeMap<u64, permission_requests::PendingPermission>,
    next_permission_request: u64,
    /// Whether each plugin has a pane; the only owner of `ScreenInstruction::AddPlugin`.
    pub(crate) panes: PluginPanes,
    independent_plugin_ids: HashSet<PluginId>,
    configured_plugins: Vec<RunPlugin>,
    resolved_plugin_locations: HashMap<RunPluginLocation, RunPluginLocation>,
    latest_plugin_errors: HashMap<PluginId, String>,
    plugin_callers: HashMap<PluginId, yinz_types::internal_ids::TerminalId>,
    temporary_root: Arc<OwnedDir>,
    temporary_cleanup_done: Option<std::sync::mpsc::Receiver<()>>,
    connected_clients: Arc<Mutex<Vec<ClientId>>>,
    senders: ThreadSenders,
    yinz_data_dir: PathBuf,
    data_access: Arc<DataAccess>,
    watch_generations: super::plugin_map::WatchGenerations,
    pub(super) plugin_map: Arc<Mutex<PluginMap>>,
    plugin_executor: Arc<PinnedExecutor>,
    next_plugin_id: u32,
    next_load_id: u64,
    // Last scheduled attempt: a delayed load result must not settle a replacement cache.
    load_ids: HashMap<PluginId, u64>,
    // plugins that run as a single instance for the whole session rather than one per client
    shared_plugin_ids: HashSet<PluginId>,
    plugin_ids_waiting_for_permission_request: HashSet<PluginId>,
    /// `yinz events` clients, each with the event types it asked for; an empty set is every
    /// streamable one. Kept here rather than on the screen because this thread is the one place
    /// every event passes through, whichever thread produced it.
    cli_event_subscribers: HashMap<ClientId, HashSet<EventType>>,
    cached_events_for_pending_plugins: HashMap<PluginId, Vec<EventOrPipeMessage>>,
    cached_resizes_for_pending_plugins: HashMap<PluginId, (usize, usize)>, // (rows, columns)
    cached_worker_messages: HashMap<PluginId, Vec<(ClientId, String, String, String)>>, // Vec<clientid,
    // worker_name,
    // message,
    // payload>
    loading_plugins: HashSet<(PluginId, RunPlugin)>, // tracks loading plugins without handles
    /// Loads unloaded before registration. The cancel token prevents publication; the loader
    /// or its queued retirement consumes this mark and releases the executor assignment.
    unloaded_while_loading: Arc<Mutex<HashSet<PluginId>>>,
    pending_plugin_reloads: HashSet<RunPlugin>,
    pending_plugin_id_reloads: HashSet<PluginId>,
    /// One cancel token per plugin, shared with every instance's `PluginEnv::cancel`. Kept here
    /// so that `unload_plugin` can set it **without taking the plugin's lock** — an instance
    /// looping inside `update()` holds that lock on its executor thread, and the plugin thread
    /// waiting for it there is exactly how a runaway plugin once wedged session shutdown.
    cancel_tokens: HashMap<PluginId, Arc<AtomicBool>>,
    // Attachment-scoped: a reused client ID must not revive pending work from its
    // previous connection. Separate from plugin cancellation so peers keep running.
    client_cancellations: Arc<Mutex<HashMap<ClientId, Arc<AtomicBool>>>>,
    path_to_default_shell: PathBuf,
    yinz_cwd: PathBuf,
    session_env_vars: std::collections::BTreeMap<String, String>,
    server_session: crate::ServerSession,
    default_shell: Option<TerminalAction>,
    pending_pipes: PendingPipes,
    // Bound before delivery, so disconnect can retire a REPL whose loader has not run yet.
    repl_pipe_owners: HashMap<PluginId, String>,
    layout_dir: Option<PathBuf>,
    user_plugin_dir: Option<PathBuf>,
    available_layouts: Vec<LayoutInfo>,
    available_layout_errors: Vec<LayoutWithError>,
    default_mode: InputMode,
    default_keybinds: Keybinds,
    keybinds: HashMap<ClientId, Keybinds>,
    base_modes: HashMap<ClientId, InputMode>,
    previous_pane_render_report: Option<PaneRenderReport>,
    /// The last `HostTerminalThemeChanged` broadcast to every plugin, replayed
    /// to a plugin that subscribes to it after the fact so it does not wait
    /// for the next host report to learn the session's hue.
    last_host_terminal_theme_mode: Option<HostTerminalThemeMode>,
    pub last_session_save_time: Arc<Mutex<Option<u64>>>, // milliseconds since UNIX epoch
    /// The `plugins { }` block, for plugins asking what a *different* alias was
    /// configured with (`PluginCommand::GetPluginAliasConfiguration`).
    ///
    /// Shared rather than cloned into each `PluginEnv` because it is the one piece of
    /// plugin state that must survive a config reload: `detect_and_notify_plugin_config_changes`
    /// is handed the new table and writes it here, so a plugin that asks after the reload
    /// is answered with what the user just wrote rather than with what was on disk at
    /// server start.
    pub plugin_aliases: Arc<Mutex<PluginAliases>>,
    #[cfg(test)]
    after_reconfigure_snapshot: Option<Box<dyn FnOnce() + Send>>,
}

#[cfg(test)]
thread_local! {
    /// A test's fixed size for the executors of the bridges built on its thread: one thread
    /// proves that a guest waiting on another plugin's pane never waits for that plugin's load
    /// to be scheduled. Per thread, so a test that sets it never changes the bridges of tests
    /// running beside it in the same process (bare `cargo test`); the test harness hands it to
    /// the plugin thread it spawns.
    pub(crate) static TEST_EXECUTOR_THREADS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

/// How many pinned executor threads a bridge runs plugins on.
pub(crate) fn executor_threads() -> usize {
    #[cfg(test)]
    {
        let fixed = TEST_EXECUTOR_THREADS.with(std::cell::Cell::get);
        if fixed > 0 {
            return fixed;
        }
    }
    num_cpus::get().clamp(4, 16)
}

impl PluginBridge {
    pub fn new(
        senders: ThreadSenders,
        yinz_data_dir: PathBuf,
        data_access: Arc<DataAccess>,
        path_to_default_shell: PathBuf,
        yinz_cwd: PathBuf,
        session_env_vars: std::collections::BTreeMap<String, String>,
        default_shell: Option<TerminalAction>,
        layout_dir: Option<PathBuf>,
        user_plugin_dir: Option<PathBuf>,
        available_layouts: Vec<LayoutInfo>,
        available_layout_errors: Vec<LayoutWithError>,
        default_mode: InputMode,
        default_keybinds: Keybinds,
        plugin_aliases: PluginAliases,
        server_session: crate::ServerSession,
    ) -> Result<Self> {
        let mut temporary_root = OwnedDir::new(&YINZ_CACHE_DIR.join("plugin-tmp"))
            .context("failed to create plugin host temporary directory")?;
        let temporary_cleanup_done = Some(
            temporary_root
                .drop_notification()
                .context("failed to observe plugin host cleanup")?,
        );
        let plugin_map = Arc::new(Mutex::new(PluginMap::default()));
        let connected_clients = plugin_map.lock().unwrap().connected_clients.clone();
        let max_threads = executor_threads();
        let plugin_executor = Arc::new(PinnedExecutor::new(
            max_threads,
            &senders,
            &plugin_map,
            &connected_clients,
        ));
        Ok(PluginBridge {
            permission_requests: BTreeMap::new(),
            next_permission_request: 0,
            panes: Default::default(),
            picks: HashMap::new(),
            independent_plugin_ids: HashSet::new(),
            configured_plugins: vec![],
            resolved_plugin_locations: HashMap::new(),
            latest_plugin_errors: HashMap::new(),
            plugin_callers: HashMap::new(),
            temporary_root,
            temporary_cleanup_done,
            connected_clients,
            senders,
            yinz_data_dir,
            data_access,
            watch_generations: Default::default(),
            plugin_map,
            plugin_executor,
            pane_hooks: HashMap::new(),
            path_to_default_shell,
            next_plugin_id: 0,
            next_load_id: 0,
            load_ids: HashMap::new(),
            shared_plugin_ids: HashSet::new(),
            cached_events_for_pending_plugins: HashMap::new(),
            plugin_ids_waiting_for_permission_request: HashSet::new(),
            cached_resizes_for_pending_plugins: HashMap::new(),
            cached_worker_messages: HashMap::new(),
            cli_event_subscribers: HashMap::new(),
            loading_plugins: HashSet::new(),
            unloaded_while_loading: Default::default(),
            pending_plugin_reloads: HashSet::new(),
            pending_plugin_id_reloads: HashSet::new(),
            cancel_tokens: HashMap::new(),
            client_cancellations: Arc::default(),
            yinz_cwd,
            session_env_vars,
            server_session,
            default_shell,
            pending_pipes: Default::default(),
            repl_pipe_owners: Default::default(),
            layout_dir,
            user_plugin_dir,
            available_layouts,
            available_layout_errors,
            default_mode,
            default_keybinds,
            keybinds: HashMap::new(),
            base_modes: HashMap::new(),
            previous_pane_render_report: None,
            last_host_terminal_theme_mode: None,
            last_session_save_time: Arc::new(Mutex::new(None)),
            plugin_aliases: Arc::new(Mutex::new(plugin_aliases)),
            #[cfg(test)]
            after_reconfigure_snapshot: None,
        })
    }
    /// The id the next [`Self::load_plugin`] gives its plugin, for a launcher that names the
    /// plugin's pane after it.
    pub(crate) fn next_plugin_id(&self) -> PluginId {
        PluginId::new(self.next_plugin_id)
    }

    pub fn resize_plugin(
        &mut self,
        pid: PluginId,
        new_columns: usize,
        new_rows: usize,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        let err_context = move || format!("failed to resize plugin {pid}");

        let plugins_to_resize = if self.cached_resizes_for_pending_plugins.contains_key(&pid) {
            vec![]
        } else {
            self.plugin_map
                .lock()
                .map_err(|_| anyhow!("plugin map lock poisoned"))
                .with_context(err_context)?
                .resize_targets(pid)
        };
        if let Some(current_size) = self.cached_resizes_for_pending_plugins.get_mut(&pid) {
            *current_size = (new_rows, new_columns);
        }
        for (client_id, running_plugin, slot) in plugins_to_resize {
            // Asked here, on the plugin thread, before the executor gets to any queued job: it
            // is how a queued resize learns a newer size is wanted. A job already queued will
            // draw it, so none is queued behind it. See `ResizeSlot`.
            if !slot.request(new_rows, new_columns) {
                continue;
            }
            let shared = self.plugin_is_shared(&pid);
            let queued_slot = slot.clone();
            // Execute directly on pinned thread (no async I/O needed for resize/render)
            let accepted = self
                .plugin_executor
                .execute_for_plugin_tagged(pid, "resize", false, {
                    let _s = shutdown_sender.clone();
                    move |senders, _plugin_map, connected_clients| {
                        // Before the guest lock: a resize some earlier render already drew
                        // must not wait for, or cost, another. Begun before any return, so a
                        // request from here on queues the job that draws it.
                        if !slot.begin_job() {
                            return;
                        }
                        // A poisoned instance is a guest that panicked: nothing to draw.
                        let Ok(mut running_plugin) = running_plugin.lock() else {
                            return;
                        };
                        // And again with it: a callback that held it can have drawn the size.
                        if running_plugin.is_unavailable() || !slot.resize_due() {
                            return;
                        }
                        // This executor preserves request order. Allocate the id
                        // here so the plugin thread can answer synchronous guest
                        // queries while a previous callback owns the guest lock.
                        let event_id = running_plugin.next_event_id(AtomicEvent::Resize);
                        let _s = _s; // guard to allow the task to complete before cleanup/shutdown
                        if running_plugin.apply_event_id(AtomicEvent::Resize, event_id) {
                            slot.note_resize();
                            // Read now rather than when this job was queued: the job that draws
                            // is the oldest one queued, and a client can have attached or left
                            // since. A plain list, so a poisoned lock still holds a usable one.
                            let render_clients = if shared {
                                connected_clients
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .clone()
                            } else {
                                vec![client_id]
                            };
                            match render_plugin_for_clients(
                                pid,
                                &mut running_plugin,
                                &render_clients,
                            )
                            .with_context(err_context)
                            {
                                Ok(plugin_render_assets) => {
                                    report_pipe_changes(
                                        &mut running_plugin,
                                        HashMap::new(),
                                        None,
                                        &senders,
                                    );
                                    // The screen may already have exited during shutdown.
                                    // Release the guest before publishing, as event and
                                    // pipe renders do, so a send cannot poison its lock.
                                    drop(running_plugin);
                                    senders
                                        .send_to_screen(ScreenInstruction::PluginBytes(
                                            plugin_render_assets,
                                        ))
                                        .with_context(err_context)
                                        .non_fatal();
                                },
                                Err(error) => {
                                    running_plugin.mark_failed();
                                    handle_plugin_crash(
                                        pid,
                                        running_plugin.env().generation,
                                        Some(running_plugin.instance_id),
                                        format!("{error:#}"),
                                        senders,
                                    );
                                },
                            }
                        }
                    }
                });
            // A refused job never begins, so it must not stay counted as queued: every later
            // request would wait on it and the instance would never resize again.
            if !accepted {
                queued_slot.cancel_job();
            }
        }
        Ok(())
    }
    pub(crate) fn resolve_agent_event_projects(
        &self,
        version: u64,
        agents: Vec<yinz_types::agent::control::AgentStatus>,
        projects: Vec<String>,
    ) {
        let data = self.data_access.clone();
        let senders = self.senders.clone();
        let requested = projects.clone();
        let spawn = || {
            #[cfg(test)]
            if plugin_bridge_tests::FAIL_AGENT_PROJECT_SPAWN.with(std::cell::Cell::get) {
                return Err(std::io::Error::from(std::io::ErrorKind::WouldBlock));
            }
            std::thread::Builder::new()
                .name("agent-projects".into())
                .spawn(move || {
                    let result = crate::yinz_exports::agent_event_projects(
                        agents, projects, &data, &senders,
                    );
                    senders
                        .send_to_screen(ScreenInstruction::AgentEventProjects(version, result))
                        .context("report agent event projects")
                        .non_fatal();
                })
        };
        if let Err(error) = spawn().context("start agent project resolver") {
            let message = format!("{error:#}");
            let result = requested
                .into_iter()
                .map(|project| (project, Err(message.clone())))
                .collect();
            self.senders
                .send_to_screen(ScreenInstruction::AgentEventProjects(version, result))
                .context("report failed agent project resolver")
                .non_fatal();
        }
    }

    /// A `yinz events` client asking for `event_types` by name. A name that is not a
    /// streamable event refuses the whole subscription, told to the client as an error, so a
    /// typo is an exit rather than a silent stream of everything else.
    pub fn subscribe_cli_client_to_events(
        &mut self,
        client_id: ClientId,
        event_types: Vec<String>,
    ) {
        let mut wanted = HashSet::new();
        for name in &event_types {
            match EventType::from_cli_name(name) {
                Some(event_type) => {
                    wanted.insert(event_type);
                },
                None => {
                    let _ = self.senders.send_to_server(ServerInstruction::LogError(
                        vec![format!(
                            "Unknown or unstreamable event type '{}'. Streamable events: {}",
                            name,
                            EventType::cli_streamable_names().join(", ")
                        )],
                        client_id,
                        None,
                    ));
                    return;
                },
            }
        }
        self.cli_event_subscribers.insert(client_id, wanted);
        // Sent for every subscriber, not just the first: the screen answers it with a state
        // report, which is the snapshot a new subscriber starts from. It also has the screen
        // report pane and tab state when no plugin wants it, addressed to
        // `CLI_EVENTS_ONLY_TARGET`.
        let _ = self
            .senders
            .send_to_screen(ScreenInstruction::SetCliEventSubscribersPresent(true));
    }

    /// Hand every streamable event in `updates` to the `yinz events` clients that want it.
    ///
    /// A state report — `PaneUpdate`, `TabUpdate`, `ModeUpdate`, `SessionUpdate` — arrives here
    /// as one copy per (plugin, client) it is addressed to, all alike but for the addressee, and
    /// a batch merged on the plugin thread can carry several generations of one. Within a run of
    /// consecutive reports only the newest of each kind is forwarded, since each describes the
    /// whole state; a non-report event ends the run, so a key is never forwarded ahead of the
    /// report that described the mode it was pressed in. Everything else is forwarded as is.
    fn forward_events_to_cli_subscribers(
        &self,
        updates: &[(Option<PluginId>, Option<ClientId>, Event)],
    ) {
        if self.cli_event_subscribers.is_empty() {
            return;
        }
        let is_state_report = |event_type: &EventType| {
            matches!(
                event_type,
                EventType::PaneUpdate
                    | EventType::TabUpdate
                    | EventType::ModeUpdate
                    | EventType::SessionUpdate
            )
        };
        // the newest index of each kind of report in the current run, in order of first
        // appearance
        let mut pending: Vec<(EventType, usize)> = vec![];
        let flush = |pending: &mut Vec<(EventType, usize)>| {
            for (event_type, index) in pending.drain(..) {
                self.forward_event_to_cli_subscribers(event_type, &updates[index].2);
            }
        };
        for (index, (_, _, event)) in updates.iter().enumerate() {
            let event_type = EventType::from(event);
            if !event_type.is_cli_streamable() {
                continue;
            }
            if is_state_report(&event_type) {
                match pending.iter_mut().find(|(kind, _)| *kind == event_type) {
                    Some(slot) => slot.1 = index,
                    None => pending.push((event_type, index)),
                }
            } else {
                flush(&mut pending);
                self.forward_event_to_cli_subscribers(event_type, event);
            }
        }
        flush(&mut pending);
    }

    fn forward_event_to_cli_subscribers(&self, event_type: EventType, event: &Event) {
        let event_json = match serde_json::to_string(event) {
            Ok(event_json) => event_json,
            Err(e) => {
                log::error!(
                    "failed to serialize {} for a cli subscriber: {}",
                    event_type,
                    e
                );
                return;
            },
        };
        for (client_id, wanted) in &self.cli_event_subscribers {
            if wanted.is_empty() || wanted.contains(&event_type) {
                let _ = self
                    .senders
                    .send_to_server(ServerInstruction::EventUpdateToClient(
                        *client_id,
                        event_type.to_string(),
                        event_json.clone(),
                    ));
            }
        }
    }

    #[cfg(test)]
    pub fn plugin_event(
        &mut self,
        plugin_id: PluginId,
        generation: u64,
        events: Vec<(ClientId, Event)>,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        self.plugin_replies(
            plugin_id,
            generation,
            events
                .into_iter()
                .map(|(client, event)| (client, event, None))
                .collect(),
            shutdown_sender,
        )
    }

    pub fn plugin_replies(
        &mut self,
        plugin_id: PluginId,
        generation: u64,
        events: Vec<(ClientId, Event, Option<yinz_bus::thread_bus::PluginReplyId>)>,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        // Pane lifecycle events still describe the session after their plugin reloads.
        for (_, event, _) in &events {
            let event_type = EventType::from(event);
            if !self.cli_event_subscribers.is_empty() && event_type.is_cli_streamable() {
                self.forward_event_to_cli_subscribers(event_type, event);
            }
        }
        // Loading can hold the map while guest code runs. The plugin thread must
        // remain free to process the cancellation that interrupts that code.
        let (batch_label, batch_has_input) =
            summarize_batch_for_stats(events.iter().map(|(_, event, _)| event));
        let messages: Vec<_> = events
            .into_iter()
            .map(|(client, event, reply)| {
                EventOrPipeMessage::PluginEvent(client, generation, event, reply)
            })
            .collect();
        if let Some(cached) = self.cached_events_for_pending_plugins.get_mut(&plugin_id) {
            cached.extend(messages);
        } else {
            self.plugin_executor.execute_for_plugin_tagged(
                plugin_id,
                batch_label,
                batch_has_input,
                move |senders, plugin_map, connected_clients| {
                    let _shutdown = shutdown_sender;
                    let instances = {
                        let mut map = plugin_map.lock().unwrap();
                        if let Some(workers) = map.worker_routes(plugin_id, generation) {
                            for message in &messages {
                                if let EventOrPipeMessage::PluginEvent(client, _, event, reply) =
                                    message
                                {
                                    workers.send(
                                        generation,
                                        crate::background_worker::Message::Event(
                                            *client,
                                            event.clone(),
                                            *reply,
                                        ),
                                    );
                                }
                            }
                            return;
                        }
                        if !map.contains_generation(plugin_id, generation) {
                            return;
                        }
                        map.running_plugins_and_subscriptions()
                    };
                    let connected_clients = connected_clients.lock().unwrap().clone();
                    for (id, owner, instance, subscriptions) in instances {
                        if id != plugin_id {
                            continue;
                        }
                        let subs = subscriptions.lock().unwrap().clone();
                        let mut plugin = instance.lock().unwrap();
                        let shared = plugin.env().plugin_config.is_shared();
                        let events: Vec<_> = messages
                            .iter()
                            .filter_map(|message| {
                                let client = message.client_for_replay(
                                    owner,
                                    plugin.generation(),
                                    shared,
                                )?;
                                let EventOrPipeMessage::PluginEvent(_, _, event, reply) = message
                                else {
                                    unreachable!();
                                };
                                if let Some(reply) = reply {
                                    reply.claim()?;
                                }
                                let event_type = EventType::from(event);
                                (subs.contains(&event_type)
                                    || event_type == EventType::PermissionRequestResult)
                                    .then(|| (client, event.clone()))
                            })
                            .collect();
                        if events.is_empty() {
                            continue;
                        }
                        let clients = if shared {
                            connected_clients.clone()
                        } else {
                            vec![owner]
                        };
                        let mut assets = vec![];
                        let result = apply_events_to_plugin(
                            plugin_id,
                            &clients,
                            &mut plugin,
                            &events,
                            &mut assets,
                            senders.clone(),
                            &subs,
                        );
                        let disposed = plugin.is_disposed();
                        let instance_id = plugin.instance_id;
                        drop(plugin);
                        match result {
                            Ok(()) => {
                                let _ =
                                    senders.send_to_screen(ScreenInstruction::PluginBytes(assets));
                            },
                            Err(_) if disposed => {},
                            Err(error) => handle_plugin_crash(
                                plugin_id,
                                Some(generation),
                                Some(instance_id),
                                format!("{error:#}"),
                                senders.clone(),
                            ),
                        }
                    }
                },
            );
        }
        Ok(())
    }

    /// A pane-tab key may hand focus to the host. Finish that plugin event before the
    /// client's router sends its next key, so native input cannot overtake queued input.
    pub fn input_barrier(&self, plugin_id: PluginId, completion: yinz_bus::route::NotificationEnd) {
        self.plugin_executor.execute_for_plugin_tagged(
            plugin_id,
            "input_completion",
            true,
            move |_, _, _| drop(completion),
        );
    }

    pub fn update_plugins(
        &mut self,
        mut updates: Vec<(Option<PluginId>, Option<ClientId>, Event)>,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        // The screen owns pane state but has no plugin map. Fill the current session's
        // plugin identities here, before forwarding or caching its manifest.
        for (_, _, event) in &mut updates {
            if let Event::SessionUpdate(sessions, _) = event
                && let Some(current) = sessions
                    .iter_mut()
                    .find(|session| session.is_current_session)
            {
                current.populate_plugin_list(self.plugin_map.lock().unwrap().list_plugins());
            }
        }
        self.forward_events_to_cli_subscribers(&updates);
        for (plugin_id, client_id, event) in updates.iter() {
            if plugin_id.is_none()
                && client_id.is_none()
                && let Event::HostTerminalThemeChanged(mode) = event
            {
                self.last_host_terminal_theme_mode = Some(*mode);
            }
        }
        // Pending plugins replay these updates after initialization.
        let plugins_to_update: Vec<RunningPluginEntry> = if updates.iter().all(|(id, _, _)| {
            id.is_some_and(|id| self.cached_events_for_pending_plugins.contains_key(&id))
        }) {
            vec![]
        } else {
            self.plugin_map
                .lock()
                .unwrap()
                .running_plugins_and_subscriptions()
                .iter()
                .filter(
                    |&(plugin_id, _client_id, _running_plugin, _subscriptions)| {
                        !&self
                            .cached_events_for_pending_plugins
                            .contains_key(plugin_id)
                    },
                )
                .cloned()
                .collect()
        };

        // One job per plugin instance carrying every event of the batch it takes, so a batch of
        // wheel notches costs one render per plugin rather than one per notch.
        let plugin_executor = self.plugin_executor.clone();
        for (plugin_id, client_id, running_plugin, subscriptions) in &plugins_to_update {
            let subs = subscriptions.lock().unwrap().clone();
            let plugin_is_shared = self.plugin_is_shared(plugin_id);
            let events: Vec<(ClientId, Event)> = updates
                .iter()
                .filter_map(|(pid, cid, event)| {
                    let event_type = EventType::from(event);
                    let wanted = (subs.contains(&event_type)
                        || event_type == EventType::PermissionRequestResult)
                        && Self::message_is_directed_at_plugin(
                            *pid,
                            *cid,
                            plugin_id,
                            client_id,
                            plugin_is_shared,
                        );
                    if !wanted {
                        return None;
                    }
                    // for a shared plugin, the acting client is whoever the message was
                    // directed at, not the client the instance is keyed under
                    let acting_client_id = if plugin_is_shared {
                        cid.unwrap_or(*client_id)
                    } else {
                        *client_id
                    };
                    Some((acting_client_id, event.clone()))
                })
                .collect();
            if events.is_empty() {
                continue;
            }
            let render_clients = if plugin_is_shared {
                self.connected_clients.lock().unwrap().clone()
            } else {
                vec![*client_id]
            };
            let (batch_label, batch_has_input) =
                summarize_batch_for_stats(events.iter().map(|(_, event)| event));
            // Execute directly on pinned thread (no async I/O needed for event processing)
            plugin_executor.execute_for_plugin_tagged(*plugin_id, batch_label, batch_has_input, {
                let plugin_id = *plugin_id;
                let running_plugin = running_plugin.clone();
                let _s = shutdown_sender.clone();
                move |senders, _plugin_map, _connected_clients| {
                    let _s = _s; // guard to allow the task to complete before cleanup/shutdown
                    let mut running_plugin = running_plugin.lock().unwrap();
                    let mut plugin_render_assets = vec![];
                    let apply_result = apply_events_to_plugin(
                        plugin_id,
                        &render_clients,
                        &mut running_plugin,
                        &events,
                        &mut plugin_render_assets,
                        senders.clone(),
                        &subs,
                    );
                    // The screen channel is bounded and blocks when the screen
                    // thread is behind; sending while holding the plugin's lock
                    // would pass that stall on to every other job for this
                    // plugin.
                    let disposed = running_plugin.is_disposed();
                    let generation = running_plugin.env().generation;
                    let instance_id = running_plugin.instance_id;
                    drop(running_plugin);
                    match apply_result {
                        Ok(()) => {
                            let _ = senders.send_to_screen(ScreenInstruction::PluginBytes(
                                plugin_render_assets,
                            ));
                        },
                        Err(_) if disposed => {},
                        Err(e) => {
                            log::error!("{:?}", e);

                            // https://stackoverflow.com/questions/66450942/in-rust-is-there-a-way-to-make-literal-newlines-in-r-using-windows-c
                            let stringified_error = format!("{:?}", e).replace("\n", "\n\r");

                            handle_plugin_crash(
                                plugin_id,
                                generation,
                                Some(instance_id),
                                stringified_error,
                                senders.clone(),
                            );
                        },
                    }
                }
            });
        }

        // loop once more to update the cached events for the pending plugins (probably currently
        // being loaded, we'll send them these events when they load)
        for (pid, cid, event) in updates.drain(..) {
            for (plugin_id, cached_events) in self.cached_events_for_pending_plugins.iter_mut() {
                if pid.is_none() || pid.as_ref() == Some(plugin_id) {
                    cached_events.push(EventOrPipeMessage::Event(cid, event.clone()));
                }
            }
        }
        Ok(())
    }
    pub fn get_plugin_cwd(&self, plugin_id: PluginId, client_id: ClientId) -> Option<PathBuf> {
        self.plugin_map
            .lock()
            .unwrap()
            .instance_cwd(plugin_id, Some(client_id))
    }
    pub fn change_plugin_host_dir(
        &mut self,
        new_host_dir: PathBuf,
        plugin_id: PluginId,
        client_id: ClientId,
        generation: Option<u64>,
    ) -> Result<()> {
        self.plugin_executor.execute_for_plugin_tagged(
            plugin_id,
            "change_host_dir",
            false,
            move |senders, plugin_map, _| {
                // Resolve on the executor: a queued reload may have replaced the requester.
                let (worker, instances) = {
                    let mut map = plugin_map.lock().unwrap();
                    let worker = generation.is_some_and(|generation| {
                        map.worker_routes(plugin_id, generation).is_some()
                    });
                    if !worker
                        && generation.is_some_and(|generation| {
                            !map.contains_generation(plugin_id, generation)
                        })
                    {
                        return;
                    }
                    (worker, map.running_plugins())
                };
                let reply = |event| {
                    if let Event::FailedToChangeHostFolder(Some(reason)) = &event {
                        Err::<(), _>(anyhow!("{reason}"))
                            .context("failed to change plugin host folder")
                            .non_fatal();
                    }
                    let _ = senders.send_to_plugin(PluginInstruction::for_plugin_event(
                        plugin_id, client_id, generation, event,
                    ));
                };
                match new_host_dir.try_exists() {
                    Ok(false) => {
                        reply(Event::FailedToChangeHostFolder(Some(format!(
                            "Folder {} does not exist",
                            new_host_dir.display()
                        ))));
                        return;
                    },
                    Err(error) => {
                        reply(Event::FailedToChangeHostFolder(Some(error.to_string())));
                        return;
                    },
                    Ok(true) => {},
                }
                if worker {
                    // The worker updates its own environment before delivering this event.
                    reply(Event::HostFolderChanged(new_host_dir));
                    return;
                }
                for (id, owner, instance) in instances {
                    if id != plugin_id {
                        continue;
                    }
                    let mut plugin = instance.lock().unwrap();
                    if generation.map_or(owner != client_id, |generation| {
                        plugin.generation() != Some(generation)
                    }) {
                        continue;
                    }
                    let env = plugin.env_mut();
                    env.set_host_folder(new_host_dir.clone());
                    drop(plugin);
                    plugin_map.lock().unwrap().set_instance_cwd(
                        plugin_id,
                        owner,
                        new_host_dir.clone(),
                    );
                    let _ = senders.send_to_pty(yinz_bus::pty::PtyInstruction::ReportPluginCwd(
                        plugin_id,
                        new_host_dir.clone(),
                    ));
                    reply(Event::HostFolderChanged(new_host_dir));
                    return;
                }
            },
        );
        Ok(())
    }
    pub fn pipe_messages(
        &mut self,
        messages: Vec<(Option<PluginId>, Option<ClientId>, PipeMessage)>,
        shutdown_sender: Sender<()>,
        notification_end: Option<NotificationEnd>,
    ) -> Result<()> {
        // URL fan-out is several destinations of the same input, not several requests.
        let mut requests = HashMap::new();
        let mut deliveries = vec![];
        for (plugin, client, message) in messages {
            if let (Some(plugin), PipeSource::Cli(pipe)) = (plugin, &message.source) {
                let run = self
                    .run_plugin_of_loading_plugin_id(plugin)
                    .cloned()
                    .or_else(|| self.run_plugin_of_plugin_id(plugin));
                if run.is_some_and(|run| matches!(run.location, RunPluginLocation::Yinz(tag) if tag.to_string() == "repl")) {
                    self.repl_pipe_owners.entry(plugin).or_insert_with(|| pipe.clone());
                }
            }
            let request = if let PipeSource::Cli(stream) = &message.source {
                Some(
                    *requests
                        .entry(stream.clone())
                        .or_insert_with(|| self.pending_pipes.begin(stream)),
                )
            } else {
                None
            };
            self.admit_pipe(plugin, client, message, request, &mut deliveries);
        }
        self.schedule_pipes(deliveries, shutdown_sender, notification_end);
        self.finish_pipe_requests();
        Ok(())
    }

    pub fn cli_pipes_disconnected(&mut self, pipes: &[String]) {
        let plugins: Vec<_> = self
            .repl_pipe_owners
            .iter()
            .filter_map(|(plugin, pipe)| pipes.contains(pipe).then_some(*plugin))
            .collect();
        for plugin in plugins {
            self.unload_plugin(plugin)
                .context("unloading disconnected session REPL")
                .non_fatal();
        }
    }

    /// Resolve live state for both direct delivery and replay. Metadata only identifies a
    /// destination; it never admits a callback. Register the entire fan-out before scheduling.
    fn admit_pipe(
        &mut self,
        plugin: Option<PluginId>,
        client: Option<ClientId>,
        message: PipeMessage,
        request: Option<PipeRequestId>,
        deliveries: &mut Vec<PipeDelivery>,
    ) -> bool {
        let mut admitted = false;
        let mut available = HashSet::new();
        let instances = self.plugin_map.lock().unwrap().pipe_instances();
        for ((id, owner, instance, _), instance_id) in instances {
            let shared = self.plugin_is_shared(&id);
            if !Self::message_is_directed_at_plugin(plugin, client, &id, &owner, shared) {
                continue;
            }
            available.insert(id);
            if self.cached_events_for_pending_plugins.contains_key(&id) {
                continue;
            }
            admitted = true;
            if let Some(request) = request {
                self.pending_pipes.processing(request, id, instance_id);
            }
            deliveries.push(PipeDelivery {
                plugin: id,
                instance_id,
                instance,
                client: if shared {
                    client.unwrap_or(owner)
                } else {
                    owner
                },
                render_clients: if shared {
                    self.connected_clients.lock().unwrap().clone()
                } else {
                    vec![owner]
                },
                message: message.clone(),
                request,
            });
        }
        for (id, cache) in &mut self.cached_events_for_pending_plugins {
            if plugin.is_some_and(|plugin| plugin != *id) {
                continue;
            }
            if !available.contains(id)
                && (self.latest_plugin_errors.contains_key(id)
                    || (self.plugin_ids_waiting_for_permission_request.contains(id)
                        && !self
                            .loading_plugins
                            .iter()
                            .any(|(loading, _)| loading == id)))
            {
                continue;
            }
            admitted = true;
            let load = self.load_ids.get(id).copied().unwrap_or(0);
            if let Some(request) = request {
                self.pending_pipes.cache(request, *id, load);
            }
            cache.push(EventOrPipeMessage::PipeMessage(
                client,
                message.clone(),
                request,
                load,
            ));
        }
        if let (false, Some(plugin), Some(request)) = (admitted, plugin, request) {
            self.pending_pipes.fail(request, format!("Plugin {plugin} is unavailable; explicitly reload it before sending another pipe"));
        }
        admitted
    }

    fn schedule_pipes(
        &mut self,
        deliveries: Vec<PipeDelivery>,
        shutdown: Sender<()>,
        mut notification_end: Option<NotificationEnd>,
    ) {
        for delivery in deliveries {
            let PipeDelivery {
                plugin,
                instance_id,
                instance,
                client,
                render_clients,
                message,
                request,
            } = delivery;
            let guard = shutdown.clone();
            let notification = notification_end.take();
            let accepted = self.plugin_executor.execute_for_plugin_tagged(
                plugin,
                format!("pipe:{}", message.name),
                false,
                move |senders, _, _| {
                    let _guard = guard;
                    let _notification = notification;
                    let mut instance = instance.lock().unwrap();
                    let mut assets = vec![];
                    let unavailable = instance.is_unavailable();
                    let result = apply_pipe_message_to_plugin(
                        plugin,
                        client,
                        &render_clients,
                        &mut instance,
                        &message,
                        request,
                        &mut assets,
                        &senders,
                    );
                    let generation = instance.env().generation;
                    drop(instance);
                    match result {
                        Ok(()) if !assets.is_empty() => {
                            senders
                                .send_to_screen(ScreenInstruction::PluginBytes(assets))
                                .context("failed to send plugin frame")
                                .non_fatal();
                        },
                        Ok(()) => {},
                        Err(_) if unavailable => {},
                        Err(error) => handle_plugin_crash(
                            plugin,
                            generation,
                            Some(instance_id),
                            format!("{error:#}"),
                            senders,
                        ),
                    }
                },
            );
            if !accepted {
                self.pending_pipes.complete(PipeCompletion {
                    plugin_id: plugin,
                    instance_id,
                    changes: HashMap::new(),
                    callback: request.map(|request| {
                        (
                            request,
                            Err(format!("Plugin {plugin} executor rejected pipe delivery")),
                        )
                    }),
                });
            }
        }
    }

    pub fn apply_cached_events_for_load(
        &mut self,
        load_id: u64,
        mut plugin_ids: Vec<PluginId>,
        done_receiving_permissions: bool,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        plugin_ids.retain(|id| self.load_is_current(*id, load_id));
        self.apply_cached_events(plugin_ids, done_receiving_permissions, shutdown_sender)
    }

    pub fn apply_cached_events(
        &mut self,
        mut plugin_ids: Vec<PluginId>,
        done_receiving_permissions: bool,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        if done_receiving_permissions {
            plugin_ids.retain(|id| {
                if !self
                    .loading_plugins
                    .iter()
                    .any(|(loading, _)| loading == id)
                {
                    return true;
                }
                // A permission answer cannot finish initialization for the remaining clients.
                if !self
                    .permission_requests
                    .values()
                    .any(|pending| pending.contains_plugin(*id))
                {
                    self.plugin_ids_waiting_for_permission_request.remove(id);
                }
                false
            });
        }
        let mut applied_plugin_paths = HashSet::new();
        for plugin_id in plugin_ids.iter().copied() {
            // The loader has returned, even if a live instance still needs permission.
            // Keeping it marked as loading would admit pipes after that instance fails.
            if let Some(run_plugin) = self.run_plugin_of_loading_plugin_id(plugin_id) {
                applied_plugin_paths.insert(run_plugin.clone());
                self.refresh_loaded_plugin_location(plugin_id);
            }
            self.loading_plugins
                .retain(|(p_id, _run_plugin)| p_id != &plugin_id);
            if self
                .permission_requests
                .values()
                .any(|pending| pending.contains_plugin(plugin_id))
                || (!done_receiving_permissions
                    && self
                        .plugin_ids_waiting_for_permission_request
                        .contains(&plugin_id)
                    && self
                        .plugin_map
                        .lock()
                        .unwrap()
                        .pipe_instances()
                        .iter()
                        .any(|((id, _, _, _), _)| *id == plugin_id))
            {
                continue;
            }
            self.plugin_ids_waiting_for_permission_request
                .remove(&plugin_id);
            // Each plugin's replay and deferred reload stand alone: one failure must not
            // strand the plugins after it, or end the plugin thread.
            self.apply_cached_events_and_resizes_for_plugin(plugin_id, shutdown_sender.clone())
                .with_context(|| format!("replay cached events for plugin {plugin_id}"))
                .non_fatal();
        }
        for plugin_id in plugin_ids {
            if self.pending_plugin_id_reloads.remove(&plugin_id) {
                self.reload_plugin_with_id(plugin_id)
                    .with_context(|| format!("run deferred reload of plugin {plugin_id}"))
                    .non_fatal();
            }
        }
        for run_plugin in applied_plugin_paths.drain() {
            if self.pending_plugin_reloads.remove(&run_plugin) {
                self.reload_plugin(&run_plugin)
                    .with_context(|| format!("run deferred reload of {}", run_plugin.location))
                    .non_fatal();
            }
        }
        Ok(())
    }
    /// A shared plugin's single instance is keyed in the PluginMap under the client that loaded
    /// it. If that client is gone, hand the instance over to `new_owner` so that it keeps
    /// receiving events, and runs with `new_owner`'s keybinds and base mode.
    fn rekey_shared_plugin_if_owner_gone(&mut self, plugin_id: PluginId, new_owner: ClientId) {
        let (keybinds, default_mode) = self.keybinds_and_mode_of(new_owner);
        let keybinds = keybinds.clone();
        self.plugin_executor.execute_for_plugin_tagged(
            plugin_id,
            "client_rekey",
            false,
            move |senders, plugin_map, connected_clients| {
                let mut plugin_map = plugin_map.lock().unwrap();
                let connected_clients = connected_clients.lock().unwrap().clone();
                let mut rekeyed = false;
                for owner in plugin_map.owner_client_ids_of_plugin_id(plugin_id) {
                    if !connected_clients.contains(&owner) {
                        rekeyed |= plugin_map.rekey_client(plugin_id, owner, new_owner);
                    }
                }
                let Some((running_plugin, subscriptions)) = rekeyed
                    .then(|| plugin_map.get_running_plugin_and_subscriptions(plugin_id, new_owner))
                    .flatten()
                else {
                    return;
                };
                {
                    let mut running_plugin = running_plugin.lock().unwrap();
                    running_plugin.update_keybinds(keybinds.clone());
                    running_plugin.update_default_mode(default_mode);
                }
                if subscriptions
                    .lock()
                    .unwrap()
                    .contains(&EventType::InitialKeybinds)
                {
                    senders
                        .send_to_plugin(PluginInstruction::Update(vec![(
                            Some(plugin_id),
                            Some(new_owner),
                            Event::InitialKeybinds(keybinds.to_keybinds_vec()),
                        )]))
                        .context("failed to send a rekeyed plugin its new owner's keybinds")
                        .non_fatal();
                }
            },
        );
    }
    pub fn plugin_dispose(&self, plugin_id: PluginId, generation: u64) {
        self.plugin_executor.execute_for_plugin_tagged(
            plugin_id,
            "plugin_dispose",
            false,
            move |_, _, _| super::steel::dispose_generation(generation),
        );
    }

    pub fn plugin_palette(
        &self,
        delivery: yinz_bus::command_palette::Delivery,
        shutdown: Sender<()>,
    ) {
        let plugin_id = delivery.owner.plugin_id;
        self.plugin_executor.execute_for_plugin_tagged(
            plugin_id,
            "plugin_palette",
            false,
            move |senders, plugin_map, connected_clients| {
                let _shutdown = shutdown;
                let instances = plugin_map.lock().unwrap().running_plugins();
                for (id, owner, instance) in instances {
                    if id != plugin_id {
                        continue;
                    }
                    let mut plugin = instance.lock().unwrap();
                    if plugin.is_unavailable()
                        || plugin.generation() != Some(delivery.owner.generation)
                        || delivery.active.as_ref().is_some_and(|active| {
                            !active.load(std::sync::atomic::Ordering::Acquire)
                        })
                        || (!plugin.env().plugin_config.is_shared() && owner != delivery.client)
                    {
                        continue;
                    }
                    let is_request = matches!(
                        delivery.message,
                        yinz_types::command_palette::Message::Discover { .. }
                            | yinz_types::command_palette::Message::Execute { .. }
                    );
                    if is_request
                        && (!connected_clients.lock().unwrap().contains(&delivery.client)
                            || matches!(
                                crate::yinz_exports::check_command_permission(
                                    plugin.env(),
                                    &yinz_types::data::PluginCommand::CommandPalette(
                                        yinz_types::command_palette::Command::Unregister {
                                            registration: String::new()
                                        }
                                    )
                                ),
                                (yinz_types::data::PermissionStatus::Denied, _)
                            ))
                    {
                        continue;
                    }
                    let previous =
                        std::mem::replace(&mut plugin.env_mut().client_id, delivery.client);
                    let result = plugin.plugin_palette(&delivery.message);
                    plugin.env_mut().client_id = previous;
                    let clients = if plugin.env().plugin_config.is_shared() {
                        connected_clients.lock().unwrap().clone()
                    } else {
                        vec![owner]
                    };
                    let result = result.and_then(|render| {
                        if render && plugin.has_area() {
                            render_plugin_for_clients(plugin_id, &mut plugin, &clients)
                        } else {
                            Ok(vec![])
                        }
                    });
                    match result {
                        Ok(assets) if !assets.is_empty() => senders
                            .send_to_screen(ScreenInstruction::PluginBytes(assets))
                            .context("rendering palette plugin")
                            .non_fatal(),
                        Ok(_) => {},
                        Err(error) => {
                            plugin.mark_failed();
                            handle_plugin_crash(
                                plugin_id,
                                Some(delivery.owner.generation),
                                Some(plugin.instance_id),
                                format!("{error:#}"),
                                senders.clone(),
                            );
                        },
                    }
                    break;
                }
            },
        );
    }

    pub fn plugin_tick(&self, plugin_id: PluginId, generation: u64, shutdown: Sender<()>) {
        self.plugin_executor.execute_for_plugin_tagged(
            plugin_id,
            "plugin_tick",
            false,
            move |senders, plugin_map, connected_clients| {
                let _shutdown = shutdown;
                if let Some(workers) = plugin_map
                    .lock()
                    .unwrap()
                    .worker_routes(plugin_id, generation)
                {
                    workers.send(generation, crate::background_worker::Message::Tick);
                    return;
                }
                let connected_clients = connected_clients.lock().unwrap().clone();
                let instances = plugin_map.lock().unwrap().running_plugins();
                for (id, client_id, instance) in instances {
                    if id != plugin_id {
                        continue;
                    }
                    let mut plugin = instance.lock().unwrap();
                    if plugin.is_unavailable() || plugin.generation() != Some(generation) {
                        continue;
                    }
                    let clients = if plugin.env().plugin_config.is_shared() {
                        connected_clients.clone()
                    } else {
                        vec![client_id]
                    };
                    let result = plugin.plugin_tick().and_then(|render| {
                        if render && plugin.has_area() {
                            render_plugin_for_clients(plugin_id, &mut plugin, &clients)
                        } else {
                            Ok(vec![])
                        }
                    });
                    if result.is_ok() {
                        report_pipe_changes(&mut plugin, HashMap::new(), None, &senders);
                    }
                    if result.is_err() {
                        plugin.mark_failed();
                    }
                    let instance_id = plugin.instance_id;
                    let disposed = plugin.is_disposed();
                    drop(plugin);
                    match result {
                        Ok(assets) if !assets.is_empty() => {
                            senders
                                .send_to_screen(ScreenInstruction::PluginBytes(assets))
                                .context("failed to deliver asynchronous plugin render")
                                .non_fatal();
                        },
                        Ok(_) => {},
                        Err(_) if disposed => {},
                        Err(error) => handle_plugin_crash(
                            plugin_id,
                            Some(generation),
                            Some(instance_id),
                            format!("{error:#}"),
                            senders.clone(),
                        ),
                    }
                    break;
                }
            },
        );
    }

    fn get_changed_panes_per_client(
        &self,
        new_contents: &HashMap<
            yinz_types::data::ClientId,
            HashMap<yinz_types::data::PaneId, PaneContents>,
        >,
        previous_contents: Option<
            &HashMap<yinz_types::data::ClientId, HashMap<yinz_types::data::PaneId, PaneContents>>,
        >,
    ) -> HashMap<yinz_types::data::ClientId, HashMap<yinz_types::data::PaneId, PaneContents>> {
        let mut result: HashMap<
            yinz_types::data::ClientId,
            HashMap<yinz_types::data::PaneId, PaneContents>,
        > = HashMap::new();

        // First report - return everything grouped by client
        let Some(prev_contents) = previous_contents else {
            for (client_id, panes) in new_contents {
                result.insert(*client_id, panes.clone());
            }
            return result;
        };

        // Compare each client's panes
        for (client_id, new_panes) in new_contents {
            let mut client_panes: HashMap<yinz_types::data::PaneId, PaneContents> = HashMap::new();
            for (pane_id, new_pane_contents) in new_panes {
                let has_changed = prev_contents
                    .get(client_id)
                    .and_then(|prev_panes| prev_panes.get(pane_id))
                    .map(|prev_pane_contents| {
                        prev_pane_contents.viewport != new_pane_contents.viewport
                            || prev_pane_contents.selected_text != new_pane_contents.selected_text
                            || prev_pane_contents.cursor != new_pane_contents.cursor
                    })
                    .unwrap_or(true);
                if has_changed {
                    client_panes.insert(*pane_id, new_pane_contents.clone());
                }
            }
            if !client_panes.is_empty() {
                result.insert(*client_id, client_panes);
            }
        }
        result
    }

    pub fn handle_pane_render_report(
        &mut self,
        pane_render_report: PaneRenderReport,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        let changed_panes_per_client = self.get_changed_panes_per_client(
            &pane_render_report.all_pane_contents,
            self.previous_pane_render_report
                .as_ref()
                .map(|r| &r.all_pane_contents),
        );
        for (client_id, client_panes) in changed_panes_per_client {
            let updates = vec![(None, Some(client_id), Event::PaneRenderReport(client_panes))];
            self.update_plugins(updates, shutdown_sender.clone())?;
        }

        // Deferred clients are absent from this report. Keep their comparison history until
        // they render again or disconnect, replacing the full pane map for clients that rendered.
        self.previous_pane_render_report
            .get_or_insert_with(PaneRenderReport::default)
            .all_pane_contents
            .extend(pane_render_report.all_pane_contents);
        Ok(())
    }

    /// Replay the last broadcast `HostTerminalThemeChanged` to a plugin that
    /// subscribed to it after the broadcast. Nothing is sent while the hue is
    /// still unknown.
    pub fn send_host_terminal_theme_mode_to_plugin(
        &self,
        plugin_id: PluginId,
        client_id: ClientId,
    ) {
        let Some(mode) = self.last_host_terminal_theme_mode else {
            return;
        };
        self.senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                Some(plugin_id),
                Some(client_id),
                Event::HostTerminalThemeChanged(mode),
            )]))
            .context("failed to replay the host terminal theme mode to a late subscriber")
            .non_fatal();
    }

    pub fn send_initial_keybinds_to_plugin(&self, plugin_id: PluginId, client_id: ClientId) {
        // Use the same authoritative input as LoadingContext and reconfigure.
        // Reading a guest's copy here can deadlock its synchronous host request.
        let (keybinds, _) = self.keybinds_and_mode_of(client_id);
        self.send_keybinds_payload_to_plugin(plugin_id, client_id, keybinds.to_keybinds_vec());
    }

    fn send_keybinds_payload_to_plugin(
        &self,
        plugin_id: PluginId,
        client_id: ClientId,
        keybinds: KeybindsVec,
    ) {
        let _ = self.senders.send_to_plugin(PluginInstruction::Update(vec![(
            Some(plugin_id),
            Some(client_id),
            Event::InitialKeybinds(keybinds),
        )]));
    }

    pub fn run_plugin_of_loading_plugin_id(&self, plugin_id: PluginId) -> Option<&RunPlugin> {
        self.loading_plugins
            .iter()
            .find(|(p_id, _run_plugin)| p_id == &plugin_id)
            .map(|(_p_id, run_plugin)| run_plugin)
    }
    pub fn run_plugin_of_plugin_id(&self, plugin_id: PluginId) -> Option<RunPlugin> {
        self.plugin_map
            .lock()
            .unwrap()
            .run_plugin_of_plugin_id(plugin_id)
    }

    fn plugin_configuration(&self, client_id: ClientId) -> PluginConfiguration {
        let (keybinds, default_mode) = self.keybinds_and_mode_of(client_id);
        PluginConfiguration {
            keybinds: keybinds.clone(),
            default_mode,
            default_shell: self.default_shell.clone(),
            layout_dir: self.layout_dir.clone(),
            user_plugin_dir: self.user_plugin_dir.clone(),
        }
    }

    /// Reconfigure `client_id`'s plugins, or with [`ClientId::NONE`] the session's. The keybinds
    /// and base mode also become the session-wide ones, as the screen's `default_mode_info` does:
    /// a plugin whose owner has none of its own runs with those, so its instances are
    /// reconfigured as well.
    pub fn reconfigure(
        &mut self,
        client_id: ClientId,
        keybinds: Option<Keybinds>,
        default_mode: Option<InputMode>,
        default_shell: Option<TerminalAction>,
        layout_dir: Option<PathBuf>,
        user_plugin_dir: Option<PathBuf>,
    ) -> Result<()> {
        if let Some(default_mode) = default_mode {
            self.default_mode = default_mode;
            if client_id != ClientId::NONE {
                self.base_modes.insert(client_id, default_mode);
            }
        }
        if let Some(keybinds) = keybinds.as_ref() {
            self.default_keybinds = keybinds.clone();
            if client_id != ClientId::NONE {
                self.keybinds.insert(client_id, keybinds.clone());
            }
        }
        self.default_shell = default_shell.clone();
        self.layout_dir = layout_dir.clone();
        self.user_plugin_dir = user_plugin_dir.clone();
        let configuration = self.plugin_configuration(client_id);
        let plugins_to_reconfigure: Vec<RunningPluginEntry> = {
            let mut map = self.plugin_map.lock().unwrap();
            // Snapshot and publication share the lock so no initializer can miss both updates.
            let plugins = map
                .running_plugins_and_subscriptions()
                .into_iter()
                .filter(|(_, owner, _, _)| {
                    *owner == client_id || !self.keybinds.contains_key(owner)
                })
                .collect();
            #[cfg(test)]
            if let Some(after_snapshot) = self.after_reconfigure_snapshot.take() {
                after_snapshot();
            }
            map.configuration_version += 1;
            let version = map.configuration_version;
            for (owner, cached) in &mut map.client_configurations {
                if *owner == client_id || !self.keybinds.contains_key(owner) {
                    *cached = (version, self.plugin_configuration(*owner));
                }
            }
            if map.needs_client_configuration(client_id) {
                map.client_configurations
                    .insert(client_id, (version, configuration.clone()));
            }
            plugins
        };
        // Plugins subscribed to InitialKeybinds hear the new ones after reconfiguration, each
        // addressed by its owner: a plugin that is not shared is routed by owner.
        let plugins_subscribed_to_initial_keybinds: Vec<(PluginId, ClientId)> =
            if keybinds.is_some() {
                plugins_to_reconfigure
                    .iter()
                    .filter(|(_, _, _, subs)| {
                        subs.lock().unwrap().contains(&EventType::InitialKeybinds)
                    })
                    .map(|(plugin_id, owner, _, _)| (*plugin_id, *owner))
                    .collect()
            } else {
                vec![]
            };

        for (plugin_id, owner, running_plugin, _) in plugins_to_reconfigure {
            self.plugin_executor
                .execute_for_plugin_tagged(plugin_id, "reconfigure", false, {
                    let configuration = self.plugin_configuration(owner);
                    move |_senders, _plugin_map, _connected_clients| {
                        configuration.apply_to(&mut running_plugin.lock().unwrap());
                    }
                });
        }
        // The payload is built from the reconfigure request rather than read back from each
        // plugin's env: the env is updated on the executor above, asynchronously, so reading it
        // here could still see the old keybinds.
        if let Some(keybinds) = keybinds.as_ref() {
            let keybinds_payload = keybinds.to_keybinds_vec();
            for (plugin_id, owner) in plugins_subscribed_to_initial_keybinds {
                self.send_keybinds_payload_to_plugin(plugin_id, owner, keybinds_payload.clone());
            }
        }
        Ok(())
    }
    fn apply_cached_events_and_resizes_for_plugin(
        &mut self,
        plugin_id: PluginId,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        if let Some(messages) = self.cached_events_for_pending_plugins.remove(&plugin_id) {
            let mut events = vec![];
            for message in messages {
                if let EventOrPipeMessage::PipeMessage(client, message, request, load) = message {
                    self.replay_events(
                        plugin_id,
                        std::mem::take(&mut events),
                        shutdown_sender.clone(),
                    );
                    let mut deliveries = vec![];
                    let admitted =
                        self.admit_pipe(Some(plugin_id), client, message, request, &mut deliveries);
                    if let Some(request) = request {
                        self.pending_pipes.finish_cached(
                            request,
                            plugin_id,
                            load,
                            if admitted {
                                Ok(())
                            } else {
                                Err(format!(
                                    "Plugin {plugin_id} has no recipient for cached pipe delivery"
                                ))
                            },
                        );
                    }
                    self.schedule_pipes(deliveries, shutdown_sender.clone(), None);
                } else {
                    events.push(message);
                }
            }
            self.replay_events(plugin_id, events, shutdown_sender.clone());
            self.finish_pipe_requests();
        }
        // A headless plugin has no pane to size.
        let headless = self.panes.lock().unwrap().surface(plugin_id) == Some(Surface::Headless);
        if let Some((rows, columns)) = self.cached_resizes_for_pending_plugins.remove(&plugin_id)
            && !headless
        {
            self.resize_plugin(plugin_id, columns, rows, shutdown_sender.clone())?;
        }
        self.apply_cached_worker_messages(plugin_id)?;
        Ok(())
    }
    // Cached events retain their subscription and acting-client semantics. Pipe messages
    // between these batches use the same admission path as live delivery.
    fn replay_events(
        &self,
        plugin_id: PluginId,
        events: Vec<EventOrPipeMessage>,
        shutdown: Sender<()>,
    ) {
        if events.is_empty() {
            return;
        }
        let instances = self
            .plugin_map
            .lock()
            .unwrap()
            .running_plugins_and_subscriptions();
        let shared = self.plugin_is_shared(&plugin_id);
        for (id, owner, instance, subscriptions) in instances {
            if id != plugin_id {
                continue;
            }
            let events = events.clone();
            let subs = subscriptions.lock().unwrap().clone();
            let clients = if shared {
                self.connected_clients.lock().unwrap().clone()
            } else {
                vec![owner]
            };
            let guard = shutdown.clone();
            self.plugin_executor.execute_for_plugin_tagged(
                plugin_id,
                "cached_events",
                false,
                move |senders, plugin_map, _| {
                    let _guard = guard;
                    // A cached notification still belongs to its original instance after
                    // unload removes that instance from the map. Leave it for cleanup.
                    if !plugin_map
                        .lock()
                        .unwrap()
                        .running_plugins_and_subscriptions()
                        .iter()
                        .any(|(_, _, current, _)| Arc::ptr_eq(current, &instance))
                    {
                        return;
                    }
                    for message in events {
                        let mut plugin = instance.lock().unwrap();
                        if plugin.is_unavailable() {
                            break;
                        }
                        if let EventOrPipeMessage::PluginEvent(client, generation, event, reply) =
                            &message
                            && let Some(workers) = &plugin.env().workers
                            && workers.contains(*generation)
                        {
                            workers.send(
                                *generation,
                                crate::background_worker::Message::Event(
                                    *client,
                                    event.clone(),
                                    *reply,
                                ),
                            );
                            continue;
                        }
                        let Some(client) = message.client_for_replay(
                            plugin.env().owner_client_id,
                            plugin.generation(),
                            shared,
                        ) else {
                            continue;
                        };
                        let event = match message {
                            EventOrPipeMessage::Event(_, event) => event,
                            EventOrPipeMessage::PluginEvent(_, _, event, reply) => {
                                if reply.is_some_and(|reply| reply.claim().is_none()) {
                                    continue;
                                }
                                event
                            },
                            EventOrPipeMessage::PipeMessage(..) => continue,
                        };
                        if !subs.contains(&EventType::from(&event))
                            && !(matches!(event, Event::PermissionRequestResult(_))
                                && plugin.generation().is_some())
                        {
                            continue;
                        }
                        let mut assets = vec![];
                        let result = apply_events_to_plugin(
                            plugin_id,
                            &clients,
                            &mut plugin,
                            &[(client, event)],
                            &mut assets,
                            senders.clone(),
                            &subs,
                        );
                        let generation = plugin.env().generation;
                        let instance_id = plugin.instance_id;
                        drop(plugin);
                        match result {
                            Ok(()) if !assets.is_empty() => {
                                senders
                                    .send_to_screen(ScreenInstruction::PluginBytes(assets))
                                    .context("failed to render cached plugin events")
                                    .non_fatal();
                            },
                            Ok(()) => {},
                            Err(error) => handle_plugin_crash(
                                plugin_id,
                                generation,
                                Some(instance_id),
                                format!("{error:#}"),
                                senders.clone(),
                            ),
                        }
                    }
                },
            );
        }
    }

    pub fn apply_cached_worker_messages(&mut self, plugin_id: PluginId) -> Result<()> {
        if let Some(mut messages) = self.cached_worker_messages.remove(&plugin_id) {
            let mut worker_messages: HashMap<(ClientId, String), Vec<(String, String)>> =
                HashMap::new();
            for (client_id, worker_name, message, payload) in messages.drain(..) {
                worker_messages
                    .entry((client_id, worker_name))
                    .or_default()
                    .push((message, payload));
            }
            for ((client_id, worker_name), messages) in worker_messages.drain() {
                self.post_messages_to_plugin_worker(plugin_id, client_id, worker_name, messages)?;
            }
        }
        Ok(())
    }
    fn plugin_id_of_loading_plugin(
        &self,
        plugin_location: &RunPluginLocation,
        plugin_configuration: &PluginUserConfiguration,
    ) -> Option<PluginId> {
        let location = self
            .normalize_plugin(RunPlugin {
                location: plugin_location.clone(),
                ..Default::default()
            })
            .location;
        let wanted = plugin_configuration.without_caller_context();
        self.loading_plugins
            .iter()
            .find_map(|(plugin_id, run_plugin)| {
                if self.normalize_plugin(run_plugin.clone()).location == location
                    && run_plugin.configuration.without_caller_context() == wanted
                {
                    Some(*plugin_id)
                } else {
                    None
                }
            })
    }
    /// Whether an instance of `run_plugin` is already running or on its way up.
    ///
    /// Retry metadata also counts here: a failed hook keeps its pane until explicit reload.
    pub(crate) fn plugin_instance_exists(&self, run_plugin: &RunPlugin) -> bool {
        if self
            .plugin_id_of_loading_plugin(&run_plugin.location, &run_plugin.configuration)
            .is_some()
        {
            return true;
        }
        self.all_plugin_ids_for_plugin_location(&run_plugin.location, &run_plugin.configuration)
            .map(|plugin_ids| !plugin_ids.is_empty())
            .unwrap_or(false)
    }

    /// Match the launch's pane and hook point, never the changing exit payload or configuration.
    pub(crate) fn pane_hook_instance(
        &mut self,
        point: yinz_types::input::hooks::HookPoint,
        pane_id: PaneId,
    ) -> Option<PluginId> {
        let plugin_id = *self.pane_hooks.get(&(point, pane_id))?;
        if self.loading_plugins.iter().any(|(id, _)| *id == plugin_id)
            || self
                .plugin_map
                .lock()
                .unwrap()
                .has_available_instance(plugin_id)
        {
            Some(plugin_id)
        } else {
            self.pane_hooks.remove(&(point, pane_id));
            None
        }
    }
    #[cfg(test)]
    pub(crate) fn get_plugin_ids_for_url(&self, url: &str) -> Result<Vec<PluginId>, String> {
        self.get_plugin_ids_for_url_with_configuration(url, None)
    }

    pub(crate) fn get_plugin_ids_for_url_with_configuration(
        &self,
        url: &str,
        configuration: Option<BTreeMap<String, String>>,
    ) -> Result<Vec<PluginId>, String> {
        let aliases = self.plugin_aliases.lock().map_err(|e| e.to_string())?;
        let plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            url,
            &configuration,
            Some(&aliases),
            None,
        )?
        .get_run_plugin()
        .ok_or_else(|| format!("{url} is not a plugin or a known alias"))?;
        drop(aliases);
        match self.all_plugin_ids_for_plugin_location(&plugin.location, &plugin.configuration) {
            Ok(ids) => Ok(ids),
            Err(error)
                if matches!(
                    error.downcast_ref::<YinzError>(),
                    Some(YinzError::PluginDoesNotExist)
                ) =>
            {
                Ok(vec![])
            },
            Err(error) => Err(format!("{error:#}")),
        }
    }

    fn all_plugin_ids_for_plugin_location(
        &self,
        plugin_location: &RunPluginLocation,
        plugin_configuration: &PluginUserConfiguration,
    ) -> Result<Vec<PluginId>> {
        self.plugin_map
            .lock()
            .unwrap()
            .all_plugin_ids_for_plugin_location(plugin_location, plugin_configuration, |location| {
                self.normalize_plugin(RunPlugin {
                    location: location.clone(),
                    ..Default::default()
                })
                .location
            })
    }
    pub(crate) fn cli_data(&self, request: yinz_types::cli::DataRequest) -> anyhow::Result<String> {
        crate::cli_data::run(
            self.data_access.plugin_data(),
            request,
            |namespace, entities| {
                self.notify_data_watchers(namespace, entities);
            },
        )
    }

    pub(crate) fn notify_data_watchers(
        &self,
        namespace: &str,
        entities: &std::collections::BTreeSet<String>,
    ) {
        crate::yinz_exports::notify_data_watchers(
            self.data_access.plugin_data(),
            &self.watch_generations,
            &self.senders,
            namespace,
            entities,
        );
    }

    pub(crate) fn start_data_watch_polling(
        &self,
    ) -> Result<yinz_storage::plugin_data_store::DataWatchPoller> {
        let generations = self.watch_generations.clone();
        let senders = self.senders.clone();
        self.data_access
            .plugin_data()
            .start_watch_polling(move |store| {
                crate::yinz_exports::poll_data_watchers(store, &generations, &senders);
            })
            .context("start cross-process data watches")
    }

    pub fn builtin_service_instance(&self, name: &str) -> Result<(PluginId, ClientId), String> {
        let instances: std::collections::BTreeMap<_, _> = self
            .plugin_map
            .lock()
            .unwrap()
            .clone_plugin_assets()
            .get(&RunPluginLocation::Yinz(yinz_types::data::PluginTag::new(
                name,
            )))
            .into_iter()
            .flat_map(|configurations| configurations.values())
            .flatten()
            .copied()
            .collect();
        match instances.len() {
            0 => Err(format!("No running {name} instance; load it as a background plugin before requesting this operation")),
            1 => instances.into_iter().next().ok_or_else(|| "service disappeared".into()),
            _ => Err(format!("Multiple {name} instances: {:?}; keep one background instance before requesting this operation", instances.keys().collect::<Vec<_>>())),
        }
    }

    pub fn all_plugin_and_client_ids_for_plugin_location(
        &mut self,
        plugin_location: &RunPluginLocation,
        plugin_configuration: &PluginUserConfiguration,
    ) -> Vec<(PluginId, Option<ClientId>)> {
        let normalized = self
            .normalize_plugin(RunPlugin {
                location: plugin_location.clone(),
                ..Default::default()
            })
            .location;
        let plugin_location = &normalized;
        let identities = self.plugin_map.lock().unwrap().clone_plugin_assets();
        let destinations: Vec<_> = identities
            .iter()
            .filter(|(location, _)| {
                self.normalize_plugin(RunPlugin {
                    location: (*location).clone(),
                    ..Default::default()
                })
                .location
                    == *plugin_location
            })
            .flat_map(|(_, configurations)| instances_of(configurations, plugin_configuration))
            .map(|(plugin, client)| (*plugin, Some(*client)))
            .collect();
        destinations
    }
    /// The plugins at `plugin_location` with `plugin_configuration` whose load is still in
    /// flight, so not yet in the plugin map. An event addressed to one of these is held in its
    /// cache and applied when the load completes - which makes a loading plugin an instance to
    /// address, not a reason to load another.
    pub fn loading_plugin_ids_for_plugin_location(
        &self,
        plugin_location: &RunPluginLocation,
        plugin_configuration: &PluginUserConfiguration,
    ) -> Vec<PluginId> {
        let normalized = self
            .normalize_plugin(RunPlugin {
                location: plugin_location.clone(),
                ..Default::default()
            })
            .location;
        let plugin_location = &normalized;
        let wanted = plugin_configuration.without_caller_context();
        self.loading_plugins
            .iter()
            .filter(|(_, run_plugin)| {
                &self.normalize_plugin(run_plugin.clone()).location == plugin_location
                    && run_plugin.configuration.without_caller_context() == wanted
            })
            .map(|(plugin_id, _)| *plugin_id)
            .collect()
    }
    #[cfg(test)]
    pub fn all_plugin_ids(&self) -> Vec<(PluginId, ClientId)> {
        self.plugin_map.lock().unwrap().all_plugin_ids()
    }
    /// Whether this plugin runs as one instance for the whole session rather than one per client.
    /// See `PluginConfig::is_shared`; it decides both who a message reaches and who it renders to.
    pub fn plugin_is_shared(&self, plugin_id: &PluginId) -> bool {
        self.shared_plugin_ids.contains(plugin_id)
    }
    /// Hand the screen the complete background subscriptions `instance` reported, unless the
    /// instance has been retired since. A reload retires the old instance at once, but a handler
    /// it was running can still subscribe: that registration would put back the registry entry
    /// the screen dropped when the replacement declared a pane, and route the plugin's events
    /// twice.
    pub(crate) fn forward_background_subscriptions(
        &self,
        plugin_id: PluginId,
        client_id: ClientId,
        subscriptions: HashSet<EventType>,
        instance: yinz_bus::plugins::PluginInstanceId,
    ) {
        let retired = self
            .plugin_map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .instance_is_retired(instance);
        if retired {
            log::debug!("dropped background subscriptions of a retired instance of {plugin_id}");
            return;
        }
        self.senders
            .send_to_screen(ScreenInstruction::UpdateBackgroundPluginSubscriptions(
                plugin_id,
                client_id,
                subscriptions,
            ))
            .context("forward background plugin subscriptions")
            .non_fatal();
    }
    fn size_of_plugin_id(&self, plugin_id: PluginId) -> Option<(usize, usize)> {
        self.plugin_map
            .lock()
            .unwrap()
            .reload_metadata(plugin_id)
            .map(|metadata| metadata.size)
    }
    fn cwd_of_plugin_id(&self, plugin_id: PluginId) -> Option<PathBuf> {
        self.plugin_map
            .lock()
            .unwrap()
            .reload_metadata(plugin_id)
            .map(|metadata| metadata.cwd)
    }
    fn plugin_config_of_plugin_id(&self, plugin_id: PluginId) -> Option<PluginConfig> {
        self.plugin_map
            .lock()
            .unwrap()
            .plugin_config_of_plugin_id(plugin_id)
    }
    fn tab_index_of_plugin_id(&self, plugin_id: PluginId) -> Option<TabId> {
        self.plugin_map
            .lock()
            .unwrap()
            .reload_metadata(plugin_id)
            .and_then(|metadata| metadata.tab_index)
    }
    fn start_plugin_loading_indication(
        &self,
        plugin_ids: &[PluginId],
        loading_indication: &LoadingIndication,
    ) {
        for plugin_id in plugin_ids {
            let _ = self
                .senders
                .send_to_screen(ScreenInstruction::StartPluginLoadingIndication(
                    *plugin_id,
                    loading_indication.clone(),
                ));
            let _ = self
                .senders
                .send_to_background_jobs(BackgroundJob::AnimatePluginLoading(*plugin_id));
        }
    }
    pub fn post_messages_to_plugin_worker(
        &mut self,
        plugin_id: PluginId,
        client_id: ClientId,
        worker_name: String,
        mut messages: Vec<(String, String)>,
    ) -> Result<()> {
        let worker = self
            .plugin_map
            .lock()
            .unwrap()
            .get_running_plugin(plugin_id, Some(client_id))
            .and_then(|plugin| plugin.lock().unwrap().env().workers.clone());
        match worker {
            Some(worker) => {
                for (message, payload) in messages.drain(..) {
                    if let Err(e) = worker.post(&worker_name, client_id, message, payload) {
                        log::error!("Failed to send message to worker: {:?}", e);
                    }
                }
            },
            None => {
                log::warn!("Worker {worker_name} not found, caching messages");
                for (message, payload) in messages.drain(..) {
                    self.cached_worker_messages
                        .entry(plugin_id)
                        .or_default()
                        .push((client_id, worker_name.clone(), message, payload));
                }
            },
        }
        Ok(())
    }
    /// Grant `permissions` to the plugin at `plugin_url`, on top of whatever it already has.
    ///
    /// The cache is keyed the way `answer_permissions` keys it - by the location's string
    /// form - so the plugin's own later `request_permission` finds the grant and never prompts.
    /// An instance already running is told too, with the same event a prompt's answer sends,
    /// so it does not have to be reloaded to use what it was given.
    pub fn grant_permissions_to_location(
        &mut self,
        plugin_url: &str,
        permissions: Vec<PermissionType>,
        shutdown_sender: Sender<()>,
    ) -> Result<()> {
        let err_context = || format!("Failed to grant permissions to {plugin_url}");
        let location = yinz_runtime::plugin_location::parse_plugin_location(plugin_url, None)
            .map_err(|e| anyhow!("{}", e))
            .with_context(err_context)?;
        let location = self
            .normalize_plugin(RunPlugin {
                location,
                ..Default::default()
            })
            .location;
        let key = location.to_string();

        let mut permission_cache = PermissionCache::from_path_or_default(None);
        permission_cache.grant(key.clone(), permissions);
        permission_cache.write_to_file().with_context(err_context)?;
        let granted = permission_cache
            .get_permissions(key)
            .cloned()
            .unwrap_or_default();

        let running: Vec<PluginId> = self
            .plugin_map
            .lock()
            .unwrap()
            .running_plugins()
            .into_iter()
            .filter_map(|(plugin_id, _client_id, running_plugin)| {
                let mut running_plugin = running_plugin.lock().unwrap();
                let env = running_plugin.env();
                let loaded_location = match (&env.plugin_config.location, &env.resolved_source_path)
                {
                    (RunPluginLocation::File(_), Some(path)) => {
                        RunPluginLocation::File(path.clone())
                    },
                    (location, _) => {
                        self.normalize_plugin(RunPlugin {
                            location: location.clone(),
                            ..Default::default()
                        })
                        .location
                    },
                };
                if loaded_location != location {
                    return None;
                }
                running_plugin
                    .env_mut()
                    .set_permissions(HashSet::from_iter(granted.clone()));
                Some(plugin_id)
            })
            .collect();
        let tokens: Vec<_> = self
            .permission_requests
            .iter()
            .filter(|(_, pending)| {
                running.iter().any(|id| pending.contains_plugin(*id))
                    && pending
                        .permissions
                        .iter()
                        .all(|permission| granted.contains(permission))
            })
            .map(|(token, _)| *token)
            .collect();
        for token in tokens {
            self.answer_permissions(
                token,
                PermissionStatus::Granted,
                None,
                shutdown_sender.clone(),
            )?;
        }
        Ok(())
    }
    pub fn cache_plugin_events(&mut self, plugin_id: PluginId) {
        self.plugin_ids_waiting_for_permission_request
            .insert(plugin_id);
        self.cached_events_for_pending_plugins
            .entry(plugin_id)
            .or_default();
    }

    // Finds running or loading recipients, starting one if missing and allow_load is true.
    pub fn get_or_load_plugins(
        &mut self,
        run_plugin_or_alias: RunPluginOrAlias,
        size: Size,
        cwd: Option<PathBuf>,
        skip_cache: bool,
        should_float: bool,
        should_be_open_in_place: bool,
        pane_title: Option<String>,
        pane_id_to_replace: Option<PaneId>,
        cli_client_id: Option<ClientId>,
        floating_pane_coordinates: Option<FloatingPaneCoordinates>,
        should_focus: bool,
        allow_load: bool,
    ) -> Result<Vec<(PluginId, Option<ClientId>)>> {
        let run_plugin = run_plugin_or_alias.get_run_plugin();
        match run_plugin {
            Some(run_plugin) => {
                let all_plugin_ids = self.all_plugin_and_client_ids_for_plugin_location(
                    &run_plugin.location,
                    &run_plugin.configuration,
                );
                if all_plugin_ids.is_empty() {
                    if let Some(loading_plugin_id) = self.plugin_id_of_loading_plugin(
                        &run_plugin.location,
                        &run_plugin.configuration,
                    ) {
                        return Ok(vec![(loading_plugin_id, None)]);
                    }
                    if !allow_load {
                        return Ok(vec![]);
                    }
                    // A recipient that declares a pane floats as asked; one that does not
                    // receives the message headless.
                    let pane = PaneRequest::Show(Box::new(crate::plugin_panes::ShowPane {
                        should_float: Some(should_float),
                        in_place: should_be_open_in_place,
                        replaced: yinz_bus::pty::ReplacedPane::Suppress,
                        run: run_plugin_or_alias,
                        title: pane_title,
                        tab: None,
                        pane_id_to_replace,
                        cwd: cwd.clone(),
                        coordinates: floating_pane_coordinates,
                        should_focus: Some(should_focus),
                        // Replaced by the client the load resolves to.
                        client_id: Some(ClientId::NONE),
                        completion: None,
                        overlay: None,
                        reserved_slot: None,
                    }));
                    match self.load_plugin(
                        &Some(run_plugin),
                        None,
                        size,
                        cwd,
                        skip_cache,
                        cli_client_id,
                        pane,
                    ) {
                        Ok((plugin_id, client_id)) => Ok(vec![(plugin_id, Some(client_id))]),
                        Err(error) => Err(error).context("Failed to load plugin"),
                    }
                } else {
                    Ok(all_plugin_ids)
                }
            },
            None => Err(anyhow!(
                "Plugin not found for alias: {}",
                run_plugin_or_alias.location_string()
            )),
        }
    }
    pub fn pipe_has_recipient(&self, plugin: PluginId) -> bool {
        self.pending_pipes.has_recipient(plugin)
    }

    pub fn finish_empty_pipe(&mut self, stream: &str, result: Result<(), String>) {
        let request = self.pending_pipes.begin(stream);
        if let Err(error) = result {
            self.pending_pipes.fail(request, error);
        }
        self.finish_pipe_requests();
    }

    fn finish_pipe_requests(&mut self) {
        let outcomes = self.pending_pipes.take_ready();
        self.send_pipe_outcomes(outcomes);
    }

    pub(crate) fn send_pipe_outcomes(&self, outcomes: PipeOutcomes) {
        for (stream, result) in outcomes {
            let instruction = match result {
                Ok(()) => ServerInstruction::UnblockCliPipeInput(stream),
                Err(error) => ServerInstruction::CliPipeError(stream, error),
            };
            self.senders
                .send_to_server(instruction)
                .context("failed to report pipe outcome")
                .non_fatal();
        }
    }

    pub fn release_plugin_instance_pipes(
        &mut self,
        plugin: PluginId,
        instance: PluginInstanceId,
    ) -> PipeOutcomes {
        {
            let mut map = self.plugin_map.lock().unwrap();
            for client in map.owner_client_ids_of_plugin_id(plugin) {
                map.prune_client_configuration(client);
            }
        }
        self.pending_pipes.retire(
            plugin,
            instance,
            "Plugin instance retired before completing its pipe",
        );
        self.pending_pipes.take_ready()
    }

    pub fn update_cli_pipe_state(&mut self, completions: Vec<PipeCompletion>) -> PipeOutcomes {
        for completion in completions {
            self.pending_pipes.complete(completion);
        }
        self.pending_pipes.take_ready()
    }
    fn message_is_directed_at_plugin(
        message_pid: Option<PluginId>,
        message_cid: Option<ClientId>,
        plugin_id: &PluginId,
        client_id: &ClientId,
        plugin_is_shared: bool,
    ) -> bool {
        // a shared plugin has a single instance for the whole session, so messages directed at any
        // client reach it (the message's client becomes the acting client)
        let client_matches = plugin_is_shared || message_cid == Some(*client_id);
        message_pid.is_none() && message_cid.is_none()
            || (message_pid.is_none() && client_matches)
            || (message_cid.is_none() && message_pid == Some(*plugin_id))
            || (client_matches && message_pid == Some(*plugin_id))
    }
    /// The keybinds and base mode of a plugin owned by `client_id`: the client's own while it
    /// has them, otherwise the session-wide ones, which every reconfigure moves. A departed
    /// client, a CLI connection in a headless session and [`ClientId::NONE`] have none.
    fn keybinds_and_mode_of(&self, client_id: ClientId) -> (&Keybinds, InputMode) {
        (
            self.keybinds
                .get(&client_id)
                .unwrap_or(&self.default_keybinds),
            self.base_modes
                .get(&client_id)
                .copied()
                .unwrap_or(self.default_mode),
        )
    }
    pub fn get_first_client_id(&self) -> Option<ClientId> {
        self.connected_clients
            .lock()
            .unwrap()
            .iter()
            .next()
            .copied()
    }
    pub fn update_available_layouts(
        &mut self,
        layouts: Vec<LayoutInfo>,
        errors: Vec<LayoutWithError>,
    ) {
        // Diff with existing layouts
        if self.available_layouts != layouts || self.available_layout_errors != errors {
            // Update the stored layouts
            self.available_layouts = layouts.clone();
            self.available_layout_errors = errors.clone();

            // Notify all plugins of the change
            let _ = self.senders.send_to_plugin(PluginInstruction::Update(vec![(
                None, // Broadcast to all plugins
                None, // Broadcast to all clients
                Event::AvailableLayoutInfo(layouts, errors),
            )]));
        }
    }
    pub fn state_update_for_plugin(&self, plugin_id: PluginId) {
        let _ = self.senders.send_to_plugin(PluginInstruction::Update(vec![(
            Some(plugin_id),
            None,
            Event::AvailableLayoutInfo(
                self.available_layouts.clone(),
                self.available_layout_errors.clone(),
            ),
        )]));
    }
    pub fn detect_and_notify_plugin_config_changes(
        &mut self,
        new_plugins: &PluginAliases,
        shutdown_send: Sender<()>,
    ) -> Result<()> {
        let err_context = || "failed to detect plugin config changes";

        // Before notifying anyone: this is the only place the thread is handed a fresh
        // alias table, and `GetPluginAliasConfiguration` answers out of it. Without this
        // a plugin asking after a config reload would be told what was on disk when the
        // server started.
        *self.plugin_aliases.lock().unwrap() = new_plugins.clone();

        // Get all running plugins
        let running_plugins = self.plugin_map.lock().unwrap().running_plugins();

        for (plugin_id, client_id, running_plugin) in running_plugins {
            let mut running_plugin = running_plugin.lock().unwrap();
            let plugin_env = &running_plugin.env();
            let plugin_location = &plugin_env.plugin_config.location;

            // Look up this plugin in the new config by location
            // Note: PluginAliases is HashMap<String, RunPlugin>, so we need to iterate
            let new_config_for_location = new_plugins
                .aliases
                .values()
                .find(|run_plugin| {
                    self.normalize_plugin((*run_plugin).clone()).location
                        == self
                            .normalize_plugin(RunPlugin {
                                location: plugin_location.clone(),
                                ..Default::default()
                            })
                            .location
                })
                .map(|run_plugin| &run_plugin.configuration);

            if let Some(new_config) = new_config_for_location {
                // Compare configurations - only fire event if changed
                if &running_plugin.current_configuration != new_config {
                    running_plugin.current_configuration = new_config.clone();
                    drop(running_plugin); // Release lock before sending

                    let event = Event::PluginConfigurationChanged(new_config.inner().clone());
                    let updates = vec![(Some(plugin_id), Some(client_id), event)];
                    self.update_plugins(updates, shutdown_send.clone())
                        .with_context(err_context)?;
                }
            }
        }

        Ok(())
    }
}

/// A short label for the executor's stats log — the batch's event kinds with
/// counts, e.g. "Key" or "PaneUpdate×2,TabUpdate" — and whether any event in it
/// is user input (a focused or intercepted key, a mouse action, a paste), which
/// is what marks the job urgent for the executor's accounting.
fn summarize_batch_for_stats<'a>(events: impl IntoIterator<Item = &'a Event>) -> (String, bool) {
    let mut counts: Vec<(EventType, usize)> = Vec::new();
    let mut has_input = false;
    for event in events {
        let event_type = EventType::from(event);
        has_input |= matches!(
            event_type,
            EventType::Key
                | EventType::Mouse
                | EventType::PastedText
                | EventType::InterceptedKeyPress
        );
        match counts.iter_mut().find(|(kind, _)| *kind == event_type) {
            Some((_, count)) => *count += 1,
            None => counts.push((event_type, 1)),
        }
    }
    let label = counts
        .iter()
        .map(|(kind, count)| match count {
            1 => kind.to_string(),
            _ => format!("{kind}×{count}"),
        })
        .collect::<Vec<_>>()
        .join(",");
    (label, has_input)
}

pub(crate) fn check_event_permission(
    plugin_env: &PluginEnv,
    event: &Event,
) -> (PermissionStatus, Option<PermissionType>) {
    if plugin_env.plugin_config.is_builtin() {
        // built-in plugins can do all the things because they're part of the application and
        // there's no use to deny them anything
        return (PermissionStatus::Granted, None);
    }
    // Matched on the discriminant rather than on `Event`, which is `#[non_exhaustive]` and so
    // could only ever be matched with a wildcard from this crate. On `EventType` the match is
    // exhaustive: a new event that is not classified here is a compile error rather than an
    // event delivered to every plugin regardless of what it has been granted.
    let permission = match EventType::from(event) {
        EventType::ModeUpdate
        | EventType::TabUpdate
        | EventType::PaneTabsUpdate
        | EventType::PaneUpdate
        | EventType::SessionUpdate
        | EventType::CopyToClipboard
        | EventType::SystemClipboardFailure
        | EventType::CommandPaneOpened
        | EventType::CommandPaneExited
        | EventType::PaneClosed
        | EventType::EditPaneOpened
        | EventType::EditPaneExited
        | EventType::FailedToWriteConfigToDisk
        | EventType::CommandPaneReRun
        | EventType::CwdChanged
        | EventType::CommandChanged
        | EventType::AvailableLayoutInfo
        | EventType::PluginConfigurationChanged
        | EventType::HighlightClicked
        | EventType::HintText
        | EventType::ActivePaneScroll
        | EventType::InputReceived
        // Which modifier keys are down says something about what the person at the keyboard is
        // doing, which is more than `InputReceived`'s bare "something happened" - so it sits
        // with the rest of the session state rather than with the ungated group below.
        | EventType::ModifiersHeld => PermissionType::ReadApplicationState,
        EventType::PluginDataChanged => PermissionType::PluginData,
        // What an agent said and was asked is session state, and more of it than the manifest
        // carries: a plugin that may read the panes may read this.
        EventType::AgentEvent => PermissionType::ReadApplicationState,
        EventType::PickResult => PermissionType::MessageAndLaunchOtherPlugins,
        EventType::AgentProvidersChanged | EventType::ProjectProvidersChanged => PermissionType::RunCommands,
        EventType::NotificationReceived | EventType::NotificationResolved => PermissionType::ReadApplicationState,
        EventType::PaneOutput | EventType::PaneRenderReport => PermissionType::ReadPaneContents,
        EventType::UserAction => PermissionType::InterceptInput,
        // Ungated on purpose. These are either addressed to this plugin alone (its own input,
        // timers, lifecycle, worker messages and the replies to commands it was already
        // permitted to issue) or carry nothing about the session a plugin could not learn by
        // being loaded into it.
        EventType::Key
        | EventType::Mouse
        | EventType::PastedText
        | EventType::InterceptedKeyPress
        | EventType::Timer
        | EventType::Visible
        | EventType::CellPixelSizeChanged
        | EventType::BeforeClose
        | EventType::CustomMessage
        | EventType::PermissionRequestResult
        | EventType::InitialKeybinds
        | EventType::HostTerminalThemeChanged
        | EventType::ActionComplete
        | EventType::RunCommandOutput
        | EventType::RunCommandResult
        | EventType::WebRequestResult
        | EventType::WebRequestStreamChunk
        | EventType::ListClients
        | EventType::HostFolderChanged
        | EventType::FailedToChangeHostFolder
        | EventType::ConfigWasWrittenToDisk
        | EventType::NotificationRequested
        | EventType::ContextMenuRequested
        | EventType::ContextMenuItemSelected
        // Addressed only to the bound hook (including pane re-entry); the binding is consent.
        | EventType::Hook
        // Paths and metadata under the host folder, not contents; see `ScanHostFolder` in
        // `yinz_exports::command_permission` for why these stay open for now.
        | EventType::FileSystemCreate
        | EventType::FileSystemRead
        | EventType::FileSystemUpdate
        | EventType::FileSystemDelete => return (PermissionStatus::Granted, None),
    };

    if let Some(permissions) = plugin_env.permissions.lock().unwrap().as_ref()
        && permissions.contains(&permission)
    {
        return (PermissionStatus::Granted, None);
    }

    (PermissionStatus::Denied, Some(permission))
}

/// Call the plugin's `render` once per client, yielding one render asset per client. For a
/// non-shared plugin `clients` is just the client owning this instance. For a shared plugin it's
/// every connected client - the plugin's acting client id is set before each call so that it may
/// vary its output per client.
///
/// ponytail: N guest render calls for a shared plugin. If a plugin's output never depends on the
/// client, render once and clone the bytes instead.
pub fn render_plugin_for_clients(
    plugin_id: PluginId,
    running_plugin: &mut RunningPlugin,
    clients: &[ClientId],
) -> Result<Vec<PluginRenderAsset>> {
    // A headless plugin has no pane to render into.
    if running_plugin.is_disposed() || running_plugin.env().surface == Surface::Headless {
        return Ok(vec![]);
    }
    let err_context = || format!("Failed to render plugin {plugin_id}");
    // The newest size the screen asked for, not the last one a resize job applied: a resize
    // can still be queued behind this render, or have been skipped for a newer one.
    let (rows, columns) = running_plugin.take_size_for_render();
    let owner_client_id = running_plugin.env().owner_client_id;
    // Guest rendering still runs while detached: it may issue host commands.
    let mut clients_to_render = vec![owner_client_id];
    clients_to_render.extend(clients.iter().filter(|c| **c != owner_client_id));

    let mut plugin_render_assets = vec![];
    for client_id in clients_to_render {
        running_plugin.env_mut().client_id = client_id;
        let rendered_bytes = running_plugin
            .render(rows, columns)
            .with_context(err_context)?;
        plugin_render_assets.push(PluginRenderAsset::new(plugin_id, client_id, rendered_bytes));
    }
    Ok(plugin_render_assets)
}

/// Deliver a batch of events to one plugin instance, in order, then render it once if any of
/// them asked for it. Each event carries the client it acts for, which for a shared plugin
/// differs from the instance's owner.
///
/// Rendering once per batch rather than once per event is what keeps a flood of wheel notches
/// from costing a guest `render()` per client per notch; the plugin still sees every event. A
/// plugin that errors mid-batch is not handed the rest of it, but the CLI pipe changes the
/// events before the error asked for are still delivered: a `yinz pipe` client one of them
/// released must not stay blocked on a later event's failure.
pub fn apply_events_to_plugin(
    plugin_id: PluginId,
    render_clients: &[ClientId],
    running_plugin: &mut RunningPlugin,
    events: &[(ClientId, Event)],
    plugin_render_assets: &mut Vec<PluginRenderAsset>,
    senders: ThreadSenders,
    plugin_subscriptions: &HashSet<EventType>,
) -> Result<()> {
    if running_plugin.is_unavailable() {
        return Ok(());
    }
    let err_context = || format!("Failed to apply event to plugin {plugin_id}");

    let mut should_render = false;
    let mut handled_any = false;
    let mut failure: Option<anyhow::Error> = None;
    // The pipe changes each event asked for, read off the plugin as soon as that event returns.
    // One drain at the end of the batch would merge them with unblock winning over block
    // regardless of order (see `pipes_to_block_or_unblock`); drained per event, the last change
    // to a pipe wins, as it did when every event was its own job.
    let mut pipe_changes: HashMap<String, PipeStateChange> = HashMap::new();
    for (client_id, event) in events {
        // host commands issued while handling this event act on behalf of the client it was
        // directed at (this only differs from the instance's owner for shared plugins)
        running_plugin.env_mut().client_id = *client_id;
        match check_event_permission(running_plugin.env(), event) {
            (PermissionStatus::Granted, _) => {
                let mut event = event.clone();
                // Registrations can change on different executor threads after a broadcast
                // queues. Mirror the current registry, never an older queued snapshot.
                if let Event::ProjectProvidersChanged(providers) = &mut event {
                    *providers = yinz_types::project_provider::providers();
                }
                if let Event::AgentProvidersChanged(providers) = &mut event {
                    *providers = yinz_types::agent::provider::providers();
                }
                if let Event::ModeUpdate(mode_info) = &mut event {
                    // A ModeUpdate that already names its base mode (Screen::change_mode sets it
                    // when a keybinding preset switches) must keep it: the plugin's env is updated
                    // asynchronously on reconfigure and may still hold the previous default mode.
                    if mode_info.base_mode.is_none() {
                        mode_info.base_mode = Some(running_plugin.env().default_mode);
                    }
                    if plugin_subscriptions.contains(&EventType::InitialKeybinds) {
                        // Plugin caches keybindings via InitialKeybinds — send lightweight ModeUpdate
                        mode_info.keybinds = vec![];
                    } else {
                        // Legacy plugin — send full keybindings as before
                        mode_info.keybinds = running_plugin.env().keybinds.to_keybinds_vec();
                    }
                }
                if let Event::NotificationRequested(notification) = &event {
                    senders
                        .send_to_screen(ScreenInstruction::BindNotificationRequest {
                            id: notification.id,
                            plugin_id,
                            instance_id: running_plugin.env().instance_id,
                        })
                        .context("bind notification policy request")
                        .non_fatal();
                }
                handled_any = true;
                let updated = running_plugin.update(&event).with_context(err_context);
                let changes = pipes_to_block_or_unblock(running_plugin);
                match updated {
                    Ok(rendered) => {
                        should_render |= rendered;
                        pipe_changes.extend(changes);
                    },
                    Err(e) => {
                        failure = Some(e);
                        break;
                    },
                }
                if let Event::PermissionRequestResult(..) = event {
                    // we always render in this case, otherwise the request permission screen stays on
                    // screen
                    should_render = true;
                }
            },
            (PermissionStatus::Denied, permission) => {
                // Once per plugin per event type: a plugin subscribed to something it may not
                // read is a standing misconfiguration, and an event as frequent as
                // `ModifiersHeld` would otherwise fill the log with the same line.
                let event_type = EventType::from(event);
                if running_plugin.note_denied_event(event_type) {
                    log::error!(
                        "PluginId '{}' permission '{}' is not allowed - Event '{:?}' denied \
                         (further denials of this event are not logged)",
                        plugin_id,
                        permission
                            .map(|p| p.to_string())
                            .unwrap_or("UNKNOWN".to_owned()),
                        event_type
                    );
                }
            },
        }
    }
    if !handled_any {
        return Ok(());
    }
    let mut rendered = None;
    let mut render_failed = false;
    if failure.is_none() && should_render && running_plugin.has_area() {
        match render_plugin_for_clients(plugin_id, running_plugin, render_clients) {
            Ok(assets) => rendered = Some(assets),
            Err(e) => {
                render_failed = true;
                failure = Some(e);
            },
        }
    }
    if let Some(mut assets) = rendered {
        plugin_render_assets.append(&mut assets);
    }
    if !render_failed {
        report_pipe_changes(running_plugin, pipe_changes, None, &senders);
    }
    if failure.is_some() {
        running_plugin.mark_failed();
    }
    failure.map_or(Ok(()), Err)
}

/// The instances running at one location whose configuration is the one asked for.
///
/// **Compared without the caller context, not looked up by key.** The map is keyed on the whole
/// `PluginUserConfiguration`, and `caller_cwd` / `caller_terminal_pane_id` are stamped onto that
/// map by whoever asked for the launch rather than written by the user: the keybinding path writes
/// both (`Pty::spawn_plugin`), a plugin asking for another plugin by url writes only the first
/// (`yinz_exports::message_to_plugin`). An exact key lookup therefore failed to see a plugin
/// that was already running and started a second copy of it - which is how a second, hidden agent
/// conversation ended up behind the agent list, placing the list into the visible conversation's
/// slot on top of it.
///
/// `RunPluginOrAlias::is_equivalent_to_run` and `configuration_matches_for_reload` have always
/// asked the question this way. This was the one lookup that did not.
fn instances_of<'a>(
    at_this_location: &'a HashMap<PluginUserConfiguration, Vec<(PluginId, ClientId)>>,
    plugin_configuration: &PluginUserConfiguration,
) -> &'a [(PluginId, ClientId)] {
    let wanted = plugin_configuration.without_caller_context();
    at_this_location
        .iter()
        .find(|(configuration, _)| configuration.without_caller_context() == wanted)
        .map(|(_, instances)| instances.as_slice())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "unit/plugin_bridge.rs"]
mod tests;

#[path = "./unit/plugin_bridge_tests.rs"]
#[cfg(test)]
mod plugin_bridge_tests;

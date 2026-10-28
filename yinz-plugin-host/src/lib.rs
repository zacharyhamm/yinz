//! The plugin host: loads Steel Scheme plugins, runs them on the plugin thread, and turns their
//! commands into instructions for the other server threads. It talks to the rest of the server
//! over [`yinz_bus`], using [`yinz_routing`] to dispatch actions. It owns plugin job queues.
mod agent_provider;
mod project_provider;
mod provider_usage;
pub use agent_provider::inspect_agent_provider;
mod background_worker;
mod compilation_worker;
mod permissions;
#[doc(hidden)]
pub use compilation_worker::run_compilation_worker;
mod binary_data;
mod event;
mod host_bridge;
mod steel;
pub mod testing;
pub use host_bridge::ServerSession;
mod pinned_executor;
mod pipes;
pub use yinz_plugin_authoring as plugin_authoring;
mod plugin_bridge;
mod plugin_loader;
mod plugin_map;
mod plugin_panes;
pub(crate) mod response;
mod watch_filesystem;
mod yinz_exports;
use log::info;
pub use yinz_bus::plugins::*;
pub use yinz_exports::{control_agent, spawn_agent_for_cli};

pub(crate) use pinned_executor::PinnedExecutor;
pub use yinz_process::pty_host;
pub use yinz_types::internal_ids::{ClientId, PluginId, TerminalId};
pub use yinz_types::pane_id::PaneId;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use yinz_bus::route::NotificationEnd;
use yinz_bus::screen::ScreenInstruction;
use yinz_bus::session_layout_metadata::SessionLayoutMetadata;
use yinz_bus::{
    pty::{PtyInstruction, ReplacedPane},
    thread_bus::Bus,
    ServerInstruction,
};
use yinz_storage::data_access::DataAccess;
use yinz_types::input::context_menu::ContextMenu;
use {
    yinz_types::input::hooks::HookPoint, yinz_types::input::hooks::HookShape,
    yinz_types::input::hooks::Hooks,
};

use plugin_bridge::PluginBridge;
use plugin_panes::{PaneRequest, ShowPane};
mod cli_data;

use yinz_runtime::errors::prelude::*;
use yinz_types::diagnostics::ContextType;
use yinz_types::diagnostics::ErrorContext;
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::pane_size::Size;
use {
    yinz_config::plugins::PluginConfig, yinz_session::session_serialization,
    yinz_types::data::ClientInfo, yinz_types::data::CommandOrPlugin,
    yinz_types::data::ContextMenuContext, yinz_types::data::Event, yinz_types::data::EventType,
    yinz_types::data::HookPayload, yinz_types::data::InputMode, yinz_types::data::LayoutInfo,
    yinz_types::data::LayoutWithError, yinz_types::data::Mouse, yinz_types::data::PipeMessage,
    yinz_types::data::PipeSource, yinz_types::input::command::TerminalAction,
    yinz_types::input::keybinds::Keybinds, yinz_types::input::layout::Layout,
    yinz_types::input::layout::Run, yinz_types::input::layout::RunPlugin,
    yinz_types::input::layout::RunPluginLocation, yinz_types::input::layout::RunPluginOrAlias,
    yinz_types::input::plugins::PluginAliases,
};

#[allow(clippy::too_many_arguments)]
pub fn plugin_thread_main(
    bus: Bus<PluginInstruction>,
    data_dir: PathBuf,
    data_access: Arc<DataAccess>,
    mut layout: Box<Layout>,
    layout_dir: Option<PathBuf>,
    user_plugin_dir: Option<PathBuf>,
    available_layouts: Vec<LayoutInfo>,
    available_layout_errors: Vec<LayoutWithError>,
    path_to_default_shell: PathBuf,
    yinz_cwd: PathBuf,
    session_env_vars: std::collections::BTreeMap<String, String>,
    default_shell: Option<TerminalAction>,
    mut plugin_aliases: PluginAliases,
    default_mode: InputMode,
    default_keybinds: Keybinds,
    background_plugins: BTreeSet<RunPluginOrAlias>,
    // Both are replaced wholesale by `PluginInstruction::Reconfigure`, so a config reload changes
    // which plugin owns a hook point and what a right-click offers without a restart.
    mut hooks: Hooks,
    mut context_menu: ContextMenu,
    // the client id that started the session,
    // we need it here because the thread's own list of connected clients might not yet be updated
    // on session start when we need to load the background plugins, and so we must have an
    // explicit client_id that has started the session
    initiating_client_id: ClientId,
    // How plugins learn the session's name. The server passes `ServerSession::Shared`, so a
    // rename reaches plugins loaded after it.
    server_session: ServerSession,
) -> Result<()> {
    info!("Plugin main thread starts");
    let mut notification_policy = None;
    let yinz_data_dir = data_dir.clone();
    layout.populate_plugin_aliases_in_layout(&plugin_aliases);

    // use this channel to ensure that tasks spawned from this thread terminate before exiting
    // https://tokio.rs/tokio/topics/shutdown#waiting-for-things-to-finish-shutting-down
    let (shutdown_send, mut shutdown_receive) = tokio::sync::mpsc::channel::<()>(1);

    let palette_data = data_access.clone();
    let mut plugin_bridge = PluginBridge::new(
        bus.senders.clone(),
        yinz_data_dir,
        data_access,
        path_to_default_shell,
        yinz_cwd.clone(),
        session_env_vars,
        default_shell,
        layout_dir,
        user_plugin_dir,
        available_layouts,
        available_layout_errors,
        default_mode,
        default_keybinds,
        plugin_aliases.clone(),
        server_session.clone(),
    )?;
    // The background plugins below and the first layout's plugins are loaded for the first
    // client before its `AddClient`, and must share its attachment token.
    plugin_bridge.reserve_client(initiating_client_id);
    let _data_watch_poller = plugin_bridge.start_data_watch_polling()?;

    let host_required: Vec<_> = background_plugins
        .iter()
        .filter(|declaration| {
            declaration
                .get_run_plugin()
                .is_some_and(|run| matches!(run.location, RunPluginLocation::Yinz(tag) if tag.to_string() == "agent-opencode"))
        })
        .cloned()
        .collect();
    plugin_bridge
        .reconcile_background_plugins(background_plugins, initiating_client_id)
        .context("reconcile startup background plugins")
        .non_fatal();
    for declaration in host_required {
        plugin_bridge
            .start_background_plugin(declaration.clone(), initiating_client_id, true)
            .with_context(|| format!("start host-required plugin {declaration:?}"))
            .non_fatal();
    }
    // A session starts once. The hook fires for its first client, below, not here: a plugin
    // loaded before any client is connected is loaded again for the first one that is
    // (`PluginBridge::add_client`), and an event handed to the first instance would be lost with
    // it.
    let mut session_started = false;

    // The one message `drain_updates` pulled off the bus that was not an `Update`; it is
    // handled by the next iteration, in the order it arrived.
    let mut queued: Option<(PluginInstruction, ErrorContext)> = None;
    loop {
        let (event, mut err_ctx) = match queued.take() {
            Some(next) => next,
            None => bus.recv().expect("failed to receive event on channel"),
        };
        // The completion handles of reloads collapsed into this one. Dropping one is what
        // answers the CLI waiting on it, so the `Reload` arm joins them onto its own handle
        // (`NotificationEnd::join`): every path answers them together with the survivor and with
        // its payload - in particular the pane id, when the plugin turns out not to be running
        // and the reload becomes an `AddPlugin`. A `ReloadPluginByUrl` carries no handle, so
        // the vec is empty for it and there is nothing to join.
        let mut collapsed_reload_completions: Vec<NotificationEnd> = vec![];
        let event = match event {
            PluginInstruction::Update(updates) => {
                PluginInstruction::Update(drain_updates(&bus, updates, &mut queued))
            },
            reload @ (PluginInstruction::Reload(..) | PluginInstruction::ReloadPluginByUrl(..)) => {
                let (reload, completions) = drain_reloads(&bus, reload, &mut queued);
                collapsed_reload_completions = completions;
                reload
            },
            other => other,
        };
        yinz_runtime::errors::add_call(&mut err_ctx, ContextType::Plugin((&event).into()));
        match event {
            PluginInstruction::LoadPaneTab {
                skip_cache,
                id,
                index,
                generation,
                descriptor,
                client_id,
                tab_id,
                size,
            } => {
                let result = (|| {
                    let plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                        &descriptor.location,
                        &Some(descriptor.configuration),
                        None,
                        descriptor.initial_cwd.clone(),
                    )
                    .map_err(|error| anyhow!(error))?;
                    let mut plugin = match descriptor.initial_cwd {
                        Some(cwd) => plugin.with_initial_cwd(Some(cwd)),
                        None => plugin,
                    };
                    plugin.populate_run_plugin_if_needed(&plugin_aliases);
                    let cwd = plugin.get_initial_cwd();
                    let (plugin_id, _) = plugin_bridge.load_plugin(
                        &plugin.get_run_plugin(),
                        Some(tab_id),
                        size,
                        cwd.clone(),
                        skip_cache,
                        Some(client_id),
                        PaneRequest::Placed {
                            run: plugin.clone(),
                        },
                    )?;
                    bus.senders.send_to_pty(PtyInstruction::ReportPluginCwd(
                        plugin_id,
                        cwd.unwrap_or_else(|| yinz_cwd.clone()),
                    ))?;
                    Ok::<_, anyhow::Error>((plugin_id, plugin))
                })()
                .map_err(|error| error.to_string());
                bus.senders
                    .send_to_screen(ScreenInstruction::PaneTabLoaded {
                        id,
                        index,
                        generation,
                        result,
                        client_id,
                    })
                    .context("failed to place pane tab plugin")
                    .non_fatal();
            },
            PluginInstruction::Load(
                should_float,
                should_be_open_in_place,
                close_replaced_pane,
                pane_title,
                run_plugin_or_alias,
                tab_index,
                pane_id_to_replace,
                client_id,
                size,
                cwd,
                focused_plugin_id,
                skip_cache,
                should_focus_plugin,
                floating_pane_coordinates,
                completion_tx,
                overlay_placement,
            ) => load_and_show(
                ShowPane {
                    should_float,
                    in_place: should_be_open_in_place,
                    replaced: ReplacedPane::closed_if(close_replaced_pane),
                    run: run_plugin_or_alias,
                    title: pane_title,
                    tab: tab_index,
                    pane_id_to_replace,
                    cwd,
                    coordinates: floating_pane_coordinates,
                    should_focus: should_focus_plugin,
                    client_id: Some(client_id),
                    completion: completion_tx,
                    overlay: overlay_placement,
                    reserved_slot: None,
                },
                PaneLoad {
                    size,
                    focused_plugin_id,
                    skip_cache,
                },
                client_id,
                &mut plugin_bridge,
                &bus,
                &plugin_aliases,
                &yinz_cwd,
            ),
            PluginInstruction::LoadIntoSlot {
                run,
                slot,
                client_id,
                cwd,
            } => load_and_show(
                ShowPane::into_slot(run, slot, client_id, cwd),
                PaneLoad {
                    size: Size { rows: 1, cols: 1 },
                    focused_plugin_id: None,
                    skip_cache: false,
                },
                client_id,
                &mut plugin_bridge,
                &bus,
                &plugin_aliases,
                &yinz_cwd,
            ),
            PluginInstruction::RunHook {
                point,
                pane_id,
                payload,
                client_id,
            } => {
                run_hook(
                    point,
                    pane_id,
                    payload,
                    client_id,
                    &hooks,
                    &mut plugin_bridge,
                    &bus,
                    &plugin_aliases,
                    &shutdown_send,
                );
            },
            PluginInstruction::RequestContextMenu { context, client_id } => {
                request_context_menu(
                    *context,
                    client_id,
                    &hooks,
                    &context_menu,
                    &mut plugin_bridge,
                    &plugin_aliases,
                    &bus,
                    &shutdown_send,
                );
            },
            PluginInstruction::LoadBackgroundPlugin(run_plugin_or_alias, client_id) => {
                load_background_plugin(
                    run_plugin_or_alias,
                    &mut plugin_bridge,
                    &bus,
                    &plugin_aliases,
                    client_id,
                );
            },
            PluginInstruction::Update(updates) => {
                plugin_bridge.update_plugins(updates, shutdown_send.clone())?;
            },
            PluginInstruction::InputBarrier(plugin_id, completion) => {
                plugin_bridge.input_barrier(plugin_id, completion);
            },
            PluginInstruction::PaletteAgent(request) => {
                let senders = bus.senders.clone();
                let data = palette_data.clone();
                let shutdown = shutdown_send.clone();
                yinz_bus::global_async_runtime::get_tokio_runtime().spawn_blocking(move || {
                    let _shutdown = shutdown;
                    yinz_exports::palette_agents::handle(*request, &senders, &data);
                });
            },
            PluginInstruction::PaletteAgentLabels(request) => {
                let senders = bus.senders.clone();
                let data = palette_data.clone();
                let shutdown = shutdown_send.clone();
                yinz_bus::global_async_runtime::get_tokio_runtime().spawn_blocking(move || {
                    let _shutdown = shutdown;
                    yinz_exports::palette_agents::labels(request, &senders, &data);
                });
            },
            PluginInstruction::PluginPalette(delivery) => {
                plugin_bridge.plugin_palette(delivery, shutdown_send.clone());
            },
            PluginInstruction::PluginTick(plugin_id, _client_id, generation) => {
                plugin_bridge.plugin_tick(plugin_id, generation, shutdown_send.clone());
            },
            PluginInstruction::PluginEvent(plugin_id, client_id, generation, event) => {
                let events = drain_plugin_events(
                    &bus,
                    plugin_id,
                    generation,
                    vec![(client_id, event, None)],
                    &mut queued,
                );
                plugin_bridge.plugin_replies(
                    plugin_id,
                    generation,
                    events,
                    shutdown_send.clone(),
                )?;
            },
            PluginInstruction::PluginReply(plugin_id, client_id, generation, event, reply) => {
                let events = drain_plugin_events(
                    &bus,
                    plugin_id,
                    generation,
                    vec![(client_id, event, Some(reply))],
                    &mut queued,
                );
                plugin_bridge.plugin_replies(
                    plugin_id,
                    generation,
                    events,
                    shutdown_send.clone(),
                )?;
            },
            PluginInstruction::PluginDispose(plugin_id, generation) => {
                plugin_bridge.plugin_dispose(plugin_id, generation);
            },
            PluginInstruction::Unload(pid) => {
                // Eventual cleanup for runtime unloads. Screen destruction paths drop notices
                // synchronously, before tab settlement can consult them.
                bus.senders
                    .send_to_screen(ScreenInstruction::SetQuitNotice(pid, None, false))
                    .non_fatal();
                plugin_bridge.unload_plugin(pid)?;
            },
            PluginInstruction::Reload(
                should_float,
                pane_title,
                mut run_plugin_or_alias,
                tab_index,
                size,
                completion_tx,
            ) => {
                let mut completion_tx = NotificationEnd::join(
                    completion_tx,
                    std::mem::take(&mut collapsed_reload_completions),
                );
                run_plugin_or_alias.populate_run_plugin_if_needed(&plugin_aliases);
                // Every waiter, collapsed ones included, hears why nothing was reloaded: a
                // dropped handle reports success.
                let failure = match run_plugin_or_alias.get_run_plugin() {
                    Some(run_plugin) => match plugin_bridge.reload_plugin(&run_plugin) {
                        Ok(_) => {
                            let _ = bus
                                .senders
                                .send_to_server(ServerInstruction::UnblockInputThread);
                            None
                        },
                        Err(err)
                            if matches!(
                                err.downcast_ref::<YinzError>(),
                                Some(YinzError::PluginDoesNotExist)
                            ) =>
                        {
                            log::warn!(
                                "Plugin {} not found, starting it instead",
                                run_plugin.location
                            );
                            let location = run_plugin.location.clone();
                            // we intentionally do not provide the client_id here because it belongs to
                            // the cli who spawned the command and is not an existing client_id
                            let skip_cache = true; // when reloading we always skip cache
                            let pane = PaneRequest::Show(Box::new(ShowPane {
                                should_float,
                                in_place: false,
                                replaced: ReplacedPane::Suppress,
                                run: run_plugin_or_alias,
                                title: pane_title,
                                tab: Some(tab_index),
                                pane_id_to_replace: None,
                                cwd: None,
                                coordinates: None,
                                should_focus: None,
                                client_id: None,
                                completion: completion_tx.take(),
                                overlay: None,
                                reserved_slot: None,
                            }));
                            match plugin_bridge.load_plugin(
                                &Some(run_plugin),
                                Some(tab_index),
                                size,
                                None,
                                skip_cache,
                                None,
                                pane,
                            ) {
                                Ok(_) => None,
                                Err(e) => Some(e.context(format!("failed to start {location}"))),
                            }
                        },
                        Err(err) => {
                            Some(err.context(format!("failed to reload {}", run_plugin.location)))
                        },
                    },
                    None => Some(anyhow!(
                        "unknown plugin or alias: {}",
                        run_plugin_or_alias.location_string()
                    )),
                };
                if let Some(error) = failure {
                    if let Some(completion_tx) = completion_tx.as_mut() {
                        completion_tx.set_error_message(format!("{error:#}"));
                    }
                    Err::<(), _>(error).non_fatal();
                }
            },
            PluginInstruction::ReloadPluginWithId(plugin_id) => {
                plugin_bridge.reload_plugin_with_id(plugin_id).non_fatal();
            },
            PluginInstruction::ReloadPluginByUrl(mut run_plugin_or_alias) => {
                run_plugin_or_alias.populate_run_plugin_if_needed(&plugin_aliases);
                match run_plugin_or_alias.get_run_plugin() {
                    Some(run_plugin) => match plugin_bridge.reload_plugin(&run_plugin) {
                        Ok(()) => {},
                        Err(e)
                            if matches!(
                                e.downcast_ref::<YinzError>(),
                                Some(YinzError::PluginDoesNotExist)
                            ) =>
                        {
                            log::warn!(
                                "Nothing is running at {}, so there is nothing to reload",
                                run_plugin.location
                            );
                        },
                        Err(e) => Err::<(), _>(e).non_fatal(),
                    },
                    None => {
                        log::error!("Failed to find plugin info for: {:?}", run_plugin_or_alias);
                    },
                }
            },
            PluginInstruction::Resize(pid, new_columns, new_rows) => {
                plugin_bridge.resize_plugin(pid, new_columns, new_rows, shutdown_send.clone())?;
            },
            PluginInstruction::ReserveClient(client_id) => {
                plugin_bridge.reserve_client(client_id);
            },
            PluginInstruction::AddClient(client_id) => {
                let started_at = std::time::Instant::now();
                log::info!("attach trace: plugin thread client_id={client_id} AddClient begin");
                let result = plugin_bridge.add_client(client_id);
                log::info!(
                    "attach trace: plugin thread client_id={client_id} AddClient finished after {:?} ({})",
                    started_at.elapsed(),
                    if result.is_ok() { "ok" } else { "error" }
                );
                // The client is attached either way; its session start still runs.
                result
                    .with_context(|| format!("add client {client_id} to plugins"))
                    .non_fatal();
                if !session_started {
                    if hooks.get(HookPoint::Notification).is_some() {
                        notification_policy = load_notification_policy(
                            &hooks,
                            client_id,
                            &mut plugin_bridge,
                            &bus,
                            &plugin_aliases,
                        );
                    }
                    session_started = true;
                    // The session is up, in the sense that matters to a plugin: the thread
                    // exists, its background plugins are loading, and there is a client to
                    // load the hook plugin for. The screen may not have its first tab yet -
                    // the hook's own suppressed pane is buffered until it does, like every
                    // background plugin's.
                    run_hook(
                        HookPoint::SessionStart,
                        None,
                        session_start_payload(&server_session, client_id),
                        client_id,
                        &hooks,
                        &mut plugin_bridge,
                        &bus,
                        &plugin_aliases,
                        &shutdown_send,
                    );
                }
            },
            PluginInstruction::RemoveClient(client_id) => {
                plugin_bridge.remove_client(client_id);
            },
            PluginInstruction::CliPipesDisconnected(pipes) => {
                plugin_bridge.cli_pipes_disconnected(&pipes);
            },
            PluginInstruction::ResolveAgentEventProjects {
                version,
                agents,
                projects,
            } => {
                plugin_bridge.resolve_agent_event_projects(version, agents, projects);
            },
            PluginInstruction::SubscribeCliClientToEvents {
                client_id,
                event_types,
            } => {
                plugin_bridge.subscribe_cli_client_to_events(client_id, event_types);
            },
            PluginInstruction::NewTab(
                cwd,
                terminal_action,
                mut tab_layout,
                floating_panes_layout,
                tab_id,
                initial_panes,
                block_on_first_terminal,
                should_change_focus_to_new_tab,
                client_id,
                completion_tx,
            ) => {
                let startup_started = std::time::Instant::now();
                log::info!("startup: plugin NewTab tab={tab_id:?} client={client_id} begin");
                // prefer connected clients so as to avoid opening plugins in the background for
                // CLI clients unless no-one else is connected
                let client_id = if plugin_bridge.client_is_connected(&client_id) {
                    client_id
                } else if let Some(first_client_id) = plugin_bridge.get_first_client_id() {
                    first_client_id
                } else {
                    client_id
                };

                let mut plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>> = HashMap::new();
                tab_layout = tab_layout.or_else(|| Some(layout.new_tab().0));
                let mut floating_panes_layout =
                    floating_panes_layout.unwrap_or_else(|| layout.new_tab().1);

                // Match initial_panes plugins to empty slots in the layout
                if let Some(ref initial_panes_vec) = initial_panes
                    && let Some(ref mut tiled_layout) = tab_layout
                {
                    for initial_pane in initial_panes_vec.iter() {
                        if let CommandOrPlugin::Plugin(run_plugin_or_alias) = initial_pane
                            && !tiled_layout.replace_next_empty_slot_with_run(Run::Plugin(
                                run_plugin_or_alias.clone(),
                            ))
                        {
                            log::warn!("More initial_panes provided than empty slots available");
                            break;
                        }
                        // Skip CommandOrPlugin::Command entries (handled by pty thread)
                    }
                }

                if let Some(t) = tab_layout.as_mut() {
                    t.populate_plugin_aliases_in_layout(&plugin_aliases);
                    if let Some(cwd) = cwd.as_ref() {
                        t.add_cwd_to_layout(cwd);
                    }
                }
                floating_panes_layout.iter_mut().for_each(|f| {
                    f.populate_plugin_aliases(&plugin_aliases);
                });
                let extracted_run_instructions = tab_layout
                    .clone()
                    .unwrap_or_else(|| layout.new_tab().0)
                    .extract_run_instructions();
                let size = Size::default();
                let mut extracted_floating_plugins: Vec<Option<Run>> = floating_panes_layout
                    .iter()
                    .filter(|f| !f.already_running)
                    .map(|f| f.run.clone())
                    .collect();
                let mut all_run_instructions = extracted_run_instructions;
                all_run_instructions.append(&mut extracted_floating_plugins);

                for run_instruction in all_run_instructions {
                    if let Some(Run::Plugin(run_plugin_or_alias)) = run_instruction {
                        let run_plugin = run_plugin_or_alias.get_run_plugin();
                        let cwd = run_plugin_or_alias
                            .get_initial_cwd()
                            .or_else(|| cwd.clone());
                        let skip_cache = false;
                        log::info!(
                            "startup: tab={tab_id:?} client={client_id} loading {:?} at {:?}",
                            run_plugin.as_ref().map(|plugin| &plugin.location),
                            startup_started.elapsed()
                        );
                        match plugin_bridge.load_plugin(
                            &run_plugin,
                            Some(tab_id),
                            size,
                            cwd,
                            skip_cache,
                            Some(client_id),
                            PaneRequest::Placed {
                                run: run_plugin_or_alias.clone(),
                            },
                        ) {
                            Ok((plugin_id, _client_id)) => {
                                log::info!("startup: tab={tab_id:?} client={client_id} loaded plugin={plugin_id} at {:?}", startup_started.elapsed());
                                plugin_ids
                                    .entry(run_plugin_or_alias.clone())
                                    .or_default()
                                    .push(plugin_id);
                            },
                            Err(e) => {
                                log::error!("Failed to load plugin: {}", e);
                            },
                        }
                    }
                }
                log::info!("startup: plugin NewTab tab={tab_id:?} client={client_id} dispatching PTY at {:?}", startup_started.elapsed());
                drop(bus.senders.send_to_pty(PtyInstruction::NewTab(
                    cwd,
                    terminal_action,
                    tab_layout,
                    floating_panes_layout,
                    tab_id,
                    plugin_ids,
                    initial_panes,
                    block_on_first_terminal,
                    should_change_focus_to_new_tab,
                    client_id,
                    completion_tx,
                )));
            },
            PluginInstruction::OverrideLayout(
                cwd,
                default_shell,
                tabs_to_close,
                tab_layouts,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                client_id,
                completion_tx,
            ) => {
                // 1. Prefer connected clients over CLI clients
                let client_id = if plugin_bridge.client_is_connected(&client_id) {
                    client_id
                } else if let Some(first_client_id) = plugin_bridge.get_first_client_id() {
                    first_client_id
                } else {
                    client_id
                };

                // 2. Process each tab layout
                let mut tab_layouts_with_plugin_ids = Vec::new();

                for (tab_id, mut tab_layout_info) in tab_layouts {
                    // Populate plugin aliases in layouts
                    tab_layout_info
                        .tiled_layout
                        .populate_plugin_aliases_in_layout(&plugin_aliases);
                    tab_layout_info.floating_layouts.iter_mut().for_each(|f| {
                        f.populate_plugin_aliases(&plugin_aliases);
                    });

                    // Extract run instructions from tiled layout
                    let extracted_run_instructions =
                        tab_layout_info.tiled_layout.extract_run_instructions();

                    // Extract run instructions from floating layouts (excluding already_running)
                    let extracted_floating_plugins: Vec<Option<Run>> = tab_layout_info
                        .floating_layouts
                        .iter()
                        .filter(|f| !f.already_running)
                        .map(|f| f.run.clone())
                        .collect();

                    // Combine all run instructions
                    let mut all_run_instructions = extracted_run_instructions;
                    all_run_instructions.extend(extracted_floating_plugins);

                    // Load plugins for all Run::Plugin instructions
                    let mut plugin_ids: HashMap<RunPluginOrAlias, Vec<PluginId>> = HashMap::new();
                    let size = Size::default();

                    for run_instruction in all_run_instructions {
                        if let Some(Run::Plugin(run_plugin_or_alias)) = run_instruction {
                            let run_plugin = run_plugin_or_alias.get_run_plugin();
                            let cwd = run_plugin_or_alias.get_initial_cwd();
                            let skip_cache = false;

                            match plugin_bridge.load_plugin(
                                &run_plugin,
                                Some(tab_id),
                                size,
                                cwd,
                                skip_cache,
                                Some(client_id),
                                PaneRequest::Placed {
                                    run: run_plugin_or_alias.clone(),
                                },
                            ) {
                                Ok((plugin_id, _client_id)) => {
                                    plugin_ids
                                        .entry(run_plugin_or_alias.clone())
                                        .or_default()
                                        .push(plugin_id);
                                },
                                Err(e) => {
                                    log::error!("Failed to load plugin: {}", e);
                                },
                            }
                        }
                    }

                    // Pair this tab's layout with its plugin IDs
                    tab_layouts_with_plugin_ids.push((tab_id, tab_layout_info, plugin_ids));
                }

                // 3. Send to pty thread with all tab layouts and their plugin IDs
                drop(bus.senders.send_to_pty(PtyInstruction::OverrideLayout(
                    cwd,
                    default_shell,
                    tabs_to_close,
                    tab_layouts_with_plugin_ids,
                    retain_existing_terminal_panes,
                    retain_existing_plugin_panes,
                    client_id,
                    completion_tx,
                )));
            },
            PluginInstruction::ApplyCachedEvents {
                load_id,
                plugin_ids,
                done_receiving_permissions,
            } => {
                plugin_bridge.apply_cached_events_for_load(
                    load_id,
                    plugin_ids,
                    done_receiving_permissions,
                    shutdown_send.clone(),
                )?;
            },
            PluginInstruction::ApplyCachedWorkerMessages(plugin_id) => {
                plugin_bridge.apply_cached_worker_messages(plugin_id)?;
            },
            PluginInstruction::PostMessagesToPluginWorker(
                plugin_id,
                client_id,
                worker_name,
                messages,
            ) => {
                plugin_bridge.post_messages_to_plugin_worker(
                    plugin_id,
                    client_id,
                    worker_name,
                    messages,
                )?;
            },
            PluginInstruction::PostMessageToPlugin(plugin_id, client_id, message, payload) => {
                let updates = vec![(
                    Some(plugin_id),
                    Some(client_id),
                    Event::CustomMessage(message, payload),
                )];
                plugin_bridge.update_plugins(updates, shutdown_send.clone())?;
            },
            PluginInstruction::NotificationClicked {
                plugin_id,
                raiser,
                clicker,
                message,
                payload,
            } => {
                let client_id = client_for_notification_click(
                    plugin_bridge.plugin_is_shared(&plugin_id),
                    raiser,
                    clicker,
                );
                let updates = vec![(
                    Some(plugin_id),
                    Some(client_id),
                    Event::CustomMessage(message, payload),
                )];
                plugin_bridge.update_plugins(updates, shutdown_send.clone())?;
            },
            PluginInstruction::PluginSubscribedToEvents(
                plugin_id,
                client_id,
                events,
                background_subscriptions,
                instance,
            ) => {
                if let Some(subscriptions) = background_subscriptions {
                    plugin_bridge.forward_background_subscriptions(
                        plugin_id,
                        client_id,
                        subscriptions,
                        instance,
                    );
                }
                if events.contains(&EventType::CellPixelSizeChanged) {
                    bus.senders
                        .send_to_screen(ScreenInstruction::ReportCellPixelSize(
                            plugin_id, client_id,
                        ))
                        .context("send current cell pixel size to subscriber")?;
                }
                if events.contains(&EventType::ProjectProvidersChanged) {
                    plugin_bridge.update_plugins(
                        vec![(
                            Some(plugin_id),
                            Some(client_id),
                            Event::ProjectProvidersChanged(
                                yinz_types::project_provider::providers(),
                            ),
                        )],
                        shutdown_send.clone(),
                    )?;
                }
                if events.contains(&EventType::AgentProvidersChanged) {
                    plugin_bridge.update_plugins(
                        vec![(
                            Some(plugin_id),
                            Some(client_id),
                            Event::AgentProvidersChanged(yinz_types::agent::provider::providers()),
                        )],
                        shutdown_send.clone(),
                    )?;
                }
                if events.contains(&EventType::InitialKeybinds) {
                    plugin_bridge.send_initial_keybinds_to_plugin(plugin_id, client_id);
                }
                if events.contains(&EventType::HostTerminalThemeChanged) {
                    plugin_bridge.send_host_terminal_theme_mode_to_plugin(plugin_id, client_id);
                }
                if events.contains(&EventType::HintText) {
                    let _ = bus
                        .senders
                        .send_to_screen(ScreenInstruction::ClearHintTextCache);
                }
            },
            PluginInstruction::RequestPermissions {
                plugin_id,
                instance_id,
                client_id,
                permissions,
            } => {
                plugin_bridge.request_permissions(
                    plugin_id,
                    instance_id,
                    client_id,
                    permissions,
                    shutdown_send.clone(),
                )?;
            },
            PluginInstruction::PermissionAnswer {
                request_id,
                status,
                cache_path,
            } => {
                plugin_bridge.answer_permissions(
                    request_id,
                    status,
                    cache_path,
                    shutdown_send.clone(),
                )?;
            },
            PluginInstruction::DumpLayout(
                mut session_layout_metadata,
                client_id,
                completion_tx,
            ) => {
                populate_session_layout_metadata(
                    &mut session_layout_metadata,
                    &plugin_bridge,
                    &plugin_aliases,
                    None,
                    None,
                );
                drop(bus.senders.send_to_pty(PtyInstruction::DumpLayout(
                    session_layout_metadata,
                    client_id,
                    completion_tx,
                )));
            },
            PluginInstruction::ListClientsMetadata(
                mut session_layout_metadata,
                client_id,
                completion_tx,
            ) => {
                populate_session_layout_metadata(
                    &mut session_layout_metadata,
                    &plugin_bridge,
                    &plugin_aliases,
                    None,
                    None,
                );
                drop(bus.senders.send_to_pty(PtyInstruction::ListClientsMetadata(
                    session_layout_metadata,
                    client_id,
                    completion_tx,
                )));
            },
            PluginInstruction::DumpLayoutToPlugin {
                mut session_layout_metadata,
                plugin_id,
                response_channel,
                trace,
            } => {
                trace.mark("plugin metadata");
                populate_session_layout_metadata(
                    &mut session_layout_metadata,
                    &plugin_bridge,
                    &plugin_aliases,
                    Some(plugin_id),
                    Some(&trace),
                );

                trace.mark("serialization");
                let layout_metadata = session_layout_metadata.to_layout_metadata();

                match session_serialization::serialize_session_layout(
                    session_layout_metadata.into(),
                ) {
                    Ok((layout, _pane_contents)) => {
                        // send synchronous response
                        let response = DumpSessionLayoutResponse {
                            layout_result: Ok(layout.clone()),
                            metadata: Some(layout_metadata),
                        };
                        trace.mark("response");
                        let _ = response_channel.send(response);

                        // send CustomMessage to plugin (backwards compatibility, should get rid of
                        // this on API version upgrade)
                        let updates = vec![(
                            Some(plugin_id),
                            None,
                            Event::CustomMessage("session_layout".to_owned(), layout),
                        )];
                        plugin_bridge.update_plugins(updates, shutdown_send.clone())?;
                    },
                    Err(e) => {
                        let error_msg = e.to_string();
                        let response = DumpSessionLayoutResponse {
                            layout_result: Err(error_msg.clone()),
                            metadata: None,
                        };
                        trace.mark("response");
                        let _ = response_channel.send(response);

                        let updates = vec![(
                            Some(plugin_id),
                            None,
                            Event::CustomMessage("session_layout_error".to_owned(), error_msg),
                        )];
                        plugin_bridge.update_plugins(updates, shutdown_send.clone())?;
                    },
                }
            },
            PluginInstruction::ListClientsToPlugin(
                mut session_layout_metadata,
                plugin_id,
                client_id,
                generation,
            ) => {
                populate_session_layout_metadata(
                    &mut session_layout_metadata,
                    &plugin_bridge,
                    &plugin_aliases,
                    Some(plugin_id),
                    None,
                );
                let mut clients_metadata = session_layout_metadata.all_clients_metadata();
                let mut client_list_for_plugin = vec![];
                let default_editor = session_layout_metadata.default_editor.clone();
                for (client_metadata_id, client_metadata) in clients_metadata.iter_mut() {
                    let is_current_client = client_metadata_id == &client_id;
                    client_list_for_plugin.push(ClientInfo::new(
                        *client_metadata_id,
                        client_metadata.get_pane_id(),
                        client_metadata.stringify_command(&default_editor),
                        is_current_client,
                    ));
                }
                let event = Event::ListClients(client_list_for_plugin);
                if let Some(generation) = generation {
                    bus.senders.send_to_plugin(PluginInstruction::PluginEvent(
                        plugin_id, client_id, generation, event,
                    ))?;
                } else {
                    plugin_bridge.update_plugins(
                        vec![(Some(plugin_id), Some(client_id), event)],
                        shutdown_send.clone(),
                    )?;
                }
            },
            PluginInstruction::PluginRequest {
                request,
                client_id,
                reply,
            } => {
                let result = plugin_bridge
                    .plugin_request(request, client_id)
                    .map_err(|error| format!("{error:#}"));
                let _ = reply.send(result);
            },
            PluginInstruction::ReconcileBackgroundPlugins { plugins, client_id } => {
                plugin_bridge
                    .reconcile_background_plugins(plugins, client_id)
                    .non_fatal();
            },
            PluginInstruction::DataRequest { request, reply } => {
                let _ = reply.send(
                    plugin_bridge
                        .cli_data(request)
                        .map_err(|error| format!("{error:#}")),
                );
            },
            PluginInstruction::RefreshDataWatches {
                namespace,
                entities,
            } => {
                plugin_bridge.notify_data_watchers(&namespace, &entities);
            },
            PluginInstruction::LogLayoutToHd(mut session_layout_metadata) => {
                populate_session_layout_metadata(
                    &mut session_layout_metadata,
                    &plugin_bridge,
                    &plugin_aliases,
                    None,
                    None,
                );
                drop(
                    bus.senders
                        .send_to_pty(PtyInstruction::LogLayoutToHd(session_layout_metadata)),
                );
            },
            PluginInstruction::CliPipe {
                focus_plugin,
                pipe_id,
                name,
                payload,
                is_eof,
                plugin,
                args,
                configuration,
                floating,
                pane_id_to_replace,
                pane_title,
                cwd,
                skip_cache,
                cli_client_id,
                source_pane_id,
            } => {
                let should_float = floating.unwrap_or(true);
                let mut pipe_messages = vec![];
                let mut resolution_error = None;
                let floating_pane_coordinates = None;
                match plugin.as_deref() {
                    Some(destination) if destination.starts_with("plugin_") => {
                        match destination.parse::<PaneId>() {
                            Ok(PaneId::Plugin(plugin_id))
                                if plugin_bridge.run_plugin_of_plugin_id(plugin_id).is_some()
                                    || plugin_bridge
                                        .run_plugin_of_loading_plugin_id(plugin_id)
                                        .is_some() =>
                            {
                                pipe_messages.push((
                                    Some(plugin_id),
                                    None,
                                    PipeMessage::new(
                                        PipeSource::Cli(pipe_id.clone()),
                                        &name,
                                        &payload,
                                        &args,
                                        true,
                                    ),
                                ));
                            },
                            Ok(PaneId::Plugin(_)) if is_eof => {},
                            _ => {
                                resolution_error = Some(format!(
                                    "Plugin destination is invalid or missing: {destination}"
                                ));
                            },
                        }
                    },
                    Some(plugin_url) => {
                        // send to specific plugin(s)
                        resolution_error = pipe_to_specific_plugins(
                            PipeSource::Cli(pipe_id.clone()),
                            plugin_url,
                            &configuration,
                            &cwd,
                            skip_cache,
                            should_float,
                            false,
                            &pane_id_to_replace,
                            &pane_title,
                            Some(cli_client_id),
                            &mut pipe_messages,
                            &name,
                            &payload,
                            &args,
                            &bus,
                            &mut plugin_bridge,
                            &plugin_aliases,
                            floating_pane_coordinates,
                            Some(focus_plugin),
                            !is_eof,
                        )
                        .err()
                        .map(|error| format!("{error:#}"));
                    },
                    None if matches!(
                        name.as_str(),
                        "store_query" | "store_mutate" | "agent_action"
                    ) =>
                    {
                        match plugin_bridge.builtin_service_instance(match name.as_str() {
                            "agent_action" => "agent-service",
                            _ => "project-store",
                        }) {
                            Ok((plugin_id, client_id)) => pipe_messages.push((
                                Some(plugin_id),
                                Some(client_id),
                                PipeMessage::new(
                                    PipeSource::Cli(pipe_id.clone()),
                                    &name,
                                    &payload,
                                    &args,
                                    true,
                                ),
                            )),
                            Err(error) if !is_eof => resolution_error = Some(error),
                            Err(_) => {},
                        }
                    },
                    None => {
                        // no specific destination, send to all plugins
                        pipe_to_all_plugins(
                            PipeSource::Cli(pipe_id.clone()),
                            &name,
                            &payload,
                            &args,
                            &mut plugin_bridge,
                            &mut pipe_messages,
                        );
                    },
                }
                if pipe_messages.is_empty() {
                    let result = if let Some(error) = resolution_error {
                        Err(error)
                    } else if is_eof
                        || (plugin.is_none()
                            && !matches!(
                                name.as_str(),
                                "store_query" | "store_mutate" | "agent_action"
                            ))
                    {
                        Ok(())
                    } else {
                        Err(format!(
                            "Plugin destination is unavailable: {}",
                            plugin.as_deref().unwrap_or("project-store")
                        ))
                    };
                    plugin_bridge.finish_empty_pipe(&pipe_id, result);
                }
                for (_, _, message) in pipe_messages.iter_mut() {
                    message.pane_id = source_pane_id;
                }
                plugin_bridge.pipe_messages(pipe_messages, shutdown_send.clone(), None)?;
            },
            PluginInstruction::ArrangePanesByProject => {
                let store = plugin_bridge.builtin_service_instance("project-store");
                if store.is_err()
                    || plugin_bridge
                        .builtin_service_instance("agent-service")
                        .is_err()
                {
                    bus.senders.send_to_screen(ScreenInstruction::Notify(
                        "Arrange panes requires the project-store and agent-service builtins"
                            .to_owned(),
                        None,
                        None,
                        None,
                    ))?;
                } else if let Ok((id, client)) = store {
                    plugin_bridge.pipe_messages(
                        vec![(
                            Some(id),
                            Some(client),
                            PipeMessage::new(
                                PipeSource::Keybind,
                                "arrange_panes_by_project".to_owned(),
                                &None,
                                &None,
                                true,
                            ),
                        )],
                        shutdown_send.clone(),
                        None,
                    )?;
                }
            },
            PluginInstruction::KeybindPipe {
                name,
                payload,
                plugin,
                args,
                configuration,
                floating,
                should_open_in_place,
                pane_id_to_replace,
                pane_title,
                cwd,
                skip_cache,
                cli_client_id,
                plugin_and_client_id,
                notification_end,
            } => {
                let should_float = floating.unwrap_or(true);
                let mut pipe_messages = vec![];
                let floating_pane_coordinates = None;
                if let Some((plugin_id, client_id)) = plugin_and_client_id {
                    let is_private = true;
                    pipe_messages.push((
                        Some(plugin_id),
                        Some(client_id),
                        PipeMessage::new(PipeSource::Keybind, name, &payload, &args, is_private),
                    ));
                } else {
                    match plugin {
                        Some(plugin_url) => {
                            // send to specific plugin(s)
                            pipe_to_specific_plugins(
                                PipeSource::Keybind,
                                &plugin_url,
                                &configuration,
                                &cwd,
                                skip_cache,
                                should_float,
                                should_open_in_place,
                                &pane_id_to_replace,
                                &pane_title,
                                Some(cli_client_id),
                                &mut pipe_messages,
                                &name,
                                &payload,
                                &args,
                                &bus,
                                &mut plugin_bridge,
                                &plugin_aliases,
                                floating_pane_coordinates,
                                None,
                                true,
                            )
                            .context("failed to resolve pipe destination")
                            .non_fatal();
                        },
                        None => {
                            // no specific destination, send to all plugins
                            pipe_to_all_plugins(
                                PipeSource::Keybind,
                                &name,
                                &payload,
                                &args,
                                &mut plugin_bridge,
                                &mut pipe_messages,
                            );
                        },
                    }
                }
                plugin_bridge.pipe_messages(
                    pipe_messages,
                    shutdown_send.clone(),
                    notification_end,
                )?;
            },
            PluginInstruction::CachePluginEvents { plugin_id } => {
                plugin_bridge.cache_plugin_events(plugin_id);
            },
            PluginInstruction::MessageFromPlugin {
                source_plugin_id,
                message,
            } => {
                let mut pipe_messages = vec![];
                let skip_cache = message
                    .new_plugin_args
                    .as_ref()
                    .map(|n| n.skip_cache)
                    .unwrap_or(false);
                let should_float = message
                    .new_plugin_args
                    .as_ref()
                    .and_then(|n| n.should_float)
                    .unwrap_or(true);
                let pane_title = message
                    .new_plugin_args
                    .as_ref()
                    .and_then(|n| n.pane_title.clone());
                let pane_id_to_replace = message
                    .new_plugin_args
                    .as_ref()
                    .and_then(|n| n.pane_id_to_replace);
                let floating_pane_coordinates = message.floating_pane_coordinates;
                match (message.plugin_url, message.destination_plugin_id) {
                    (Some(plugin_url), None) => {
                        // send to specific plugin(s)
                        pipe_to_specific_plugins(
                            PipeSource::Plugin(source_plugin_id),
                            &plugin_url,
                            &Some(message.plugin_config),
                            &None,
                            skip_cache,
                            should_float,
                            false,
                            &pane_id_to_replace,
                            &pane_title,
                            None,
                            &mut pipe_messages,
                            &message.message_name,
                            &message.message_payload,
                            &Some(message.message_args),
                            &bus,
                            &mut plugin_bridge,
                            &plugin_aliases,
                            floating_pane_coordinates,
                            message.new_plugin_args.and_then(|n| n.should_focus),
                            true,
                        )
                        .context("failed to resolve pipe destination")
                        .non_fatal();
                    },
                    (None, Some(destination_plugin_id)) => {
                        let is_private = true;
                        pipe_messages.push((
                            Some(destination_plugin_id),
                            None,
                            PipeMessage::new(
                                PipeSource::Plugin(source_plugin_id),
                                message.message_name,
                                &message.message_payload,
                                &Some(message.message_args),
                                is_private,
                            ),
                        ));
                    },
                    (Some(plugin_url), Some(destination_plugin_id)) => {
                        log::warn!("Message contains both a destination plugin url: {plugin_url} and a destination plugin id: {destination_plugin_id}, ignoring the url and prioritizing the id");
                        let is_private = true;
                        pipe_messages.push((
                            Some(destination_plugin_id),
                            None,
                            PipeMessage::new(
                                PipeSource::Plugin(source_plugin_id),
                                message.message_name,
                                &message.message_payload,
                                &Some(message.message_args),
                                is_private,
                            ),
                        ));
                    },
                    (None, None) => {
                        // send to all plugins
                        pipe_to_all_plugins(
                            PipeSource::Plugin(source_plugin_id),
                            &message.message_name,
                            &message.message_payload,
                            &Some(message.message_args),
                            &mut plugin_bridge,
                            &mut pipe_messages,
                        );
                    },
                }
                plugin_bridge.pipe_messages(pipe_messages, shutdown_send.clone(), None)?;
            },
            PluginInstruction::PipeCompleted(completion) => {
                let outcomes = plugin_bridge.update_cli_pipe_state(vec![completion]);
                plugin_bridge.send_pipe_outcomes(outcomes);
            },
            PluginInstruction::ReleasePluginInstancePipes(plugin_id, instance_id) => {
                plugin_bridge.cancel_permission_requests(plugin_id, Some(instance_id));
                let outcomes = plugin_bridge.release_plugin_instance_pipes(plugin_id, instance_id);
                plugin_bridge.send_pipe_outcomes(outcomes);
            },
            PluginInstruction::Reconfigure {
                plugin_aliases: new_aliases,
                client_id,
                keybinds,
                default_mode,
                default_shell,
                layout_dir,
                user_plugin_dir,
                hooks: new_hooks,
                context_menu: new_context_menu,
                was_written_to_disk,
            } => {
                // Wholesale rather than merged: `Hooks` and `ContextMenu` are both "what the
                // configuration says", and the config layer has already done the merging. A hook
                // bound here that is not loaded is not a problem - `run_hook` loads on demand, and
                // `request_context_menu` answers that one click from the configuration while it
                // starts.
                let notification_binding_changed = hooks.get(HookPoint::Notification)
                    != new_hooks.get(HookPoint::Notification)
                    || plugin_aliases != new_aliases;
                plugin_aliases = new_aliases;
                *plugin_bridge.plugin_aliases.lock().unwrap() = plugin_aliases.clone();
                if notification_binding_changed
                    && (hooks.get(HookPoint::Notification).is_some()
                        || new_hooks.get(HookPoint::Notification).is_some())
                {
                    if let Some(plugin_id) = notification_policy.take() {
                        plugin_bridge.unload_plugin(plugin_id).non_fatal();
                    }
                    notification_policy = load_notification_policy(
                        &new_hooks,
                        client_id,
                        &mut plugin_bridge,
                        &bus,
                        &plugin_aliases,
                    );
                }
                hooks = new_hooks;
                context_menu = new_context_menu;
                plugin_bridge
                    .reconfigure(
                        client_id,
                        keybinds,
                        default_mode,
                        default_shell,
                        layout_dir,
                        user_plugin_dir,
                    )
                    .non_fatal();
                // reconfigure resends InitialKeybinds; the screen also sends ModeUpdate.
                if was_written_to_disk {
                    let updates = vec![(None, None, Event::ConfigWasWrittenToDisk)];
                    plugin_bridge
                        .update_plugins(updates, shutdown_send.clone())
                        .non_fatal();
                }
            },
            PluginInstruction::FailedToWriteConfigToDisk { file_path, reason } => {
                let updates = vec![(
                    None,
                    None,
                    Event::FailedToWriteConfigToDisk {
                        file_path: file_path.map(|f| f.display().to_string()),
                        reason,
                    },
                )];
                plugin_bridge
                    .update_plugins(updates, shutdown_send.clone())
                    .non_fatal();
            },
            PluginInstruction::ChangePluginHostDir(
                new_host_folder,
                plugin_id,
                client_id,
                generation,
            ) => {
                plugin_bridge
                    .change_plugin_host_dir(new_host_folder, plugin_id, client_id, generation)
                    .non_fatal();
            },
            PluginInstruction::PaneRenderReport(pane_render_report) => {
                plugin_bridge
                    .handle_pane_render_report(pane_render_report, shutdown_send.clone())
                    .non_fatal();
            },
            PluginInstruction::UserInput {
                client_id,
                action,
                terminal_id,
                cli_client_id,
            } => {
                // Fire Event::UserAction to all subscribed plugins with InterceptInput permission
                let updates = vec![(
                    None,
                    None,
                    Event::UserAction(action, client_id, terminal_id, cli_client_id),
                )];
                plugin_bridge.update_plugins(updates, shutdown_send.clone())?;
            },
            PluginInstruction::LayoutListUpdate(layouts, errors) => {
                plugin_bridge.update_available_layouts(layouts, errors);
            },
            PluginInstruction::RequestStateUpdateForPlugin(plugin_id) => {
                plugin_bridge.state_update_for_plugin(plugin_id);
            },
            PluginInstruction::UpdateSessionSaveTime(timestamp_millis) => {
                // Store timestamp in PluginBridge (as Unix epoch for internal use)
                *plugin_bridge.last_session_save_time.lock().unwrap() = Some(timestamp_millis);
            },
            PluginInstruction::Pick {
                plugin_id,
                instance_id,
                generation,
                client_id,
                cwd,
                focused_plugin,
                command,
                response,
            } => {
                let started = match &command {
                    yinz_types::pick::PickCommand::Start { request_id, .. } => {
                        Some(request_id.clone())
                    },
                    _ => None,
                };
                let result = plugin_bridge
                    .pick_cwd(focused_plugin, client_id, cwd)
                    .and_then(|cwd| {
                        plugin_bridge.pick_command(
                            plugin_id,
                            instance_id,
                            generation,
                            client_id,
                            cwd,
                            command,
                        )
                    });
                if let (Some(request_id), Err(error)) = (started, &result) {
                    bus.senders
                        .send_to_plugin(PluginInstruction::for_plugin_event(
                            plugin_id,
                            client_id,
                            generation,
                            Event::PickResult(yinz_types::pick::PickResult {
                                request_id,
                                result: Err(error.clone()),
                            }),
                        ))
                        .context("deliver picker start failure")
                        .non_fatal();
                }
                let _ = response.send(result);
            },
            PluginInstruction::PickRetired(finder) => plugin_bridge.pick_retired(finder),
            PluginInstruction::OpenPluginPaneInPlaceOfPaneId {
                pane_id,
                plugin_url,
                configuration,
                cwd,
                tab_id,
                client_id,
                completion,
            } => {
                let mut completion = Some(completion);
                let loaded = load_plugin_in_place_of(
                    &mut plugin_bridge,
                    &plugin_aliases,
                    &plugin_url,
                    configuration,
                    cwd,
                    tab_id,
                    client_id,
                    |run, cwd| {
                        // Lent: closing the plugin's pane gives the slot back. A plugin that
                        // declares no pane runs headless and leaves the target where it is.
                        PaneRequest::Show(Box::new(ShowPane {
                            should_float: Some(false),
                            in_place: true,
                            replaced: ReplacedPane::Lend,
                            run,
                            title: None,
                            tab: Some(tab_id),
                            pane_id_to_replace: Some(pane_id),
                            cwd,
                            coordinates: None,
                            should_focus: Some(false),
                            client_id: Some(client_id),
                            completion: completion.take(),
                            overlay: None,
                            reserved_slot: None,
                        }))
                    },
                );
                // Answered by the load when it got that far.
                if let (Err(error), Some(mut completion)) = (loaded, completion) {
                    completion.set_exit_status(1);
                    completion.set_error_message(format!("{error:#}"));
                }
            },
            PluginInstruction::GetPluginIdsForUrl {
                url,
                configuration,
                response_channel,
            } => {
                // The matcher reads cached metadata, never a guest runtime lock.
                let _ = response_channel.send(
                    plugin_bridge.get_plugin_ids_for_url_with_configuration(&url, configuration),
                );
            },
            PluginInstruction::GetLastSessionSaveTime { response_channel } => {
                let timestamp = *plugin_bridge.last_session_save_time.lock().unwrap();
                let _ = response_channel.send(timestamp);
            },
            PluginInstruction::GrantPermissionsToLocation {
                plugin_url,
                permissions,
            } => {
                plugin_bridge
                    .grant_permissions_to_location(&plugin_url, permissions, shutdown_send.clone())
                    .non_fatal();
            },
            PluginInstruction::PluginFailed {
                load_id,
                plugin_id,
                generation,
                instance_id,
                loading_client,
                error,
                failed_to_load,
            } => {
                if let Some(client) = loading_client.filter(|_| {
                    load_id.is_none_or(|id| plugin_bridge.load_is_current(plugin_id, id))
                        && !plugin_bridge.pipe_has_recipient(plugin_id)
                }) {
                    bus.senders
                        .send_to_server(ServerInstruction::LogError(
                            vec![error.clone()],
                            client,
                            None,
                        ))
                        .context("failed to report plugin loading error")
                        .non_fatal();
                }
                plugin_bridge
                    .plugin_failed(
                        plugin_id,
                        load_id,
                        generation,
                        instance_id,
                        error,
                        failed_to_load,
                    )
                    .non_fatal();
            },
            PluginInstruction::DetectPluginConfigChanges(new_plugins) => {
                plugin_bridge
                    .detect_and_notify_plugin_config_changes(&new_plugins, shutdown_send.clone())?;
            },
            PluginInstruction::HighlightClicked {
                plugin_id,
                client_id,
                pane_id,
                pattern,
                matched_string,
                context,
            } => {
                let event = Event::HighlightClicked {
                    pane_id,
                    pattern,
                    matched_string,
                    context,
                };
                let updates = vec![(Some(plugin_id), Some(client_id), event)];
                plugin_bridge.update_plugins(updates, shutdown_send.clone())?;
            },
            PluginInstruction::Exit => {
                break;
            },
        }
    }
    info!("Plugin main thread exits");

    // first drop our sender, then call recv.
    // once all senders are dropped or the timeout is reached, recv will return an error, that we ignore
    //
    // One deadline, `EXIT_TIMEOUT` from now, for this wait and the cleanup after it. The thread
    // has `PLUGIN_THREAD_EXIT_TIMEOUT` in all before its join is abandoned, and a separate
    // budget for each step used to add up to exactly that: the join gave up on a thread about
    // to exit.
    //
    // An in-flight job is what holds a `shutdown_send` clone. It gets `EXIT_GRACE` to finish
    // on its own — a job that is merely in progress when the exit arrives (the plugin whose
    // key sent the quit, say) has its work to complete, and must not be trapped mid-way — and
    // only one that has not finished by then is a runaway, which is cancelled and given the
    // same again to reach its next check of the token. Either way at least `EXIT_GRACE` of the
    // budget is left for `cleanup`.
    let deadline = Instant::now() + EXIT_TIMEOUT;
    drop(shutdown_send);
    let runtime = yinz_bus::global_async_runtime::get_tokio_runtime();
    runtime.block_on(async {
        if tokio::time::timeout(EXIT_GRACE, shutdown_receive.recv())
            .await
            .is_ok()
        {
            return;
        }
        log::warn!(
            "plugin jobs still running {EXIT_GRACE:?} after the exit was requested; cancelling \
             them"
        );
        plugin_bridge.cancel_all_plugins();
        if let Err(err) = tokio::time::timeout(EXIT_GRACE, shutdown_receive.recv()).await {
            log::error!("timeout waiting for plugin tasks to finish: {}", err);
        }
    });

    plugin_bridge.cleanup(deadline);
    plugin_bridge.wait_for_cleanup(deadline);

    Ok(())
}

fn populate_session_layout_metadata(
    session_layout_metadata: &mut SessionLayoutMetadata,
    plugin_bridge: &PluginBridge,
    plugin_aliases: &PluginAliases,
    exclude_plugin_id: Option<PluginId>,
    trace: Option<&yinz_bus::plugins::LayoutQueryTrace>,
) {
    // Remove the requesting plugin from the layout to prevent deadlock
    if let Some(plugin_id) = exclude_plugin_id {
        session_layout_metadata.remove_plugin_from_layout(plugin_id);
    }

    let plugin_ids = session_layout_metadata.all_plugin_ids();
    let mut plugin_ids_to_cmds: HashMap<PluginId, RunPlugin> = HashMap::new();
    for plugin_id in plugin_ids {
        if let Some(trace) = trace {
            trace.mark("plugin map lock");
        }
        let plugin_map = plugin_bridge.plugin_map.lock().unwrap();
        if let Some(trace) = trace {
            trace.mark("plugin map metadata");
        }
        let plugin_cmd = plugin_map.run_plugin_of_plugin_id(plugin_id);
        drop(plugin_map);
        match plugin_cmd {
            Some(plugin_cmd) => {
                plugin_ids_to_cmds.insert(plugin_id, plugin_cmd.clone());
            },
            None => log::error!("Plugin with id: {plugin_id} not found"),
        }
    }
    session_layout_metadata.update_plugin_cmds(plugin_ids_to_cmds);
    session_layout_metadata.update_plugin_aliases_in_default_layout(plugin_aliases);
}

/// Which client a clicked toast is addressed to.
///
/// The one client id an update carries is read two ways, and which of them matters depends on
/// whether the plugin is shared - so no single choice is right for both and the caller hands over
/// each candidate rather than picking.
///
/// A shared plugin is one instance for the whole session, which every client reaches: the id is
/// not consulted for delivery at all, and becomes the *acting* client instead. The client who
/// should act is the one who clicked - it is their pane the reveal has to land in, and the host
/// has already focused it for them.
///
/// A per-client plugin has an instance per client, and only the raiser's knows what the toast was
/// about; naming anybody else drops the click on the floor, silently, which is what this is here
/// to stop. The acting client there is that instance's own whatever we pass, so the raiser costs
/// the clicker nothing.
fn client_for_notification_click(
    plugin_is_shared: bool,
    raiser: ClientId,
    clicker: ClientId,
) -> ClientId {
    if plugin_is_shared {
        clicker
    } else {
        raiser
    }
}

fn pipe_to_all_plugins(
    pipe_source: PipeSource,
    name: &str,
    payload: &Option<String>,
    args: &Option<BTreeMap<String, String>>,
    _plugin_bridge: &mut PluginBridge,
    pipe_messages: &mut Vec<(Option<PluginId>, Option<ClientId>, PipeMessage)>,
) {
    let is_private = false;
    pipe_messages.push((
        None,
        None,
        PipeMessage::new(pipe_source, name, payload, args, is_private),
    ));
}

fn pipe_to_specific_plugins(
    pipe_source: PipeSource,
    plugin_url: &str,
    configuration: &Option<BTreeMap<String, String>>,
    cwd: &Option<PathBuf>,
    skip_cache: bool,
    should_float: bool,
    should_open_in_place: bool,
    pane_id_to_replace: &Option<PaneId>,
    pane_title: &Option<String>,
    cli_client_id: Option<ClientId>,
    pipe_messages: &mut Vec<(Option<PluginId>, Option<ClientId>, PipeMessage)>,
    name: &str,
    payload: &Option<String>,
    args: &Option<BTreeMap<String, String>>,
    _bus: &Bus<PluginInstruction>,
    plugin_bridge: &mut PluginBridge,
    plugin_aliases: &PluginAliases,
    floating_pane_coordinates: Option<FloatingPaneCoordinates>,
    should_focus: Option<bool>,
    allow_load: bool,
) -> Result<()> {
    let is_private = true;
    let size = Size::default();
    match yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        plugin_url,
        configuration,
        Some(plugin_aliases),
        cwd.clone(),
    ) {
        Ok(run_plugin_or_alias) => {
            let initial_cwd = run_plugin_or_alias.get_initial_cwd();
            let all_plugin_ids = plugin_bridge.get_or_load_plugins(
                run_plugin_or_alias,
                size,
                initial_cwd.or_else(|| cwd.clone()),
                skip_cache,
                should_float,
                should_open_in_place || pane_id_to_replace.is_some(),
                pane_title.clone(),
                *pane_id_to_replace,
                cli_client_id,
                floating_pane_coordinates,
                should_focus.unwrap_or(false),
                allow_load,
            )?;
            for (plugin_id, client_id) in all_plugin_ids {
                pipe_messages.push((
                    Some(plugin_id),
                    client_id,
                    PipeMessage::new(pipe_source.clone(), name, payload, args, is_private),
                ));
            }
        },
        Err(error) => return Err(anyhow!("Failed to parse plugin url: {error}")),
    }
    Ok(())
}

/// Notification policies always own a dedicated shared headless instance, even if their
/// entrypoint is also used by a visible plugin or another hook.
fn load_notification_policy(
    hooks: &Hooks,
    client_id: ClientId,
    bridge: &mut PluginBridge,
    bus: &Bus<PluginInstruction>,
    aliases: &PluginAliases,
) -> Option<PluginId> {
    let policy = hooks.get(HookPoint::Notification).and_then(|binding| {
        let mut binding = binding.clone();
        binding.populate_run_plugin_if_needed(aliases);
        let mut plugin = binding.get_run_plugin()?;
        plugin.configuration.insert("shared", "true");
        plugin
            .configuration
            .insert("__yinz_notification_policy", "true");
        load_background_plugin(
            RunPluginOrAlias::RunPlugin(plugin),
            bridge,
            bus,
            aliases,
            client_id,
        )
        .map(|(id, _)| id)
    });
    bus.senders
        .send_to_screen(ScreenInstruction::NotificationPolicy(
            policy,
            (hooks.get(HookPoint::Notification).is_some() && policy.is_none())
                .then(|| "policy_load_failed".into()),
        ))
        .context("assign notification policy")
        .non_fatal();
    policy
}

/// What the `session_start` hook is told: enough to address the session from a plugin that was
/// loaded before any tab existed.
fn session_start_payload(session: &ServerSession, client_id: ClientId) -> HookFiring {
    HookFiring::Headless(HookPayload::SessionStart {
        session_name: session.current(),
        client_id,
    })
}

/// Run the plugin bound to `point`, in the point's shape.
///
/// A `ReplacePane` point opens the plugin in place of `pane_id`. The hook's payload becomes the
/// plugin's configuration. Record (hook point, pane id) at launch so later firings reach that
/// instance as `Event::Hook`, even when the payload changes. Different panes get separate
/// instances; loading instances queue events, and unloading releases the association.
///
/// Loaded and placed here rather than through `PluginBridge::get_or_load_plugins`: the
/// latter opens a pane only when it has to *load* the plugin, and answers a request for one
/// that already exists with its id and no
/// screen instruction at all. That is the right shape for "open it, or focus the one already
/// open". It is the wrong shape for a hook, whose whole contract is that a pane appears - a
/// hook that quietly resolved to an existing instance would leave the pane it was fired for
/// showing nothing but its exit code, which is exactly the failure it exists to replace.
///
/// A `Headless` point uses one long-lived instance found by its configured identity and receives
/// every firing as `Event::Hook`; see [`deliver_to_headless_hook`].
#[allow(clippy::too_many_arguments)]
fn run_hook(
    point: HookPoint,
    pane_id: Option<PaneId>,
    payload: HookFiring,
    client_id: ClientId,
    hooks: &Hooks,
    plugin_bridge: &mut PluginBridge,
    bus: &Bus<PluginInstruction>,
    plugin_aliases: &PluginAliases,
    shutdown_send: &tokio::sync::mpsc::Sender<()>,
) {
    let Some(run_plugin_or_alias) = hooks.get(point) else {
        // An unbound hook point. Every hook site fires unconditionally so that "is this
        // configured" is answered here and not at each of them.
        log::info!("hook '{point}' is not bound to a plugin ({pane_id:?})");
        return;
    };
    // One line per hook, because until now every way this could come to nothing was silent:
    // an unmarked pane never reaches here at all, an unbound point returns above, and a
    // plugin that will not load says so below. Which of those happened is the whole of
    // diagnosing "the hook screen did not appear".
    log::info!(
        "running hook '{point}' for {pane_id:?}: {}",
        run_plugin_or_alias.location_string()
    );
    let payload = match (point.shape(), payload) {
        (HookShape::Headless, HookFiring::Headless(payload)) => {
            deliver_to_headless_hook(
                point,
                run_plugin_or_alias,
                client_id,
                Event::Hook { point, payload },
                plugin_bridge,
                bus,
                plugin_aliases,
                shutdown_send,
            );
            return;
        },
        (HookShape::ReplacePane, HookFiring::ReplacePane(payload)) => payload,
        (shape, payload) => {
            log::error!("hook '{point}' has the {shape:?} shape and was fired with {payload:?}");
            return;
        },
    };
    let Some(pane_id) = pane_id else {
        log::error!("hook '{point}' replaces a pane, and was fired without one");
        return;
    };
    if let Some(plugin_id) = plugin_bridge.pane_hook_instance(point, pane_id) {
        let client_id = if !plugin_bridge.plugin_is_shared(&plugin_id) {
            None
        } else if plugin_bridge.client_is_connected(&client_id) {
            Some(client_id)
        } else {
            Some(plugin_bridge.get_first_client_id().unwrap_or(client_id))
        };
        plugin_bridge
            .update_plugins(
                vec![(
                    Some(plugin_id),
                    client_id,
                    Event::Hook {
                        point,
                        payload: HookPayload::AgentExit(payload),
                    },
                )],
                shutdown_send.clone(),
            )
            .context("delivering a pane hook re-entry")
            .non_fatal();
        return;
    }
    let mut run_plugin_or_alias = run_plugin_or_alias.clone();
    run_plugin_or_alias.populate_run_plugin_if_needed(plugin_aliases);
    let cwd = run_plugin_or_alias.get_initial_cwd();
    let Some(run_plugin) = run_plugin_or_alias.get_run_plugin() else {
        log::error!("hook '{point}' names a plugin that could not be resolved");
        return;
    };
    let agent_pane_title = match point {
        HookPoint::AgentExit => payload.get("agent_pane_title").cloned(),
        _ => None,
    };
    // Merged rather than replaced: a hook declared with its own configuration block keeps it,
    // and the payload wins where the two name the same key.
    let run_plugin = run_plugin.merge_configuration(&Some(payload));
    let skip_cache = false;
    // The id that addresses the pane the user is looking at, which is the hook pane in front
    // rather than the terminal suppressed behind it. A terminal frame leads with its own id
    // (`TerminalPane::framed_title`) because that is how the pane is named from outside; a
    // plugin frame does not label itself at all, so the id is written into the name here - and
    // it is this plugin pane's, or it names something else.
    let pane_title = agent_pane_title.map(|title| {
        format!(
            "[{}] {title}",
            PaneId::Plugin(plugin_bridge.next_plugin_id())
        )
    });
    // A hook plugin that declares no pane runs headless, leaving the agent's pane alone; its
    // re-entries still reach it through `pane_hooks`.
    let pane = PaneRequest::Show(Box::new(ShowPane::in_place_of_hooked_pane(
        RunPluginOrAlias::RunPlugin(run_plugin.clone()),
        pane_id,
        pane_title,
        cwd.clone(),
        client_id,
    )));
    match plugin_bridge.load_plugin(
        &Some(run_plugin),
        None,
        Size::default(),
        cwd,
        skip_cache,
        Some(client_id),
        pane,
    ) {
        Ok((plugin_id, _)) => {
            plugin_bridge.pane_hooks.insert((point, pane_id), plugin_id);
        },
        Err(e) => {
            log::error!("hook '{point}' could not load its plugin: {e}");
        },
    }
}

/// Ask the `context_menu` hook plugin what a menu should say, and make sure the screen hears
/// something either way.
///
/// Two cases:
///
/// 1. **No hook bound, or one that cannot be resolved.** The configured items are sent straight
///    back. The screen is holding an empty menu open waiting to hear, so staying silent here -
///    which is what `run_hook` does for an unbound point - would leave the menu resolved only by
///    its timeout.
/// 2. **A hook bound.** It gets `Event::ContextMenuRequested` through
///    [`deliver_to_headless_hook`] - loaded first if this is the first right-click this client
///    has made - and answers with `set_context_menu` when it is ready. The timeout still provides
///    the configured fallback if it is slow.
#[allow(clippy::too_many_arguments)]
fn request_context_menu(
    context: ContextMenuContext,
    client_id: ClientId,
    hooks: &Hooks,
    context_menu: &ContextMenu,
    plugin_bridge: &mut PluginBridge,
    plugin_aliases: &PluginAliases,
    bus: &Bus<PluginInstruction>,
    shutdown_send: &tokio::sync::mpsc::Sender<()>,
) {
    let answer_with_configured_items = || {
        drop(
            bus.senders
                .send_to_screen(ScreenInstruction::SetContextMenu {
                    request_id: context.request_id,
                    items: context_menu.items().to_vec(),
                    // No plugin answered, so there is no plugin an item's context could go back to.
                    plugin_id: None,
                    client_id,
                }),
        );
    };

    let Some(run_plugin_or_alias) = hooks.get(HookPoint::ContextMenu) else {
        answer_with_configured_items();
        return;
    };
    // The context already carries the items, stripped of their actions and carrying their ids -
    // the screen put them there when it opened the menu, so that what the plugin is shown and what
    // the screen re-attaches against cannot drift.
    let delivered = deliver_to_headless_hook(
        HookPoint::ContextMenu,
        run_plugin_or_alias,
        client_id,
        Event::ContextMenuRequested(Box::new(context.clone())),
        plugin_bridge,
        bus,
        plugin_aliases,
        shutdown_send,
    );
    if !delivered {
        answer_with_configured_items();
    }
}

/// Hand `event` to the instance of the headless hook plugin bound to `point` that `client_id`
/// can reach, loading it - suppressed, with no pane on screen - the first time. Answers whether
/// anything could be addressed: false means the plugin did not resolve or did not load.
///
/// The instance is found by the plugin's *configured* identity, never by the payload: that is
/// what makes one long-lived instance receive every firing, and it is the deliberate opposite
/// of `run_hook`'s pane-replacing shape, where the first payload is merged into the configuration
/// and each pane has its own instance.
///
/// Loaded lazily rather than at session start, because every hook point is opt-in: loading a
/// plugin into every session for a point most of them never bind would be a cost paid by
/// everyone for the benefit of a few. An event sent to a plugin that is still loading is held
/// by the bridge and delivered once the load completes.
#[allow(clippy::too_many_arguments)]
fn deliver_to_headless_hook(
    point: HookPoint,
    run_plugin_or_alias: &RunPluginOrAlias,
    client_id: ClientId,
    event: Event,
    plugin_bridge: &mut PluginBridge,
    bus: &Bus<PluginInstruction>,
    plugin_aliases: &PluginAliases,
    shutdown_send: &tokio::sync::mpsc::Sender<()>,
) -> bool {
    let mut run_plugin_or_alias = run_plugin_or_alias.clone();
    run_plugin_or_alias.populate_run_plugin_if_needed(plugin_aliases);
    let Some(run_plugin) = run_plugin_or_alias.get_run_plugin() else {
        log::error!("hook '{point}' names a plugin that could not be resolved");
        return false;
    };
    let is_shared = PluginConfig::from_run_plugin(&run_plugin)
        .map(|plugin| plugin.is_shared())
        .unwrap_or(false);
    let plugin_ids = plugin_bridge.all_plugin_and_client_ids_for_plugin_location(
        &run_plugin.location,
        &run_plugin.configuration,
    );
    // A non-shared hook has one instance per client, so an instance existing for some other
    // client says nothing about this one - only an instance this client owns (or a shared
    // instance, which every client can reach) counts. A shared hook still has to have been loaded
    // once: "every client can reach it" says nothing when there is nothing to reach, and taking
    // `is_shared` alone as proof of an instance would skip the load below and then dispatch to an
    // empty list, leaving the firing undelivered.
    let has_own_instance = !plugin_ids.is_empty()
        && (is_shared || plugin_ids.iter().any(|(_, cid)| *cid == Some(client_id)));
    if !has_own_instance {
        // A load already in flight is the instance: the bridge holds an event addressed to a
        // loading plugin and applies it, when the load completes, to every connected client's
        // instance - the loader provisions one per connected client, so a connected client is
        // served by the load in flight (and a shared hook serves everyone). Loading again here
        // split one hook's firings across two plugins whenever a second point fired within the
        // first load's time - `agent_start` and `agent_waiting` a hundred milliseconds apart
        // was enough.
        let in_flight = plugin_bridge.loading_plugin_ids_for_plugin_location(
            &run_plugin.location,
            &run_plugin.configuration,
        );
        if let Some(plugin_id) = in_flight
            .first()
            .filter(|_| is_shared || plugin_bridge.client_is_connected(&client_id))
        {
            plugin_bridge
                .update_plugins(
                    vec![(Some(*plugin_id), Some(client_id), event)],
                    shutdown_send.clone(),
                )
                .non_fatal();
            return true;
        }
        let Some((plugin_id, loaded_client_id)) = load_background_plugin(
            run_plugin_or_alias.clone(),
            plugin_bridge,
            bus,
            plugin_aliases,
            client_id,
        ) else {
            return false;
        };
        plugin_bridge
            .update_plugins(
                vec![(Some(plugin_id), Some(loaded_client_id), event)],
                shutdown_send.clone(),
            )
            .non_fatal();
        return true;
    }

    let mut seen_plugin_ids = HashSet::new();
    let updates: Vec<_> = plugin_ids
        .into_iter()
        .filter(|(_, cid)| is_shared || *cid == Some(client_id))
        .filter_map(|(plugin_id, _)| {
            seen_plugin_ids.insert(plugin_id).then_some((
                Some(plugin_id),
                Some(client_id),
                event.clone(),
            ))
        })
        .collect();
    plugin_bridge
        .update_plugins(updates, shutdown_send.clone())
        .non_fatal();
    true
}

/// How a plugin pane load runs, beside the pane it asks for.
struct PaneLoad {
    size: Size,
    focused_plugin_id: Option<PluginId>,
    skip_cache: bool,
}

/// Load the plugin a `Load` or `LoadIntoSlot` names, asking for `show`. Its run is resolved
/// here, and its directory found, before the request is made.
fn load_and_show(
    mut show: ShowPane,
    load: PaneLoad,
    client_id: ClientId,
    plugin_bridge: &mut PluginBridge,
    bus: &Bus<PluginInstruction>,
    plugin_aliases: &PluginAliases,
    yinz_cwd: &std::path::Path,
) {
    let PaneLoad {
        size,
        focused_plugin_id,
        skip_cache,
    } = load;
    show.run.populate_run_plugin_if_needed(plugin_aliases);
    let cwd = show.run.get_initial_cwd().or(show.cwd).or_else(|| {
        if let Some(plugin_id) = focused_plugin_id {
            plugin_bridge.get_plugin_cwd(plugin_id, client_id)
        } else {
            None
        }
    });
    show.cwd = cwd.clone();
    let run_plugin = show.run.get_run_plugin();
    let tab_index = show.tab;
    match plugin_bridge.load_plugin(
        &run_plugin,
        tab_index,
        size,
        cwd.clone(),
        skip_cache,
        Some(client_id),
        PaneRequest::Show(Box::new(show)),
    ) {
        Ok((plugin_id, _)) => {
            drop(bus.senders.send_to_pty(PtyInstruction::ReportPluginCwd(
                plugin_id,
                cwd.unwrap_or_else(|| yinz_cwd.to_path_buf()),
            )));
        },
        Err(e) => {
            log::error!("Failed to load plugin: {e}");
        },
    }
}

fn load_background_plugin(
    run_plugin_or_alias: RunPluginOrAlias,
    plugin_bridge: &mut PluginBridge,
    _bus: &Bus<PluginInstruction>,
    _plugin_aliases: &PluginAliases,
    client_id: ClientId,
) -> Option<(PluginId, ClientId)> {
    plugin_bridge
        .start_background_plugin(run_plugin_or_alias, client_id, true)
        .map_err(|error| log::error!("Failed to start background plugin: {error:#}"))
        .ok()
}

/// Load `plugin_url` for a pane that will open in place of another one, refusing a url and
/// configuration that already has an instance: that instance has a pane of its own already,
/// and loading it again is the silent no-op this exists to replace.
///
/// `pane` builds the request from the resolved plugin and its directory, and is called only
/// once the load is going ahead.
pub(crate) fn load_plugin_in_place_of(
    plugin_bridge: &mut PluginBridge,
    plugin_aliases: &PluginAliases,
    plugin_url: &str,
    configuration: BTreeMap<String, String>,
    cwd: PathBuf,
    tab_id: yinz_types::internal_ids::TabId,
    client_id: ClientId,
    pane: impl FnOnce(RunPluginOrAlias, Option<PathBuf>) -> PaneRequest,
) -> Result<PluginId> {
    let mut run_plugin_or_alias = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        plugin_url,
        &Some(configuration),
        Some(plugin_aliases),
        Some(cwd.clone()),
    )
    .map_err(|error| anyhow!("Failed to parse plugin url: {error}"))?;
    run_plugin_or_alias.populate_run_plugin_if_needed(plugin_aliases);
    let run_plugin = run_plugin_or_alias
        .get_run_plugin()
        .with_context(|| format!("{plugin_url} is not a plugin or a known alias"))?;
    if plugin_bridge.plugin_instance_exists(&run_plugin) {
        bail!(
            "an instance of {plugin_url} with this configuration already exists; \
             focus or close it instead, or open one with a different configuration"
        );
    }
    let cwd = run_plugin_or_alias.get_initial_cwd().or(Some(cwd));
    let pane = pane(run_plugin_or_alias, cwd.clone());
    let (plugin_id, _) = plugin_bridge.load_plugin(
        &Some(run_plugin),
        Some(tab_id),
        Size::default(),
        cwd,
        false,
        Some(client_id),
        pane,
    )?;
    Ok(plugin_id)
}

/// The plugin thread's whole budget for exiting: the wait for in-flight jobs and the unloads
/// after it. Must leave room inside `PLUGIN_THREAD_EXIT_TIMEOUT`, which is how long the server
/// waits for the thread before leaving it behind.
const EXIT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long an in-flight job gets to finish on its own at exit before it is cancelled; the
/// same again is then allowed for the cancel to take. Three of these fit in `EXIT_TIMEOUT`,
/// the third being the unloads.
const EXIT_GRACE: Duration = Duration::from_secs(1);

/// How many queued `Update` messages one pass merges before letting the other instruction
/// kinds through. A wheel flood is a few hundred messages at most; anything past that is
/// starving a `Load`, a pipe or `Exit` waiting behind it.
const MAX_UPDATE_DRAIN: usize = 256;

type PluginUpdate = (Option<PluginId>, Option<ClientId>, Event);

/// Keep tagged callbacks adjacent and ordered, including their acting client identities.
fn drain_plugin_events(
    bus: &Bus<PluginInstruction>,
    plugin_id: PluginId,
    generation: u64,
    mut events: Vec<(ClientId, Event, Option<yinz_bus::thread_bus::PluginReplyId>)>,
    queued: &mut Option<(PluginInstruction, ErrorContext)>,
) -> Vec<(ClientId, Event, Option<yinz_bus::thread_bus::PluginReplyId>)> {
    for _ in 0..MAX_UPDATE_DRAIN {
        match bus.try_recv() {
            Some((PluginInstruction::PluginEvent(id, client, expected, event), _))
                if id == plugin_id && expected == generation =>
            {
                events.push((client, event, None));
            },
            Some((PluginInstruction::PluginReply(id, client, expected, event, reply), _))
                if id == plugin_id && expected == generation =>
            {
                events.push((client, event, Some(reply)));
            },
            Some(other) => {
                *queued = Some(other);
                break;
            },
            None => break,
        }
    }
    events
}

/// Merge the `Update`s already waiting on the bus into the one just taken, so a flood of wheel
/// notches costs one `update_plugins` call (and one render per plugin) instead of one each.
///
/// Only *adjacent* `Update`s merge, so nothing is reordered: the first message of another kind
/// stops the drain and is left in `queued` for the caller to handle next. Every event is kept
/// and delivered in order with its count — `tab-bar` and `strider` step once per event and
/// would misbehave if two `ScrollUp(3)`s became a `ScrollUp(6)`. The one exception is hovers,
/// see `coalesce_hovers`.
fn drain_updates(
    bus: &Bus<PluginInstruction>,
    mut updates: Vec<PluginUpdate>,
    queued: &mut Option<(PluginInstruction, ErrorContext)>,
) -> Vec<PluginUpdate> {
    let mut drained = 0;
    while drained < MAX_UPDATE_DRAIN {
        match bus.try_recv() {
            Some((PluginInstruction::Update(more), _)) => {
                updates.extend(more);
                drained += 1;
            },
            Some(other) => {
                *queued = Some(other);
                break;
            },
            None => break,
        }
    }
    coalesce_hovers(updates)
}

/// Collapse the identical reloads waiting behind `first` into it, the way `drain_updates`
/// merges `Update`s: adjacent only, and the first message that is not the same reload stops the
/// drain and is left in `queued`.
///
/// A reload is a full unload and load, on the plugin thread and under the global plugin map
/// lock, so it is the most expensive instruction there is — and a plugin can ask for one with a
/// single host call. One that asks in a loop (agent once did, thousands of times a second) fills
/// this bus with reloads faster than one can run, and every other plugin's events wait behind
/// them: the whole plugin subsystem reads as frozen. `pending_plugin_reloads` already folds
/// reloads that arrive *while* one is in flight into a single follow-up; this closes the gap
/// before that, where they queue up unseen.
///
/// "Identical" is every field but the completion handle: a `Reload` also says where the pane
/// goes if the plugin turns out not to be running, and two that differ there are two requests.
/// The collapsed reloads' completion handles come back to the caller, which holds them until the
/// surviving reload has run — dropping one is what answers the CLI waiting on it, and answering
/// before the reload ran would report a success that had not happened yet.
fn drain_reloads(
    bus: &Bus<PluginInstruction>,
    first: PluginInstruction,
    queued: &mut Option<(PluginInstruction, ErrorContext)>,
) -> (PluginInstruction, Vec<NotificationEnd>) {
    let mut completions = vec![];
    if reload_target(&first).is_none() {
        return (first, completions);
    }
    let mut collapsed = 0;
    while collapsed < MAX_UPDATE_DRAIN {
        match bus.try_recv() {
            Some((next, ctx)) => {
                if same_reload(&first, &next) {
                    collapsed += 1;
                    if let PluginInstruction::Reload(_, _, _, _, _, Some(completion)) = next {
                        completions.push(completion);
                    }
                } else {
                    *queued = Some((next, ctx));
                    break;
                }
            },
            None => break,
        }
    }
    if collapsed > 0 {
        log::warn!(
            "collapsed {collapsed} queued reload(s) of {} into one",
            reload_target(&first)
                .map(|target| target.location_string())
                .unwrap_or_default()
        );
    }
    (first, completions)
}

/// The plugin a reload instruction names, or `None` for any other instruction.
fn reload_target(instruction: &PluginInstruction) -> Option<&RunPluginOrAlias> {
    match instruction {
        PluginInstruction::Reload(_, _, run_plugin, _, _, _)
        | PluginInstruction::ReloadPluginByUrl(run_plugin) => Some(run_plugin),
        _ => None,
    }
}

/// Whether `b` asks for exactly what `a` does — same kind of reload, same plugin, and for a
/// pane reload the same placement — so that running `a` satisfies `b`. Completion handles
/// are not compared: they are who is waiting, not what for.
fn same_reload(a: &PluginInstruction, b: &PluginInstruction) -> bool {
    match (a, b) {
        (
            PluginInstruction::Reload(float_a, title_a, run_a, tab_a, size_a, _),
            PluginInstruction::Reload(float_b, title_b, run_b, tab_b, size_b, _),
        ) => {
            float_a == float_b
                && title_a == title_b
                && run_a == run_b
                && tab_a == tab_b
                && size_a == size_b
        },
        (
            PluginInstruction::ReloadPluginByUrl(run_a),
            PluginInstruction::ReloadPluginByUrl(run_b),
        ) => run_a == run_b,
        _ => false,
    }
}

/// Keep the newest hover per (plugin, client) between other events. A wheel consumes the
/// preceding position sample, so coalescing must not cross an input event.
fn coalesce_hovers(mut updates: Vec<PluginUpdate>) -> Vec<PluginUpdate> {
    if updates.len() < 2 {
        return updates;
    }
    let mut seen: HashSet<(Option<PluginId>, Option<ClientId>)> = HashSet::new();
    let mut keep = vec![true; updates.len()];
    for (index, (plugin_id, client_id, event)) in updates.iter().enumerate().rev() {
        if let Event::Mouse(Mouse::Hover(..)) = event {
            keep[index] = seen.insert((*plugin_id, *client_id));
        } else {
            seen.clear();
        }
    }
    let mut keep = keep.into_iter();
    updates.retain(|_| keep.next().unwrap_or(true));
    updates
}

#[path = "./unit/plugin_tests/mod.rs"]
#[cfg(test)]
mod plugin_tests;

#[cfg(test)]
mod test_env;

mod session_destinations;

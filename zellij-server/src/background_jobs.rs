#[allow(unused_imports)] // some imports used only with web_server_capability feature
use zellij_utils::consts::{
    session_info_cache_file_name, session_info_folder_for_session, session_layout_cache_file_name,
    VERSION, ZELLIJ_SESSION_INFO_CACHE_DIR, ZELLIJ_SOCK_DIR,
};
#[allow(unused_imports)]
use zellij_utils::data::{Event, HttpVerb, LayoutInfo, SessionInfo, WebServerStatus};
use zellij_utils::errors::{prelude::*, BackgroundJobContext, ContextType};
use zellij_utils::input::layout::RunPlugin;
#[allow(unused_imports)]
use zellij_utils::shared::parse_base_url;

#[cfg(feature = "web_server_capability")]
use zellij_utils::web_server_commands::{
    discover_webserver_sockets, query_webserver_with_response, InstructionForWebServer,
    WebServerResponse,
};

use futures_lite::io::AsyncReadExt;
use isahc::prelude::*;
use isahc::AsyncReadResponseExt;
use isahc::{config::RedirectPolicy, HttpClient, Request};

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use crate::panes::PaneId;
use crate::plugins::{PluginId, PluginInstruction};
use crate::pty::PtyInstruction;
use crate::screen::ScreenInstruction;
use crate::thread_bus::Bus;
use crate::{ClientId, ServerInstruction};

/// Identifies an in-flight streaming web request. Scoped to the plugin and client that started
/// it, so one plugin cannot cancel another's stream.
pub type StreamKey = (PluginId, ClientId, String);

struct ActiveWebRequestStream {
    abort_handle: tokio::task::AbortHandle,
    generation: u64,
    state: Arc<Mutex<WebRequestStreamState>>,
    context: BTreeMap<String, String>,
}

#[derive(Default)]
struct WebRequestStreamState {
    response_sent: bool,
    finished: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum BackgroundJob {
    DisplayPaneError(Vec<PaneId>, String),
    AnimatePluginLoading(u32),                            // u32 - plugin_id
    StopPluginLoadingAnimation(u32),                      // u32 - plugin_id
    ReportSessionInfo(String, SessionInfo),               // String - session name
    ReportPluginList(BTreeMap<PluginId, RunPlugin>),      // String - session name
    ReportLayoutInfo((String, BTreeMap<String, String>)), // BTreeMap<file_name, pane_contents>
    RunCommand(
        PluginId,
        ClientId,
        String,
        Vec<String>,
        BTreeMap<String, String>,
        PathBuf,
        BTreeMap<String, String>,
    ), // command, args, env_variables, cwd, context
    WebRequest(
        PluginId,
        ClientId,
        String, // url
        HttpVerb,
        BTreeMap<String, String>, // headers
        Vec<u8>,                  // body
        BTreeMap<String, String>, // context
        Option<String>,           // stream_id, when set the response body is streamed back
    ),
    CancelWebRequest(PluginId, ClientId, String), // String - stream_id
    /// Sent by a streaming request task back to this thread when its body ends, so that the
    /// terminating chunk is always emitted from one place (see `finish_stream`)
    WebRequestStreamFinished(StreamKey, u64, Option<String>), // u64 - generation, Option<String> - error
    HighlightPanesWithMessage(Vec<PaneId>, String),
    RenderToClients,
    QueryZellijWebServerStatus,
    ClearHelpText {
        client_id: ClientId,
    },
    FlashPaneBell(Vec<PaneId>),
    StopFlashPaneBell(Vec<PaneId>),
    FlashTabBell(usize),     // usize = tab_id
    StopFlashTabBell(usize), // usize = tab_id
    StartNestedGuestPing(PaneId),
    StopNestedGuestPing(PaneId),
    Exit,
}

impl From<&BackgroundJob> for BackgroundJobContext {
    fn from(background_job: &BackgroundJob) -> Self {
        match *background_job {
            BackgroundJob::DisplayPaneError(..) => BackgroundJobContext::DisplayPaneError,
            BackgroundJob::AnimatePluginLoading(..) => BackgroundJobContext::AnimatePluginLoading,
            BackgroundJob::StopPluginLoadingAnimation(..) => {
                BackgroundJobContext::StopPluginLoadingAnimation
            },
            BackgroundJob::ReportSessionInfo(..) => BackgroundJobContext::ReportSessionInfo,
            BackgroundJob::ReportLayoutInfo(..) => BackgroundJobContext::ReportLayoutInfo,
            BackgroundJob::RunCommand(..) => BackgroundJobContext::RunCommand,
            BackgroundJob::WebRequest(..)
            | BackgroundJob::CancelWebRequest(..)
            | BackgroundJob::WebRequestStreamFinished(..) => BackgroundJobContext::WebRequest,
            BackgroundJob::ReportPluginList(..) => BackgroundJobContext::ReportPluginList,
            BackgroundJob::RenderToClients => BackgroundJobContext::ReportPluginList,
            BackgroundJob::HighlightPanesWithMessage(..) => {
                BackgroundJobContext::HighlightPanesWithMessage
            },
            BackgroundJob::QueryZellijWebServerStatus => {
                BackgroundJobContext::QueryZellijWebServerStatus
            },
            BackgroundJob::ClearHelpText { .. } => BackgroundJobContext::ClearHelpText,
            BackgroundJob::FlashPaneBell(..) => BackgroundJobContext::FlashPaneBell,
            BackgroundJob::StopFlashPaneBell(..) => BackgroundJobContext::StopFlashPaneBell,
            BackgroundJob::FlashTabBell(..) => BackgroundJobContext::FlashTabBell,
            BackgroundJob::StopFlashTabBell(..) => BackgroundJobContext::StopFlashTabBell,
            BackgroundJob::StartNestedGuestPing(..) => BackgroundJobContext::StartNestedGuestPing,
            BackgroundJob::StopNestedGuestPing(..) => BackgroundJobContext::StopNestedGuestPing,
            BackgroundJob::Exit => BackgroundJobContext::Exit,
        }
    }
}

static LONG_FLASH_DURATION_MS: u64 = 1000;
static FLASH_DURATION_MS: u64 = 400; // Doherty threshold
static PLUGIN_ANIMATION_OFFSET_DURATION_MD: u64 = 500;
static SESSION_METADATA_WRITE_INTERVAL_MS: u64 = 1000;
static UPDATE_AND_REPORT_CWDS_INTERVAL_MS: u64 = 1000;
static DEFAULT_SERIALIZATION_INTERVAL: u64 = 60000;
static REPAINT_DELAY_MS: u64 = 10;
/// no web request gets a total cap - a long generation or a large download is legitimate - but a
/// connection that stops producing bytes for this long is reaped
static WEB_REQUEST_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// the idle timeout above is a transfer-rate guard and does not cover connection setup, so a
/// blackholed host needs its own bound
static WEB_REQUEST_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const WEB_REQUEST_STREAM_CHUNK_SIZE: usize = 8192;
static HELP_TEXT_DEBOUNCE_DURATION: u64 = 5000;

#[derive(Clone)]
pub struct SessionScanState {
    pub current_session_name: Arc<Mutex<String>>,
    pub current_session_info: Arc<Mutex<SessionInfo>>,
    pub current_session_plugin_list: Arc<Mutex<BTreeMap<PluginId, RunPlugin>>>,
}

static SESSION_SCAN_STATE: std::sync::OnceLock<SessionScanState> = std::sync::OnceLock::new();

pub fn session_scan_state() -> Option<&'static SessionScanState> {
    SESSION_SCAN_STATE.get()
}

#[allow(unused_variables)] // web_server_base_url used only with web_server_capability feature
pub(crate) fn background_jobs_main(
    bus: Bus<BackgroundJob>,
    serialization_interval: Option<u64>,
    disable_session_metadata: bool,
    web_server_base_url: String,
) -> Result<()> {
    let err_context = || "failed to write to pty".to_string();
    let mut running_jobs: HashMap<BackgroundJob, Instant> = HashMap::new();
    let mut loading_plugins: HashMap<u32, Arc<AtomicBool>> = HashMap::new(); // u32 - plugin_id
    let current_session_name = Arc::new(Mutex::new(String::default()));
    let current_session_info = Arc::new(Mutex::new(SessionInfo::default()));
    let current_session_plugin_list: Arc<Mutex<BTreeMap<PluginId, RunPlugin>>> =
        Arc::new(Mutex::new(BTreeMap::new()));
    let current_session_layout = Arc::new(Mutex::new((String::new(), BTreeMap::new())));

    let _ = SESSION_SCAN_STATE.set(SessionScanState {
        current_session_name: current_session_name.clone(),
        current_session_info: current_session_info.clone(),
        current_session_plugin_list: current_session_plugin_list.clone(),
    });
    let last_serialization_time = Arc::new(Mutex::new(Instant::now()));
    let serialization_interval = serialization_interval.map(|s| s * 1000); // convert to
                                                                           // milliseconds
    let last_render_request: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let pending_help_text_clear: Arc<Mutex<HashMap<ClientId, Instant>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let mut flashing_pane_bells: HashMap<PaneId, Arc<AtomicBool>> = HashMap::new();
    let mut flashing_tab_bells: HashMap<usize, Arc<AtomicBool>> = HashMap::new();
    let mut nested_guest_pings: HashMap<PaneId, Arc<AtomicBool>> = HashMap::new();
    let mut active_streams: HashMap<StreamKey, ActiveWebRequestStream> = HashMap::new();
    let mut next_stream_generation = 0u64;

    let http_client = HttpClient::builder()
        .connect_timeout(WEB_REQUEST_CONNECT_TIMEOUT)
        // the idle timeout is per-request, in send_web_request
        .redirect_policy(RedirectPolicy::Follow)
        .build()
        .ok();
    // We needn't do anything with the runtime, but it should exist at this point.
    let runtime = crate::global_async_runtime::get_tokio_runtime();

    {
        let senders = bus.senders.clone();
        let serialization_ms = serialization_interval.unwrap_or(DEFAULT_SERIALIZATION_INTERVAL);
        runtime.spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_millis(serialization_ms));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let _ = senders.send_to_screen(ScreenInstruction::SerializeLayoutForResurrection);
            }
        });
    }

    {
        let senders = bus.senders.clone();
        runtime.spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_millis(
                UPDATE_AND_REPORT_CWDS_INTERVAL_MS,
            ));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let _ = senders.send_to_pty(PtyInstruction::UpdateAndReportCwds);
            }
        });
    }

    if !disable_session_metadata {
        let current_session_name = current_session_name.clone();
        let current_session_info = current_session_info.clone();
        let current_session_layout = current_session_layout.clone();
        runtime.spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_millis(
                SESSION_METADATA_WRITE_INTERVAL_MS,
            ));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let name = current_session_name.lock().unwrap().clone();
                if name.is_empty() {
                    continue;
                }
                let info = current_session_info.lock().unwrap().clone();
                let layout = current_session_layout.lock().unwrap().clone();
                write_session_state_to_disk(name, info, layout);
            }
        });
    }

    loop {
        let (event, mut err_ctx) = bus.recv().with_context(err_context)?;
        err_ctx.add_call(ContextType::BackgroundJob((&event).into()));
        let job = event.clone();
        match event {
            BackgroundJob::DisplayPaneError(pane_ids, text) => {
                if job_already_running(job, &mut running_jobs) {
                    continue;
                }
                runtime.spawn({
                    let senders = bus.senders.clone();
                    async move {
                        let _ = senders.send_to_screen(
                            ScreenInstruction::AddRedPaneFrameColorOverride(
                                pane_ids.clone(),
                                Some(text),
                            ),
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(
                            LONG_FLASH_DURATION_MS,
                        ))
                        .await;
                        let _ = senders.send_to_screen(
                            ScreenInstruction::ClearPaneFrameColorOverride(pane_ids),
                        );
                    }
                });
            },
            BackgroundJob::AnimatePluginLoading(pid) => {
                let loading_plugin = Arc::new(AtomicBool::new(true));
                if job_already_running(job, &mut running_jobs) {
                    continue;
                }
                runtime.spawn({
                    let senders = bus.senders.clone();
                    let loading_plugin = loading_plugin.clone();
                    async move {
                        while loading_plugin.load(Ordering::SeqCst) {
                            let _ = senders.send_to_screen(
                                ScreenInstruction::ProgressPluginLoadingOffset(pid),
                            );
                            tokio::time::sleep(std::time::Duration::from_millis(
                                PLUGIN_ANIMATION_OFFSET_DURATION_MD,
                            ))
                            .await;
                        }
                    }
                });
                loading_plugins.insert(pid, loading_plugin);
            },
            BackgroundJob::StopPluginLoadingAnimation(pid) => {
                if let Some(loading_plugin) = loading_plugins.remove(&pid) {
                    loading_plugin.store(false, Ordering::SeqCst);
                }
            },
            BackgroundJob::ReportSessionInfo(session_name, session_info) => {
                *current_session_name.lock().unwrap() = session_name;
                *current_session_info.lock().unwrap() = session_info;
            },
            BackgroundJob::ReportPluginList(plugin_list) => {
                *current_session_plugin_list.lock().unwrap() = plugin_list;
            },
            BackgroundJob::ReportLayoutInfo(session_layout) => {
                *current_session_layout.lock().unwrap() = session_layout;

                // Update session save time for plugin query
                let timestamp_millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let _ = bus
                    .senders
                    .send_to_plugin(PluginInstruction::UpdateSessionSaveTime(timestamp_millis));
            },
            BackgroundJob::RunCommand(
                plugin_id,
                client_id,
                command,
                args,
                env_variables,
                cwd,
                context,
            ) => {
                runtime.spawn({
                    let senders = bus.senders.clone();
                    async move {
                        let output = tokio::process::Command::new(&command)
                            .args(&args)
                            .envs(env_variables)
                            .current_dir(cwd)
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::piped())
                            .output()
                            .await;
                        match output {
                            Ok(output) => {
                                let stdout = output.stdout.to_vec();
                                let stderr = output.stderr.to_vec();
                                let exit_code = output.status.code();
                                let _ = senders.send_to_plugin(PluginInstruction::Update(vec![(
                                    Some(plugin_id),
                                    Some(client_id),
                                    Event::RunCommandResult(exit_code, stdout, stderr, context),
                                )]));
                            },
                            Err(e) => {
                                log::error!("Failed to run command: {}", e);
                                let stdout = vec![];
                                let stderr = format!("{}", e).as_bytes().to_vec();
                                let exit_code = Some(2);
                                let _ = senders.send_to_plugin(PluginInstruction::Update(vec![(
                                    Some(plugin_id),
                                    Some(client_id),
                                    Event::RunCommandResult(exit_code, stdout, stderr, context),
                                )]));
                            },
                        }
                    }
                });
            },
            BackgroundJob::WebRequest(
                plugin_id,
                client_id,
                url,
                verb,
                headers,
                body,
                context,
                stream_id,
            ) => {
                let Some(http_client) = http_client.clone() else {
                    log::error!(
                        "Cannot perform http request, likely due to a misconfigured http client"
                    );
                    continue;
                };
                let senders = bus.senders.clone();
                match stream_id {
                    None => {
                        runtime.spawn(async move {
                            let event =
                                match buffered_web_request(url, verb, headers, body, http_client)
                                    .await
                                {
                                    Ok((status, headers, body)) => {
                                        Event::WebRequestResult(status, headers, body, context)
                                    },
                                    Err(e) => {
                                        log::error!("Failed to send web request: {}", e);
                                        Event::WebRequestResult(
                                            400,
                                            BTreeMap::new(),
                                            e.to_string().into_bytes(),
                                            context,
                                        )
                                    },
                                };
                            let _ = senders.send_to_plugin(PluginInstruction::Update(vec![(
                                Some(plugin_id),
                                Some(client_id),
                                event,
                            )]));
                        });
                    },
                    Some(stream_id) => {
                        let key: StreamKey = (plugin_id, client_id, stream_id);
                        // reusing the id of a stream that is still in flight ends the old one,
                        // so it can't be left running with nothing able to cancel it
                        finish_stream(
                            &mut active_streams,
                            &bus.senders,
                            key.clone(),
                            None,
                            Some("superseded".to_owned()),
                        );
                        next_stream_generation = next_stream_generation.wrapping_add(1);
                        let generation = next_stream_generation;
                        let state = Arc::new(Mutex::new(WebRequestStreamState::default()));
                        let handle = runtime.spawn(streamed_web_request(
                            key.clone(),
                            generation,
                            state.clone(),
                            senders,
                            url,
                            verb,
                            headers,
                            body,
                            context.clone(),
                            http_client,
                        ));
                        // ponytail: unloading a plugin does not cancel its in-flight streams -
                        // they drain to completion and the events are dropped by
                        // update_plugins' plugin-id filter. Add a
                        // BackgroundJob::CancelPluginWebRequests(plugin_id) from wasm_bridge's
                        // unload path if abandoned streams become a bandwidth problem.
                        active_streams.insert(
                            key,
                            ActiveWebRequestStream {
                                abort_handle: handle.abort_handle(),
                                generation,
                                state,
                                context,
                            },
                        );
                    },
                }
            },
            BackgroundJob::CancelWebRequest(plugin_id, client_id, stream_id) => {
                let key: StreamKey = (plugin_id, client_id, stream_id);
                finish_stream(
                    &mut active_streams,
                    &bus.senders,
                    key,
                    None,
                    Some("cancelled".to_owned()),
                );
            },
            BackgroundJob::WebRequestStreamFinished(key, generation, error) => {
                finish_stream(
                    &mut active_streams,
                    &bus.senders,
                    key,
                    Some(generation),
                    error,
                );
            },
            BackgroundJob::QueryZellijWebServerStatus => {
                #[cfg(feature = "web_server_capability")]
                {
                    let status = query_webserver_via_ipc(&web_server_base_url)
                        .unwrap_or(WebServerStatus::Offline);
                    runtime.spawn({
                        let senders = bus.senders.clone();
                        let _web_server_base_url = web_server_base_url.clone();
                        async move {
                            let _ = senders.send_to_plugin(PluginInstruction::Update(vec![(
                                None,
                                None,
                                Event::WebServerStatus(status),
                            )]));
                        }
                    });
                }
            },
            BackgroundJob::RenderToClients => {
                // last_render_request being Some() represents a render request that is pending
                // last_render_request is only ever set to Some() if an async task is spawned to
                // send the actual render instruction
                //
                // given this:
                // - if last_render_request is None and we received this job, we should spawn an
                // async task to send the render instruction and log the current task time
                // - if last_render_request is Some(), it means we're currently waiting to render,
                // so we should log the render request and do nothing, once the async task has
                // finished running, it will check to see if the render time was updated while it
                // was running, and if so send this instruction again so the process can start anew
                let (should_run_task, current_time) = {
                    let mut last_render_request = last_render_request.lock().unwrap();
                    let should_run_task = last_render_request.is_none();
                    let current_time = Instant::now();
                    *last_render_request = Some(current_time);
                    (should_run_task, current_time)
                };
                if should_run_task {
                    runtime.spawn({
                        let senders = bus.senders.clone();
                        let last_render_request = last_render_request.clone();
                        let task_start_time = current_time;
                        async move {
                            tokio::time::sleep(std::time::Duration::from_millis(REPAINT_DELAY_MS))
                                .await;
                            let _ = senders.send_to_screen(ScreenInstruction::RenderToClients);
                            {
                                let mut last_render_request = last_render_request.lock().unwrap();
                                if let Some(last_render_request) = *last_render_request {
                                    if last_render_request > task_start_time {
                                        // another render request was received while we were
                                        // sleeping, schedule this job again so that we can also
                                        // render that request
                                        let _ = senders.send_to_background_jobs(
                                            BackgroundJob::RenderToClients,
                                        );
                                    }
                                }
                                // reset the last_render_request so that the task will be spawned
                                // again once a new request is received
                                *last_render_request = None;
                            }
                        }
                    });
                }
            },
            BackgroundJob::HighlightPanesWithMessage(pane_ids, text) => {
                if job_already_running(job, &mut running_jobs) {
                    continue;
                }
                runtime.spawn({
                    let senders = bus.senders.clone();
                    async move {
                        let _ = senders.send_to_screen(
                            ScreenInstruction::AddHighlightPaneFrameColorOverride(
                                pane_ids.clone(),
                                Some(text),
                            ),
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(FLASH_DURATION_MS))
                            .await;
                        let _ = senders.send_to_screen(
                            ScreenInstruction::ClearPaneFrameColorOverride(pane_ids),
                        );
                    }
                });
            },
            BackgroundJob::ClearHelpText { client_id } => {
                let should_spawn = {
                    let mut pending = pending_help_text_clear.lock().unwrap();
                    let current_time = Instant::now();
                    let should_spawn = !pending.contains_key(&client_id);
                    pending.insert(client_id, current_time);
                    should_spawn
                };

                if should_spawn {
                    runtime.spawn({
                        let senders = bus.senders.clone();
                        let pending = pending_help_text_clear.clone();
                        let debounce_duration = Duration::from_millis(HELP_TEXT_DEBOUNCE_DURATION);
                        async move {
                            tokio::time::sleep(debounce_duration).await;
                            loop {
                                let next_sleep_duration = {
                                    let mut pending = pending.lock().unwrap();
                                    match pending.get(&client_id) {
                                        Some(&last_motion_time) => {
                                            let time_since_motion =
                                                Instant::now().duration_since(last_motion_time);
                                            if time_since_motion >= debounce_duration {
                                                pending.remove(&client_id);
                                                None
                                            } else {
                                                let remaining = debounce_duration
                                                    .saturating_sub(time_since_motion);
                                                Some(remaining)
                                            }
                                        },
                                        None => break,
                                    }
                                };

                                match next_sleep_duration {
                                    Some(duration) => {
                                        tokio::time::sleep(duration).await;
                                    },
                                    None => {
                                        let _ = senders.send_to_server(
                                            ServerInstruction::ClearMouseHelpText(client_id),
                                        );
                                        break;
                                    },
                                }
                            }
                        }
                    });
                }
            },
            BackgroundJob::FlashPaneBell(pane_ids) => {
                let is_flashing = Arc::new(AtomicBool::new(true));
                for &pane_id in &pane_ids {
                    flashing_pane_bells.insert(pane_id, is_flashing.clone());
                }
                runtime.spawn({
                    let senders = bus.senders.clone();
                    let pane_ids_clone = pane_ids.clone();
                    let flag = is_flashing.clone();
                    async move {
                        let _ = senders.send_to_screen(
                            ScreenInstruction::AddHighlightPaneFrameColorOverride(
                                pane_ids_clone.clone(),
                                None,
                            ),
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(FLASH_DURATION_MS))
                            .await;
                        if flag.load(Ordering::SeqCst) {
                            let _ = senders.send_to_screen(
                                ScreenInstruction::ClearPaneFrameColorOverride(pane_ids_clone),
                            );
                        }
                    }
                });
            },
            BackgroundJob::StopFlashPaneBell(pane_ids) => {
                for &pane_id in &pane_ids {
                    if let Some(flag) = flashing_pane_bells.remove(&pane_id) {
                        flag.store(false, Ordering::SeqCst);
                    }
                }
                let _ = bus
                    .senders
                    .send_to_screen(ScreenInstruction::ClearPaneFrameColorOverride(pane_ids));
            },
            BackgroundJob::FlashTabBell(tab_id) => {
                let is_flashing = Arc::new(AtomicBool::new(true));
                flashing_tab_bells.insert(tab_id, is_flashing.clone());
                runtime.spawn({
                    let senders = bus.senders.clone();
                    let flag = is_flashing.clone();
                    async move {
                        let _ = senders
                            .send_to_screen(ScreenInstruction::SetTabBellFlash(tab_id, true));
                        tokio::time::sleep(std::time::Duration::from_millis(FLASH_DURATION_MS))
                            .await;
                        if flag.load(Ordering::SeqCst) {
                            let _ = senders
                                .send_to_screen(ScreenInstruction::SetTabBellFlash(tab_id, false));
                        }
                    }
                });
            },
            BackgroundJob::StopFlashTabBell(tab_id) => {
                if let Some(flag) = flashing_tab_bells.remove(&tab_id) {
                    flag.store(false, Ordering::SeqCst);
                }
                let _ = bus
                    .senders
                    .send_to_screen(ScreenInstruction::SetTabBellFlash(tab_id, false));
            },
            BackgroundJob::StartNestedGuestPing(pane_id) => {
                if let std::collections::hash_map::Entry::Vacant(e) =
                    nested_guest_pings.entry(pane_id)
                {
                    let is_pinging = Arc::new(AtomicBool::new(true));
                    e.insert(is_pinging.clone());
                    runtime.spawn({
                        let senders = bus.senders.clone();
                        let flag = is_pinging.clone();
                        let ping_interval_ms = crate::nested_guest::ping_interval_ms();
                        async move {
                            while flag.load(Ordering::SeqCst) {
                                let _ = senders.send_to_screen(
                                    ScreenInstruction::NestedGuestPingTick { pane_id },
                                );
                                tokio::time::sleep(Duration::from_millis(ping_interval_ms)).await;
                            }
                        }
                    });
                }
            },
            BackgroundJob::StopNestedGuestPing(pane_id) => {
                if let Some(flag) = nested_guest_pings.remove(&pane_id) {
                    flag.store(false, Ordering::SeqCst);
                }
            },
            BackgroundJob::Exit => {
                for loading_plugin in loading_plugins.values() {
                    loading_plugin.store(false, Ordering::SeqCst);
                }
                for nested_guest_ping in nested_guest_pings.values() {
                    nested_guest_ping.store(false, Ordering::SeqCst);
                }

                let cache_file_name =
                    session_info_cache_file_name(&current_session_name.lock().unwrap().to_owned());
                let _ = std::fs::remove_file(cache_file_name);
                return Ok(());
            },
        }
    }
}

/// Build and send a web request. Buffered and streamed requests are sent identically - see
/// `WEB_REQUEST_IDLE_TIMEOUT` for why neither gets a total cap.
async fn send_web_request(
    url: String,
    verb: HttpVerb,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    http_client: HttpClient,
) -> Result<isahc::Response<isahc::AsyncBody>, isahc::Error> {
    let mut request = match verb {
        HttpVerb::Get => Request::get(url),
        HttpVerb::Post => Request::post(url),
        HttpVerb::Put => Request::put(url),
        HttpVerb::Delete => Request::delete(url),
    };
    for (header, value) in headers {
        request = request.header(header.as_str(), value);
    }
    request = request.low_speed_timeout(1, WEB_REQUEST_IDLE_TIMEOUT);
    if !body.is_empty() {
        http_client.send_async(request.body(body)?).await
    } else {
        http_client.send_async(request.body(())?).await
    }
}

fn response_headers(response: &isahc::Response<isahc::AsyncBody>) -> BTreeMap<String, String> {
    response
        .headers()
        .iter()
        .filter_map(|(name, value)| match value.to_str() {
            Ok(value) => Some((name.to_string(), value.to_string())),
            Err(e) => {
                log::error!("Failed to convert header {:?} to string: {:?}", name, e);
                None
            },
        })
        .collect()
}

async fn buffered_web_request(
    url: String,
    verb: HttpVerb,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    http_client: HttpClient,
) -> Result<(u16, BTreeMap<String, String>, Vec<u8>), isahc::Error> {
    let mut res = send_web_request(url, verb, headers, body, http_client).await?;
    let status_code = res.status().as_u16();
    let headers = response_headers(&res);
    let body = res.bytes().await?;
    Ok((status_code, headers, body))
}

/// Sends the leading `WebRequestResult` and then one `WebRequestStreamChunk` per read off the
/// socket. Never emits the terminating chunk itself: it reports back with
/// `BackgroundJob::WebRequestStreamFinished` so the terminator is emitted from a single place
/// even when this task is aborted mid-read by a cancellation.
#[allow(clippy::too_many_arguments)]
async fn streamed_web_request(
    key: StreamKey,
    generation: u64,
    state: Arc<Mutex<WebRequestStreamState>>,
    senders: crate::thread_bus::ThreadSenders,
    url: String,
    verb: HttpVerb,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    context: BTreeMap<String, String>,
    http_client: HttpClient,
) {
    let (plugin_id, client_id, stream_id) = key.clone();
    let send_event = |event: Event| {
        let mut state = state.lock().unwrap();
        if state.finished {
            return;
        }
        if matches!(event, Event::WebRequestResult(..)) {
            state.response_sent = true;
        }
        let _ = senders.send_to_plugin(PluginInstruction::Update(vec![(
            Some(plugin_id),
            Some(client_id),
            event,
        )]));
    };
    let finished =
        |error| {
            // not `let _ =`: dropping this leaves the stream registered and the plugin waiting for a
            // terminator that will never come
            if let Err(e) = senders.send_to_background_jobs(
                BackgroundJob::WebRequestStreamFinished(key.clone(), generation, error),
            ) {
                log::error!("Failed to report web request stream as finished: {}", e);
            }
        };

    let mut res = match send_web_request(url, verb, headers, body, http_client).await {
        Ok(res) => res,
        Err(e) => {
            log::error!("Failed to send web request: {}", e);
            // no response at all - report it the same way a buffered request would
            send_event(Event::WebRequestResult(
                400,
                BTreeMap::new(),
                e.to_string().into_bytes(),
                context,
            ));
            return finished(Some(e.to_string()));
        },
    };

    let status = res.status().as_u16();
    let headers = response_headers(&res);
    if !(200..300).contains(&status) {
        // an error response is not a stream - buffer it whole so the plugin gets the body
        let (body, error) = match res.bytes().await {
            Ok(body) => (body, None),
            Err(e) => {
                let error = e.to_string();
                (error.clone().into_bytes(), Some(error))
            },
        };
        send_event(Event::WebRequestResult(status, headers, body, context));
        return finished(error);
    }
    send_event(Event::WebRequestResult(status, headers, vec![], context));

    // ponytail: chunks are forwarded as they come off the socket, one plugin update() per chunk
    // (~50/s for a token stream, ~100us each). Coalesce on a ~30ms window here if it ever shows
    // up in a profile.
    let mut buf = [0u8; WEB_REQUEST_STREAM_CHUNK_SIZE];
    let error = loop {
        match res.body_mut().read(&mut buf).await {
            Ok(0) => break None,
            Ok(bytes_read) => send_event(Event::WebRequestStreamChunk {
                stream_id: stream_id.clone(),
                chunk: buf[..bytes_read].to_vec(),
                done: false,
                error: None,
            }),
            Err(e) => {
                log::error!("Web request stream failed: {}", e);
                break Some(e.to_string());
            },
        }
    };
    finished(error);
}

/// Ends a stream: drop it from the registry, abort its task if it is still running, and emit its
/// terminating chunk. Both the cancellation and the natural-completion paths go through here, and
/// removing the entry is the *sole* mechanism making the terminator exactly-once - whichever of
/// the two arrives second finds the key already gone. Aborting an already-finished task is a
/// no-op. The state lock is held across the terminator send so that a chunk racing in from the
/// task lands before it or not at all.
fn finish_stream(
    active_streams: &mut HashMap<StreamKey, ActiveWebRequestStream>,
    senders: &crate::thread_bus::ThreadSenders,
    key: StreamKey,
    generation: Option<u64>,
    error: Option<String>,
) {
    if generation.is_some_and(|generation| {
        active_streams
            .get(&key)
            .is_none_or(|stream| stream.generation != generation)
    }) {
        return;
    }
    let Some(stream) = active_streams.remove(&key) else {
        return;
    };
    let mut state = stream.state.lock().unwrap();
    // `finished` is only ever set below, on a state whose entry has already been removed, and a
    // removed entry is never re-inserted - so a state still reachable here cannot be finished
    debug_assert!(!state.finished, "a registered stream was already finished");
    stream.abort_handle.abort();
    let (plugin_id, client_id, stream_id) = key;
    if !state.response_sent {
        let _ = senders.send_to_plugin(PluginInstruction::Update(vec![(
            Some(plugin_id),
            Some(client_id),
            Event::WebRequestResult(
                400,
                BTreeMap::new(),
                error
                    .clone()
                    .unwrap_or_else(|| "cancelled".to_owned())
                    .into_bytes(),
                stream.context,
            ),
        )]));
        state.response_sent = true;
    }
    state.finished = true;
    let _ = senders.send_to_plugin(PluginInstruction::Update(vec![(
        Some(plugin_id),
        Some(client_id),
        Event::WebRequestStreamChunk {
            stream_id,
            chunk: vec![],
            done: true,
            error,
        },
    )]));
}

fn job_already_running(
    job: BackgroundJob,
    running_jobs: &mut HashMap<BackgroundJob, Instant>,
) -> bool {
    match running_jobs.get_mut(&job) {
        Some(current_running_job_start_time) => {
            if current_running_job_start_time.elapsed()
                > Duration::from_millis(LONG_FLASH_DURATION_MS)
            {
                *current_running_job_start_time = Instant::now();
                false
            } else {
                true
            }
        },
        None => {
            running_jobs.insert(job.clone(), Instant::now());
            false
        },
    }
}

fn file_content_changed(path: &std::path::Path, new_content: &[u8]) -> bool {
    match std::fs::read(path) {
        Ok(existing) => existing != new_content,
        Err(_) => true,
    }
}

pub fn write_session_state_to_disk(
    current_session_name: String,
    current_session_info: SessionInfo,
    current_session_layout: (String, BTreeMap<String, String>),
) {
    let metadata_cache_file_name = session_info_cache_file_name(&current_session_name);
    let (current_session_layout, layout_files_to_write) = current_session_layout;
    let new_metadata = current_session_info.to_string();
    if file_content_changed(&metadata_cache_file_name, new_metadata.as_bytes()) {
        let _wrote_metadata_file = std::fs::create_dir_all(
            session_info_folder_for_session(&current_session_name).as_path(),
        )
        .and_then(|_| std::fs::File::create(&metadata_cache_file_name))
        .and_then(|mut f| write!(f, "{}", new_metadata));
    }

    if !current_session_layout.is_empty() {
        let layout_cache_file_name = session_layout_cache_file_name(&current_session_name);
        if file_content_changed(&layout_cache_file_name, current_session_layout.as_bytes()) {
            let _wrote_layout_file = std::fs::create_dir_all(
                session_info_folder_for_session(&current_session_name).as_path(),
            )
            .and_then(|_| std::fs::File::create(&layout_cache_file_name))
            .and_then(|mut f| write!(f, "{}", current_session_layout));
        }
        let session_info_folder = session_info_folder_for_session(&current_session_name);
        for (external_file_name, external_file_contents) in layout_files_to_write {
            let external_file_path = session_info_folder.join(&external_file_name);
            if file_content_changed(&external_file_path, external_file_contents.as_bytes()) {
                std::fs::File::create(&external_file_path)
                    .and_then(|mut f| write!(f, "{}", external_file_contents))
                    .unwrap_or_else(|e| {
                        log::error!("Failed to write layout metadata file: {:?}", e);
                    });
            }
        }
    }
}

pub fn scan_session_list(
    current_session_name: &str,
    available_layouts: &[LayoutInfo],
    current_session_plugin_list: &BTreeMap<PluginId, RunPlugin>,
    sock_dir: &Path,
    session_info_cache_dir: &Path,
) -> (BTreeMap<String, SessionInfo>, BTreeMap<String, Duration>) {
    let mut session_infos_on_machine = zellij_utils::sessions::read_live_session_states(
        current_session_name,
        sock_dir,
        session_info_cache_dir,
    );
    for (name, info) in session_infos_on_machine.iter_mut() {
        if name == current_session_name {
            info.populate_plugin_list(current_session_plugin_list.clone());
            info.available_layouts = available_layouts.to_vec();
        }
    }
    let resurrectable_sessions =
        find_resurrectable_sessions(&session_infos_on_machine, session_info_cache_dir);
    (session_infos_on_machine, resurrectable_sessions)
}

pub fn scan_session_list_default_dirs(
    current_session_name: &str,
    available_layouts: &[LayoutInfo],
    current_session_plugin_list: &BTreeMap<PluginId, RunPlugin>,
) -> (BTreeMap<String, SessionInfo>, BTreeMap<String, Duration>) {
    scan_session_list(
        current_session_name,
        available_layouts,
        current_session_plugin_list,
        &ZELLIJ_SOCK_DIR,
        &ZELLIJ_SESSION_INFO_CACHE_DIR,
    )
}

fn find_resurrectable_sessions(
    session_infos_on_machine: &BTreeMap<String, SessionInfo>,
    session_info_cache_dir: &Path,
) -> BTreeMap<String, Duration> {
    match fs::read_dir(session_info_cache_dir) {
        Ok(files_in_session_info_folder) => {
            let files_that_are_folders = files_in_session_info_folder
                .filter_map(|f| f.ok().map(|f| f.path()))
                .filter(|f| f.is_dir());
            files_that_are_folders
                .filter_map(|folder_name| {
                    let session_name = folder_name.file_name()?.to_str()?.to_owned();
                    if session_infos_on_machine.contains_key(&session_name) {
                        // this is not a dead session...
                        return None;
                    }
                    let layout_file_name = folder_name.join("session-layout.kdl");
                    let ctime = match std::fs::metadata(&layout_file_name)
                        .and_then(|metadata| metadata.created())
                    {
                        Ok(created) => Some(created),
                        Err(e) => {
                            if e.kind() == std::io::ErrorKind::NotFound {
                                return None; // no layout file, cannot resurrect session, let's not
                                             // list it
                            } else {
                                log::error!(
                                    "Failed to read created stamp of resurrection file: {:?}",
                                    e
                                );
                            }
                            None
                        },
                    };
                    let elapsed_duration = ctime
                        .and_then(|ctime| ctime.elapsed().ok())
                        .unwrap_or_default();
                    Some((session_name, elapsed_duration))
                })
                .collect()
        },
        Err(e) => {
            log::error!("Failed to read session info cache dir: {:?}", e);
            BTreeMap::new()
        },
    }
}

#[cfg(feature = "web_server_capability")]
fn query_webserver_via_ipc(web_server_base_url: &str) -> Result<WebServerStatus> {
    let expected_addr =
        parse_base_url(web_server_base_url).context("Failed to parse web server base URL")?;

    let sockets = discover_webserver_sockets().context("Failed to discover web server sockets")?;

    if sockets.is_empty() {
        return Ok(WebServerStatus::Offline);
    }

    for socket_path in sockets {
        let path_str = socket_path.to_str().unwrap_or("");

        match query_webserver_with_response(path_str, InstructionForWebServer::QueryVersion, 500) {
            Ok(WebServerResponse::Version(info)) => {
                let matches_expected =
                    info.ip == expected_addr.ip && info.port == expected_addr.port;

                if !matches_expected {
                    continue;
                }

                if info.version == VERSION {
                    return Ok(WebServerStatus::Online(web_server_base_url.to_string()));
                } else {
                    return Ok(WebServerStatus::DifferentVersion(info.version));
                }
            },
            Err(_) => continue,
        }
    }

    Ok(WebServerStatus::Offline)
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use tempfile::tempdir;
    use zellij_utils::data::SessionInfo;

    fn make_socket(dir: &std::path::Path, name: &str) -> UnixListener {
        UnixListener::bind(dir.join(name)).expect("bind unix socket")
    }

    fn write_metadata(info_dir: &std::path::Path, session: &str, info: &SessionInfo) {
        let folder = info_dir.join(session);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("session-metadata.kdl"), info.to_string()).unwrap();
    }

    fn write_layout(info_dir: &std::path::Path, session: &str) {
        let folder = info_dir.join(session);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("session-layout.kdl"), "layout { }").unwrap();
    }

    #[test]
    fn scan_session_list_returns_empty_when_no_peers() {
        let sock_dir = tempdir().unwrap();
        let info_dir = tempdir().unwrap();
        let (live, resurrectable) = scan_session_list(
            "me",
            &[],
            &BTreeMap::new(),
            sock_dir.path(),
            info_dir.path(),
        );
        assert!(live.is_empty());
        assert!(resurrectable.is_empty());
    }

    #[test]
    fn scan_session_list_finds_peer_from_socket_and_metadata() {
        let sock_dir = tempdir().unwrap();
        let info_dir = tempdir().unwrap();
        let peer = "peer-alpha";
        let _listener = make_socket(sock_dir.path(), peer);
        write_metadata(info_dir.path(), peer, &SessionInfo::new(peer.to_string()));

        let (live, resurrectable) = scan_session_list(
            "me",
            &[],
            &BTreeMap::new(),
            sock_dir.path(),
            info_dir.path(),
        );
        assert_eq!(live.len(), 1);
        assert!(live.contains_key(peer));
        assert!(resurrectable.is_empty());
    }

    #[test]
    fn scan_session_list_finds_resurrectable_from_orphan_metadata() {
        let sock_dir = tempdir().unwrap();
        let info_dir = tempdir().unwrap();
        write_layout(info_dir.path(), "dead-beta");

        let (live, resurrectable) = scan_session_list(
            "me",
            &[],
            &BTreeMap::new(),
            sock_dir.path(),
            info_dir.path(),
        );
        assert!(live.is_empty());
        assert_eq!(resurrectable.len(), 1);
        assert!(resurrectable.contains_key("dead-beta"));
    }

    #[test]
    fn scan_session_list_separates_live_from_resurrectable() {
        let sock_dir = tempdir().unwrap();
        let info_dir = tempdir().unwrap();
        for name in ["live-a", "live-b", "live-c"] {
            let _listener = make_socket(sock_dir.path(), name);
            write_metadata(info_dir.path(), name, &SessionInfo::new(name.to_string()));
            std::mem::forget(_listener);
        }
        for name in ["dead-a", "dead-b"] {
            write_layout(info_dir.path(), name);
        }

        let (live, resurrectable) = scan_session_list(
            "me",
            &[],
            &BTreeMap::new(),
            sock_dir.path(),
            info_dir.path(),
        );
        assert_eq!(live.len(), 3);
        assert_eq!(resurrectable.len(), 2);
        for name in ["live-a", "live-b", "live-c"] {
            assert!(!resurrectable.contains_key(name));
        }
    }
}

#[cfg(test)]
mod web_request_stream_tests {
    use super::*;
    use crate::thread_bus::ThreadSenders;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use zellij_utils::channels::{unbounded, Receiver, SenderWithContext};
    use zellij_utils::errors::ErrorContext;

    const PLUGIN_ID: PluginId = 1;
    const CLIENT_ID: ClientId = 2;
    const GENERATION: u64 = 7;

    type PluginRx = Receiver<(PluginInstruction, ErrorContext)>;
    type JobRx = Receiver<(BackgroundJob, ErrorContext)>;

    fn test_senders() -> (ThreadSenders, PluginRx, JobRx) {
        let (to_plugin, plugin_rx) = unbounded();
        let (to_background_jobs, job_rx) = unbounded();
        let senders = ThreadSenders {
            to_plugin: Some(SenderWithContext::new(to_plugin)),
            to_background_jobs: Some(SenderWithContext::new(to_background_jobs)),
            ..Default::default()
        };
        (senders, plugin_rx, job_rx)
    }

    /// Serves exactly one request, then responds with `status` and the given body parts, each
    /// written as a separate `Transfer-Encoding: chunked` chunk with a gap in between so they
    /// reach the client as distinct reads. Returns the url to hit.
    async fn serve_once(
        status: &'static str,
        content_type: &'static str,
        parts: Vec<&'static str>,
    ) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            // read the request headers so curl isn't left writing into a full buffer
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                match socket.read(&mut byte).await {
                    Ok(0) | Err(_) => return,
                    Ok(_) => request.push(byte[0]),
                }
            }
            let head = format!(
                "HTTP/1.1 {}\r\nContent-Type: {}\r\nTransfer-Encoding: chunked\r\n\r\n",
                status, content_type
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.flush().await.unwrap();
            for part in parts {
                let chunk = format!("{:x}\r\n{}\r\n", part.len(), part);
                socket.write_all(chunk.as_bytes()).await.unwrap();
                socket.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            let _ = socket.write_all(b"0\r\n\r\n").await;
            let _ = socket.flush().await;
        });
        url
    }

    async fn serve_truncated_response() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                match socket.read(&mut byte).await {
                    Ok(0) | Err(_) => return,
                    Ok(_) => request.push(byte[0]),
                }
            }
            socket
                .write_all(
                    b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 100\r\n\r\nshort",
                )
                .await
                .unwrap();
        });
        url
    }

    fn drain_events(plugin_rx: &PluginRx) -> Vec<Event> {
        let mut events = vec![];
        while let Ok((PluginInstruction::Update(updates), _)) = plugin_rx.try_recv() {
            events.extend(updates.into_iter().map(|(_, _, event)| event));
        }
        events
    }

    fn stream_key() -> StreamKey {
        (PLUGIN_ID, CLIENT_ID, "stream-1".to_owned())
    }

    async fn run_stream(url: String) -> (Vec<Event>, Vec<BackgroundJob>) {
        let (senders, plugin_rx, job_rx) = test_senders();
        streamed_web_request(
            stream_key(),
            GENERATION,
            Arc::new(Mutex::new(WebRequestStreamState::default())),
            senders,
            url,
            HttpVerb::Get,
            BTreeMap::new(),
            vec![],
            BTreeMap::new(),
            HttpClient::new().unwrap(),
        )
        .await;
        let events = drain_events(&plugin_rx);
        let jobs = job_rx.try_iter().map(|(job, _)| job).collect();
        (events, jobs)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn streams_body_in_chunks_and_reports_finished_once() {
        let url = serve_once(
            "200 OK",
            "text/event-stream",
            vec![
                "data: {\"n\":1}\n\n",
                "data: {\"n\":2}\n\n",
                "data: [DONE]\n\n",
            ],
        )
        .await;
        let (events, jobs) = run_stream(url).await;

        // the leading WebRequestResult carries the status and headers with an empty body
        match &events[0] {
            Event::WebRequestResult(status, headers, body, _) => {
                assert_eq!(*status, 200);
                assert_eq!(
                    headers.get("content-type").map(|s| s.as_str()),
                    Some("text/event-stream")
                );
                assert!(body.is_empty(), "streamed body must not be buffered");
            },
            other => panic!("expected a leading WebRequestResult, got {:?}", other),
        }

        let chunks: Vec<&Vec<u8>> = events[1..]
            .iter()
            .map(|e| match e {
                Event::WebRequestStreamChunk { chunk, done, .. } => {
                    assert!(!done, "the task must never emit the terminator itself");
                    chunk
                },
                other => panic!("expected a chunk, got {:?}", other),
            })
            .collect();
        assert!(
            chunks.len() >= 2,
            "body should arrive incrementally, got {} chunk(s)",
            chunks.len()
        );
        let joined: Vec<u8> = chunks.into_iter().flatten().copied().collect();
        assert_eq!(
            String::from_utf8(joined).unwrap(),
            "data: {\"n\":1}\n\ndata: {\"n\":2}\n\ndata: [DONE]\n\n"
        );

        assert_eq!(
            jobs,
            vec![BackgroundJob::WebRequestStreamFinished(
                (PLUGIN_ID, CLIENT_ID, "stream-1".to_owned()),
                GENERATION,
                None
            )],
            "exactly one finish report, with no error"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn non_2xx_response_is_buffered_whole_with_no_chunks() {
        let url = serve_once(
            "401 Unauthorized",
            "application/json",
            vec!["{\"error\":", "\"bad key\"}"],
        )
        .await;
        let (events, jobs) = run_stream(url).await;

        assert_eq!(events.len(), 1, "an error response is not a stream");
        match &events[0] {
            Event::WebRequestResult(status, _, body, _) => {
                assert_eq!(*status, 401);
                assert_eq!(
                    String::from_utf8(body.clone()).unwrap(),
                    "{\"error\":\"bad key\"}"
                );
            },
            other => panic!("expected a buffered WebRequestResult, got {:?}", other),
        }
        assert_eq!(
            jobs,
            vec![BackgroundJob::WebRequestStreamFinished(
                (PLUGIN_ID, CLIENT_ID, "stream-1".to_owned()),
                GENERATION,
                None
            )]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unreachable_host_reports_an_error_and_still_finishes() {
        let (events, jobs) = run_stream("http://127.0.0.1:1/".to_owned()).await;

        assert!(matches!(events[0], Event::WebRequestResult(400, ..)));
        match &jobs[..] {
            [BackgroundJob::WebRequestStreamFinished(_, GENERATION, Some(_))] => {},
            other => panic!(
                "expected one finish report carrying an error, got {:?}",
                other
            ),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn truncated_non_2xx_response_finishes_with_an_error() {
        let (events, jobs) = run_stream(serve_truncated_response().await).await;

        assert!(matches!(events[0], Event::WebRequestResult(500, ..)));
        assert!(matches!(
            &jobs[..],
            [BackgroundJob::WebRequestStreamFinished(
                _,
                GENERATION,
                Some(_)
            )]
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn terminator_is_emitted_exactly_once_per_stream() {
        let (senders, plugin_rx, _job_rx) = test_senders();
        let mut active_streams = HashMap::new();
        let task = tokio::spawn(async { tokio::time::sleep(Duration::from_secs(60)).await });
        active_streams.insert(
            stream_key(),
            ActiveWebRequestStream {
                abort_handle: task.abort_handle(),
                generation: GENERATION,
                state: Arc::new(Mutex::new(WebRequestStreamState::default())),
                context: BTreeMap::from([("request_id".to_owned(), "7".to_owned())]),
            },
        );

        // a cancel arrives, then the task's own finish report races in behind it - as would
        // happen if the body ended just as the user hit the stop key
        finish_stream(
            &mut active_streams,
            &senders,
            stream_key(),
            None,
            Some("cancelled".to_owned()),
        );
        finish_stream(
            &mut active_streams,
            &senders,
            stream_key(),
            Some(GENERATION),
            None,
        );

        let events = drain_events(&plugin_rx);
        assert_eq!(
            events.len(),
            2,
            "cancellation before headers must still lead with a result"
        );
        assert!(matches!(
            &events[0],
            Event::WebRequestResult(400, _, _, context)
                if context.get("request_id").map(String::as_str) == Some("7")
        ));
        assert!(matches!(
            &events[1],
            Event::WebRequestStreamChunk { done: true, error: Some(e), .. } if e == "cancelled"
        ));
        assert!(
            task.await.unwrap_err().is_cancelled(),
            "task should be aborted"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn completed_stream_terminates_without_a_second_result() {
        let (senders, plugin_rx, _job_rx) = test_senders();
        let mut active_streams = HashMap::new();
        active_streams.insert(
            stream_key(),
            ActiveWebRequestStream {
                abort_handle: tokio::spawn(async {}).abort_handle(),
                generation: GENERATION,
                state: Arc::new(Mutex::new(WebRequestStreamState {
                    // the task already emitted the leading result and its chunks
                    response_sent: true,
                    finished: false,
                })),
                context: BTreeMap::new(),
            },
        );

        finish_stream(
            &mut active_streams,
            &senders,
            stream_key(),
            Some(GENERATION),
            None,
        );

        let events = drain_events(&plugin_rx);
        assert!(
            matches!(
                &events[..],
                [Event::WebRequestStreamChunk {
                    done: true,
                    error: None,
                    chunk,
                    ..
                }] if chunk.is_empty()
            ),
            "a completed stream ends with only the terminator, got {:?}",
            events
        );
        assert!(active_streams.is_empty(), "the stream must be deregistered");
    }

    /// reusing an in-flight stream_id supersedes the old stream - the registry ends up holding
    /// only the new one, and the old one is terminated rather than orphaned
    #[tokio::test(flavor = "multi_thread")]
    async fn reusing_a_stream_id_supersedes_the_old_stream() {
        let (senders, plugin_rx, _job_rx) = test_senders();
        let mut active_streams = HashMap::new();
        let old_task = tokio::spawn(async { tokio::time::sleep(Duration::from_secs(60)).await });
        active_streams.insert(
            stream_key(),
            ActiveWebRequestStream {
                abort_handle: old_task.abort_handle(),
                generation: GENERATION,
                state: Arc::new(Mutex::new(WebRequestStreamState {
                    response_sent: true,
                    finished: false,
                })),
                context: BTreeMap::new(),
            },
        );

        // what the WebRequest arm does before spawning a replacement under the same key
        finish_stream(
            &mut active_streams,
            &senders,
            stream_key(),
            None,
            Some("superseded".to_owned()),
        );
        let new_state = Arc::new(Mutex::new(WebRequestStreamState::default()));
        let new_task = tokio::spawn(async { tokio::time::sleep(Duration::from_secs(60)).await });
        active_streams.insert(
            stream_key(),
            ActiveWebRequestStream {
                abort_handle: new_task.abort_handle(),
                generation: GENERATION + 1,
                state: new_state,
                context: BTreeMap::new(),
            },
        );

        assert!(
            matches!(
                &drain_events(&plugin_rx)[..],
                [Event::WebRequestStreamChunk { done: true, error: Some(e), .. }] if e == "superseded"
            ),
            "the superseded stream gets its terminator"
        );
        assert!(old_task.await.unwrap_err().is_cancelled());

        // the old stream's finish report must not disturb its replacement
        finish_stream(
            &mut active_streams,
            &senders,
            stream_key(),
            Some(GENERATION),
            None,
        );
        assert!(drain_events(&plugin_rx).is_empty());
        assert_eq!(
            active_streams.get(&stream_key()).map(|s| s.generation),
            Some(GENERATION + 1)
        );
        new_task.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancelling_an_unknown_stream_is_a_no_op() {
        let (senders, plugin_rx, _job_rx) = test_senders();
        finish_stream(&mut HashMap::new(), &senders, stream_key(), None, None);
        assert!(drain_events(&plugin_rx).is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stale_finish_does_not_cancel_replacement_stream() {
        let (senders, plugin_rx, _job_rx) = test_senders();
        let mut active_streams = HashMap::new();
        let task = tokio::spawn(async { tokio::time::sleep(Duration::from_secs(60)).await });
        active_streams.insert(
            stream_key(),
            ActiveWebRequestStream {
                abort_handle: task.abort_handle(),
                generation: GENERATION + 1,
                state: Arc::new(Mutex::new(WebRequestStreamState::default())),
                context: BTreeMap::new(),
            },
        );

        finish_stream(
            &mut active_streams,
            &senders,
            stream_key(),
            Some(GENERATION),
            None,
        );

        assert!(active_streams.contains_key(&stream_key()));
        assert!(drain_events(&plugin_rx).is_empty());
        finish_stream(
            &mut active_streams,
            &senders,
            stream_key(),
            None,
            Some("cancelled".to_owned()),
        );
        assert!(task.await.unwrap_err().is_cancelled());
    }
}

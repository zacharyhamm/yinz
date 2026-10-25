use yinz_types::internal_ids::SessionName;
use yinz_types::units::ElapsedSeconds;
use {yinz_runtime::consts::YINZ_SESSION_INFO_CACHE_DIR, yinz_runtime::consts::YINZ_SOCK_DIR};

use std::str::FromStr;
use yinz_ipc::ipc::MAX_IPC_FRAME_LEN;
use yinz_runtime::errors::prelude::*;
use yinz_types::diagnostics::BackgroundJobContext;
use yinz_types::diagnostics::ContextType;
use yinz_types::input::layout::RunPlugin;
use {
    yinz_types::data::Event, yinz_types::data::HttpVerb, yinz_types::data::LayoutInfo,
    yinz_types::data::SessionInfo,
};

use futures_lite::io::AsyncReadExt;
use isahc::prelude::*;
use isahc::{config::RedirectPolicy, HttpClient, Request};

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use crate::plugins::{PluginId, PluginInstruction};
use crate::pty::PtyInstruction;
use crate::screen::ScreenInstruction;
use crate::thread_bus::Bus;
use crate::PaneId;
use crate::{ClientId, ServerInstruction};

/// Identifies an in-flight streaming web request. Scoped to the plugin and client that started
/// it, so one plugin cannot cancel another's stream.
pub type StreamKey = (PluginId, ClientId, String);

struct ActiveWebRequestStream {
    abort_handle: tokio::task::AbortHandle,
    generation: u64, // distinguishes successive requests using the same stream key
    runtime_generation: Option<u64>, // identifies the runtime that receives this request's events
    state: Arc<Mutex<WebRequestStreamState>>,
    context: BTreeMap<String, String>,
}

const OUTPUT_TRUNCATED: &[u8] = b"\n[truncated]";
// Bound each captured stream even when a plugin omits or inflates its requested limit.
const MAX_COMMAND_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

#[cfg(test)]
#[path = "unit/command_output_limits.rs"]
mod command_output_limits;

async fn read_command_stream<R>(
    mut stream: R,
    max_output_bytes: Option<usize>,
    mut on_chunk: impl FnMut(&[u8]),
) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let limit = max_output_bytes
        .unwrap_or(MAX_COMMAND_OUTPUT_BYTES)
        .min(MAX_COMMAND_OUTPUT_BYTES);
    let mut output = vec![];
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        let read = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer).await?;
        if read == 0 {
            break;
        }
        on_chunk(&buffer[..read]);
        let keep = limit.saturating_sub(output.len()).min(read);
        output.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    if truncated {
        output.truncate(limit.saturating_sub(OUTPUT_TRUNCATED.len()));
        output.extend_from_slice(&OUTPUT_TRUNCATED[..OUTPUT_TRUNCATED.len().min(limit)]);
    }
    Ok(output)
}

#[cfg(test)]
#[path = "unit/background_jobs_command_output_tests.rs"]
mod command_output_tests;

#[derive(Default)]
struct WebRequestStreamState {
    response_sent: bool,
    finished: bool,
    pending: Vec<u8>,
}

#[derive(
    Debug, Clone, Eq, PartialEq, Hash, strum_macros::IntoStaticStr, strum_macros::VariantNames,
)]
pub enum BackgroundJob {
    DisplayPaneError(Vec<PaneId>, String),
    AnimatePluginLoading(PluginId),       // u32 - plugin_id
    StopPluginLoadingAnimation(PluginId), // u32 - plugin_id
    WatchCommandPaletteSessions(bool),
    ReportSessionInfo(SessionName, SessionInfo), // String - session name
    ReportPluginList(BTreeMap<PluginId, RunPlugin>), // String - session name
    ReportLayoutInfo((String, BTreeMap<String, String>)), // BTreeMap<file_name, pane_contents>
    RunCommand(
        PluginId,
        ClientId,
        String,
        Vec<String>,
        BTreeMap<String, String>,
        PathBuf,
        BTreeMap<String, String>,
        Option<usize>,
        Option<bool>,
        Option<u64>,
    ), // command, args, env_variables, cwd, context, max_output_bytes, stream_output, generation
    WebRequest(
        PluginId,
        ClientId,
        String, // url
        HttpVerb,
        BTreeMap<String, String>, // headers
        Vec<u8>,                  // body
        BTreeMap<String, String>, // context
        Option<String>,           // stream_id, when set the response body is streamed back
        Option<String>,           // proxy, when set the request is sent through this proxy URL
        Option<u64>,              // generation of the requesting runtime, if any
    ),
    CancelWebRequest(PluginId, ClientId, String), // String - stream_id
    /// Sent by a streaming request task back to this thread when its body ends, so that the
    /// terminating chunk is always emitted from one place (see `finish_stream`)
    WebRequestStreamFinished(StreamKey, u64, Option<String>), // u64 - generation, Option<String> - error
    HighlightPanesWithMessage(Vec<PaneId>, String),
    DismissNotification(yinz_types::internal_ids::NotificationId, ElapsedSeconds), // notification id, timeout in seconds
    /// Wake the screen for a frame every `Duration`, until [`BackgroundJob::StopAnimationClock`].
    /// The period is `Screen`'s to choose - see `Screen::animation_clock_period` - because only
    /// the screen knows the coarsest cadence every live surface can live with.
    StartAnimationClock(Duration),
    StopAnimationClock,
    /// Give up waiting for a context menu's hook plugin and show the configured items instead.
    /// (request id, client id, timeout in milliseconds)
    ContextMenuTimeout(
        yinz_types::internal_ids::ContextMenuRequestId,
        ClientId,
        u64,
    ),
    RenderToClients,
    ClearHelpText {
        client_id: ClientId,
    },
    /// End the feedback flash `CopyLastCommandOutput` painted over the copied output.
    ClearCommandOutputFlash {
        pane_id: PaneId,
    },
    FlashPaneBell(Vec<PaneId>),
    StopFlashPaneBell(Vec<PaneId>),
    FlashTabBell(yinz_types::internal_ids::TabId), // usize = tab_id
    StopFlashTabBell(yinz_types::internal_ids::TabId), // usize = tab_id
    StartNestedGuestPing(PaneId),
    StopNestedGuestPing(PaneId),
    Exit,
}

impl From<&BackgroundJob> for BackgroundJobContext {
    fn from(background_job: &BackgroundJob) -> Self {
        // The two enums are kept name-for-name; `instruction_context_tests` fails the test run if
        // a variant is added without either a matching `BackgroundJobContext` or an arm here.
        match background_job {
            BackgroundJob::CancelWebRequest(..) | BackgroundJob::WebRequestStreamFinished(..) => {
                BackgroundJobContext::WebRequest
            },
            other => {
                let name: &'static str = other.into();
                BackgroundJobContext::from_str(name).unwrap_or_else(|_| {
                    debug_assert!(false, "no BackgroundJobContext variant named {name}");
                    BackgroundJobContext::Exit
                })
            },
        }
    }
}

static LONG_FLASH_DURATION_MS: u64 = 1000;
static FLASH_DURATION_MS: u64 = 400; // Doherty threshold
static SESSION_METADATA_WRITE_INTERVAL_MS: u64 = 1000;
static UPDATE_AND_REPORT_CWDS_INTERVAL_MS: u64 = 1000;
static DEFAULT_SERIALIZATION_INTERVAL: u64 = 60000;
static REPAINT_DELAY_MS: u64 = 10;
/// Successful streamed responses have no total cap - a long generation or a large download is
/// legitimate - but a connection that stops producing bytes for this long is reaped. Responses
/// buffered in host memory use `MAX_BUFFERED_WEB_RESPONSE_BYTES` instead.
static WEB_REQUEST_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// the idle timeout above is a transfer-rate guard and does not cover connection setup, so a
/// blackholed host needs its own bound
static WEB_REQUEST_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const WEB_REQUEST_STREAM_CHUNK_SIZE: usize = 8192;
const MAX_WEB_REQUEST_REDIRECTS: usize = 10;
/// Borrows the IPC frame length as a size that is already agreed to be "big enough", but the
/// events carrying these bodies are never IPC-framed: `Event::WebRequestResult` reaches the
/// plugin thread on an in-process channel and is decoded straight into the wasm guest, so a body
/// admitted at exactly this limit needs no envelope allowance for its status, headers and
/// context. If a result ever crosses a framed transport, budget for the envelope here (see
/// `screen/warp_ops.rs`, which keeps to half a frame for that reason).
const MAX_BUFFERED_WEB_RESPONSE_BYTES: usize = MAX_IPC_FRAME_LEN;
/// How long a streamed byte may wait to be joined by the ones behind it.
///
/// A token stream comes off the socket in ~50 small reads a second. Forwarding each as its own
/// event gave every one of them a `PluginInstruction`, a plugin `update()` and a place in the
/// plugin thread's queue ahead of whatever the user was typing. Batching them costs a reply
/// nothing a reader can see - the frames still arrive in order, and this is well inside the 100ms
/// a plugin repaints at - while cutting that traffic by roughly a third.
static WEB_REQUEST_STREAM_COALESCE_WINDOW: Duration = Duration::from_millis(20);
/// The most that may accumulate inside one window, so a fast producer is bounded by size as well
/// as by time. Two reads' worth: enough to be worth joining, small enough to stay responsive.
const WEB_REQUEST_STREAM_COALESCE_MAX: usize = WEB_REQUEST_STREAM_CHUNK_SIZE * 2;
static HELP_TEXT_DEBOUNCE_DURATION: u64 = 5000;
static COMMAND_OUTPUT_FLASH_DURATION_MS: u64 = 400;

#[derive(Clone)]
pub struct SessionScanState {
    pub current_session_name: Arc<Mutex<SessionName>>,
    pub current_session_info: Arc<Mutex<SessionInfo>>,
    pub current_session_plugin_list: Arc<Mutex<BTreeMap<PluginId, RunPlugin>>>,
}

static SESSION_SCAN_STATE: std::sync::OnceLock<SessionScanState> = std::sync::OnceLock::new();

pub fn session_scan_state() -> Option<&'static SessionScanState> {
    SESSION_SCAN_STATE.get()
}

pub fn background_jobs_main(
    bus: Bus<BackgroundJob>,
    serialization_interval: Option<ElapsedSeconds>,
    disable_session_metadata: bool,
) -> Result<()> {
    background_jobs_with_runtime(
        bus,
        serialization_interval,
        disable_session_metadata,
        crate::global_async_runtime::get_tokio_runtime(),
    )
}

fn background_jobs_with_runtime(
    bus: Bus<BackgroundJob>,
    serialization_interval: Option<ElapsedSeconds>,
    disable_session_metadata: bool,
    runtime: &tokio::runtime::Runtime,
) -> Result<()> {
    let err_context = || "failed to write to pty".to_string();
    let mut running_jobs: HashMap<BackgroundJob, Instant> = HashMap::new();
    // the shared animation clock's cancel flag and the period it is ticking at, `Some` while it is
    // running.
    let mut animation_clock: Option<(Arc<AtomicBool>, Duration)> = None;
    let current_session_name = Arc::new(Mutex::new(SessionName::default()));
    let current_session_info = Arc::new(Mutex::new(SessionInfo::default()));
    let current_session_plugin_list: Arc<Mutex<BTreeMap<PluginId, RunPlugin>>> =
        Arc::new(Mutex::new(BTreeMap::new()));
    let current_session_layout = Arc::new(Mutex::new((String::new(), BTreeMap::new())));

    let _ = SESSION_SCAN_STATE.set(SessionScanState {
        current_session_name: current_session_name.clone(),
        current_session_info: current_session_info.clone(),
        current_session_plugin_list: current_session_plugin_list.clone(),
    });
    let mut command_palette_sessions: Option<Arc<AtomicBool>> = None;
    let last_render_request: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let pending_help_text_clear: Arc<Mutex<HashMap<ClientId, Instant>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_command_output_flash_clear: Arc<Mutex<HashMap<PaneId, Instant>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let mut flashing_pane_bells: HashMap<PaneId, Arc<AtomicBool>> = HashMap::new();
    let mut flashing_tab_bells: HashMap<yinz_types::internal_ids::TabId, Arc<AtomicBool>> =
        HashMap::new();
    let mut nested_guest_pings: HashMap<PaneId, Arc<AtomicBool>> = HashMap::new();
    let mut active_streams: HashMap<StreamKey, ActiveWebRequestStream> = HashMap::new();
    let mut next_stream_generation = 0u64;

    let http_client = web_http_client();

    let serialization_task = {
        let senders = bus.senders.clone();
        let serialization_period = serialization_interval
            .map(|seconds| Duration::from_secs(seconds.get()))
            .unwrap_or(Duration::from_millis(DEFAULT_SERIALIZATION_INTERVAL));
        runtime.spawn(async move {
            let mut ticker = tokio::time::interval(serialization_period);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let _ = senders.send_to_screen(ScreenInstruction::SerializeLayoutForResurrection);
            }
        })
    };

    let cwd_task = {
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
        })
    };

    let checkpoint_task = if !disable_session_metadata {
        let session_checkpoint = bus.senders.session_checkpoint.clone();
        let current_session_name = current_session_name.clone();
        let current_session_info = current_session_info.clone();
        let current_session_layout = current_session_layout.clone();
        Some(runtime.spawn(async move {
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
                session_checkpoint.write(name, info, &layout).non_fatal();
                // An overdue tick can be ready immediately; let abort stop catch-up writes.
                tokio::task::yield_now().await;
            }
        }))
    } else {
        None
    };

    let result = loop {
        let (event, mut err_ctx) = match bus.recv().with_context(err_context) {
            Ok(event) => event,
            Err(error) => break Err(error),
        };
        yinz_runtime::errors::add_call(&mut err_ctx, ContextType::BackgroundJob((&event).into()));
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
            // Registration only. The dots used to have one tokio task per plugin, each with its
            // own 500ms sleep - at session start that is a task per builtin plugin, all of them
            // sleeping in parallel to drive four characters. They take their frames from the
            // shared clock now, and their offset is derived from how long the load has been
            // running rather than counted per wakeup.
            BackgroundJob::AnimatePluginLoading(pid) => {
                let _ = bus
                    .senders
                    .send_to_screen(ScreenInstruction::StartPluginLoadingAnimation(pid));
            },
            BackgroundJob::StopPluginLoadingAnimation(pid) => {
                let _ = bus
                    .senders
                    .send_to_screen(ScreenInstruction::StopPluginLoadingAnimation(pid));
            },
            // One clock for every animated surface, not one per surface: they are all sampled by
            // the same `render_to_clients` at the same instant, so a second clock would only buy
            // duplicate wakeups. What varies is its *period*, and the screen sends this again with
            // a new one whenever the coarsest cadence its live surfaces can live with changes - so
            // a request for the period already running is a no-op, and any other request replaces
            // the task rather than adding to it.
            BackgroundJob::StartAnimationClock(period) => {
                if animation_clock
                    .as_ref()
                    .is_some_and(|(flag, running_period)| {
                        *running_period == period && flag.load(Ordering::SeqCst)
                    })
                {
                    continue;
                }
                if let Some((previous, _)) = animation_clock.take() {
                    previous.store(false, Ordering::SeqCst);
                }
                let running = Arc::new(AtomicBool::new(true));
                runtime.spawn({
                    let senders = bus.senders.clone();
                    let running = running.clone();
                    async move {
                        let mut tick = tokio::time::interval(period);
                        // `Skip` rather than the default `Burst`: a tick the screen thread was too
                        // busy to service must be dropped, not replayed as a backlog of wakeups
                        // that all arrive at once and then ask for the same frame several times.
                        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        loop {
                            tick.tick().await;
                            if !running.load(Ordering::SeqCst) {
                                break;
                            }
                            let _ = senders.send_to_screen(ScreenInstruction::AnimationTick);
                        }
                    }
                });
                animation_clock = Some((running, period));
            },
            BackgroundJob::StopAnimationClock => {
                if let Some((running, _)) = animation_clock.take() {
                    running.store(false, Ordering::SeqCst);
                }
            },
            BackgroundJob::WatchCommandPaletteSessions(watch) => {
                if let Some(running) = command_palette_sessions.take() {
                    running.store(false, Ordering::SeqCst);
                }
                if watch {
                    let running = Arc::new(AtomicBool::new(true));
                    command_palette_sessions = Some(running.clone());
                    let senders = bus.senders.clone();
                    let name = current_session_name.clone();
                    let info = current_session_info.clone();
                    let plugins = current_session_plugin_list.clone();
                    runtime.spawn(async move {
                        while running.load(Ordering::SeqCst) {
                            let (Ok(name), Ok(current), Ok(plugins)) = (
                                name.lock().map(|name| name.clone()),
                                info.lock().map(|info| info.clone()),
                                plugins.lock().map(|plugins| plugins.clone()),
                            ) else {
                                log::error!(
                                    "cannot scan palette sessions: session state lock poisoned"
                                );
                                break;
                            };
                            let scanned = tokio::task::spawn_blocking(move || {
                                scan_session_list_default_dirs(
                                    &name,
                                    &current.available_layouts,
                                    &plugins,
                                )
                            })
                            .await;
                            if !running.load(Ordering::SeqCst) {
                                break;
                            }
                            match scanned {
                                Ok((live, saved)) => {
                                    if senders
                                        .send_to_screen(ScreenInstruction::UpdateSessionInfos(
                                            live, saved,
                                        ))
                                        .is_err()
                                    {
                                        break;
                                    }
                                },
                                Err(error) => {
                                    log::error!("failed to scan palette sessions: {error}")
                                },
                            }
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    });
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
                max_output_bytes,
                stream_output,
                generation,
            ) => {
                runtime.spawn({
                    let senders = bus.senders.clone();
                    async move {
                        let output = async {
                            let mut child = tokio::process::Command::new(&command)
                                .args(&args)
                                .envs(env_variables)
                                .current_dir(cwd)
                                .stdin(std::process::Stdio::null())
                                .stdout(std::process::Stdio::piped())
                                .stderr(std::process::Stdio::piped())
                                .spawn()?;
                            let stdout = child.stdout.take().ok_or_else(|| {
                                std::io::Error::other("command stdout was not piped")
                            })?;
                            let stderr = child.stderr.take().ok_or_else(|| {
                                std::io::Error::other("command stderr was not piped")
                            })?;
                            let emit = |stderr, data: &[u8]| {
                                if stream_output == Some(true) {
                                    let _ = senders.send_to_plugin(
                                        PluginInstruction::for_plugin_event(
                                            plugin_id,
                                            client_id,
                                            generation,
                                            Event::RunCommandOutput {
                                                data: data.to_vec(),
                                                stderr,
                                                context: context.clone(),
                                            },
                                        ),
                                    );
                                }
                            };
                            let (stdout, stderr, status) = tokio::join!(
                                read_command_stream(stdout, max_output_bytes, |data| emit(
                                    false, data
                                )),
                                read_command_stream(stderr, max_output_bytes, |data| emit(
                                    true, data
                                )),
                                child.wait(),
                            );
                            Ok::<_, std::io::Error>((status?, stdout?, stderr?))
                        }
                        .await;
                        match output {
                            Ok((status, stdout, stderr)) => {
                                let exit_code = status.code();
                                let _ =
                                    senders.send_to_plugin(PluginInstruction::for_plugin_event(
                                        plugin_id,
                                        client_id,
                                        generation,
                                        Event::RunCommandResult(exit_code, stdout, stderr, context),
                                    ));
                            },
                            Err(e) => {
                                log::error!("Failed to run command: {}", e);
                                let stdout = vec![];
                                let stderr = format!("{}", e).as_bytes().to_vec();
                                let exit_code = Some(2);
                                let _ =
                                    senders.send_to_plugin(PluginInstruction::for_plugin_event(
                                        plugin_id,
                                        client_id,
                                        generation,
                                        Event::RunCommandResult(exit_code, stdout, stderr, context),
                                    ));
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
                proxy,
                runtime_generation,
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
                            let event = match buffered_web_request(
                                url,
                                verb,
                                headers,
                                body,
                                http_client,
                                proxy,
                            )
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
                            let _ = senders.send_to_plugin(PluginInstruction::for_plugin_event(
                                plugin_id,
                                client_id,
                                runtime_generation,
                                event,
                            ));
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
                            proxy,
                            runtime_generation,
                        ));
                        // ponytail: unloading a plugin does not cancel its in-flight streams -
                        // they drain to completion and the events are dropped by
                        // plugin-id and runtime-generation filters. Add a
                        // BackgroundJob::CancelPluginWebRequests(plugin_id) from plugin_bridge's
                        // unload path if abandoned streams become a bandwidth problem.
                        active_streams.insert(
                            key,
                            ActiveWebRequestStream {
                                abort_handle: handle.abort_handle(),
                                generation,
                                runtime_generation,
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
                                if let Some(last_render_request) = *last_render_request
                                    && last_render_request > task_start_time
                                {
                                    // another render request was received while we were
                                    // sleeping, schedule this job again so that we can also
                                    // render that request
                                    let _ = senders
                                        .send_to_background_jobs(BackgroundJob::RenderToClients);
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
            // deliberately not gated on job_already_running: every notification has its own id and
            // needs its own timer
            BackgroundJob::DismissNotification(notification_id, timeout_secs) => {
                runtime.spawn({
                    let senders = bus.senders.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(timeout_secs.get())).await;
                        let _ = senders.send_to_screen(ScreenInstruction::DismissNotification(
                            notification_id,
                        ));
                    }
                });
            },
            // Like `DismissNotification`, deliberately not gated on `job_already_running`: every
            // menu has its own request id and needs its own timer, and a right-click while another
            // menu's timer is pending is ordinary.
            BackgroundJob::ContextMenuTimeout(request_id, client_id, timeout_ms) => {
                runtime.spawn({
                    let senders = bus.senders.clone();
                    async move {
                        tokio::time::sleep(Duration::from_millis(timeout_ms)).await;
                        let _ = senders.send_to_screen(ScreenInstruction::ContextMenuTimedOut(
                            request_id, client_id,
                        ));
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
            BackgroundJob::ClearCommandOutputFlash { pane_id } => {
                // debounced like ClearHelpText: a second copy restarts the flash rather than
                // ending it early
                let should_spawn = {
                    let mut pending = pending_command_output_flash_clear.lock().unwrap();
                    let should_spawn = !pending.contains_key(&pane_id);
                    pending.insert(pane_id, Instant::now());
                    should_spawn
                };

                if should_spawn {
                    runtime.spawn({
                        let senders = bus.senders.clone();
                        let pending = pending_command_output_flash_clear.clone();
                        let flash_duration =
                            Duration::from_millis(COMMAND_OUTPUT_FLASH_DURATION_MS);
                        async move {
                            tokio::time::sleep(flash_duration).await;
                            loop {
                                let next_sleep_duration = {
                                    let mut pending = pending.lock().unwrap();
                                    match pending.get(&pane_id) {
                                        Some(&last_flash_time) => {
                                            let time_since_flash =
                                                Instant::now().duration_since(last_flash_time);
                                            if time_since_flash >= flash_duration {
                                                pending.remove(&pane_id);
                                                None
                                            } else {
                                                Some(
                                                    flash_duration.saturating_sub(time_since_flash),
                                                )
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
                                            ServerInstruction::ClearCommandOutputFlash(pane_id),
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
                let mut flashes = Vec::with_capacity(pane_ids.len());
                for &pane_id in &pane_ids {
                    let flag = Arc::new(AtomicBool::new(true));
                    if let Some(previous) = flashing_pane_bells.insert(pane_id, flag.clone()) {
                        previous.store(false, Ordering::SeqCst);
                    }
                    flashes.push((pane_id, flag));
                }
                let _ = bus
                    .senders
                    .send_to_screen(ScreenInstruction::FlashPaneFrameColor(pane_ids));
                runtime.spawn({
                    let senders = bus.senders.clone();
                    async move {
                        // the override eases itself out over exactly this window; the clear that
                        // follows is what actually frees the slot, and stays on the old schedule
                        tokio::time::sleep(std::time::Duration::from_millis(FLASH_DURATION_MS))
                            .await;
                        let pane_ids = flashes
                            .into_iter()
                            .filter_map(|(pane_id, flag)| {
                                flag.load(Ordering::SeqCst).then_some(pane_id)
                            })
                            .collect::<Vec<_>>();
                        if !pane_ids.is_empty() {
                            let _ = senders.send_to_screen(
                                ScreenInstruction::ClearPaneFrameColorOverride(pane_ids),
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
                if let Some(previous) = flashing_tab_bells.insert(tab_id, is_flashing.clone()) {
                    previous.store(false, Ordering::SeqCst);
                }
                let _ = bus
                    .senders
                    .send_to_screen(ScreenInstruction::SetTabBellFlash(tab_id, true));
                runtime.spawn({
                    let senders = bus.senders.clone();
                    let flag = is_flashing.clone();
                    async move {
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
                log::info!("shutdown: background jobs received Exit");
                break Ok(());
            },
        }
    };
    if let Some((clock, _)) = animation_clock {
        clock.store(false, Ordering::SeqCst);
    }
    for flag in command_palette_sessions
        .iter()
        .chain(nested_guest_pings.values())
        .chain(flashing_pane_bells.values())
        .chain(flashing_tab_bells.values())
    {
        flag.store(false, Ordering::SeqCst);
    }
    serialization_task.abort();
    cwd_task.abort();
    runtime.block_on(async {
        let _ = tokio::join!(serialization_task, cwd_task);
    });
    if let Some(task) = checkpoint_task {
        let started = Instant::now();
        log::info!(
            "shutdown: aborting checkpoint task; {}",
            bus.senders.session_checkpoint.progress()
        );
        task.abort();
        #[cfg(test)]
        if let Some(after_abort) = &bus.senders.session_checkpoint.after_abort {
            after_abort();
        }
        // Cancellation takes effect at an await point. Join so an in-flight synchronous
        // checkpoint cannot publish after the server confirms shutdown.
        runtime.block_on(async {
            let mut task = task;
            loop {
                tokio::select! {
                    _ = &mut task => break,
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        log::warn!("shutdown: checkpoint join waiting after {:?}; {}",
                            started.elapsed(), bus.senders.session_checkpoint.progress());
                    }
                }
            }
        });
        log::info!(
            "shutdown: checkpoint task joined after {:?}",
            started.elapsed()
        );
    }
    result
}

/// The client every plugin web request goes through. Shared with the tests so the redirect
/// checks below are exercised against the shipped configuration: `RedirectPolicy::None` is what
/// hands each hop to `send_web_request` for vetting, and a client that follows redirects itself
/// would carry curl straight past `web_url`.
pub fn web_http_client() -> Option<HttpClient> {
    HttpClient::builder()
        .connect_timeout(WEB_REQUEST_CONNECT_TIMEOUT)
        // the idle timeout is per-request, in send_web_request
        // Redirects are followed by send_web_request so every target can be checked before curl
        // sees it.
        .redirect_policy(RedirectPolicy::None)
        .build()
        .ok()
}

fn web_url(url: &str) -> Result<url::Url> {
    let url = url::Url::parse(url).with_context(|| format!("invalid web request URL: {url}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(anyhow!(
            "web requests only support http and https URLs: {url}"
        ));
    }
    Ok(url)
}

/// An allowlist, not a denylist: a plugin's custom header (`x-api-key`, `private-token`, …) may be
/// a key, and the host cannot tell which, so only headers that cannot carry one cross origins.
fn scrub_redirect_headers(headers: &mut BTreeMap<String, String>) {
    headers.retain(|name, _| {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "accept"
                | "accept-encoding"
                | "accept-language"
                | "content-language"
                | "content-type"
                | "user-agent"
                | "range"
                | "cache-control"
                | "pragma"
        )
    });
}

/// Build and send a web request, validating every redirect before curl sees it.
async fn send_web_request(
    url: String,
    mut verb: HttpVerb,
    mut headers: BTreeMap<String, String>,
    mut body: Vec<u8>,
    http_client: HttpClient,
    proxy: Option<String>,
) -> Result<isahc::Response<isahc::AsyncBody>> {
    let mut url = web_url(&url)?;
    for redirects_followed in 0..=MAX_WEB_REQUEST_REDIRECTS {
        let mut request = match verb {
            HttpVerb::Get => Request::get(url.as_str()),
            HttpVerb::Post => Request::post(url.as_str()),
            HttpVerb::Put => Request::put(url.as_str()),
            HttpVerb::Delete => Request::delete(url.as_str()),
            HttpVerb::Patch => Request::patch(url.as_str()),
        };
        for (header, value) in &headers {
            request = request.header(header.as_str(), value);
        }
        if let Some(proxy) = &proxy {
            match proxy.parse::<isahc::http::Uri>() {
                Ok(uri) => request = request.proxy(Some(uri)),
                Err(e) => log::error!("Ignoring invalid web request proxy {:?}: {}", proxy, e),
            }
        }
        request = request.low_speed_timeout(1, WEB_REQUEST_IDLE_TIMEOUT);
        let response = if body.is_empty() {
            http_client.send_async(request.body(())?).await?
        } else {
            http_client.send_async(request.body(body.clone())?).await?
        };

        let location = if response.status().is_redirection() {
            response
                .headers()
                .get(isahc::http::header::LOCATION)
                .and_then(|location| location.to_str().ok())
                .and_then(|location| url.join(location).ok())
        } else {
            None
        };
        let Some(next_url) = location else {
            return Ok(response);
        };
        if redirects_followed == MAX_WEB_REQUEST_REDIRECTS {
            return Err(anyhow!(
                "web request exceeded {MAX_WEB_REQUEST_REDIRECTS} redirects"
            ));
        }
        let next_url = web_url(next_url.as_str())?;

        if url.origin() != next_url.origin() {
            scrub_redirect_headers(&mut headers);
        }
        let status = response.status().as_u16();
        if status == 303 || (matches!(status, 301 | 302) && verb == HttpVerb::Post) {
            verb = HttpVerb::Get;
            body.clear();
            // RFC 9110 section 15.4: body metadata cannot describe the rewritten GET.
            headers.retain(|name, _| {
                !matches!(
                    name.to_ascii_lowercase().as_str(),
                    "content-encoding"
                        | "content-language"
                        | "content-location"
                        | "content-type"
                        | "content-length"
                        | "content-range"
                        | "digest"
                        | "last-modified"
                        | "transfer-encoding"
                        | "trailer"
                        | "expect"
                )
            });
        }
        url = next_url;
    }
    unreachable!()
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
    proxy: Option<String>,
) -> Result<(u16, BTreeMap<String, String>, Vec<u8>)> {
    let mut res = send_web_request(url, verb, headers, body, http_client, proxy).await?;
    let status_code = res.status().as_u16();
    let headers = response_headers(&res);
    let body = read_bounded_web_body(res.body_mut(), MAX_BUFFERED_WEB_RESPONSE_BYTES).await?;
    Ok((status_code, headers, body))
}

async fn read_bounded_web_body<R>(body: &mut R, limit: usize) -> Result<Vec<u8>>
where
    R: futures_lite::io::AsyncRead + Unpin,
{
    let mut bytes = vec![];
    body.take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > limit {
        return Err(anyhow!(
            "web response exceeded the {limit}-byte buffered-body limit"
        ));
    }
    Ok(bytes)
}

/// Sends the leading `WebRequestResult` and then coalesced `WebRequestStreamChunk`s from the
/// socket. Never emits the terminating chunk itself: it reports back with
/// `BackgroundJob::WebRequestStreamFinished` so the terminator is emitted from a single place
/// even when this task is aborted mid-read by a cancellation.
// Clippy allow kept — see REVIEW_CHECKLIST.md "Many clippy allows hide warnings".
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
    proxy: Option<String>,
    runtime_generation: Option<u64>,
) {
    let (plugin_id, client_id, stream_id) = key.clone();
    let emit_event = |event: Event| {
        let _ = senders.send_to_plugin(PluginInstruction::for_plugin_event(
            plugin_id,
            client_id,
            runtime_generation,
            event,
        ));
    };
    let send_event = |event: Event| {
        let mut state = state.lock().unwrap();
        if state.finished {
            return;
        }
        if matches!(event, Event::WebRequestResult(..)) {
            state.response_sent = true;
        }
        emit_event(event);
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

    let mut res = match send_web_request(url, verb, headers, body, http_client, proxy).await {
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
        let (body, error) =
            match read_bounded_web_body(res.body_mut(), MAX_BUFFERED_WEB_RESPONSE_BYTES).await {
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
    let error = forward_streamed_body(res.body_mut(), &stream_id, &state, &emit_event).await;
    finished(error);
}

async fn forward_streamed_body<R, F>(
    body: &mut R,
    stream_id: &str,
    state: &Arc<Mutex<WebRequestStreamState>>,
    send_event: F,
) -> Option<String>
where
    R: futures_lite::io::AsyncRead + Unpin,
    F: Fn(Event),
{
    // Reads are coalesced over a short window rather than forwarded one-for-one: a token stream
    // produces ~50 socket reads a second, and each one used to be its own `PluginInstruction`,
    // its own plugin `update()` and its own trip through the plugin thread's queue - the same
    // queue the user's keystrokes have to cross. Concatenating is safe because the framing is
    // reassembled on the far side (agent feeds these to an `SseParser`, which already handles a
    // frame split across reads); all this changes is how many events carry the same bytes.
    let mut buf = [0u8; WEB_REQUEST_STREAM_CHUNK_SIZE];
    // Every exit from the loop below owes the pending bytes to the plugin before the terminator
    // goes out. They live in the shared state so cancellation can keep that promise too.
    macro_rules! flush {
        () => {
            let mut state = state.lock().unwrap();
            if !state.finished && !state.pending.is_empty() {
                send_event(Event::WebRequestStreamChunk {
                    stream_id: stream_id.to_owned(),
                    chunk: std::mem::take(&mut state.pending),
                    done: false,
                    error: None,
                });
            }
        };
    }
    // When the bytes we are holding have to go out, whether or not more have arrived by then.
    // `None` means nothing is being held and the read below may wait indefinitely, exactly as it
    // used to.
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        // The read future is created once and then *held across* any deadline that expires while
        // it is pending, rather than being re-issued. `AsyncRead::read` makes no cancellation-
        // safety promise: dropping a partially-completed read could take buffered bytes with it,
        // which on a token stream means silently corrupting the reply. So the timeout only ever
        // borrows the future, and the future itself lives until it resolves.
        let outcome: std::io::Result<usize> = {
            let read = body.read(&mut buf);
            tokio::pin!(read);
            loop {
                let finished = match deadline {
                    None => read.as_mut().await,
                    Some(at) => match tokio::time::timeout_at(at, read.as_mut()).await {
                        Ok(finished) => finished,
                        // The window expired with the socket still quiet, which is precisely when
                        // what we are holding should go out. The read carries on untouched.
                        Err(_) => {
                            flush!();
                            deadline = None;
                            continue;
                        },
                    },
                };
                break finished;
            }
        };
        match outcome {
            Ok(0) => {
                flush!();
                break None;
            },
            Ok(bytes_read) => {
                let pending_len = {
                    let mut state = state.lock().unwrap();
                    if state.finished {
                        return None;
                    }
                    state.pending.extend_from_slice(&buf[..bytes_read]);
                    state.pending.len()
                };
                // A window bounds latency, not memory - a fast producer must not be allowed to
                // grow this without limit between deadlines.
                if pending_len >= WEB_REQUEST_STREAM_COALESCE_MAX {
                    flush!();
                    deadline = None;
                } else if deadline.is_none() {
                    deadline =
                        Some(tokio::time::Instant::now() + WEB_REQUEST_STREAM_COALESCE_WINDOW);
                }
            },
            Err(e) => {
                log::error!("Web request stream failed: {}", e);
                // What arrived before the failure is still real output, and the plugin has a
                // better chance of saying something useful with it than without.
                flush!();
                break Some(e.to_string());
            },
        }
    }
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
        let _ = senders.send_to_plugin(PluginInstruction::for_plugin_event(
            plugin_id,
            client_id,
            stream.runtime_generation,
            Event::WebRequestResult(
                400,
                BTreeMap::new(),
                error
                    .clone()
                    .unwrap_or_else(|| "cancelled".to_owned())
                    .into_bytes(),
                stream.context,
            ),
        ));
        state.response_sent = true;
    }
    if !state.pending.is_empty() {
        let _ = senders.send_to_plugin(PluginInstruction::for_plugin_event(
            plugin_id,
            client_id,
            stream.runtime_generation,
            Event::WebRequestStreamChunk {
                stream_id: stream_id.clone(),
                chunk: std::mem::take(&mut state.pending),
                done: false,
                error: None,
            },
        ));
    }
    state.finished = true;
    let _ = senders.send_to_plugin(PluginInstruction::for_plugin_event(
        plugin_id,
        client_id,
        stream.runtime_generation,
        Event::WebRequestStreamChunk {
            stream_id,
            chunk: vec![],
            done: true,
            error,
        },
    ));
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

/// Serializes renames with both periodic checkpoints and explicit saves. A snapshot can wait
/// on either thread after a rename, so its destination and metadata use the current name.
pub struct SessionCheckpointWriter {
    cache_dir: PathBuf,
    name: Mutex<Option<SessionName>>,
    progress: Mutex<CheckpointProgress>,
    #[cfg(test)]
    before_write: Option<Box<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    after_abort: Option<Box<dyn Fn() + Send + Sync>>,
}

#[derive(Default)]
struct CheckpointProgress {
    waiting: usize,
    phase: Option<(&'static str, Instant)>,
}

impl Default for SessionCheckpointWriter {
    fn default() -> Self {
        Self {
            cache_dir: YINZ_SESSION_INFO_CACHE_DIR.clone(),
            name: Mutex::new(None),
            progress: Mutex::default(),
            #[cfg(test)]
            before_write: None,
            #[cfg(test)]
            after_abort: None,
        }
    }
}

impl SessionCheckpointWriter {
    fn progress(&self) -> String {
        let progress = self
            .progress
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        format!(
            "writer_mutex_waiters={} active={:?}",
            progress.waiting,
            progress
                .phase
                .map(|(phase, since)| (phase, since.elapsed()))
        )
    }
    pub fn write(
        &self,
        session_name: SessionName,
        mut session_info: SessionInfo,
        layout: &(String, BTreeMap<String, String>),
    ) -> Result<()> {
        #[cfg(test)]
        if let Some(before_write) = &self.before_write {
            before_write();
        }
        self.progress
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .waiting += 1;
        let name = self.name.lock();
        self.progress
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .waiting -= 1;
        let mut name = name.map_err(|_| anyhow!("checkpoint lock poisoned"))?;
        let name = name.get_or_insert(session_name);
        session_info.name = name.clone();
        let result = yinz_runtime::session_checkpoint::write_checkpoint_with_progress(
            &self.cache_dir.join(&*name),
            &yinz_session::metadata::session_info::to_string(&session_info)
                .context("Failed to encode session metadata")?,
            layout,
            |phase| {
                self.progress
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .phase = Some((phase, Instant::now()));
            },
        );
        self.progress
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .phase = None;
        result
    }

    pub fn rename(&self, old: &SessionName, new: &SessionName) -> Result<()> {
        let mut name = self
            .name
            .lock()
            .map_err(|_| anyhow!("checkpoint lock poisoned"))?;
        let old_dir = self.cache_dir.join(old);
        // A session renamed before its first checkpoint has no folder yet.
        if old_dir
            .try_exists()
            .context("Failed to inspect session checkpoint folder")?
        {
            fs::rename(&old_dir, self.cache_dir.join(new))
                .context("Failed to rename session checkpoint folder")?;
        }
        *name = Some(new.clone());
        Ok(())
    }
}

pub fn scan_session_list(
    current_session_name: &SessionName,
    available_layouts: &[LayoutInfo],
    current_session_plugin_list: &BTreeMap<PluginId, RunPlugin>,
    sock_dir: &Path,
    session_info_cache_dir: &Path,
) -> (
    BTreeMap<SessionName, SessionInfo>,
    BTreeMap<SessionName, Duration>,
) {
    let mut session_infos_on_machine = yinz_session::sessions::read_live_session_states(
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
    current_session_name: &SessionName,
    available_layouts: &[LayoutInfo],
    current_session_plugin_list: &BTreeMap<PluginId, RunPlugin>,
) -> (
    BTreeMap<SessionName, SessionInfo>,
    BTreeMap<SessionName, Duration>,
) {
    scan_session_list(
        current_session_name,
        available_layouts,
        current_session_plugin_list,
        &YINZ_SOCK_DIR,
        &YINZ_SESSION_INFO_CACHE_DIR,
    )
}

fn find_resurrectable_sessions(
    session_infos_on_machine: &BTreeMap<SessionName, SessionInfo>,
    session_info_cache_dir: &Path,
) -> BTreeMap<SessionName, Duration> {
    match fs::read_dir(session_info_cache_dir) {
        Ok(files_in_session_info_folder) => {
            let files_that_are_folders = files_in_session_info_folder
                .filter_map(|f| f.ok().map(|f| f.path()))
                .filter(|f| f.is_dir());
            files_that_are_folders
                .filter_map(|folder_name| {
                    let session_name = SessionName::new(folder_name.file_name()?.to_str()?);
                    if session_infos_on_machine.contains_key(&session_name) {
                        // this is not a dead session...
                        return None;
                    }
                    let checkpoint =
                        yinz_runtime::session_checkpoint::SessionCheckpoint::read(&folder_name)
                            .to_log()
                            .ok()?;
                    let layout_file_name = checkpoint.layout_path();
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
                    // A layout whose creation stamp cannot be read (a filesystem without
                    // birth times, or a clock that has gone backwards) is listed as dead "0s
                    // ago" rather than dropped. Neither failure can be produced portably in a
                    // test, so this fallback is documented here and not pinned by one.
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

#[cfg(test)]
#[cfg(unix)]
#[path = "unit/background_jobs.rs"]
mod tests;

#[cfg(test)]
#[path = "unit/background_jobs_web_request_stream_tests.rs"]
mod web_request_stream_tests;

#[cfg(test)]
#[path = "unit/checkpoint_shutdown_tests.rs"]
mod checkpoint_shutdown_tests;

#[cfg(test)]
#[path = "unit/background_job_timers.rs"]
mod background_job_timers;

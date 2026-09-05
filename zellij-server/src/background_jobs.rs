use zellij_utils::consts::{
    session_info_folder_for_session, ZELLIJ_SESSION_INFO_CACHE_DIR, ZELLIJ_SOCK_DIR,
};

use std::str::FromStr;
use zellij_utils::data::{Event, HttpVerb, LayoutInfo, SessionInfo};
use zellij_utils::errors::{prelude::*, BackgroundJobContext, ContextType};
use zellij_utils::input::layout::RunPlugin;
use zellij_utils::ipc::MAX_IPC_FRAME_LEN;

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

const OUTPUT_TRUNCATED: &[u8] = b"\n[truncated]";

async fn read_command_stream<R>(
    mut stream: R,
    max_output_bytes: Option<usize>,
) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut output = vec![];
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        let read = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer).await?;
        if read == 0 {
            break;
        }
        match max_output_bytes {
            Some(limit) => {
                let keep = limit.saturating_sub(output.len()).min(read);
                output.extend_from_slice(&buffer[..keep]);
                truncated |= keep < read;
            },
            None => output.extend_from_slice(&buffer[..read]),
        }
    }
    if truncated {
        let limit = max_output_bytes.unwrap_or_default();
        output.truncate(limit.saturating_sub(OUTPUT_TRUNCATED.len()));
        output.extend_from_slice(&OUTPUT_TRUNCATED[..OUTPUT_TRUNCATED.len().min(limit)]);
    }
    Ok(output)
}

#[cfg(test)]
mod command_output_tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn bounded_command_output_is_truncated_and_drained() {
        let (mut writer, reader) = tokio::io::duplex(8);
        let write = tokio::spawn(async move {
            writer.write_all(&vec![b'x'; 100_000]).await.unwrap();
        });

        let output = tokio::time::timeout(
            Duration::from_secs(1),
            read_command_stream(reader, Some(40)),
        )
        .await
        .unwrap()
        .unwrap();
        write.await.unwrap();

        assert_eq!(output.len(), 40);
        assert!(output.ends_with(OUTPUT_TRUNCATED));
    }

    #[tokio::test]
    async fn unbounded_command_output_is_unchanged() {
        let (mut writer, reader) = tokio::io::duplex(8);
        let write = tokio::spawn(async move {
            writer.write_all(b"complete output").await.unwrap();
        });

        assert_eq!(
            read_command_stream(reader, None).await.unwrap(),
            b"complete output"
        );
        write.await.unwrap();
    }
}

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
    AnimatePluginLoading(PluginId),                  // u32 - plugin_id
    StopPluginLoadingAnimation(PluginId),            // u32 - plugin_id
    ReportSessionInfo(String, SessionInfo),          // String - session name
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
    ), // command, args, env_variables, cwd, context, max_output_bytes
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
    ),
    CancelWebRequest(PluginId, ClientId, String), // String - stream_id
    /// Sent by a streaming request task back to this thread when its body ends, so that the
    /// terminating chunk is always emitted from one place (see `finish_stream`)
    WebRequestStreamFinished(StreamKey, u64, Option<String>), // u64 - generation, Option<String> - error
    HighlightPanesWithMessage(Vec<PaneId>, String),
    DismissNotification(u64, u64), // notification id, timeout in seconds
    /// Wake the screen for a frame every `Duration`, until [`BackgroundJob::StopAnimationClock`].
    /// The period is `Screen`'s to choose - see `Screen::animation_clock_period` - because only
    /// the screen knows the coarsest cadence every live surface can live with.
    StartAnimationClock(Duration),
    StopAnimationClock,
    /// Give up waiting for a context menu's hook plugin and show the configured items instead.
    /// (request id, client id, timeout in milliseconds)
    ContextMenuTimeout(u64, ClientId, u64),
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
    FlashTabBell(usize),     // usize = tab_id
    StopFlashTabBell(usize), // usize = tab_id
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
const MAX_BUFFERED_WEB_RESPONSE_BYTES: usize = MAX_IPC_FRAME_LEN;
/// How long a streamed byte may wait to be joined by the ones behind it.
///
/// A token stream comes off the socket in ~50 small reads a second. Forwarding each as its own
/// event gave every one of them a `PluginInstruction`, a wasm `update()` and a place in the
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
    pub current_session_name: Arc<Mutex<String>>,
    pub current_session_info: Arc<Mutex<SessionInfo>>,
    pub current_session_plugin_list: Arc<Mutex<BTreeMap<PluginId, RunPlugin>>>,
}

static SESSION_SCAN_STATE: std::sync::OnceLock<SessionScanState> = std::sync::OnceLock::new();

pub fn session_scan_state() -> Option<&'static SessionScanState> {
    SESSION_SCAN_STATE.get()
}

pub(crate) fn background_jobs_main(
    bus: Bus<BackgroundJob>,
    serialization_interval: Option<u64>,
    disable_session_metadata: bool,
) -> Result<()> {
    let err_context = || "failed to write to pty".to_string();
    let mut running_jobs: HashMap<BackgroundJob, Instant> = HashMap::new();
    // the shared animation clock's cancel flag and the period it is ticking at, `Some` while it is
    // running.
    let mut animation_clock: Option<(Arc<AtomicBool>, Duration)> = None;
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
    let serialization_interval = serialization_interval.map(|s| s * 1000); // convert to
                                                                           // milliseconds
    let last_render_request: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let pending_help_text_clear: Arc<Mutex<HashMap<ClientId, Instant>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_command_output_flash_clear: Arc<Mutex<HashMap<PaneId, Instant>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let mut flashing_pane_bells: HashMap<PaneId, Arc<AtomicBool>> = HashMap::new();
    let mut flashing_tab_bells: HashMap<usize, Arc<AtomicBool>> = HashMap::new();
    let mut nested_guest_pings: HashMap<PaneId, Arc<AtomicBool>> = HashMap::new();
    let mut active_streams: HashMap<StreamKey, ActiveWebRequestStream> = HashMap::new();
    let mut next_stream_generation = 0u64;

    let http_client = HttpClient::builder()
        .connect_timeout(WEB_REQUEST_CONNECT_TIMEOUT)
        // the idle timeout is per-request, in send_web_request
        // Redirects are followed by send_web_request so every target can be checked before curl
        // sees it.
        .redirect_policy(RedirectPolicy::None)
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
                write_session_state_to_disk(name, info, layout).non_fatal();
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
                        while running.load(Ordering::SeqCst) {
                            tick.tick().await;
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
                            let (stdout, stderr, status) = tokio::join!(
                                read_command_stream(stdout, max_output_bytes),
                                read_command_stream(stderr, max_output_bytes),
                                child.wait(),
                            );
                            Ok::<_, std::io::Error>((status?, stdout?, stderr?))
                        }
                        .await;
                        match output {
                            Ok((status, stdout, stderr)) => {
                                let exit_code = status.code();
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
                proxy,
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
                            proxy,
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
            // deliberately not gated on job_already_running: every notification has its own id and
            // needs its own timer
            BackgroundJob::DismissNotification(notification_id, timeout_secs) => {
                runtime.spawn({
                    let senders = bus.senders.clone();
                    async move {
                        tokio::time::sleep(Duration::from_secs(timeout_secs)).await;
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
                let is_flashing = Arc::new(AtomicBool::new(true));
                for &pane_id in &pane_ids {
                    flashing_pane_bells.insert(pane_id, is_flashing.clone());
                }
                runtime.spawn({
                    let senders = bus.senders.clone();
                    let pane_ids_clone = pane_ids.clone();
                    let flag = is_flashing.clone();
                    async move {
                        let _ = senders.send_to_screen(ScreenInstruction::FlashPaneFrameColor(
                            pane_ids_clone.clone(),
                        ));
                        // the override eases itself out over exactly this window; the clear that
                        // follows is what actually frees the slot, and stays on the old schedule
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
                if let Some((clock, _)) = animation_clock.take() {
                    clock.store(false, Ordering::SeqCst);
                }
                for nested_guest_ping in nested_guest_pings.values() {
                    nested_guest_ping.store(false, Ordering::SeqCst);
                }

                return Ok(());
            },
        }
    }
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

fn scrub_redirect_headers(headers: &mut BTreeMap<String, String>) {
    headers.retain(|name, _| {
        !matches!(
            name.to_ascii_lowercase().as_str(),
            "authorization" | "cookie" | "cookie2" | "proxy-authorization" | "www-authenticate"
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
) {
    let (plugin_id, client_id, stream_id) = key.clone();
    let emit_event = |event: Event| {
        let _ = senders.send_to_plugin(PluginInstruction::Update(vec![(
            Some(plugin_id),
            Some(client_id),
            event,
        )]));
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
    // its own wasm `update()` and its own trip through the plugin thread's queue - the same
    // queue the user's keystrokes have to cross. Concatenating is safe because the framing is
    // reassembled on the far side (hocus feeds these to an `SseParser`, which already handles a
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
    if !state.pending.is_empty() {
        let _ = senders.send_to_plugin(PluginInstruction::Update(vec![(
            Some(plugin_id),
            Some(client_id),
            Event::WebRequestStreamChunk {
                stream_id: stream_id.clone(),
                chunk: std::mem::take(&mut state.pending),
                done: false,
                error: None,
            },
        )]));
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

pub fn write_session_state_to_disk(
    current_session_name: String,
    current_session_info: SessionInfo,
    current_session_layout: (String, BTreeMap<String, String>),
) -> Result<()> {
    zellij_utils::session_checkpoint::write_checkpoint(
        &session_info_folder_for_session(&current_session_name),
        &current_session_info.to_string(),
        &current_session_layout,
    )
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
            info.populate_plugin_list(
                current_session_plugin_list
                    .clone()
                    .into_iter()
                    .map(|(id, plugin)| (id.0, plugin))
                    .collect(),
            );
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
                    let checkpoint =
                        zellij_utils::session_checkpoint::SessionCheckpoint::read(&folder_name)
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
    fn scan_session_list_reads_published_checkpoints() {
        let sock_dir = tempdir().unwrap();
        let info_dir = tempdir().unwrap();
        let _listener = make_socket(sock_dir.path(), "live");
        for name in ["live", "dead"] {
            zellij_utils::session_checkpoint::write_checkpoint(
                &info_dir.path().join(name),
                &SessionInfo::new(name.to_owned()).to_string(),
                &("layout {}".to_owned(), BTreeMap::new()),
            )
            .unwrap();
        }
        let (live, resurrectable) = scan_session_list(
            "me",
            &[],
            &BTreeMap::new(),
            sock_dir.path(),
            info_dir.path(),
        );
        assert_eq!(live.keys().collect::<Vec<_>>(), vec!["live"]);
        assert_eq!(resurrectable.keys().collect::<Vec<_>>(), vec!["dead"]);
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
    use futures_lite::io::AsyncRead;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use zellij_utils::channels::{unbounded, Receiver, SenderWithContext};
    use zellij_utils::errors::ErrorContext;

    const PLUGIN_ID: PluginId = PluginId(1);
    const CLIENT_ID: ClientId = ClientId(2);
    const GENERATION: u64 = 7;

    type PluginRx = Receiver<(PluginInstruction, ErrorContext)>;
    type JobRx = Receiver<(BackgroundJob, ErrorContext)>;

    struct ControlledReader {
        chunks: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
        waiting: Option<tokio::sync::oneshot::Sender<()>>,
        current: Vec<u8>,
        offset: usize,
    }

    impl ControlledReader {
        fn new() -> (
            Self,
            tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
            tokio::sync::oneshot::Receiver<()>,
        ) {
            let (sender, chunks) = tokio::sync::mpsc::unbounded_channel();
            let (waiting, reader_is_waiting) = tokio::sync::oneshot::channel();
            (
                Self {
                    chunks,
                    waiting: Some(waiting),
                    current: vec![],
                    offset: 0,
                },
                sender,
                reader_is_waiting,
            )
        }
    }

    impl AsyncRead for ControlledReader {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<std::io::Result<usize>> {
            let this = self.get_mut();
            loop {
                if this.offset < this.current.len() {
                    let bytes_read = buf.len().min(this.current.len() - this.offset);
                    let end = this.offset + bytes_read;
                    buf[..bytes_read].copy_from_slice(&this.current[this.offset..end]);
                    this.offset = end;
                    return Poll::Ready(Ok(bytes_read));
                }

                match Pin::new(&mut this.chunks).poll_recv(cx) {
                    Poll::Ready(Some(next)) => {
                        this.current = next;
                        this.offset = 0;
                    },
                    Poll::Ready(None) => return Poll::Ready(Ok(0)),
                    Poll::Pending => {
                        if let Some(waiting) = this.waiting.take() {
                            let _ = waiting.send(());
                        }
                        return Poll::Pending;
                    },
                }
            }
        }
    }

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

    async fn serve_redirect(location: String) -> (String, tokio::sync::oneshot::Receiver<Vec<u8>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();
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
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.flush().await.unwrap();
            drop(socket);

            let (mut socket, _) = listener.accept().await.unwrap();
            request.clear();
            while !request.ends_with(b"\r\n\r\n") {
                match socket.read(&mut byte).await {
                    Ok(0) | Err(_) => return,
                    Ok(_) => request.push(byte[0]),
                }
            }
            let _ = request_tx.send(request);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nredirected")
                .await
                .unwrap();
        });
        (url, request_rx)
    }

    #[test]
    fn web_requests_accept_only_http_and_https_urls() {
        assert!(web_url("https://example.com/path").is_ok());
        assert!(web_url("http://example.com/path").is_ok());
        assert!(web_url("file://localhost/etc/passwd").is_err());
    }

    #[test]
    fn cross_origin_redirects_drop_sensitive_headers() {
        let mut headers = BTreeMap::from([
            ("Authorization".to_owned(), "secret".to_owned()),
            ("Cookie".to_owned(), "secret".to_owned()),
            ("X-Request-Id".to_owned(), "safe".to_owned()),
        ]);

        scrub_redirect_headers(&mut headers);

        assert_eq!(
            headers,
            BTreeMap::from([("X-Request-Id".to_owned(), "safe".to_owned())])
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_http_redirect_to_a_file_url_is_rejected() {
        let (url, _) = serve_redirect("file://localhost/etc/passwd".to_owned()).await;
        let error = buffered_web_request(
            url,
            HttpVerb::Get,
            BTreeMap::new(),
            vec![],
            HttpClient::new().unwrap(),
            None,
        )
        .await
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("web requests only support http and https"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_safe_http_redirect_is_followed() {
        let (url, redirected_request) = serve_redirect("/final".to_owned()).await;
        let (status, _, body) = buffered_web_request(
            url,
            HttpVerb::Get,
            BTreeMap::new(),
            vec![],
            HttpClient::new().unwrap(),
            None,
        )
        .await
        .unwrap();

        assert_eq!(status, 200);
        assert_eq!(body, b"redirected");
        assert!(redirected_request
            .await
            .unwrap()
            .starts_with(b"GET /final "));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_302_redirect_only_rewrites_post_to_get() {
        for (verb, expected_method) in [(HttpVerb::Post, b"GET"), (HttpVerb::Put, b"PUT")] {
            let (url, redirected_request) = serve_redirect("/final".to_owned()).await;
            buffered_web_request(
                url,
                verb,
                BTreeMap::new(),
                vec![],
                HttpClient::new().unwrap(),
                None,
            )
            .await
            .unwrap();

            assert!(redirected_request
                .await
                .unwrap()
                .starts_with(expected_method));
        }
    }

    #[tokio::test]
    async fn buffered_web_bodies_stop_at_the_limit() {
        let (mut reader, sender, _reader_is_waiting) = ControlledReader::new();
        sender.send(b"five!".to_vec()).unwrap();
        drop(sender);

        let error = read_bounded_web_body(&mut reader, 4).await.unwrap_err();
        assert!(error
            .to_string()
            .contains("exceeded the 4-byte buffered-body limit"));
    }

    /// Frames arriving back-to-back are joined into one event rather than one apiece. Every one
    /// used to be its own `PluginInstruction`, its own wasm `update()` and its own place in the
    /// plugin thread's queue - ahead of whatever the user was typing.
    #[tokio::test(start_paused = true)]
    async fn a_burst_of_frames_arrives_as_one_event_after_the_coalescing_window() {
        let parts: &[&[u8]] = &[
            b"data: {\"n\":1}\n\n",
            b"data: {\"n\":2}\n\n",
            b"data: {\"n\":3}\n\n",
            b"data: {\"n\":4}\n\n",
            b"data: [DONE]\n\n",
        ];
        let expected = parts.concat();
        let (mut reader, reader_handle, reader_is_waiting) = ControlledReader::new();
        for part in parts {
            reader_handle.send(part.to_vec()).unwrap();
        }
        let state = Arc::new(Mutex::new(WebRequestStreamState::default()));
        let (emitted, mut emitted_chunks) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            forward_streamed_body(&mut reader, "stream-1", &state, move |event| match event {
                Event::WebRequestStreamChunk { chunk, done, .. } => {
                    assert!(!done, "the task must never emit the terminator itself");
                    emitted.send(chunk).unwrap();
                },
                other => panic!("expected a stream chunk, got {:?}", other),
            })
            .await
        });

        reader_is_waiting.await.unwrap();
        tokio::time::advance(WEB_REQUEST_STREAM_COALESCE_WINDOW - Duration::from_millis(1)).await;
        assert!(
            matches!(
                emitted_chunks.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "the burst must wait for the coalescing deadline"
        );

        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(emitted_chunks.recv().await.unwrap(), expected);
        assert!(
            matches!(
                emitted_chunks.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "the burst must produce exactly one chunk"
        );

        drop(reader_handle);
        assert_eq!(task.await.unwrap(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_flushes_bytes_read_before_the_coalescing_deadline() {
        let (mut reader, reader_handle, reader_is_waiting) = ControlledReader::new();
        reader_handle.send(b"final bytes".to_vec()).unwrap();
        let (senders, plugin_rx, _job_rx) = test_senders();
        let state = Arc::new(Mutex::new(WebRequestStreamState {
            response_sent: true,
            ..Default::default()
        }));
        let task_state = state.clone();
        let task_senders = senders.clone();
        let task = tokio::spawn(async move {
            forward_streamed_body(&mut reader, "stream-1", &task_state, move |event| {
                let _ = task_senders.send_to_plugin(PluginInstruction::Update(vec![(
                    Some(PLUGIN_ID),
                    Some(CLIENT_ID),
                    event,
                )]));
            })
            .await
        });

        reader_is_waiting.await.unwrap();
        assert_eq!(state.lock().unwrap().pending, b"final bytes");
        let mut active_streams = HashMap::from([(
            stream_key(),
            ActiveWebRequestStream {
                abort_handle: task.abort_handle(),
                generation: GENERATION,
                state,
                context: BTreeMap::new(),
            },
        )]);
        finish_stream(
            &mut active_streams,
            &senders,
            stream_key(),
            None,
            Some("cancelled".to_owned()),
        );

        let events = drain_events(&plugin_rx);
        assert!(matches!(
            &events[..],
            [
                Event::WebRequestStreamChunk { chunk, done: false, error: None, .. },
                Event::WebRequestStreamChunk { chunk: done_chunk, done: true, error: Some(error), .. }
            ] if chunk == b"final bytes" && done_chunk.is_empty() && error == "cancelled"
        ));
        assert!(task.await.unwrap_err().is_cancelled());
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
            None,
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
                    ..Default::default()
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
                    pending: b"final bytes".to_vec(),
                    ..Default::default()
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

        let events = drain_events(&plugin_rx);
        assert!(matches!(
            &events[..],
            [
                Event::WebRequestStreamChunk { chunk, done: false, error: None, .. },
                Event::WebRequestStreamChunk { done: true, error: Some(error), .. }
            ] if chunk == b"final bytes" && error == "superseded"
        ));
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

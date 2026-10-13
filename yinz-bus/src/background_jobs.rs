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
    deno_generation: Option<u64>, // identifies the runtime that receives this request's events
    state: Arc<Mutex<WebRequestStreamState>>,
    context: BTreeMap<String, String>,
}

const OUTPUT_TRUNCATED: &[u8] = b"\n[truncated]";

async fn read_command_stream<R>(
    mut stream: R,
    max_output_bytes: Option<usize>,
    mut on_chunk: impl FnMut(&[u8]),
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
        on_chunk(&buffer[..read]);
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

    /// A payload no two windows of which are alike, so a reader that kept the wrong bytes -
    /// the tail, or a middle - could not pass as one that kept the head.
    fn distinguishable_payload() -> Vec<u8> {
        (0..100_000u32).map(|i| (i % 251) as u8).collect()
    }

    /// Runs the bounded reader against a writer that never stops early, and returns the output
    /// once the writer has confirmed every byte went in - so a reader that dropped the pipe
    /// before EOF fails as "not drained" rather than as a join error.
    async fn read_bounded(payload: Vec<u8>, limit: usize) -> Vec<u8> {
        let (mut writer, reader) = tokio::io::duplex(8);
        let expected_len = payload.len();
        let write = tokio::spawn(async move {
            writer.write_all(&payload).await?;
            Ok::<usize, std::io::Error>(expected_len)
        });

        let output = tokio::time::timeout(
            Duration::from_secs(1),
            read_command_stream(reader, Some(limit), |_| {}),
        )
        .await
        .expect("the reader must reach EOF")
        .unwrap();
        let written = write
            .await
            .unwrap()
            .expect("the pipe was not drained: the reader hung up before EOF");
        assert_eq!(written, expected_len, "the pipe was not drained");
        output
    }

    #[tokio::test]
    async fn bounded_command_output_is_truncated_and_drained() {
        let payload = distinguishable_payload();
        let limit = 40;
        let output = read_bounded(payload.clone(), limit).await;

        assert_eq!(output.len(), limit);
        let kept = limit - OUTPUT_TRUNCATED.len();
        assert_eq!(
            &output[..kept],
            &payload[..kept],
            "the kept bytes must be the first {kept} of the output"
        );
        assert!(output.ends_with(OUTPUT_TRUNCATED));
    }

    /// A limit smaller than the marker keeps only as much of the marker as fits, and never more
    /// than the limit.
    #[tokio::test]
    async fn bounded_command_output_honours_a_limit_smaller_than_the_marker() {
        assert!(OUTPUT_TRUNCATED.len() > 5);
        for limit in [5, 0] {
            let output = read_bounded(distinguishable_payload(), limit).await;
            assert_eq!(output.len(), limit, "limit {limit}");
            assert_eq!(output, &OUTPUT_TRUNCATED[..limit], "limit {limit}");
        }
    }

    #[tokio::test]
    async fn unbounded_command_output_is_unchanged() {
        let (mut writer, reader) = tokio::io::duplex(8);
        let write = tokio::spawn(async move {
            writer.write_all(b"complete output").await.unwrap();
        });

        assert_eq!(
            read_command_stream(reader, None, |_| {}).await.unwrap(),
            b"complete output"
        );
        write.await.unwrap();
    }

    #[test]
    fn streaming_spawn_failure_returns_one_correlated_completion() {
        use yinz_runtime::channels::{unbounded, SenderWithContext};
        let (jobs, receiver) = unbounded();
        let jobs = SenderWithContext::new(jobs);
        let (events, received) = unbounded();
        let events = SenderWithContext::new(events);
        let mut bus = Bus::new(
            vec![receiver],
            None,
            None,
            Some(&events),
            None,
            None,
            None,
            None,
        );
        bus.senders.should_silently_fail = true;
        let worker = std::thread::spawn(move || background_jobs_main(bus, None, true));
        let context = BTreeMap::from([("operation".to_owned(), "missing".to_owned())]);
        let directory = tempfile::tempdir().unwrap();
        jobs.send(BackgroundJob::RunCommand(
            PluginId::new(1),
            ClientId::new(1),
            directory
                .path()
                .join("absent-command")
                .display()
                .to_string(),
            vec![],
            BTreeMap::new(),
            directory.path().to_owned(),
            context.clone(),
            Some(64),
            Some(true),
            None,
        ))
        .unwrap();
        let (event, _) = received.recv_timeout(Duration::from_secs(10)).unwrap();
        jobs.send(BackgroundJob::Exit).unwrap();
        worker.join().unwrap().unwrap();
        match event {
            PluginInstruction::Update(events) => {
                assert_eq!(events.len(), 1);
                match &events[0].2 {
                    Event::RunCommandResult(Some(2), stdout, stderr, result_context) => {
                        assert!(stdout.is_empty());
                        assert!(!stderr.is_empty());
                        assert_eq!(result_context, &context);
                    },
                    event => panic!("expected completion, got {event:?}"),
                }
            },
            event => panic!("expected plugin update, got {event:?}"),
        }
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn real_command_streams_before_exit_with_generation_and_drains_beyond_cap() {
        use std::io::Write;
        use std::os::unix::ffi::OsStrExt;
        use yinz_runtime::channels::{unbounded, SenderWithContext};
        let directory = tempfile::tempdir().unwrap();
        let fifo = directory.path().join("release");
        let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let mut release = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo)
            .unwrap();
        let (jobs, receiver) = unbounded();
        let jobs = SenderWithContext::new(jobs);
        let (events, received) = unbounded();
        let events = SenderWithContext::new(events);
        let mut bus = Bus::new(
            vec![receiver],
            None,
            None,
            Some(&events),
            None,
            None,
            None,
            None,
        );
        bus.senders.should_silently_fail = true;
        let worker = std::thread::spawn(move || background_jobs_main(bus, None, true));
        let context = BTreeMap::from([("operation".into(), "live".into())]);
        jobs.send(BackgroundJob::RunCommand(
            PluginId::new(1),
            ClientId::new(2),
            "/bin/sh".into(),
            vec![
                "-c".into(),
                "printf out; printf err >&2; read line < \"$1\"; printf tail; printf tail >&2"
                    .into(),
                "sh".into(),
                fifo.display().to_string(),
            ],
            BTreeMap::new(),
            directory.path().to_owned(),
            context.clone(),
            Some(2),
            Some(true),
            Some(42),
        ))
        .unwrap();
        let mut stdout = vec![];
        let mut stderr = vec![];
        let mut released = false;
        loop {
            let (instruction, _) = received.recv_timeout(Duration::from_secs(10)).unwrap();
            let PluginInstruction::DenoEvent(plugin, client, generation, event) = instruction
            else {
                panic!("live output must carry its generation: {instruction:?}");
            };
            assert_eq!(
                (plugin, client, generation),
                (PluginId::new(1), ClientId::new(2), 42)
            );
            match event {
                Event::RunCommandOutput {
                    data,
                    stderr: is_stderr,
                    context: actual,
                } => {
                    assert_eq!(actual, context);
                    if is_stderr {
                        stderr.extend(data);
                    } else {
                        stdout.extend(data);
                    }
                    if !released && stdout == b"out" && stderr == b"err" {
                        // The child cannot exit until we observe both live chunks.
                        release.write_all(b"go\n").unwrap();
                        released = true;
                    }
                },
                Event::RunCommandResult(Some(0), out, err, actual) => {
                    assert!(released);
                    assert_eq!(actual, context);
                    assert!(out.len() <= 2 && err.len() <= 2);
                    break;
                },
                event => panic!("unexpected event: {event:?}"),
            }
        }
        assert_eq!(stdout, b"outtail");
        assert_eq!(stderr, b"errtail");
        jobs.send(BackgroundJob::Exit).unwrap();
        worker.join().unwrap().unwrap();
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn chunks_arrive_before_exit_and_both_streams_drain_after_release() {
        let (mut out, stdout) = tokio::io::duplex(8);
        let (mut err, stderr) = tokio::io::duplex(8);
        let (chunks, mut received) = tokio::sync::mpsc::unbounded_channel();
        let reading = tokio::spawn(async move {
            tokio::join!(
                read_command_stream(stdout, Some(4), |data| {
                    chunks.send((false, data.to_vec())).unwrap();
                }),
                read_command_stream(stderr, Some(4), |data| {
                    chunks.send((true, data.to_vec())).unwrap();
                }),
            )
        });
        out.write_all(b"out").await.unwrap();
        err.write_all(b"err").await.unwrap();
        let mut first = vec![];
        for _ in 0..2 {
            first.push(
                tokio::time::timeout(Duration::from_secs(5), received.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        first.sort();
        assert_eq!(
            first,
            vec![(false, b"out".to_vec()), (true, b"err".to_vec())]
        );
        assert!(!reading.is_finished(), "output must precede stream closure");
        out.write_all(b"more").await.unwrap();
        err.write_all(b"more").await.unwrap();
        drop((out, err));
        let (stdout, stderr) = reading.await.unwrap();
        assert_eq!(stdout.unwrap().len(), 4);
        assert_eq!(stderr.unwrap().len(), 4);
        let mut count = 0;
        while received.recv().await.is_some() {
            count += 1;
        }
        assert!(count >= 2, "caps must not suppress live chunks");
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
    ), // command, args, env_variables, cwd, context, max_output_bytes, stream_output, deno_generation
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
        Option<u64>,              // deno_generation of the requesting runtime, if any
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
    // We needn't do anything with the runtime, but it should exist at this point.
    let runtime = crate::global_async_runtime::get_tokio_runtime();

    {
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
                deno_generation,
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
                                            deno_generation,
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
                                        deno_generation,
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
                                        deno_generation,
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
                deno_generation,
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
                                deno_generation,
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
                            deno_generation,
                        ));
                        // ponytail: unloading a plugin does not cancel its in-flight streams -
                        // they drain to completion and the events are dropped by
                        // plugin-id and runtime-generation filters. Add a
                        // BackgroundJob::CancelPluginWebRequests(plugin_id) from wasm_bridge's
                        // unload path if abandoned streams become a bandwidth problem.
                        active_streams.insert(
                            key,
                            ActiveWebRequestStream {
                                abort_handle: handle.abort_handle(),
                                generation,
                                deno_generation,
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
                log::info!("shutdown: background jobs received Exit");
                if let Some((clock, _)) = animation_clock.take() {
                    clock.store(false, Ordering::SeqCst);
                }
                for nested_guest_ping in nested_guest_pings.values() {
                    nested_guest_ping.store(false, Ordering::SeqCst);
                }

                break Ok(());
            },
        }
    };
    if let Some(task) = checkpoint_task {
        let started = Instant::now();
        log::info!(
            "shutdown: aborting checkpoint task; {}",
            bus.senders.session_checkpoint.progress()
        );
        task.abort();
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
    deno_generation: Option<u64>,
) {
    let (plugin_id, client_id, stream_id) = key.clone();
    let emit_event = |event: Event| {
        let _ = senders.send_to_plugin(PluginInstruction::for_plugin_event(
            plugin_id,
            client_id,
            deno_generation,
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
    // its own wasm `update()` and its own trip through the plugin thread's queue - the same
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
            stream.deno_generation,
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
            stream.deno_generation,
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
        stream.deno_generation,
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
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use tempfile::tempdir;
    use yinz_types::data::SessionInfo;

    #[test]
    fn checkpoint_rename_redirects_queued_saves_and_metadata() {
        let root = tempdir().unwrap();
        let writer = Arc::new(SessionCheckpointWriter {
            cache_dir: root.path().to_owned(),
            name: Mutex::new(None),
            progress: Mutex::default(),
            #[cfg(test)]
            before_write: None,
        });
        let old = SessionName::new("old");
        let renamed = SessionName::new("renamed");
        let snapshot = SessionInfo::new(old.clone());
        let periodic_writer = writer.clone();
        let explicit_writer = writer.clone();
        writer
            .write(
                old.clone(),
                snapshot.clone(),
                &("original".into(), BTreeMap::new()),
            )
            .unwrap();
        writer.rename(&old, &renamed).unwrap();

        for (queued_writer, contents) in
            [(periodic_writer, "periodic"), (explicit_writer, "explicit")]
        {
            queued_writer
                .write(
                    old.clone(),
                    snapshot.clone(),
                    &(contents.into(), BTreeMap::new()),
                )
                .unwrap();
            assert!(
                !root.path().join(&old).exists(),
                "a stale snapshot recreated the old session"
            );
            let checkpoint = yinz_runtime::session_checkpoint::SessionCheckpoint::read(
                &root.path().join(&renamed),
            )
            .unwrap();
            let metadata = fs::read_to_string(checkpoint.metadata_path()).unwrap();
            assert_eq!(
                yinz_session::metadata::session_info::from_string(&metadata, &renamed)
                    .unwrap()
                    .name,
                renamed
            );
            assert_eq!(
                fs::read_to_string(checkpoint.layout_path()).unwrap(),
                contents
            );
        }
    }

    #[test]
    fn checkpoint_rename_before_first_save_and_failed_rename_keep_the_right_owner() {
        let root = tempdir().unwrap();
        let writer = SessionCheckpointWriter {
            cache_dir: root.path().to_owned(),
            name: Mutex::new(None),
            progress: Mutex::default(),
            #[cfg(test)]
            before_write: None,
        };
        let old = SessionName::new("old");
        let renamed = SessionName::new("renamed");
        let blocked = SessionName::new("blocked");
        let layout = ("layout {}".into(), BTreeMap::new());
        writer.rename(&old, &renamed).unwrap();
        writer
            .write(old.clone(), SessionInfo::new(old.clone()), &layout)
            .unwrap();
        assert!(!root.path().join(&old).exists());

        fs::write(root.path().join(&blocked), "existing data").unwrap();
        assert!(writer.rename(&renamed, &blocked).is_err());
        assert_eq!(*writer.name.lock().unwrap(), Some(renamed.clone()));
        writer
            .write(old.clone(), SessionInfo::new(old.clone()), &layout)
            .unwrap();
        assert!(!root.path().join(&old).exists());
        assert!(root.path().join(&renamed).is_dir());
        assert_eq!(
            fs::read_to_string(root.path().join(&blocked)).unwrap(),
            "existing data"
        );
    }

    fn make_socket(dir: &std::path::Path, name: &str) -> UnixListener {
        UnixListener::bind(dir.join(name)).expect("bind unix socket")
    }

    /// The legacy on-disk shape: bare files in the session folder, no `.checkpoint.lock` and no
    /// `current` pointer. Only `scan_session_list_reads_the_legacy_flat_files` should use these
    /// two; everything a running server writes goes through `write_checkpoint`.
    fn write_flat_metadata(info_dir: &std::path::Path, session: &str, info: &SessionInfo) {
        let folder = info_dir.join(session);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("session-metadata.kdl"),
            yinz_session::metadata::session_info::to_string(info).unwrap(),
        )
        .unwrap();
    }

    fn write_flat_layout(info_dir: &std::path::Path, session: &str) {
        let folder = info_dir.join(session);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("session-layout.kdl"), "layout { }").unwrap();
    }

    /// Publishes a session the way `SessionCheckpointWriter` does: metadata and layout
    /// under a `.checkpoint-*` directory behind a `current` symlink.
    fn publish(info_dir: &std::path::Path, session: &str) {
        yinz_runtime::session_checkpoint::write_checkpoint(
            &info_dir.join(session),
            &yinz_session::metadata::session_info::to_string(&SessionInfo::new(
                session.to_owned().into(),
            ))
            .unwrap(),
            &("layout {}".to_owned(), BTreeMap::new()),
        )
        .unwrap();
    }

    fn scan(
        current: &str,
        sock_dir: &std::path::Path,
        info_dir: &std::path::Path,
    ) -> (
        BTreeMap<SessionName, SessionInfo>,
        BTreeMap<SessionName, Duration>,
    ) {
        scan_session_list(
            &SessionName::new(current),
            &[],
            &BTreeMap::new(),
            sock_dir,
            info_dir,
        )
    }

    #[test]
    fn scan_session_list_reads_published_checkpoints() {
        let sock_dir = tempdir().unwrap();
        let info_dir = tempdir().unwrap();
        let _listener = make_socket(sock_dir.path(), "live");
        for name in ["live", "dead"] {
            publish(info_dir.path(), name);
        }
        let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
        assert_eq!(
            live.keys().map(SessionName::as_str).collect::<Vec<_>>(),
            vec!["live"]
        );
        assert_eq!(
            resurrectable
                .keys()
                .map(SessionName::as_str)
                .collect::<Vec<_>>(),
            vec!["dead"]
        );
    }

    #[test]
    fn scan_session_list_returns_empty_when_no_peers() {
        let sock_dir = tempdir().unwrap();
        let info_dir = tempdir().unwrap();
        let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
        assert!(live.is_empty());
        assert!(resurrectable.is_empty());
    }

    #[test]
    fn scan_session_list_finds_peer_from_socket_and_metadata() {
        let sock_dir = tempdir().unwrap();
        let info_dir = tempdir().unwrap();
        let peer = "peer-alpha";
        let _listener = make_socket(sock_dir.path(), peer);
        publish(info_dir.path(), peer);

        let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
        assert_eq!(live.len(), 1);
        assert!(live.contains_key(peer));
        assert!(resurrectable.is_empty());
    }

    /// Only the current session's entry is enriched with the plugin list and the layouts this
    /// server knows about; a peer's entry is left as its metadata describes it.
    #[test]
    fn scan_session_list_enriches_only_the_current_session() {
        let sock_dir = tempdir().unwrap();
        let info_dir = tempdir().unwrap();
        let _listeners: Vec<UnixListener> = ["me", "other"]
            .into_iter()
            .map(|name| {
                publish(info_dir.path(), name);
                make_socket(sock_dir.path(), name)
            })
            .collect();
        let layouts = vec![LayoutInfo::BuiltIn("compact".to_owned())];
        let plugin = RunPlugin {
            location: yinz_types::input::layout::RunPluginLocation::File(PathBuf::from(
                "/plugins/status.wasm",
            )),
            ..Default::default()
        };
        let plugins = BTreeMap::from([(PluginId::new(7), plugin.clone())]);

        let (live, _) = scan_session_list(
            &SessionName::new("me"),
            &layouts,
            &plugins,
            sock_dir.path(),
            info_dir.path(),
        );

        let me = &live[&SessionName::new("me")];
        assert_eq!(me.available_layouts, layouts);
        assert_eq!(
            me.plugins,
            BTreeMap::from([(PluginId::new(7), yinz_types::data::PluginInfo::from(plugin))])
        );
        let other = &live[&SessionName::new("other")];
        assert!(other.available_layouts.is_empty(), "{other:?}");
        assert!(other.plugins.is_empty(), "{other:?}");
    }

    /// A dead session is resurrectable when its layout is on disk; nothing else about it is
    /// consulted. The reported age is how long ago that layout was written.
    #[test]
    fn scan_session_list_finds_resurrectable_from_orphan_layout() {
        let sock_dir = tempdir().unwrap();
        let info_dir = tempdir().unwrap();
        publish(info_dir.path(), "dead-beta");

        let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
        assert!(live.is_empty());
        assert_eq!(
            resurrectable
                .keys()
                .map(SessionName::as_str)
                .collect::<Vec<_>>(),
            vec!["dead-beta"]
        );
        let dead_for = resurrectable[&SessionName::new("dead-beta")];
        assert!(
            dead_for < Duration::from_secs(60),
            "the layout was just written, so the session died {dead_for:?} ago at most"
        );
    }

    /// Metadata alone is not enough to bring a session back, so a folder with no layout is left
    /// off the list rather than offered as resurrectable.
    #[test]
    fn scan_session_list_drops_a_folder_with_metadata_but_no_layout() {
        let sock_dir = tempdir().unwrap();
        let info_dir = tempdir().unwrap();
        write_flat_metadata(
            info_dir.path(),
            "no-layout",
            &SessionInfo::new("no-layout".to_owned().into()),
        );

        let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
        assert!(live.is_empty());
        assert!(resurrectable.is_empty(), "{resurrectable:?}");
    }

    /// The pre-checkpoint layout: bare `session-metadata.kdl` / `session-layout.kdl` in the
    /// session folder, which `SessionCheckpoint::read` still accepts for sessions written by
    /// an older server.
    #[test]
    fn scan_session_list_reads_the_legacy_flat_files() {
        let sock_dir = tempdir().unwrap();
        let info_dir = tempdir().unwrap();
        let _listener = make_socket(sock_dir.path(), "live");
        write_flat_metadata(
            info_dir.path(),
            "live",
            &SessionInfo::new("live".to_owned().into()),
        );
        write_flat_layout(info_dir.path(), "dead");

        let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
        assert_eq!(
            live.keys().map(SessionName::as_str).collect::<Vec<_>>(),
            vec!["live"]
        );
        assert_eq!(
            resurrectable
                .keys()
                .map(SessionName::as_str)
                .collect::<Vec<_>>(),
            vec!["dead"]
        );
    }

    /// "Live" means a socket file of that name exists: nothing here connects to it, so a socket
    /// left behind by a crashed server reads as live too. That is production's definition as
    /// well (`read_live_session_states` only stats the directory entry), not a shortcut of the
    /// test.
    #[test]
    fn scan_session_list_separates_live_from_resurrectable() {
        let sock_dir = tempdir().unwrap();
        let info_dir = tempdir().unwrap();
        let _listeners: Vec<UnixListener> = ["live-a", "live-b", "live-c"]
            .into_iter()
            .map(|name| {
                publish(info_dir.path(), name);
                make_socket(sock_dir.path(), name)
            })
            .collect();
        for name in ["dead-a", "dead-b"] {
            publish(info_dir.path(), name);
        }

        let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
        assert_eq!(
            live.keys().map(SessionName::as_str).collect::<Vec<_>>(),
            vec!["live-a", "live-b", "live-c"]
        );
        assert_eq!(
            resurrectable
                .keys()
                .map(SessionName::as_str)
                .collect::<Vec<_>>(),
            vec!["dead-a", "dead-b"]
        );
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
    use yinz_runtime::channels::{unbounded, Receiver, SenderWithContext};
    use yinz_types::diagnostics::ErrorContext;

    const PLUGIN_ID: PluginId = PluginId::new(1);
    const CLIENT_ID: ClientId = ClientId::new(2);
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

    /// Reads one HTTP request off `socket`: the head up to the blank line, then as many body
    /// bytes as its `Content-Length` announces, so a test can see whether a body was sent at all.
    /// `None` if the peer hung up first.
    async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Vec<u8>> {
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            match socket.read(&mut byte).await {
                Ok(0) | Err(_) => return None,
                Ok(_) => request.push(byte[0]),
            }
        }
        let head = String::from_utf8_lossy(&request).to_ascii_lowercase();
        let content_length: usize = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            socket.read_exact(&mut body).await.ok()?;
        }
        request.extend(body);
        Some(request)
    }

    fn header_line<'a>(request: &'a str, name: &str) -> Option<&'a str> {
        request
            .split("\r\n\r\n")
            .next()
            .unwrap_or("")
            .lines()
            .find(|line| {
                line.to_ascii_lowercase()
                    .starts_with(&format!("{}:", name.to_ascii_lowercase()))
            })
    }

    fn request_body(request: &str) -> &str {
        request.split_once("\r\n\r\n").map_or("", |(_, body)| body)
    }

    /// Serves one request with `status` and `location`, then a second one with `200 OK` and the
    /// body `redirected`; the second request is handed back whole so the test can see what the
    /// client sent after following the redirect. Returns the url to hit.
    async fn serve_redirect(
        status: &'static str,
        location: String,
    ) -> (String, tokio::sync::oneshot::Receiver<Vec<u8>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            if read_request(&mut socket).await.is_none() {
                return;
            }
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.flush().await.unwrap();
            drop(socket);

            let (mut socket, _) = listener.accept().await.unwrap();
            let Some(request) = read_request(&mut socket).await else {
                return;
            };
            let _ = request_tx.send(request);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nredirected")
                .await
                .unwrap();
        });
        (url, request_rx)
    }

    /// Serves one `200 OK` and hands back the request it received. A second origin for the
    /// cross-origin redirect test: it listens on its own port, which is all `Url::origin`
    /// needs to tell it apart from the redirecting server.
    async fn serve_capture() -> (String, tokio::sync::oneshot::Receiver<Vec<u8>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let Some(request) = read_request(&mut socket).await else {
                return;
            };
            let _ = request_tx.send(request);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nredirected")
                .await
                .unwrap();
        });
        (url, request_rx)
    }

    /// Answers every request with a 302 back to itself, and counts them.
    async fn serve_redirect_loop() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/loop", listener.local_addr().unwrap());
        let hops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hops.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                if read_request(&mut socket).await.is_none() {
                    continue;
                }
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                let _ = socket.flush().await;
            }
        });
        (url, hops)
    }

    /// The allowlist is `http` and `https` with a host; a foreign scheme is refused whether or
    /// not it names a file, and so is an `http` URL with nothing to connect to. The `url` crate
    /// never yields a parsed `http` URL without a host (it refuses `http://` outright and reads
    /// `http:///x` as `http://x/`), so the hostless cases are refused at parse time and the
    /// `host_str().is_none()` clause in `web_url` is a second line behind that.
    #[test]
    fn web_requests_accept_only_http_and_https_urls() {
        assert!(web_url("https://example.com/path").is_ok());
        assert!(web_url("http://example.com/path").is_ok());
        for rejected in [
            "file://localhost/etc/passwd",
            "ftp://example.com/pub",
            "data:text/plain,hello",
            "http://",
            "http:///",
            "http://:8080/",
        ] {
            assert!(web_url(rejected).is_err(), "{rejected} should be refused");
        }
    }

    /// Only the safelisted headers are kept, in whatever case they were written: a credential
    /// header and a custom one alike are dropped, since a custom header may be a key.
    #[test]
    fn scrub_redirect_headers_keeps_only_safelisted_headers() {
        let mut headers = BTreeMap::from([
            ("Authorization".to_owned(), "secret".to_owned()),
            ("cookie".to_owned(), "secret".to_owned()),
            ("Cookie2".to_owned(), "secret".to_owned()),
            ("PROXY-AUTHORIZATION".to_owned(), "secret".to_owned()),
            ("Www-Authenticate".to_owned(), "secret".to_owned()),
            ("X-Api-Key".to_owned(), "secret".to_owned()),
            ("X-Request-Id".to_owned(), "custom".to_owned()),
            ("Content-Type".to_owned(), "safe".to_owned()),
            ("USER-AGENT".to_owned(), "safe".to_owned()),
        ]);

        scrub_redirect_headers(&mut headers);

        assert_eq!(
            headers,
            BTreeMap::from([
                ("Content-Type".to_owned(), "safe".to_owned()),
                ("USER-AGENT".to_owned(), "safe".to_owned()),
            ])
        );
    }

    fn credentialed_headers() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("Authorization".to_owned(), "Bearer secret".to_owned()),
            ("Cookie".to_owned(), "session=secret".to_owned()),
            ("X-Api-Key".to_owned(), "test_secret".to_owned()),
            ("Accept".to_owned(), "application/json".to_owned()),
        ])
    }

    /// A redirect to another origin is followed without the credentials: the second request
    /// carries neither the bearer token, the cookie nor the API key, but does keep the harmless
    /// header.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_cross_origin_redirect_drops_credentials() {
        let (target, redirected_request) = serve_capture().await;
        let (url, _) = serve_redirect("302 Found", target).await;
        let (status, _, body) = buffered_web_request(
            url,
            HttpVerb::Get,
            credentialed_headers(),
            vec![],
            web_http_client().unwrap(),
            None,
        )
        .await
        .unwrap();

        assert_eq!((status, body.as_slice()), (200, &b"redirected"[..]));
        let request = String::from_utf8(redirected_request.await.unwrap()).unwrap();
        assert!(
            header_line(&request, "authorization").is_none(),
            "the bearer token followed a cross-origin redirect:\n{request}"
        );
        assert!(
            header_line(&request, "cookie").is_none(),
            "the cookie followed a cross-origin redirect:\n{request}"
        );
        assert!(
            header_line(&request, "x-api-key").is_none(),
            "the API key followed a cross-origin redirect:\n{request}"
        );
        assert_eq!(
            header_line(&request, "accept"),
            Some("accept: application/json"),
            "{request}"
        );
    }

    /// Within one origin the credentials stay: an authenticated API that redirects must keep
    /// working, so over-scrubbing is as much a bug as under-scrubbing.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_same_origin_redirect_keeps_credentials() {
        let (url, redirected_request) = serve_redirect("302 Found", "/final".to_owned()).await;
        buffered_web_request(
            url,
            HttpVerb::Get,
            credentialed_headers(),
            vec![],
            web_http_client().unwrap(),
            None,
        )
        .await
        .unwrap();

        let request = String::from_utf8(redirected_request.await.unwrap()).unwrap();
        assert!(request.starts_with("GET /final "), "{request}");
        assert_eq!(
            header_line(&request, "authorization"),
            Some("authorization: Bearer secret"),
            "{request}"
        );
        assert_eq!(
            header_line(&request, "cookie"),
            Some("cookie: session=secret"),
            "{request}"
        );
        assert_eq!(
            header_line(&request, "x-api-key"),
            Some("x-api-key: test_secret"),
            "{request}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_http_redirect_to_a_file_url_is_rejected() {
        let (url, _) = serve_redirect("302 Found", "file://localhost/etc/passwd".to_owned()).await;
        let error = buffered_web_request(
            url,
            HttpVerb::Get,
            BTreeMap::new(),
            vec![],
            web_http_client().unwrap(),
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
        let (url, redirected_request) = serve_redirect("302 Found", "/final".to_owned()).await;
        let (status, _, body) = buffered_web_request(
            url,
            HttpVerb::Get,
            BTreeMap::new(),
            vec![],
            web_http_client().unwrap(),
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

    /// The status and the verb together decide whether the redirected request is rewritten to a
    /// bodiless GET: 303 rewrites any verb, 301 and 302 rewrite only POST, and 307 and 308
    /// rewrite nothing - so PUT and PATCH keep their verb and body on everything but a 303.
    /// Rewriting drops the payload too, so a POST body is never replayed as a GET.
    #[tokio::test(flavor = "multi_thread")]
    async fn redirect_status_and_verb_decide_the_rewrite() {
        for (status, verb, expected_method) in [
            ("301 Moved Permanently", HttpVerb::Post, "GET"),
            ("302 Found", HttpVerb::Post, "GET"),
            ("302 Found", HttpVerb::Put, "PUT"),
            ("303 See Other", HttpVerb::Put, "GET"),
            ("301 Moved Permanently", HttpVerb::Patch, "PATCH"),
            ("302 Found", HttpVerb::Patch, "PATCH"),
            ("303 See Other", HttpVerb::Patch, "GET"),
            ("307 Temporary Redirect", HttpVerb::Patch, "PATCH"),
            ("308 Permanent Redirect", HttpVerb::Patch, "PATCH"),
            ("307 Temporary Redirect", HttpVerb::Post, "POST"),
        ] {
            let (url, redirected_request) = serve_redirect(status, "/final".to_owned()).await;
            buffered_web_request(
                url,
                verb,
                BTreeMap::new(),
                b"payload".to_vec(),
                web_http_client().unwrap(),
                None,
            )
            .await
            .unwrap();

            let request = String::from_utf8(redirected_request.await.unwrap()).unwrap();
            let case = format!("{status} + {verb:?}");
            assert!(
                request.starts_with(&format!("{expected_method} /final ")),
                "{case}: {request}"
            );
            if expected_method == "GET" {
                assert!(
                    header_line(&request, "content-length").is_none(),
                    "{case}: a rewritten request must not announce a body:\n{request}"
                );
                assert_eq!(request_body(&request), "", "{case}: body replayed as GET");
            } else {
                assert_eq!(
                    request_body(&request),
                    "payload",
                    "{case}: the body must survive a redirect that keeps the verb"
                );
            }
        }
    }

    /// A redirect loop is a hang without the cap; with it the request fails after exactly
    /// `MAX_WEB_REQUEST_REDIRECTS` hops.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_redirect_loop_fails_at_the_redirect_cap() {
        let (url, hops) = serve_redirect_loop().await;
        let error = buffered_web_request(
            url,
            HttpVerb::Get,
            BTreeMap::new(),
            vec![],
            web_http_client().unwrap(),
            None,
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains(&format!("exceeded {MAX_WEB_REQUEST_REDIRECTS} redirects")),
            "{error}"
        );
        assert_eq!(
            hops.load(Ordering::SeqCst),
            MAX_WEB_REQUEST_REDIRECTS + 1,
            "the first request plus one per permitted redirect"
        );
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

    /// The limit is inclusive: a body of exactly the maximum size is the largest one accepted,
    /// and it comes back whole.
    #[tokio::test]
    async fn buffered_web_bodies_of_exactly_the_limit_are_returned_whole() {
        let (mut reader, sender, _reader_is_waiting) = ControlledReader::new();
        sender.send(b"four".to_vec()).unwrap();
        drop(sender);

        let body = read_bounded_web_body(&mut reader, 4).await.unwrap();
        assert_eq!(body, b"four");
    }

    /// Frames arriving back-to-back are joined into one event rather than one apiece. Every one
    /// used to be its own `PluginInstruction`, its own wasm `update()` and its own place in the
    /// plugin thread's queue - ahead of whatever the user was typing.
    #[tokio::test(start_paused = true)]
    async fn a_burst_of_frames_arrives_as_one_event_after_the_coalescing_window() {
        assert_eq!(
            WEB_REQUEST_STREAM_COALESCE_WINDOW,
            Duration::from_millis(20)
        );
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
                deno_generation: None,
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
        drain_events_for_generation(plugin_rx, None)
    }

    fn drain_events_for_generation(plugin_rx: &PluginRx, generation: Option<u64>) -> Vec<Event> {
        plugin_rx
            .try_iter()
            .flat_map(|(instruction, _)| match instruction {
                PluginInstruction::Update(updates) => {
                    assert_eq!(generation, None);
                    updates
                        .into_iter()
                        .map(|(plugin_id, client_id, event)| {
                            assert_eq!(plugin_id, Some(PLUGIN_ID));
                            assert_eq!(client_id, Some(CLIENT_ID));
                            event
                        })
                        .collect::<Vec<_>>()
                },
                PluginInstruction::DenoEvent(plugin_id, client_id, actual_generation, event) => {
                    assert_eq!(generation, Some(actual_generation));
                    assert_eq!(plugin_id, PLUGIN_ID);
                    assert_eq!(client_id, CLIENT_ID);
                    vec![event]
                },
                other => panic!("unexpected plugin instruction: {other:?}"),
            })
            .collect()
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
            web_http_client().unwrap(),
            None,
            None,
        )
        .await;
        let events = drain_events(&plugin_rx);
        let jobs = job_rx.try_iter().map(|(job, _)| job).collect();
        (events, jobs)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn streamed_results_and_finish_keep_the_originating_deno_generation() {
        const DENO_GENERATION: u64 = 101; // distinct from the stream registry generation
        let url = serve_once("200 OK", "text/plain", vec!["body"]).await;
        let (senders, plugin_rx, job_rx) = test_senders();
        let state = Arc::new(Mutex::new(WebRequestStreamState::default()));
        streamed_web_request(
            stream_key(),
            GENERATION,
            state.clone(),
            senders.clone(),
            url,
            HttpVerb::Get,
            BTreeMap::new(),
            vec![],
            BTreeMap::new(),
            HttpClient::new().unwrap(),
            None,
            Some(DENO_GENERATION),
        )
        .await;
        let mut active_streams = HashMap::from([(
            stream_key(),
            ActiveWebRequestStream {
                abort_handle: tokio::spawn(async {}).abort_handle(),
                generation: GENERATION,
                deno_generation: Some(DENO_GENERATION),
                state,
                context: BTreeMap::new(),
            },
        )]);
        let (job, _) = job_rx.try_recv().unwrap();
        let BackgroundJob::WebRequestStreamFinished(key, generation, error) = job else {
            panic!("stream must report completion");
        };
        assert_eq!(generation, GENERATION);
        finish_stream(&mut active_streams, &senders, key, Some(generation), error);
        let events = drain_events_for_generation(&plugin_rx, Some(DENO_GENERATION));
        assert!(matches!(&events[..], [
            Event::WebRequestResult(200, _, body, _),
            Event::WebRequestStreamChunk { chunk, done: false, .. },
            Event::WebRequestStreamChunk { done: true, error: None, .. },
        ] if body.is_empty() && chunk == b"body"));
        assert!(active_streams.is_empty());
    }

    #[tokio::test]
    async fn cancelled_stream_tags_synthetic_result_pending_bytes_and_terminator() {
        const DENO_GENERATION: u64 = 101;
        for response_sent in [false, true] {
            let (senders, plugin_rx, _) = test_senders();
            let mut active_streams = HashMap::from([(
                stream_key(),
                ActiveWebRequestStream {
                    abort_handle: tokio::spawn(async {}).abort_handle(),
                    generation: GENERATION,
                    deno_generation: Some(DENO_GENERATION),
                    state: Arc::new(Mutex::new(WebRequestStreamState {
                        response_sent,
                        pending: if response_sent {
                            b"pending".to_vec()
                        } else {
                            vec![]
                        },
                        ..Default::default()
                    })),
                    context: BTreeMap::new(),
                },
            )]);
            finish_stream(
                &mut active_streams,
                &senders,
                stream_key(),
                None,
                Some("cancelled".into()),
            );
            let events = drain_events_for_generation(&plugin_rx, Some(DENO_GENERATION));
            assert_eq!(events.len(), 2);
            if response_sent {
                assert!(matches!(&events[0], Event::WebRequestStreamChunk {
                    chunk, done: false, ..
                } if chunk == b"pending"));
            } else {
                assert!(matches!(&events[0], Event::WebRequestResult(400, ..)));
            }
            assert!(matches!(&events[1], Event::WebRequestStreamChunk {
                done: true, error: Some(error), ..
            } if error == "cancelled"));
        }
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

        // one synthetic result and nothing after it: no chunk, no second result
        match &events[..] {
            [Event::WebRequestResult(400, _, body, _)] => {
                assert!(
                    !body.is_empty(),
                    "the body is what the plugin shows for the failure"
                );
            },
            other => panic!("expected exactly one 400 result, got {other:?}"),
        }
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

        // The partial body is discarded on purpose: the plugin gets the error text in its
        // place, and the same text in the finish report, rather than a body that looks
        // complete but is not.
        let error = match &jobs[..] {
            [BackgroundJob::WebRequestStreamFinished(_, GENERATION, Some(error))] => error,
            other => panic!("expected one finish report carrying an error, got {other:?}"),
        };
        match &events[..] {
            [Event::WebRequestResult(500, _, body, _)] => {
                assert_ne!(body, b"short", "the partial body must not pass as the body");
                assert_eq!(
                    std::str::from_utf8(body).unwrap(),
                    error,
                    "the body is the error text"
                );
            },
            other => panic!("expected exactly one 500 result, got {other:?}"),
        }
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
                deno_generation: None,
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
                deno_generation: None,
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

    /// `finish_stream`'s supersede semantics: ending a stream with an error flushes what it
    /// had pending before the terminator, and a stale finish for the old generation cannot touch
    /// the replacement registered under the same key. Whether the `WebRequest` arm *uses* these
    /// semantics is `the_web_request_arm_supersedes_and_cancels_through_the_bus`'s business.
    #[tokio::test(flavor = "multi_thread")]
    async fn finish_stream_supersede_flushes_pending_bytes_then_terminates() {
        let (senders, plugin_rx, _job_rx) = test_senders();
        let mut active_streams = HashMap::new();
        let old_task = tokio::spawn(async { tokio::time::sleep(Duration::from_secs(60)).await });
        active_streams.insert(
            stream_key(),
            ActiveWebRequestStream {
                abort_handle: old_task.abort_handle(),
                generation: GENERATION,
                deno_generation: None,
                state: Arc::new(Mutex::new(WebRequestStreamState {
                    response_sent: true,
                    pending: b"final bytes".to_vec(),
                    ..Default::default()
                })),
                context: BTreeMap::new(),
            },
        );

        // the call the WebRequest arm makes before spawning a replacement under the same key -
        // made by hand here, so this test says nothing about whether the arm makes it
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
                deno_generation: None,
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
                deno_generation: None,
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

    /// Replies to every connection with a `200 OK` chunked head and a first chunk `hello`, then
    /// holds the socket open without ending the body, so a stream against it stays in flight
    /// until something on the server side ends it.
    async fn serve_open_stream() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = vec![];
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                if read_request(&mut socket).await.is_none() {
                    continue;
                }
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n",
                    )
                    .await
                    .unwrap();
                socket.flush().await.unwrap();
                held.push(socket);
            }
        });
        url
    }

    /// Blocks for the next `count` plugin events, however they are batched.
    fn recv_events(plugin_rx: &PluginRx, count: usize) -> Vec<Event> {
        let mut events = vec![];
        while events.len() < count {
            let (instruction, _) = plugin_rx
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|e| {
                    panic!("waiting for {count} plugin events, got {events:?}: {e}")
                });
            match instruction {
                PluginInstruction::Update(updates) => {
                    events.extend(updates.into_iter().map(|(_, _, event)| event));
                },
                other => panic!("unexpected plugin instruction {other:?}"),
            }
        }
        events
    }

    fn terminator(error: Option<&str>) -> Event {
        Event::WebRequestStreamChunk {
            stream_id: "stream-1".to_owned(),
            chunk: vec![],
            done: true,
            error: error.map(str::to_owned),
        }
    }

    /// Drives the real `WebRequest`, `CancelWebRequest` and `WebRequestStreamFinished` arms of
    /// `background_jobs_main` over a bus, rather than calling `finish_stream` by hand. Reusing a
    /// stream id must end the stream that holds it before the replacement starts, cancelling
    /// must end the replacement, and a stream that completes must be terminated by its own
    /// finish report arriving back on the bus. That report carries the generation the arm
    /// assigned, so a generation counter that stopped advancing would let a stale report from
    /// the superseded stream terminate its replacement here.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_web_request_arm_supersedes_and_cancels_through_the_bus() {
        let open_url = serve_open_stream().await;
        let (to_plugin, plugin_rx) = unbounded();
        let (to_jobs, job_rx) = unbounded();
        let to_plugin = SenderWithContext::new(to_plugin);
        let to_jobs = SenderWithContext::new(to_jobs);
        let bus = Bus::new(
            vec![job_rx],
            None,
            None,
            Some(&to_plugin),
            None,
            None,
            Some(&to_jobs),
            None,
        )
        .should_silently_fail();
        let main = std::thread::spawn(move || background_jobs_main(bus, None, true));
        let request = |url: &str, stream_id: &str| {
            BackgroundJob::WebRequest(
                PLUGIN_ID,
                CLIENT_ID,
                url.to_owned(),
                HttpVerb::Get,
                BTreeMap::new(),
                vec![],
                BTreeMap::new(),
                Some(stream_id.to_owned()),
                None,
                None,
            )
        };

        let hello = Event::WebRequestStreamChunk {
            stream_id: "stream-1".to_owned(),
            chunk: b"hello".to_vec(),
            done: false,
            error: None,
        };
        // waiting for the first chunk as well as the result makes the sequence below exact:
        // the stream has nothing pending to flush when it is superseded
        to_jobs.send(request(&open_url, "stream-1")).unwrap();
        let events = recv_events(&plugin_rx, 2);
        assert!(
            matches!(events[0], Event::WebRequestResult(200, ..)),
            "the first stream must be in flight before its id is reused, got {events:?}"
        );
        assert_eq!(events[1], hello);

        // the same id again: the first stream is terminated as superseded before the second
        // one reports in
        to_jobs.send(request(&open_url, "stream-1")).unwrap();
        let events = recv_events(&plugin_rx, 3);
        assert_eq!(events[0], terminator(Some("superseded")));
        assert!(
            matches!(events[1], Event::WebRequestResult(200, ..)),
            "expected the replacement's result, got {:?}",
            events[1]
        );
        assert_eq!(events[2], hello);

        to_jobs
            .send(BackgroundJob::CancelWebRequest(
                PLUGIN_ID,
                CLIENT_ID,
                "stream-1".to_owned(),
            ))
            .unwrap();
        assert_eq!(
            recv_events(&plugin_rx, 1),
            vec![terminator(Some("cancelled"))]
        );

        // a stream that runs to completion is terminated by the finish report the task sends
        // back to this same bus, with no error
        let done_url = serve_once("200 OK", "text/event-stream", vec!["data: 1\n\n"]).await;
        to_jobs.send(request(&done_url, "stream-1")).unwrap();
        let events = recv_events(&plugin_rx, 3);
        assert!(
            matches!(events[0], Event::WebRequestResult(200, ..)),
            "{events:?}"
        );
        assert_eq!(
            events[1],
            Event::WebRequestStreamChunk {
                stream_id: "stream-1".to_owned(),
                chunk: b"data: 1\n\n".to_vec(),
                done: false,
                error: None,
            }
        );
        assert_eq!(events[2], terminator(None));

        to_jobs.send(BackgroundJob::Exit).unwrap();
        main.join().unwrap().unwrap();
        assert!(
            plugin_rx.try_recv().is_err(),
            "nothing may follow a terminator"
        );
    }
}

#[cfg(test)]
#[path = "unit/checkpoint_shutdown_tests.rs"]
mod checkpoint_shutdown_tests;

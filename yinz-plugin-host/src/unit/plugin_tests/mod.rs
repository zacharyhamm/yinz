pub(crate) mod reference_trace;
use super::plugin_thread_main;
use insta::assert_snapshot;
use lazy_static::lazy_static;
use serial_test::serial;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;
use yinz_bus::screen::ScreenInstruction;
use yinz_bus::{channels::SenderWithContext, thread_bus::Bus, ServerInstruction};
use yinz_storage::data_access::DataAccess;
use yinz_types::keys::{BareKey, KeyWithModifier};
use {
    yinz_types::data::Event, yinz_types::data::InputMode, yinz_types::data::ModeInfo,
    yinz_types::data::PermissionStatus, yinz_types::data::PermissionType,
};

use yinz_types::diagnostics::ErrorContext;
use yinz_types::input::actions::Action;
use yinz_types::input::keybinds::Keybinds;
use {
    yinz_types::input::layout::PluginAlias, yinz_types::input::layout::PluginUserConfiguration,
    yinz_types::input::layout::RunPlugin, yinz_types::input::layout::RunPluginLocation,
    yinz_types::input::layout::RunPluginOrAlias,
};

use crate::PluginInstruction;
use yinz_bus::background_jobs::BackgroundJob;
use yinz_bus::pty::PtyInstruction;
use yinz_bus::pty_writer::PtyWriteInstruction;
use yinz_config::permission::PermissionCache;
use yinz_runtime::channels::{self, ChannelWithContext, Receiver};
use yinz_types::input::plugins::PluginAliases;
use yinz_types::pane_size::Size;

use crate::ClientId;
use yinz_test_support::{default_timeout, Watched};

/// How long a fake screen thread waits for the next event before declaring the test stuck.
///
/// Generous, because these tests start a real plugin runtime — the point is only to
/// bound the wait, not to police it.
const FAKE_SCREEN_RECV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Receives the next event for a fake screen thread, panicking rather than blocking forever.
///
/// The fake screen threads below loop until they have counted N exit events and are then `join`ed
/// by the test. If the plugin never loads, that event never arrives and the whole test binary
/// hangs with no output. Bounding the wait turns such a failure into an ordinary, diagnosable test
/// failure.
fn recv_from_fake_screen<T>(
    receiver: &Receiver<(T, ErrorContext)>,
    exit_events_seen: impl std::fmt::Display,
    exit_events_expected: impl std::fmt::Display,
) -> (T, ErrorContext) {
    receiver
        .recv_timeout(FAKE_SCREEN_RECV_TIMEOUT)
        .unwrap_or_else(|err| {
            panic!(
                "fake screen thread waited {:?} for the next event and gave up ({err}). It had \
                 seen {exit_events_seen} of {exit_events_expected} expected exit events. This \
                 usually means the plugin never loaded — fixture in use: {}",
                FAKE_SCREEN_RECV_TIMEOUT, *PLUGIN_FIXTURE
            )
        })
}

macro_rules! log_actions_in_thread {
    ( $arc_mutex_log:expr_2021, $exit_event:path, $receiver:expr_2021, $exit_after_count:expr_2021 ) => {
        std::thread::Builder::new()
            .name("logger thread".to_string())
            .spawn({
                let log = $arc_mutex_log.clone();
                let mut exit_event_count = 0;
                move || loop {
                    let (event, _err_ctx) =
                        recv_from_fake_screen(&$receiver, exit_event_count, $exit_after_count);
                    match event {
                        $exit_event(..) => {
                            exit_event_count += 1;
                            log.lock().push(event);
                            if exit_event_count == $exit_after_count {
                                break;
                            }
                        },
                        _ => {
                            log.lock().push(event);
                        },
                    }
                }
            })
            .unwrap()
    };
}

macro_rules! log_actions_in_thread_struct {
    ( $arc_mutex_log:expr_2021, $exit_event:path, $receiver:expr_2021, $exit_after_count:expr_2021 ) => {
        std::thread::Builder::new()
            .name("logger thread".to_string())
            .spawn({
                let log = $arc_mutex_log.clone();
                let mut exit_event_count = 0;
                move || loop {
                    let (event, _err_ctx) =
                        recv_from_fake_screen(&$receiver, exit_event_count, $exit_after_count);
                    match event {
                        $exit_event { .. } => {
                            exit_event_count += 1;
                            log.lock().push(event);
                            if exit_event_count == $exit_after_count {
                                break;
                            }
                        },
                        _ => {
                            log.lock().push(event);
                        },
                    }
                }
            })
            .unwrap()
    };
}

macro_rules! grant_permissions_and_log_actions_in_thread {
    ( $arc_mutex_log:expr_2021, $exit_event:path, $receiver:expr_2021, $exit_after_count:expr_2021, $permission_type:expr_2021, $cache_path:expr_2021, $plugin_thread_sender:expr_2021, $client_id:expr_2021 ) => {
        std::thread::Builder::new()
            .name("fake_screen_thread".to_string())
            .spawn({
                let log = $arc_mutex_log.clone();
                let mut exit_event_count = 0;
                let cache_path = $cache_path.clone();
                let plugin_thread_sender = $plugin_thread_sender.clone();
                move || loop {
                    let (event, _err_ctx) =
                        recv_from_fake_screen(&$receiver, exit_event_count, $exit_after_count);
                    match event {
                        $exit_event(..) => {
                            exit_event_count += 1;
                            log.lock().push(event);
                            if exit_event_count == $exit_after_count {
                                break;
                            }
                        },
                        ScreenInstruction::RequestPluginPermissions(_, plugin_permission) => {
                            if plugin_permission.permissions.contains($permission_type) {
                                let _ = plugin_thread_sender.send(
                                    PluginInstruction::PermissionAnswer {
                                        request_id: plugin_permission.request_id,
                                        status: PermissionStatus::Granted,
                                        cache_path: Some(cache_path.clone()),
                                    },
                                );
                            } else {
                                let _ = plugin_thread_sender.send(
                                    PluginInstruction::PermissionAnswer {
                                        request_id: plugin_permission.request_id,
                                        status: PermissionStatus::Denied,
                                        cache_path: Some(cache_path.clone()),
                                    },
                                );
                            }
                        },
                        _ => {
                            log.lock().push(event);
                        },
                    }
                }
            })
            .unwrap()
    };
}

/// Answers the plugin's permission request with `Denied`, then keeps logging every screen
/// instruction until the channel disconnects.
///
/// A denial test asserts that something did *not* happen, so its observer has no positive event
/// to count and stop on. It has to keep reading until nothing can send any more: the plugin
/// thread, its executor workers and any thread a host command spawned (`run_action` runs the
/// action on one) each hold a sender, and all of them are gone once `teardown()` has joined the
/// plugin thread. The macro this replaced broke out of its loop right after answering, which made
/// every "no forbidden instruction" assertion inevitable rather than evidence.
///
/// Join the returned thread *after* `teardown()`, not before: it returns only on disconnect.
fn deny_permissions_and_log_until_disconnect(
    log: &Watched<Vec<ScreenInstruction>>,
    screen_receiver: Receiver<(ScreenInstruction, ErrorContext)>,
    cache_path: PathBuf,
    plugin_thread_sender: SenderWithContext<PluginInstruction>,
    client_id: ClientId,
) -> std::thread::JoinHandle<()> {
    permissions_and_log_until_disconnect(
        log,
        screen_receiver,
        cache_path,
        plugin_thread_sender,
        client_id,
        PermissionStatus::Denied,
    )
}

/// Answers permissions before logging the request; joins only after teardown.
fn permissions_and_log_until_disconnect(
    log: &Watched<Vec<ScreenInstruction>>,
    screen_receiver: Receiver<(ScreenInstruction, ErrorContext)>,
    cache_path: PathBuf,
    plugin_thread_sender: SenderWithContext<PluginInstruction>,
    _client_id: ClientId,
    status: PermissionStatus,
) -> std::thread::JoinHandle<()> {
    let log = log.clone();
    std::thread::Builder::new()
        .name("fake_screen_thread".to_string())
        .spawn(move || loop {
            match screen_receiver.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT) {
                Ok((event, _err_ctx)) => {
                    if let ScreenInstruction::RequestPluginPermissions(_plugin_id, permission) =
                        &event
                    {
                        let _ = plugin_thread_sender.send(PluginInstruction::PermissionAnswer {
                            request_id: permission.request_id,
                            status,
                            cache_path: Some(cache_path.clone()),
                        });
                    }
                    log.lock().push(event);
                },
                Err(channels::RecvTimeoutError::Disconnected) => break,
                Err(err @ channels::RecvTimeoutError::Timeout) => panic!(
                    "fake screen thread waited {:?} for the next event and gave up ({err}). It \
                     had logged {} instruction(s) and the channel never disconnected, so some \
                     sender is still alive - was `teardown()` called before joining this thread? \
                     Fixture in use: {}",
                    FAKE_SCREEN_RECV_TIMEOUT,
                    log.lock().len(),
                    *PLUGIN_FIXTURE
                ),
            }
        })
        .unwrap()
}

/// Logs every instruction on `receiver` until its channel disconnects.
///
/// The counterpart of [`deny_permissions_and_log_until_disconnect`] for a channel that needs no
/// answering - the server's, when a denial test has to show that nothing was routed there. Like
/// it, join the thread after `teardown()`.
fn log_until_disconnect<T: Send + 'static>(
    log: &Watched<Vec<T>>,
    receiver: Receiver<(T, ErrorContext)>,
) -> std::thread::JoinHandle<()> {
    let log = log.clone();
    std::thread::Builder::new()
        .name("logger thread".to_string())
        .spawn(move || loop {
            match receiver.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT) {
                Ok((event, _err_ctx)) => log.lock().push(event),
                Err(channels::RecvTimeoutError::Disconnected) => break,
                Err(err @ channels::RecvTimeoutError::Timeout) => panic!(
                    "logger thread waited {:?} for the next event and gave up ({err}). It had \
                     logged {} instruction(s) and the channel never disconnected, so some sender \
                     is still alive - was `teardown()` called before joining this thread?",
                    FAKE_SCREEN_RECV_TIMEOUT,
                    log.lock().len(),
                ),
            }
        })
        .unwrap()
}

/// A rendered frame as text, with the plugin's `\n\r` line endings folded to `\n`.
fn frame_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\n\r", "\n")
}

fn fixture_events(bytes: &[u8]) -> Vec<Event> {
    frame_of(bytes)
        .split_once(", Received events: ")
        .map(|(_, json)| serde_json::from_str(json.trim()).expect("fixture event JSON"))
        .unwrap_or_default()
}

/// Every non-empty frame rendered so far, as (client, frame) pairs in arrival order.
fn rendered_assets(instructions: &[ScreenInstruction]) -> Vec<(ClientId, String)> {
    instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::PluginBytes(assets) => Some(assets),
            _ => None,
        })
        .flatten()
        .map(|asset| (asset.client_id, frame_of(&asset.bytes)))
        .filter(|(_, frame)| !frame.is_empty())
        .collect()
}

fn describe_renders(instructions: &[ScreenInstruction]) -> String {
    // Frames in full, everything else abbreviated: a `PluginBytes` debug-prints as its bytes.
    let summary: Vec<String> = instructions
        .iter()
        .map(|instruction| match instruction {
            ScreenInstruction::PluginBytes(assets) => {
                format!(
                    "PluginBytes for plugin(s) {:?}",
                    assets.iter().map(|a| a.plugin_id).collect::<Vec<_>>()
                )
            },
            other => {
                let text = format!("{other:?}");
                text.chars().take(200).collect()
            },
        })
        .collect();
    format!(
        "{} instruction(s) so far: {:#?}; frames rendered: {:#?}. The fixture in use is {}",
        instructions.len(),
        summary,
        rendered_assets(instructions),
        *PLUGIN_FIXTURE
    )
}

/// Blocks until the fixture's permission request has been answered with `Denied`.
///
/// The observer logs the request only after sending its answer, and the answer and any key the
/// test sends next travel the plugin thread's one FIFO inbox - so a key sent after this returns
/// is handled after the denial. (The fixture does not subscribe to `PermissionRequestResult`, so
/// nothing it renders can say the denial arrived; the render the host forces after one shows an
/// empty event list.)
fn wait_for_permissions_to_be_denied(log: &Watched<Vec<ScreenInstruction>>) {
    wait_for_permissions(log);
}

fn wait_for_permissions(log: &Watched<Vec<ScreenInstruction>>) {
    log.wait_until_described(
        "the plugin's permission request to be answered",
        default_timeout(),
        |instructions| {
            instructions
                .iter()
                .any(|i| matches!(i, ScreenInstruction::RequestPluginPermissions(..)))
                .then_some(())
        },
        |log| describe_renders(log),
    )
}

/// Blocks until some rendered frame contains `needle`, and returns that frame.
///
/// Waiting on what a frame *says* is the only way to know the plugin got somewhere: renders are
/// asynchronous and coalesce, so neither how many arrived nor which one is last means anything.
/// The fixture prints every event it has received into its frame, so a key shows up there once
/// the `update` that took it has returned - and anything a host command sent to the screen
/// during that `update` is already in the log by then, because it went down the same channel
/// first.
fn wait_for_render_containing(log: &Watched<Vec<ScreenInstruction>>, needle: &str) -> String {
    log.wait_until_described(
        &format!("a render containing {needle:?}"),
        default_timeout(),
        |instructions| {
            rendered_assets(instructions)
                .into_iter()
                .map(|(_, frame)| frame)
                .find(|frame| frame.contains(needle))
        },
        |log| describe_renders(log),
    )
}

/// Blocks until `plugin_id` has finished with an event one way or another: a frame containing
/// one of `outcomes` was rendered, or the guest trapped and the host reported the crash.
///
/// For a key whose handling ends in a frame that names its outcome, list every outcome the
/// fixture can print - the success too - so that a wrongly permitted command gets past this wait
/// and fails on the assertion that names it, not on a timeout. The crash is listened for as well
/// so that a guest which does trap fails the assertion after this wait, not the wait itself.
fn wait_for_plugin_to_handle(
    log: &Watched<Vec<ScreenInstruction>>,
    plugin_id: yinz_types::internal_ids::PluginId,
    outcomes: &[&str],
) {
    log.wait_until_described(
        &format!("plugin {plugin_id} to render one of {outcomes:?} or to report a crash"),
        default_timeout(),
        |instructions| {
            let rendered = rendered_assets(instructions)
                .iter()
                .any(|(_, frame)| outcomes.iter().any(|outcome| frame.contains(outcome)));
            let crashed = instructions.iter().any(|instruction| {
                matches!(
                    instruction,
                    ScreenInstruction::UpdatePluginLoadingStage(id, indication)
                        if *id == plugin_id && indication.is_error()
                )
            });
            (rendered || crashed).then_some(())
        },
        |log| describe_renders(log),
    )
}

macro_rules! grant_permissions_and_log_actions_in_thread_naked_variant {
    ( $arc_mutex_log:expr_2021, $exit_event:path, $receiver:expr_2021, $exit_after_count:expr_2021, $permission_type:expr_2021, $cache_path:expr_2021, $plugin_thread_sender:expr_2021, $client_id:expr_2021 ) => {
        std::thread::Builder::new()
            .name("fake_screen_thread".to_string())
            .spawn({
                let log = $arc_mutex_log.clone();
                let mut exit_event_count = 0;
                let cache_path = $cache_path.clone();
                let plugin_thread_sender = $plugin_thread_sender.clone();
                move || loop {
                    let (event, _err_ctx) =
                        recv_from_fake_screen(&$receiver, exit_event_count, $exit_after_count);
                    match event {
                        $exit_event => {
                            exit_event_count += 1;
                            log.lock().push(event);
                            if exit_event_count == $exit_after_count {
                                break;
                            }
                        },
                        ScreenInstruction::RequestPluginPermissions(_, plugin_permission) => {
                            if plugin_permission.permissions.contains($permission_type) {
                                let _ = plugin_thread_sender.send(
                                    PluginInstruction::PermissionAnswer {
                                        request_id: plugin_permission.request_id,
                                        status: PermissionStatus::Granted,
                                        cache_path: Some(cache_path.clone()),
                                    },
                                );
                            } else {
                                let _ = plugin_thread_sender.send(
                                    PluginInstruction::PermissionAnswer {
                                        request_id: plugin_permission.request_id,
                                        status: PermissionStatus::Denied,
                                        cache_path: Some(cache_path.clone()),
                                    },
                                );
                            }
                        },
                        _ => {
                            log.lock().push(event);
                        },
                    }
                }
            })
            .unwrap()
    };
}

macro_rules! grant_permissions_and_log_actions_in_thread_struct_variant {
    ( $arc_mutex_log:expr_2021, $exit_event:path, $receiver:expr_2021, $exit_after_count:expr_2021, $permission_type:expr_2021, $cache_path:expr_2021, $plugin_thread_sender:expr_2021, $client_id:expr_2021 ) => {
        std::thread::Builder::new()
            .name("fake_screen_thread".to_string())
            .spawn({
                let log = $arc_mutex_log.clone();
                let mut exit_event_count = 0;
                let cache_path = $cache_path.clone();
                let plugin_thread_sender = $plugin_thread_sender.clone();
                move || loop {
                    let (event, _err_ctx) =
                        recv_from_fake_screen(&$receiver, exit_event_count, $exit_after_count);
                    match event {
                        $exit_event { .. } => {
                            exit_event_count += 1;
                            log.lock().push(event);
                            if exit_event_count == $exit_after_count {
                                break;
                            }
                        },
                        ScreenInstruction::RequestPluginPermissions(_, plugin_permission) => {
                            if plugin_permission.permissions.contains($permission_type) {
                                let _ = plugin_thread_sender.send(
                                    PluginInstruction::PermissionAnswer {
                                        request_id: plugin_permission.request_id,
                                        status: PermissionStatus::Granted,
                                        cache_path: Some(cache_path.clone()),
                                    },
                                );
                            } else {
                                let _ = plugin_thread_sender.send(
                                    PluginInstruction::PermissionAnswer {
                                        request_id: plugin_permission.request_id,
                                        status: PermissionStatus::Denied,
                                        cache_path: Some(cache_path.clone()),
                                    },
                                );
                            }
                        },
                        _ => {
                            log.lock().push(event);
                        },
                    }
                }
            })
            .unwrap()
    };
}

/// What the plugin-thread harnesses hand back: the sender into the plugin thread, the receivers
/// for whichever threads the test listens to, and a teardown closure.
type PluginThreadHarness = (
    SenderWithContext<PluginInstruction>,
    Receiver<(ScreenInstruction, ErrorContext)>,
    Box<dyn FnOnce()>,
);

type PluginThreadHarnessWith<T> = (
    SenderWithContext<PluginInstruction>,
    Receiver<(T, ErrorContext)>,
    Receiver<(ScreenInstruction, ErrorContext)>,
    Box<dyn FnOnce()>,
);

fn join_plugin_thread(thread: std::thread::JoinHandle<()>) {
    if let Err(panic) = thread.join() {
        // Some harnesses tear down from Drop; preserve an existing test failure without
        // aborting the process with a second panic during unwinding.
        if !std::thread::panicking() {
            std::panic::resume_unwind(panic);
        }
    }
}

#[test]
fn a_plugin_thread_panic_fails_its_test() {
    let thread = std::thread::spawn(|| panic!("plugin thread failed after its last render"));
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        join_plugin_thread(thread);
    }))
    .expect_err("the harness must propagate a plugin thread panic");
    assert_eq!(
        panic.downcast_ref::<&str>(),
        Some(&"plugin thread failed after its last render"),
        "preserve the failure's original diagnostic"
    );
}

fn create_plugin_thread(
    yinz_cwd: Option<PathBuf>,
    session_env_vars: Option<std::collections::BTreeMap<String, String>>,
) -> PluginThreadHarness {
    create_plugin_thread_with_user_plugin_dir(yinz_cwd, session_env_vars, None)
}

/// The same thread, with the folder Yinz searches for the user's own plugins under the test's
/// control - which is what lets a test prove a plugin named rather than pathed is found there.
fn create_plugin_thread_with_user_plugin_dir(
    yinz_cwd: Option<PathBuf>,
    session_env_vars: Option<std::collections::BTreeMap<String, String>>,
    user_plugin_dir: Option<PathBuf>,
) -> PluginThreadHarness {
    create_plugin_thread_with_hooks(
        yinz_cwd,
        session_env_vars,
        user_plugin_dir,
        Default::default(),
    )
}

/// A thread whose `hooks { }` is `hooks` from the start - the only way to bind a hook before the
/// thread fires `session_start`, which happens before it reads its first instruction.
fn create_plugin_thread_with_hooks(
    yinz_cwd: Option<PathBuf>,
    session_env_vars: Option<std::collections::BTreeMap<String, String>>,
    user_plugin_dir: Option<PathBuf>,
    hooks: yinz_types::input::hooks::Hooks,
) -> PluginThreadHarness {
    create_plugin_thread_with_hooks_and_event_sender(
        yinz_cwd,
        session_env_vars,
        user_plugin_dir,
        hooks,
        None,
        None,
        None,
    )
}

/// Override only the host's outgoing plugin channel, so a test can hold a real
/// asynchronous completion while control instructions still reach the plugin thread.
fn create_plugin_thread_with_hooks_and_event_sender(
    yinz_cwd: Option<PathBuf>,
    session_env_vars: Option<std::collections::BTreeMap<String, String>>,
    user_plugin_dir: Option<PathBuf>,
    hooks: yinz_types::input::hooks::Hooks,
    event_sender: Option<SenderWithContext<PluginInstruction>>,
    data_access: Option<DataAccess>,
    repository_data_dir: Option<PathBuf>,
) -> PluginThreadHarness {
    let (sender, _, screen, teardown) = create_plugin_thread_with_services(
        yinz_cwd,
        session_env_vars,
        user_plugin_dir,
        hooks,
        event_sender,
        data_access,
        repository_data_dir,
    );
    (sender, screen, teardown)
}

fn create_plugin_thread_with_services(
    yinz_cwd: Option<PathBuf>,
    session_env_vars: Option<std::collections::BTreeMap<String, String>>,
    user_plugin_dir: Option<PathBuf>,
    hooks: yinz_types::input::hooks::Hooks,
    event_sender: Option<SenderWithContext<PluginInstruction>>,
    data_access: Option<DataAccess>,
    repository_data_dir: Option<PathBuf>,
) -> PluginThreadHarnessWith<ServerInstruction> {
    let yinz_cwd = yinz_cwd.unwrap_or_else(|| PathBuf::from("."));
    let session_env_vars = session_env_vars.unwrap_or_else(|| std::env::vars().collect());
    let initiating_client_id = ClientId::new(1);
    let (to_server, server_receiver): ChannelWithContext<ServerInstruction> = channels::bounded(50);
    let to_server = SenderWithContext::new(to_server);

    let (to_screen, screen_receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    let to_screen = SenderWithContext::new(to_screen);

    let (to_plugin, plugin_receiver): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_plugin = SenderWithContext::new(to_plugin);
    let (to_pty, _pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    let to_pty = SenderWithContext::new(to_pty);

    let (to_pty_writer, _pty_writer_receiver): ChannelWithContext<PtyWriteInstruction> =
        channels::unbounded();
    let to_pty_writer = SenderWithContext::new(to_pty_writer);

    let (to_background_jobs, _background_jobs_receiver): ChannelWithContext<BackgroundJob> =
        channels::unbounded();
    let to_background_jobs = SenderWithContext::new(to_background_jobs);

    let plugin_bus = Bus::new(
        vec![plugin_receiver],
        Some(&to_screen),
        Some(&to_pty),
        Some(event_sender.as_ref().unwrap_or(&to_plugin)),
        Some(&to_server),
        Some(&to_pty_writer),
        Some(&to_background_jobs),
        None,
    )
    .should_silently_fail();
    let data_dir = repository_data_dir.unwrap_or_else(|| PathBuf::from(tempdir().unwrap().path()));
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &data_dir).unwrap();
    let data_access = Arc::new(data_access.unwrap_or_else(|| DataAccess::sqlite(&data_dir)));
    let layout_dir = PathBuf::from(tempdir().unwrap().path());
    let user_plugin_dir =
        user_plugin_dir.unwrap_or_else(|| PathBuf::from(tempdir().unwrap().path()));
    let default_shell = PathBuf::from(".");
    let default_shell_action = None;
    let mut plugin_aliases = PluginAliases::default();
    plugin_aliases.aliases.insert(
        "fixture_plugin_for_tests".to_owned(),
        yinz_runtime::plugin_location::run_plugin_from_url(&format!("file:{}", *PLUGIN_FIXTURE))
            .unwrap(),
    );
    let mut modal_fixture =
        yinz_runtime::plugin_location::run_plugin_from_url(&format!("file:{}", *PLUGIN_FIXTURE))
            .unwrap();
    modal_fixture.configuration.insert("modal", "true");
    plugin_aliases
        .aliases
        .insert("modal_fixture_plugin_for_tests".to_owned(), modal_fixture);
    let plugin_thread = std::thread::Builder::new()
        .name("plugin_thread".to_string())
        .spawn(move || {
            plugin_thread_main(
                plugin_bus,
                data_dir,
                data_access,
                Box::default(),
                Some(layout_dir),
                Some(user_plugin_dir),
                vec![],
                vec![],
                default_shell,
                yinz_cwd,
                session_env_vars,
                default_shell_action,
                plugin_aliases,
                InputMode::Normal,
                Keybinds::default(),
                Default::default(),
                hooks,
                Default::default(), // context_menu
                initiating_client_id,
                crate::ServerSession::Fixed("yinz-test".into()),
            )
            .expect("TEST")
        })
        .unwrap();
    let teardown = {
        let to_plugin = to_plugin.clone();
        move || {
            let _ = to_pty.send(PtyInstruction::Exit);
            let _ = to_pty_writer.send(PtyWriteInstruction::Exit);
            let _ = to_screen.send(ScreenInstruction::Exit);
            let _ = to_server.send(ServerInstruction::KillSession);
            let _ = to_plugin.send(PluginInstruction::Exit);
            join_plugin_thread(plugin_thread);
        }
    };
    (
        to_plugin,
        server_receiver,
        screen_receiver,
        Box::new(teardown),
    )
}

fn create_plugin_thread_with_server_receiver(
    yinz_cwd: Option<PathBuf>,
    session_env_vars: Option<std::collections::BTreeMap<String, String>>,
) -> PluginThreadHarnessWith<ServerInstruction> {
    let yinz_cwd = yinz_cwd.unwrap_or_else(|| PathBuf::from("."));
    let session_env_vars = session_env_vars.unwrap_or_else(|| std::env::vars().collect());
    let (to_server, server_receiver): ChannelWithContext<ServerInstruction> = channels::bounded(50);
    let to_server = SenderWithContext::new(to_server);

    let (to_screen, screen_receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    let to_screen = SenderWithContext::new(to_screen);

    let (to_plugin, plugin_receiver): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_plugin = SenderWithContext::new(to_plugin);
    let (to_pty, _pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    let to_pty = SenderWithContext::new(to_pty);

    let (to_pty_writer, _pty_writer_receiver): ChannelWithContext<PtyWriteInstruction> =
        channels::unbounded();
    let to_pty_writer = SenderWithContext::new(to_pty_writer);

    let (to_background_jobs, _background_jobs_receiver): ChannelWithContext<BackgroundJob> =
        channels::unbounded();
    let to_background_jobs = SenderWithContext::new(to_background_jobs);

    let plugin_bus = Bus::new(
        vec![plugin_receiver],
        Some(&to_screen),
        Some(&to_pty),
        Some(&to_plugin),
        Some(&to_server),
        Some(&to_pty_writer),
        Some(&to_background_jobs),
        None,
    )
    .should_silently_fail();
    let data_dir = PathBuf::from(tempdir().unwrap().path());
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &data_dir).unwrap();
    let data_access = Arc::new(DataAccess::sqlite(&data_dir));
    let default_shell = PathBuf::from(".");
    let default_shell_action = None;
    let initiating_client_id = ClientId::new(1);
    let plugin_thread = std::thread::Builder::new()
        .name("plugin_thread".to_string())
        .spawn(move || {
            plugin_thread_main(
                plugin_bus,
                data_dir,
                data_access,
                Box::default(),
                None,
                None,
                vec![],
                vec![],
                default_shell,
                yinz_cwd,
                session_env_vars,
                default_shell_action,
                PluginAliases::default(),
                InputMode::Normal,
                Keybinds::default(),
                Default::default(),
                Default::default(), // hooks
                Default::default(), // context_menu
                initiating_client_id,
                crate::ServerSession::Fixed("yinz-test".into()),
            )
            .expect("TEST");
        })
        .unwrap();
    let teardown = {
        let to_plugin = to_plugin.clone();
        move || {
            let _ = to_pty.send(PtyInstruction::Exit);
            let _ = to_pty_writer.send(PtyWriteInstruction::Exit);
            let _ = to_screen.send(ScreenInstruction::Exit);
            let _ = to_server.send(ServerInstruction::KillSession);
            let _ = to_plugin.send(PluginInstruction::Exit);
            join_plugin_thread(plugin_thread);
        }
    };
    (
        to_plugin,
        server_receiver,
        screen_receiver,
        Box::new(teardown),
    )
}

fn create_plugin_thread_with_pty_receiver(
    yinz_cwd: Option<PathBuf>,
    layout_dir: Option<PathBuf>,
    session_env_vars: Option<std::collections::BTreeMap<String, String>>,
) -> PluginThreadHarnessWith<PtyInstruction> {
    create_plugin_thread_with_pty_receiver_and_layout(
        yinz_cwd,
        layout_dir,
        session_env_vars,
        Box::default(),
    )
}

fn create_plugin_thread_with_pty_receiver_and_layout(
    yinz_cwd: Option<PathBuf>,
    layout_dir: Option<PathBuf>,
    session_env_vars: Option<std::collections::BTreeMap<String, String>>,
    layout: Box<yinz_types::input::layout::Layout>,
) -> PluginThreadHarnessWith<PtyInstruction> {
    let yinz_cwd = yinz_cwd.unwrap_or_else(|| PathBuf::from("."));
    let session_env_vars = session_env_vars.unwrap_or_else(|| std::env::vars().collect());
    let (to_server, _server_receiver): ChannelWithContext<ServerInstruction> =
        channels::bounded(50);
    let to_server = SenderWithContext::new(to_server);

    let (to_screen, screen_receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    let to_screen = SenderWithContext::new(to_screen);

    let (to_plugin, plugin_receiver): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_plugin = SenderWithContext::new(to_plugin);
    let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    let to_pty = SenderWithContext::new(to_pty);

    let (to_pty_writer, _pty_writer_receiver): ChannelWithContext<PtyWriteInstruction> =
        channels::unbounded();
    let to_pty_writer = SenderWithContext::new(to_pty_writer);

    let (to_background_jobs, _background_jobs_receiver): ChannelWithContext<BackgroundJob> =
        channels::unbounded();
    let to_background_jobs = SenderWithContext::new(to_background_jobs);

    let plugin_bus = Bus::new(
        vec![plugin_receiver],
        Some(&to_screen),
        Some(&to_pty),
        Some(&to_plugin),
        Some(&to_server),
        Some(&to_pty_writer),
        Some(&to_background_jobs),
        None,
    )
    .should_silently_fail();
    let data_dir = PathBuf::from(tempdir().unwrap().path());
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &data_dir).unwrap();
    let data_access = Arc::new(DataAccess::sqlite(&data_dir));
    let layout_dir = layout_dir.unwrap_or_else(|| PathBuf::from(tempdir().unwrap().path()));
    let user_plugin_dir = PathBuf::from(tempdir().unwrap().path());
    let default_shell = PathBuf::from(".");
    let default_shell_action = None;
    let initiating_client_id = ClientId::new(1);
    let plugin_thread = std::thread::Builder::new()
        .name("plugin_thread".to_string())
        .spawn(move || {
            plugin_thread_main(
                plugin_bus,
                data_dir,
                data_access,
                layout,
                Some(layout_dir),
                Some(user_plugin_dir),
                vec![],
                vec![],
                default_shell,
                yinz_cwd,
                session_env_vars,
                default_shell_action,
                PluginAliases::default(),
                InputMode::Normal,
                Keybinds::default(),
                Default::default(),
                Default::default(), // hooks
                Default::default(), // context_menu
                initiating_client_id,
                crate::ServerSession::Fixed("yinz-test".into()),
            )
            .expect("TEST")
        })
        .unwrap();
    let teardown = {
        let to_plugin = to_plugin.clone();
        move || {
            let _ = to_pty.send(PtyInstruction::Exit);
            let _ = to_pty_writer.send(PtyWriteInstruction::Exit);
            let _ = to_screen.send(ScreenInstruction::Exit);
            let _ = to_server.send(ServerInstruction::KillSession);
            let _ = to_plugin.send(PluginInstruction::Exit);
            join_plugin_thread(plugin_thread);
        }
    };
    (to_plugin, pty_receiver, screen_receiver, Box::new(teardown))
}

fn create_plugin_thread_with_background_jobs_receiver(
    yinz_cwd: Option<PathBuf>,
    session_env_vars: Option<std::collections::BTreeMap<String, String>>,
) -> PluginThreadHarnessWith<BackgroundJob> {
    let yinz_cwd = yinz_cwd.unwrap_or_else(|| PathBuf::from("."));
    let session_env_vars = session_env_vars.unwrap_or_else(|| std::env::vars().collect());
    let (to_server, _server_receiver): ChannelWithContext<ServerInstruction> =
        channels::bounded(50);
    let to_server = SenderWithContext::new(to_server);

    let (to_screen, screen_receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    let to_screen = SenderWithContext::new(to_screen);

    let (to_plugin, plugin_receiver): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_plugin = SenderWithContext::new(to_plugin);
    let (to_pty, _pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    let to_pty = SenderWithContext::new(to_pty);

    let (to_pty_writer, _pty_writer_receiver): ChannelWithContext<PtyWriteInstruction> =
        channels::unbounded();
    let to_pty_writer = SenderWithContext::new(to_pty_writer);

    let (to_background_jobs, background_jobs_receiver): ChannelWithContext<BackgroundJob> =
        channels::unbounded();
    let to_background_jobs = SenderWithContext::new(to_background_jobs);

    let plugin_bus = Bus::new(
        vec![plugin_receiver],
        Some(&to_screen),
        Some(&to_pty),
        Some(&to_plugin),
        Some(&to_server),
        Some(&to_pty_writer),
        Some(&to_background_jobs),
        None,
    )
    .should_silently_fail();
    let data_dir = PathBuf::from(tempdir().unwrap().path());
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &data_dir).unwrap();
    let data_access = Arc::new(DataAccess::sqlite(&data_dir));
    let default_shell = PathBuf::from(".");
    let default_shell_action = None;
    let initiating_client_id = ClientId::new(1);
    let plugin_thread = std::thread::Builder::new()
        .name("plugin_thread".to_string())
        .spawn(move || {
            plugin_thread_main(
                plugin_bus,
                data_dir,
                data_access,
                Box::default(),
                None,
                None,
                vec![],
                vec![],
                default_shell,
                yinz_cwd,
                session_env_vars,
                default_shell_action,
                PluginAliases::default(),
                InputMode::Normal,
                Keybinds::default(),
                Default::default(),
                Default::default(), // hooks
                Default::default(), // context_menu
                initiating_client_id,
                crate::ServerSession::Fixed("yinz-test".into()),
            )
            .expect("TEST")
        })
        .unwrap();
    let teardown = {
        let to_plugin = to_plugin.clone();
        move || {
            let _ = to_pty.send(PtyInstruction::Exit);
            let _ = to_pty_writer.send(PtyWriteInstruction::Exit);
            let _ = to_screen.send(ScreenInstruction::Exit);
            let _ = to_server.send(ServerInstruction::KillSession);
            let _ = to_plugin.send(PluginInstruction::Exit);
            let _ = to_background_jobs.send(BackgroundJob::Exit);
            join_plugin_thread(plugin_thread);
        }
    };
    (
        to_plugin,
        background_jobs_receiver,
        screen_receiver,
        Box::new(teardown),
    )
}

/// The checked-in Scheme fixture stays outside the production builtin source tree.
fn plugin_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../yinz-steel-builtins/fixtures/fixture-plugin-for-tests.scm")
}

/// Locates the Scheme fixture plugin.
///
/// There is nothing to build: this source is checked in next to
/// these tests - so this always resolves and needs no prior `cargo x build`.
fn script_plugin_fixture_path() -> PathBuf {
    PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"),
    )
    .join("src/unit/plugin_tests/fixtures/fixture.scm")
}

lazy_static! {
    static ref PLUGIN_FIXTURE: String = plugin_fixture_path().display().to_string();
    static ref SCRIPT_PLUGIN_FIXTURE: String = script_plugin_fixture_path().display().to_string();
    static ref SCRIPT_PLUGIN_FIXTURE_WITH_PERMISSIONS: String = script_plugin_fixture_path()
        .with_file_name("fixture_with_permissions.scm")
        .display()
        .to_string();
    static ref SCRIPT_PLUGIN_FIXTURE_SPAWN_AGENT: String = script_plugin_fixture_path()
        .with_file_name("fixture_spawn_agent.scm")
        .display()
        .to_string();
}

// =====================================================================
// Plugin Highlight API Integration Tests
// =====================================================================

use crate::PaneId;

mod batching;
mod break_panes_commands;
mod cli_events;
mod config_events;
mod focused_pane_commands;
mod highlights;
mod host_theme;
mod keybinds;
mod layout_api;
mod loading;
mod mode_and_tab_commands;
mod open_and_session_commands;
mod pane_id_commands;
mod permissions;
mod plugin_lifecycle_commands;
mod run_and_pipe_commands;
mod runaway;
mod script_harness;
mod script_plugins;
mod self_and_pane_commands;
mod steel_binary;
mod steel_cleanup;
mod steel_data;
mod steel_filesystem;
mod steel_lifecycle;
mod steel_plugins;
mod steel_services;
mod steel_sources;
mod tab_commands;

mod agents_parity;
pub(crate) mod data_parity;
mod file_finder_parity;
mod layout_manager_parity;
mod legacy_parity;
mod link_parity;
mod multiple_select_parity;
mod plugin_manager_parity;
pub(crate) mod runtime_parity;
mod runtime_parity_dialogs;
mod session_manager_parity;

mod project_store_parity;
mod todos_parity;

mod new_agent_parity;

mod agent_service_parity;

mod steel_workers;

mod bar_parity;

mod status_bar;

mod tab_bar;

mod compact_bar;

mod steel_builtin_parity;

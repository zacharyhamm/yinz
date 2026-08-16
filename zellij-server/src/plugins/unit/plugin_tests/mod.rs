use super::plugin_thread_main;
use crate::screen::ScreenInstruction;
use crate::{channels::SenderWithContext, thread_bus::Bus, ServerInstruction};
use insta::assert_snapshot;
use lazy_static::lazy_static;
use serial_test::serial;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tempfile::tempdir;
use wasmi::Engine;
use zellij_utils::data::{
    BareKey, Event, InputMode, KeyWithModifier, ModeInfo, PermissionStatus, PermissionType,
};

use zellij_utils::errors::ErrorContext;
use zellij_utils::input::actions::Action;
use zellij_utils::input::keybinds::Keybinds;
use zellij_utils::input::layout::{
    PluginAlias, PluginUserConfiguration, RunPlugin, RunPluginLocation, RunPluginOrAlias,
};

use crate::background_jobs::BackgroundJob;
use crate::pty_writer::PtyWriteInstruction;
use crate::{plugins::PluginInstruction, pty::PtyInstruction};
use std::env::set_var;
use std::sync::{Arc, Mutex};
use zellij_utils::channels::{self, ChannelWithContext, Receiver};
use zellij_utils::input::permission::PermissionCache;
use zellij_utils::input::plugins::PluginAliases;
use zellij_utils::pane_size::Size;

/// How long a fake screen thread waits for the next event before declaring the test stuck.
///
/// Generous, because these tests compile and instantiate a real wasm plugin — the point is only to
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
                FAKE_SCREEN_RECV_TIMEOUT, &*PLUGIN_FIXTURE
            )
        })
}

macro_rules! log_actions_in_thread {
    ( $arc_mutex_log:expr, $exit_event:path, $receiver:expr, $exit_after_count:expr ) => {
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
                            log.lock().unwrap().push(event);
                            if exit_event_count == $exit_after_count {
                                break;
                            }
                        },
                        _ => {
                            log.lock().unwrap().push(event);
                        },
                    }
                }
            })
            .unwrap()
    };
}

macro_rules! log_actions_in_thread_struct {
    ( $arc_mutex_log:expr, $exit_event:path, $receiver:expr, $exit_after_count:expr ) => {
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
                            log.lock().unwrap().push(event);
                            if exit_event_count == $exit_after_count {
                                break;
                            }
                        },
                        _ => {
                            log.lock().unwrap().push(event);
                        },
                    }
                }
            })
            .unwrap()
    };
}

macro_rules! grant_permissions_and_log_actions_in_thread {
    ( $arc_mutex_log:expr, $exit_event:path, $receiver:expr, $exit_after_count:expr, $permission_type:expr, $cache_path:expr, $plugin_thread_sender:expr, $client_id:expr ) => {
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
                            log.lock().unwrap().push(event);
                            if exit_event_count == $exit_after_count {
                                break;
                            }
                        },
                        ScreenInstruction::RequestPluginPermissions(_, plugin_permission) => {
                            if plugin_permission.permissions.contains($permission_type) {
                                let _ = plugin_thread_sender.send(
                                    PluginInstruction::PermissionRequestResult(
                                        0,
                                        Some($client_id),
                                        plugin_permission.permissions,
                                        PermissionStatus::Granted,
                                        Some(cache_path.clone()),
                                    ),
                                );
                            } else {
                                let _ = plugin_thread_sender.send(
                                    PluginInstruction::PermissionRequestResult(
                                        0,
                                        Some($client_id),
                                        plugin_permission.permissions,
                                        PermissionStatus::Denied,
                                        Some(cache_path.clone()),
                                    ),
                                );
                            }
                        },
                        _ => {
                            log.lock().unwrap().push(event);
                        },
                    }
                }
            })
            .unwrap()
    };
}

macro_rules! deny_permissions_and_log_actions_in_thread {
    ( $arc_mutex_log:expr, $exit_event:path, $receiver:expr, $exit_after_count:expr, $permission_type:expr, $cache_path:expr, $plugin_thread_sender:expr, $client_id:expr ) => {
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
                            log.lock().unwrap().push(event);
                            if exit_event_count == $exit_after_count {
                                break;
                            }
                        },
                        ScreenInstruction::RequestPluginPermissions(_, plugin_permission) => {
                            let _ = plugin_thread_sender.send(
                                PluginInstruction::PermissionRequestResult(
                                    0,
                                    Some($client_id),
                                    plugin_permission.permissions,
                                    PermissionStatus::Denied,
                                    Some(cache_path.clone()),
                                ),
                            );
                            break;
                        },
                        _ => {
                            log.lock().unwrap().push(event);
                        },
                    }
                }
            })
            .unwrap()
    };
}

macro_rules! grant_permissions_and_log_actions_in_thread_naked_variant {
    ( $arc_mutex_log:expr, $exit_event:path, $receiver:expr, $exit_after_count:expr, $permission_type:expr, $cache_path:expr, $plugin_thread_sender:expr, $client_id:expr ) => {
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
                            log.lock().unwrap().push(event);
                            if exit_event_count == $exit_after_count {
                                break;
                            }
                        },
                        ScreenInstruction::RequestPluginPermissions(_, plugin_permission) => {
                            if plugin_permission.permissions.contains($permission_type) {
                                let _ = plugin_thread_sender.send(
                                    PluginInstruction::PermissionRequestResult(
                                        0,
                                        Some($client_id),
                                        plugin_permission.permissions,
                                        PermissionStatus::Granted,
                                        Some(cache_path.clone()),
                                    ),
                                );
                            } else {
                                let _ = plugin_thread_sender.send(
                                    PluginInstruction::PermissionRequestResult(
                                        0,
                                        Some($client_id),
                                        plugin_permission.permissions,
                                        PermissionStatus::Denied,
                                        Some(cache_path.clone()),
                                    ),
                                );
                            }
                        },
                        _ => {
                            log.lock().unwrap().push(event);
                        },
                    }
                }
            })
            .unwrap()
    };
}

macro_rules! grant_permissions_and_log_actions_in_thread_struct_variant {
    ( $arc_mutex_log:expr, $exit_event:path, $receiver:expr, $exit_after_count:expr, $permission_type:expr, $cache_path:expr, $plugin_thread_sender:expr, $client_id:expr ) => {
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
                            log.lock().unwrap().push(event);
                            if exit_event_count == $exit_after_count {
                                break;
                            }
                        },
                        ScreenInstruction::RequestPluginPermissions(_, plugin_permission) => {
                            if plugin_permission.permissions.contains($permission_type) {
                                let _ = plugin_thread_sender.send(
                                    PluginInstruction::PermissionRequestResult(
                                        0,
                                        Some($client_id),
                                        plugin_permission.permissions,
                                        PermissionStatus::Granted,
                                        Some(cache_path.clone()),
                                    ),
                                );
                            } else {
                                let _ = plugin_thread_sender.send(
                                    PluginInstruction::PermissionRequestResult(
                                        0,
                                        Some($client_id),
                                        plugin_permission.permissions,
                                        PermissionStatus::Denied,
                                        Some(cache_path.clone()),
                                    ),
                                );
                            }
                        },
                        _ => {
                            log.lock().unwrap().push(event);
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

fn create_plugin_thread(
    zellij_cwd: Option<PathBuf>,
    session_env_vars: Option<std::collections::BTreeMap<String, String>>,
) -> PluginThreadHarness {
    let zellij_cwd = zellij_cwd.unwrap_or_else(|| PathBuf::from("."));
    let session_env_vars = session_env_vars.unwrap_or_else(|| std::env::vars().collect());
    let initiating_client_id = 1;
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
    let mut config = wasmi::Config::default();
    config.set_max_stack_height(1024 * 1024);
    config.set_max_recursion_depth(1000);
    let engine = Engine::new(&config);
    let data_dir = PathBuf::from(tempdir().unwrap().path());
    let layout_dir = PathBuf::from(tempdir().unwrap().path());
    let default_shell = PathBuf::from(".");
    let default_shell_action = None; // TODO: change me
    let mut plugin_aliases = PluginAliases::default();
    plugin_aliases.aliases.insert(
        "fixture_plugin_for_tests".to_owned(),
        RunPlugin::from_url(&format!("file:{}", &*PLUGIN_FIXTURE)).unwrap(),
    );
    let plugin_thread = std::thread::Builder::new()
        .name("plugin_thread".to_string())
        .spawn(move || {
            set_var("ZELLIJ_SESSION_NAME", "zellij-test");
            plugin_thread_main(
                plugin_bus,
                engine,
                data_dir,
                Box::default(),
                Some(layout_dir),
                vec![],
                vec![],
                default_shell,
                zellij_cwd,
                session_env_vars,
                default_shell_action,
                plugin_aliases,
                InputMode::Normal,
                Keybinds::default(),
                Default::default(),
                None,
                initiating_client_id,
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
            let _ = plugin_thread.join();
        }
    };
    (to_plugin, screen_receiver, Box::new(teardown))
}

fn create_plugin_thread_with_server_receiver(
    zellij_cwd: Option<PathBuf>,
    session_env_vars: Option<std::collections::BTreeMap<String, String>>,
) -> PluginThreadHarnessWith<ServerInstruction> {
    let zellij_cwd = zellij_cwd.unwrap_or_else(|| PathBuf::from("."));
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
    let mut config = wasmi::Config::default();
    config.set_max_stack_height(1024 * 1024);
    config.set_max_recursion_depth(1000);
    let engine = Engine::new(&config);
    let data_dir = PathBuf::from(tempdir().unwrap().path());
    let default_shell = PathBuf::from(".");
    let default_shell_action = None; // TODO: change me
    let initiating_client_id = 1;
    let plugin_thread = std::thread::Builder::new()
        .name("plugin_thread".to_string())
        .spawn(move || {
            set_var("ZELLIJ_SESSION_NAME", "zellij-test");
            plugin_thread_main(
                plugin_bus,
                engine,
                data_dir,
                Box::default(),
                None,
                vec![],
                vec![],
                default_shell,
                zellij_cwd,
                session_env_vars,
                default_shell_action,
                PluginAliases::default(),
                InputMode::Normal,
                Keybinds::default(),
                Default::default(),
                None,
                initiating_client_id,
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
            let _ = plugin_thread.join();
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
    zellij_cwd: Option<PathBuf>,
    layout_dir: Option<PathBuf>,
    session_env_vars: Option<std::collections::BTreeMap<String, String>>,
) -> PluginThreadHarnessWith<PtyInstruction> {
    let zellij_cwd = zellij_cwd.unwrap_or_else(|| PathBuf::from("."));
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
    let mut config = wasmi::Config::default();
    config.set_max_stack_height(1024 * 1024);
    config.set_max_recursion_depth(1000);
    let engine = Engine::new(&config);
    let data_dir = PathBuf::from(tempdir().unwrap().path());
    let layout_dir = layout_dir.unwrap_or_else(|| PathBuf::from(tempdir().unwrap().path()));
    let default_shell = PathBuf::from(".");
    let default_shell_action = None; // TODO: change me
    let initiating_client_id = 1;
    let plugin_thread = std::thread::Builder::new()
        .name("plugin_thread".to_string())
        .spawn(move || {
            set_var("ZELLIJ_SESSION_NAME", "zellij-test");
            plugin_thread_main(
                plugin_bus,
                engine,
                data_dir,
                Box::default(),
                Some(layout_dir),
                vec![],
                vec![],
                default_shell,
                zellij_cwd,
                session_env_vars,
                default_shell_action,
                PluginAliases::default(),
                InputMode::Normal,
                Keybinds::default(),
                Default::default(),
                None,
                initiating_client_id,
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
            let _ = plugin_thread.join();
        }
    };
    (to_plugin, pty_receiver, screen_receiver, Box::new(teardown))
}

fn create_plugin_thread_with_background_jobs_receiver(
    zellij_cwd: Option<PathBuf>,
    session_env_vars: Option<std::collections::BTreeMap<String, String>>,
) -> PluginThreadHarnessWith<BackgroundJob> {
    let zellij_cwd = zellij_cwd.unwrap_or_else(|| PathBuf::from("."));
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
    let mut config = wasmi::Config::default();
    config.set_max_stack_height(1024 * 1024);
    config.set_max_recursion_depth(1000);
    let engine = Engine::new(&config);
    let data_dir = PathBuf::from(tempdir().unwrap().path());
    let default_shell = PathBuf::from(".");
    let default_shell_action = None; // TODO: change me
    let initiating_client_id = 1;
    let plugin_thread = std::thread::Builder::new()
        .name("plugin_thread".to_string())
        .spawn(move || {
            set_var("ZELLIJ_SESSION_NAME", "zellij-test");
            plugin_thread_main(
                plugin_bus,
                engine,
                data_dir,
                Box::default(),
                None,
                vec![],
                vec![],
                default_shell,
                zellij_cwd,
                session_env_vars,
                default_shell_action,
                PluginAliases::default(),
                InputMode::Normal,
                Keybinds::default(),
                Default::default(),
                None,
                initiating_client_id,
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
            let _ = plugin_thread.join();
        }
    };
    (
        to_plugin,
        background_jobs_receiver,
        screen_receiver,
        Box::new(teardown),
    )
}

/// Locates the fixture plugin's wasm blob.
///
/// Prefers the debug artifact, which `cargo x test` guarantees is present and fresh because it
/// builds the plugins before running any tests. Falls back to the checked-in asset so that a bare
/// `cargo test -p zellij-server` works too, at the cost of the blob being as old as the last
/// `cargo x assets`.
fn plugin_fixture_path() -> PathBuf {
    let repo_root = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"),
    )
    .join("..");

    let candidates = [
        repo_root.join("target/wasm32-wasip1/debug/fixture-plugin-for-tests.wasm"),
        repo_root.join("zellij-utils/assets/plugins/fixture-plugin-for-tests.wasm"),
    ];

    candidates
        .iter()
        .find(|path| path.exists())
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "could not find the fixture plugin. Run `cargo x build -p` to build the plugins. \
                 Looked in:\n  {}",
                candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n  ")
            )
        })
}

lazy_static! {
    static ref PLUGIN_FIXTURE: String = plugin_fixture_path().display().to_string();
}

// =====================================================================
// Plugin Highlight API Integration Tests
// =====================================================================

use crate::panes::PaneId;

mod break_panes_commands;
mod config_events;
mod focused_pane_commands;
mod highlights;
mod keybinds;
mod layout_api;
mod loading;
mod mode_and_tab_commands;
mod open_and_session_commands;
mod pane_id_commands;
mod permissions;
mod plugin_lifecycle_commands;
mod run_and_pipe_commands;
mod self_and_pane_commands;
mod tab_commands;

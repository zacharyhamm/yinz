//! A plugin that never returns from `update` must not take the session with it.
//!
//! An instance looping inside an entrypoint holds its own lock on the executor thread it is
//! pinned to. Unloading it used to take that lock on the plugin thread — forever — and session
//! shutdown joins the plugin thread, so one runaway plugin held a dead session's process open,
//! with every hook client that connected afterwards piling up behind the join. Now an unload
//! whose cleanup the guest keeps from running sets the plugin's cancel token after a grace
//! period (shutdown sets it at once), the confined guest process is terminated, and the unload proceeds.
//!
//! The assertion in each test is partly the test finishing: `teardown` joins the plugin thread,
//! which without the token would never return.

#[cfg(test)]
const PLUGIN_0: yinz_types::internal_ids::PluginId = yinz_types::internal_ids::PluginId::new(0);

use super::*;
use crate::ClientId;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;

/// A fake screen thread that grants every permission a plugin asks for and logs everything
/// else, until `done` is set. Tests use `Watched::wait_until` to observe the first frame and
/// the marker sent by the Scheme fixture when it enters its loop.
fn observe_screen(
    receiver: Receiver<(ScreenInstruction, ErrorContext)>,
    plugin_thread_sender: SenderWithContext<PluginInstruction>,
    _client_id: ClientId,
    cache_path: PathBuf,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> (Watched<Vec<ScreenInstruction>>, std::thread::JoinHandle<()>) {
    let log = Watched::new(vec![]);
    let handle = std::thread::Builder::new()
        .name("fake_screen_thread".to_string())
        .spawn({
            let log = log.clone();
            move || loop {
                let event = match receiver.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok((event, _)) => event,
                    Err(err)
                        if err.is_timeout() && !done.load(std::sync::atomic::Ordering::Relaxed) =>
                    {
                        continue
                    },
                    Err(_) => break,
                };
                match event {
                    ScreenInstruction::RequestPluginPermissions(_plugin_id, permissions) => {
                        let _ = plugin_thread_sender.send(PluginInstruction::PermissionAnswer {
                            request_id: permissions.request_id,
                            status: PermissionStatus::Granted,
                            cache_path: Some(cache_path.clone()),
                        });
                    },
                    other => log.lock().push(other),
                }
            }
        })
        .unwrap();
    (log, handle)
}

fn pure_runaway_plugin() -> RunPluginOrAlias {
    let path = script_plugin_fixture_path().with_file_name("fixture_runaway.scm");
    RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(path),
        ..Default::default()
    })
}

fn host_call_runaway_plugin() -> RunPluginOrAlias {
    RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: PluginUserConfiguration::new(BTreeMap::from([(
            "runaway".to_owned(),
            "true".to_owned(),
        )])),
        ..Default::default()
    })
}

/// A plugin thread with `run_plugin` loaded as plugin 0 and a fake screen observing it.
struct RunawayHarness {
    plugin_thread_sender: SenderWithContext<PluginInstruction>,
    log: Watched<Vec<ScreenInstruction>>,
    client_id: ClientId,
    teardown: Option<Box<dyn FnOnce()>>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    screen_thread: Option<std::thread::JoinHandle<()>>,
    _temp_folder: tempfile::TempDir,
}

impl RunawayHarness {
    fn load(run_plugin: RunPluginOrAlias) -> Self {
        let temp_folder = tempdir().unwrap();
        let plugin_host_folder = PathBuf::from(temp_folder.path());
        let cache_path = plugin_host_folder.join("permissions_test.kdl");
        let (plugin_thread_sender, screen_receiver, teardown) =
            create_plugin_thread(Some(plugin_host_folder), None);
        let client_id = ClientId::new(1);
        let size = Size { cols: 80, rows: 10 };
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (log, screen_thread) = observe_screen(
            screen_receiver,
            plugin_thread_sender.clone(),
            client_id,
            cache_path,
            done.clone(),
        );

        let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
        let _ = plugin_thread_sender.send(PluginInstruction::Load(
            Some(false),
            false,
            false,
            Some("runaway".to_owned()),
            run_plugin,
            Some(TabId::new(1)),
            None,
            client_id,
            size,
            None,
            None,
            false,
            None,
            None,
            None,
            None,
        ));
        Self {
            plugin_thread_sender,
            log,
            client_id,
            teardown: Some(teardown),
            done,
            screen_thread: Some(screen_thread),
            _temp_folder: temp_folder,
        }
    }

    /// The first plugin loaded by a fresh thread is id 0. Its first frame means it is loaded,
    /// has its permissions, and is taking events.
    fn wait_for_first_frame(&self) {
        self.log.wait_until(
            "the runaway plugin's first frame",
            yinz_test_support::default_timeout(),
            |instructions| {
                instructions
                    .iter()
                    .any(|i| matches!(i, ScreenInstruction::PluginBytes(assets) if assets.iter().any(|a| {
                        let frame = String::from_utf8_lossy(&a.bytes);
                        a.plugin_id == crate::PluginId::new(0)
                            && (frame.contains("runaway plugin, idle") || frame.contains("Received events:"))
                    })))
                    .then_some(())
            },
        );
    }

    /// The key that starts the plugin's loop.
    fn start_the_loop(&self) {
        let _ = self
            .plugin_thread_sender
            .send(PluginInstruction::Update(vec![(
                Some(crate::PluginId::new(0)),
                Some(self.client_id),
                Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
            )]));
    }

    fn wait_for_loop_entry(&self) {
        self.log.wait_until(
            "the guest entered its runaway callback",
            yinz_test_support::default_timeout(),
            |log| {
                log.iter()
                    .any(|instruction| {
                        matches!(
                            instruction,
                            ScreenInstruction::SetSelectable(PaneId::Plugin(PLUGIN_0), false)
                        )
                    })
                    .then_some(())
            },
        );
    }

    /// Tear the plugin thread down — the join a runaway plugin used to hang — and return how
    /// long that took, plus everything the screen saw.
    fn finish(mut self) -> (std::time::Duration, Vec<ScreenInstruction>) {
        let started = std::time::Instant::now();
        (self.teardown.take().unwrap())();
        let elapsed = started.elapsed();
        self.done.store(true, std::sync::atomic::Ordering::Relaxed);
        self.screen_thread.take().unwrap().join().unwrap();
        let seen = self.log.lock().clone();
        (elapsed, seen)
    }
}

impl Drop for RunawayHarness {
    fn drop(&mut self) {
        // `finish` took the thread; a test that panicked before it did not, and must not leave
        // the fake screen thread spinning.
        self.done.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.screen_thread.take() {
            let _ = thread.join();
        }
    }
}

/// Retiring a runtime must let the executing callback return (or cancel it), then finish
/// shutdown within its budget. Its late failure must not paint an unloaded pane.
fn a_runaway_plugin_is_stopped_by_its_unload(run_plugin: RunPluginOrAlias) {
    let harness = RunawayHarness::load(run_plugin);
    harness.wait_for_first_frame();
    harness.start_the_loop();
    harness.wait_for_loop_entry();
    let _ = harness
        .plugin_thread_sender
        .send(PluginInstruction::Unload(crate::PluginId::new(0)));

    let (elapsed, seen) = harness.finish();
    assert!(
        elapsed < yinz_test_support::default_timeout(),
        "unload did not cancel the callback: {elapsed:?}"
    );
    let crash_reports: Vec<String> = seen
        .iter()
        .filter(|i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(PLUGIN_0, _)))
        .map(|i| format!("{i:?}"))
        .filter(|report| report.contains("Panic!") || report.contains("already borrowed"))
        .collect();
    assert!(
        crash_reports.is_empty(),
        "retired runtime failures must not paint a pane: {crash_reports:#?}"
    );
}

/// Shutting the session down while the plugin is inside its loop. The plugin thread's exit is
/// what the server's join waits on, with `PLUGIN_THREAD_EXIT_TIMEOUT` before it gives up, and
/// `EXIT_TIMEOUT` is the plugin thread's own budget for the in-flight jobs *and* the unloads
/// after them: a runaway plugin is cancelled once it has overrun its grace, not left to hold
/// the whole wait and then be cancelled by the unloads, so the thread is gone inside the budget.
fn session_shutdown_stops_a_runaway_plugin_inside_its_budget(run_plugin: RunPluginOrAlias) {
    let harness = RunawayHarness::load(run_plugin);
    harness.wait_for_first_frame();
    harness.start_the_loop();
    harness.wait_for_loop_entry();

    let (elapsed, _) = harness.finish();
    assert!(
        elapsed < crate::EXIT_TIMEOUT,
        "the plugin thread took {elapsed:?} to exit, which is not inside its {:?} budget",
        crate::EXIT_TIMEOUT
    );
}

/// The callback remains interruptible while it continuously dispatches host commands.
#[test]
#[serial(plugin_tests)]
pub fn a_runaway_scheme_plugin_is_stopped_by_its_unload() {
    a_runaway_plugin_is_stopped_by_its_unload(host_call_runaway_plugin());
}

#[test]
#[serial(plugin_tests)]
pub fn session_shutdown_stops_a_pure_scheme_loop_inside_its_budget() {
    session_shutdown_stops_a_runaway_plugin_inside_its_budget(pure_runaway_plugin());
}

#[test]
#[serial(plugin_tests)]
pub fn session_shutdown_stops_a_scheme_host_call_loop_inside_its_budget() {
    session_shutdown_stops_a_runaway_plugin_inside_its_budget(host_call_runaway_plugin());
}

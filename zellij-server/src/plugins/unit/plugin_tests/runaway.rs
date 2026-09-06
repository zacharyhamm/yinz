//! A plugin that never returns from `update` must not take the session with it.
//!
//! An instance looping inside an entrypoint holds its own lock on the executor thread it is
//! pinned to. Unloading it used to take that lock on the plugin thread — forever — and session
//! shutdown joins the plugin thread, so one runaway plugin held a dead session's process open,
//! with every hook client that connected afterwards piling up behind the join. Now an unload
//! whose cleanup the guest keeps from running sets the plugin's cancel token after a grace
//! period (shutdown sets it at once), the guest is trapped at its next host call (wasm) or
//! within a few thousand instructions (Lua), and the unload proceeds.
//!
//! The assertion in each test is partly the test finishing: `teardown` joins the plugin thread,
//! which without the token would never return.

#[cfg(test)]
const PLUGIN_0: zellij_types::internal_ids::PluginId = zellij_types::internal_ids::PluginId::new(0);

use super::*;
use crate::plugins::plugin_map::CANCELLED_BY_HOST;
use crate::ClientId;
use zellij_test_support::Watched;
use zellij_types::internal_ids::TabId;

/// A fake screen thread that grants every permission a plugin asks for and logs everything
/// else, until `done` is set. Not driven by a count of some instruction: what these tests wait
/// for is a *content*, the crash report carrying `CANCELLED_BY_HOST`, and `Watched::wait_until`
/// reads that off the log.
fn observe_screen(
    receiver: Receiver<(ScreenInstruction, ErrorContext)>,
    plugin_thread_sender: SenderWithContext<PluginInstruction>,
    client_id: ClientId,
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
                    ScreenInstruction::RequestPluginPermissions(plugin_id, permissions) => {
                        let _ =
                            plugin_thread_sender.send(PluginInstruction::PermissionRequestResult(
                                plugin_id,
                                Some(client_id),
                                permissions.permissions,
                                PermissionStatus::Granted,
                                Some(cache_path.clone()),
                            ));
                    },
                    other => log.lock().push(other),
                }
            }
        })
        .unwrap();
    (log, handle)
}

fn lua_runaway_plugin() -> RunPluginOrAlias {
    let path = lua_plugin_fixture_path().with_file_name("fixture_runaway.lua");
    RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(path),
        ..Default::default()
    })
}

fn wasm_runaway_plugin() -> RunPluginOrAlias {
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
            zellij_test_support::default_timeout(),
            |instructions| {
                instructions
                    .iter()
                    .any(|i| matches!(i, ScreenInstruction::PluginBytes(assets) if assets.iter().any(|a| a.plugin_id == crate::PluginId::new(0))))
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

    fn wait_for_cancel_report(&self) {
        // An unload gives the guest a grace period before cancelling it, so this wait is longer
        // than the usual one: it covers the grace and then some.
        self.log.wait_until(
            "the crash report saying the host cancelled the plugin",
            zellij_test_support::default_timeout() * 3,
            |instructions| {
                instructions
                    .iter()
                    .any(|i| format!("{i:?}").contains(CANCELLED_BY_HOST))
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

/// Load `run_plugin`, wait for its first frame, send it the key that starts its loop, unload
/// it, and wait for the crash report that says the host stopped it. Then tear the thread down,
/// which is the join a runaway plugin used to hang.
///
/// The cancel is reported once. The instance was stopped mid-call, and a wasm instance stopped
/// that way cannot be called again (the trap leaves `zellij-tile`'s state cell borrowed), so
/// its `BeforeClose` is skipped rather than reported as a second, `already borrowed`, crash.
fn a_runaway_plugin_is_stopped_by_its_unload(run_plugin: RunPluginOrAlias) {
    let harness = RunawayHarness::load(run_plugin);
    harness.wait_for_first_frame();
    harness.start_the_loop();
    let _ = harness
        .plugin_thread_sender
        .send(PluginInstruction::Unload(crate::PluginId::new(0)));
    harness.wait_for_cancel_report();

    let (_, seen) = harness.finish();
    let crash_reports: Vec<String> = seen
        .iter()
        .filter(|i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(PLUGIN_0, _)))
        .map(|i| format!("{i:?}"))
        .filter(|report| report.contains("Panic!") || report.contains("already borrowed"))
        .collect();
    assert_eq!(
        crash_reports.len(),
        1,
        "the cancel is reported once and BeforeClose is not attempted on the stopped instance: \
         {crash_reports:#?}"
    );
    assert!(crash_reports[0].contains(CANCELLED_BY_HOST));
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
    // Nothing observable marks the start of the loop — the plugin never renders or calls the
    // host again once it is in it — so give the key a moment to land. If it has not by the
    // time the exit arrives, the exit finds an idle plugin and the test proves less, not more.
    std::thread::sleep(std::time::Duration::from_millis(200));

    let (elapsed, _) = harness.finish();
    assert!(
        elapsed < crate::plugins::EXIT_TIMEOUT,
        "the plugin thread took {elapsed:?} to exit, which is not inside its {:?} budget",
        crate::plugins::EXIT_TIMEOUT
    );
}

/// Lua: the loop never touches the host, and the instruction-count hook stops it anyway.
#[test]
#[serial(plugin_tests)]
pub fn a_runaway_lua_plugin_is_stopped_by_its_unload() {
    a_runaway_plugin_is_stopped_by_its_unload(lua_runaway_plugin());
}

/// Wasm: the loop calls the host every iteration, which is where the call hook stops it. (A
/// wasm loop that never calls the host is not stopped; that would take fuel metering.)
#[test]
#[serial(plugin_tests)]
pub fn a_runaway_wasm_plugin_is_stopped_by_its_unload() {
    a_runaway_plugin_is_stopped_by_its_unload(wasm_runaway_plugin());
}

#[test]
#[serial(plugin_tests)]
pub fn session_shutdown_stops_a_runaway_lua_plugin_inside_its_budget() {
    session_shutdown_stops_a_runaway_plugin_inside_its_budget(lua_runaway_plugin());
}

#[test]
#[serial(plugin_tests)]
pub fn session_shutdown_stops_a_runaway_wasm_plugin_inside_its_budget() {
    session_shutdown_stops_a_runaway_plugin_inside_its_budget(wasm_runaway_plugin());
}

/// A plugin that never returns from `load`. Its loader holds the plugin map across that call,
/// and an unload takes the plugin map — so the unload must set the plugin's token *before*
/// waiting for the lock, or it waits forever, with nothing left that could set the token.
#[test]
#[serial(plugin_tests)]
pub fn a_lua_plugin_stuck_in_load_is_stopped_by_its_unload() {
    let path = lua_plugin_fixture_path().with_file_name("fixture_runaway_load.lua");
    let harness = RunawayHarness::load(RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(path),
        ..Default::default()
    }));
    // The plugin's `load` reaches the screen before it starts looping: once this is seen, the
    // loader is inside `load`, holding the plugin map.
    harness.log.wait_until(
        "the plugin's load to be running",
        zellij_test_support::default_timeout(),
        |instructions| {
            instructions
                .iter()
                .any(|i| {
                    matches!(
                        i,
                        ScreenInstruction::SetSelectable(PaneId::Plugin(PLUGIN_0), false)
                    )
                })
                .then_some(())
        },
    );
    let _ = harness
        .plugin_thread_sender
        .send(PluginInstruction::Unload(crate::PluginId::new(0)));
    harness.wait_for_cancel_report();
    harness.finish();
}

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

use super::*;
use crate::plugins::plugin_map::CANCELLED_BY_HOST;
use crate::ClientId;
use zellij_test_support::Watched;

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

/// Load `run_plugin`, wait for its first frame, send it the key that starts its loop, unload
/// it, and wait for the crash report that says the host stopped it. Then tear the thread down,
/// which is the join a runaway plugin used to hang.
fn a_runaway_plugin_is_stopped_by_its_unload(run_plugin: RunPluginOrAlias) {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = 1;
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
        Some(1),
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
    // The first plugin loaded by a fresh thread is id 0. Its first frame means it is loaded,
    // has its permissions, and is taking events.
    log.wait_until(
        "the runaway plugin's first frame",
        zellij_test_support::default_timeout(),
        |instructions| {
            instructions
                .iter()
                .any(|i| matches!(i, ScreenInstruction::PluginBytes(assets) if assets.iter().any(|a| a.plugin_id == 0)))
                .then_some(())
        },
    );
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        Some(0),
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
    )]));
    let _ = plugin_thread_sender.send(PluginInstruction::Unload(0));
    // An unload gives the guest a grace period before cancelling it, so this wait is longer
    // than the usual one: it covers the grace and then some.
    log.wait_until(
        "the crash report saying the host cancelled the plugin",
        zellij_test_support::default_timeout() * 3,
        |instructions| {
            instructions
                .iter()
                .any(|i| format!("{i:?}").contains(CANCELLED_BY_HOST))
                .then_some(())
        },
    );

    teardown();
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    screen_thread.join().unwrap();
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

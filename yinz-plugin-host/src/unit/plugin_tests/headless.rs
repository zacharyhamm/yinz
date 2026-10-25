//! Plugins run headless unless they declare a pane: the invariant, through the real plugin
//! thread and executor. A plugin with no declaration never gets a pane record (no `AddPlugin`),
//! never renders, and still runs; one that declares a pane gets exactly the record its launcher
//! asked for, before anything its `load` sends.
use super::*;
use crate::plugin_panes::Surface;
use yinz_bus::route::{ActionCompletionResult, NotificationEnd};
use yinz_types::data::{PluginCommand, PluginInstanceInfo};
use yinz_types::internal_ids::{PluginId, TabId, TerminalId};
use yinz_types::pane_id::PaneId;
use yinz_types::pane_tabs::{PaneTabDescriptor, PaneTabsId};

const CLIENT: ClientId = ClientId::new(1);
const SIZE: Size = Size { rows: 10, cols: 80 };

/// Declares its pane, and uses it from `load`.
const DECLARED: &str = r#"
(yinz-declare-pane)
(define (load config) (yinz-command (hash "SetSelectable" #f)))
(define (render rows cols) "declared pane")
"#;

/// Declares nothing. A `CustomMessage` names what to do next.
const UNDECLARED: &str = r#"
(define (load config)
  (yinz-command (hash "Subscribe" (list "CustomMessage")))
  (yinz-command (hash "RequestPluginPermissions" (list "ReadApplicationState"))))
(define (update event)
  (when (and (hash? event) (hash-contains? event "CustomMessage"))
    (define name (car (hash-ref event "CustomMessage")))
    (cond
      ((equal? name "hide") (yinz-command "HideSelf"))
      ((equal? name "close") (yinz-command "CloseSelf"))))
  #t)
(define (render rows cols) "HEADLESS PLUGIN MUST NEVER RENDER")
"#;

/// A plugin thread with a fake screen that grants permissions, places plugin panes opened
/// through actions the way the real screen asks for them, and answers the completion of each
/// pane it is handed.
struct Session {
    sender: SenderWithContext<PluginInstruction>,
    log: Watched<Vec<ScreenInstruction>>,
    directory: tempfile::TempDir,
    teardown: Option<Box<dyn FnOnce()>>,
    loggers: Vec<std::thread::JoinHandle<()>>,
}

impl Session {
    fn new() -> Self {
        let directory = tempdir().unwrap();
        let (sender, server_receiver, screen_receiver, teardown) =
            create_plugin_thread_with_services(
                Some(directory.path().to_owned()),
                None,
                None,
                Default::default(),
                None,
                None,
                Some(directory.path().to_owned()),
            );
        let server_log = Watched::new(vec![]);
        let log = Watched::new(vec![]);
        let screen = {
            let log = log.clone();
            let sender = sender.clone();
            let cache = directory.path().join("permissions.kdl");
            std::thread::Builder::new()
                .name("fake_screen_thread".into())
                .spawn(move || loop {
                    match screen_receiver.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT) {
                        Ok((instruction, _)) => {
                            log.lock().push(fake_screen(instruction, &sender, &cache));
                        },
                        Err(channels::RecvTimeoutError::Disconnected) => break,
                        Err(error) => panic!("fake screen gave up waiting: {error}"),
                    }
                })
                .unwrap()
        };
        sender.send(PluginInstruction::AddClient(CLIENT)).unwrap();
        Self {
            sender,
            log,
            directory,
            teardown: Some(teardown),
            loggers: vec![screen, log_until_disconnect(&server_log, server_receiver)],
        }
    }

    fn write(&self, name: &str, source: &str) -> RunPluginOrAlias {
        let path = self.directory.path().join(name);
        std::fs::write(&path, source).unwrap();
        RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::File(path),
            ..Default::default()
        })
    }

    /// `PluginInstruction::Load`, as a keybind's `LaunchPlugin` sends it.
    fn launch(
        &self,
        run: RunPluginOrAlias,
    ) -> tokio::sync::oneshot::Receiver<ActionCompletionResult> {
        let (completion, answer) = tokio::sync::oneshot::channel();
        self.sender
            .send(PluginInstruction::Load(
                Some(true),
                false,
                false,
                None,
                run,
                Some(TabId::new(1)),
                None,
                CLIENT,
                SIZE,
                None,
                None,
                false,
                None,
                None,
                Some(NotificationEnd::new(completion)),
                None,
            ))
            .unwrap();
        answer
    }

    fn message(&self, plugin: PluginId, name: &str) {
        self.sender
            .send(PluginInstruction::Update(vec![(
                Some(plugin),
                Some(CLIENT),
                Event::CustomMessage(name.into(), String::new()),
            )]))
            .unwrap();
    }

    fn wait_for(&self, what: &str, found: impl Fn(&ScreenInstruction) -> bool) {
        self.log.wait_until(what, default_timeout(), |log| {
            log.iter().any(&found).then_some(())
        });
    }

    fn logged(&self) -> Vec<String> {
        self.log.lock().iter().map(|i| format!("{i:?}")).collect()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let _ = self.sender.send(PluginInstruction::Exit);
            return;
        }
        if let Some(teardown) = self.teardown.take() {
            teardown();
        }
        for logger in self.loggers.drain(..) {
            logger.join().unwrap();
        }
    }
}

/// What the fake screen does with `instruction` before logging it.
fn fake_screen(
    mut instruction: ScreenInstruction,
    sender: &SenderWithContext<PluginInstruction>,
    cache: &std::path::Path,
) -> ScreenInstruction {
    match &mut instruction {
        ScreenInstruction::RequestPluginPermissions(_, permission) => {
            let _ = sender.send(PluginInstruction::PermissionAnswer {
                request_id: permission.request_id,
                status: PermissionStatus::Granted,
                cache_path: Some(cache.to_owned()),
            });
        },
        ScreenInstruction::NewFloatingPluginPane(
            run,
            title,
            skip_cache,
            cwd,
            _,
            client,
            completion,
            ..,
        ) => {
            let _ = sender.send(PluginInstruction::Load(
                Some(true),
                false,
                false,
                title.clone(),
                run.clone(),
                None,
                None,
                *client,
                SIZE,
                cwd.clone(),
                None,
                *skip_cache,
                Some(false),
                None,
                completion.take(),
                None,
            ));
        },
        ScreenInstruction::AddPlugin(.., plugin_id, _, _, _, _, _, _, completion, _) => {
            if let Some(mut completion) = completion.take() {
                completion.set_affected_pane_id(PaneId::Plugin(*plugin_id));
            }
        },
        // The screen answers these launches as it records the plugin; the answer is filled in.
        ScreenInstruction::PluginPaneless { completion, .. }
        | ScreenInstruction::PluginResolving { completion, .. } => drop(completion.take()),
        _ => {},
    }
    instruction
}

fn answer(
    answer: tokio::sync::oneshot::Receiver<ActionCompletionResult>,
) -> ActionCompletionResult {
    yinz_bus::global_async_runtime::get_tokio_runtime()
        .block_on(async { tokio::time::timeout(default_timeout(), answer).await })
        .expect("the launch is answered")
        .expect("the completion sends its result")
}

fn added(log: &[ScreenInstruction], plugin: PluginId) -> Vec<&ScreenInstruction> {
    log.iter()
        .filter(|i| matches!(i, ScreenInstruction::AddPlugin(.., id, _, _, _, _, _, _, _, _) if *id == plugin))
        .collect()
}

fn paneless(instruction: &ScreenInstruction, plugin: PluginId) -> bool {
    matches!(instruction, ScreenInstruction::PluginPaneless { plugin_id, .. } if *plugin_id == plugin)
}

fn unloaded(instruction: &ScreenInstruction, plugin: PluginId) -> bool {
    matches!(instruction, ScreenInstruction::PluginUnloaded(id) if *id == plugin)
}

fn toast(instruction: &ScreenInstruction, plugin: PluginId, needle: &str) -> bool {
    matches!(instruction, ScreenInstruction::PluginNotify { plugin_id, text, .. }
        if *plugin_id == plugin && text.contains(needle))
}

fn rendered_by(log: &[ScreenInstruction], plugin: PluginId) -> bool {
    log.iter().any(|i| match i {
        ScreenInstruction::PluginBytes(assets) => {
            assets.iter().any(|asset| asset.plugin_id == plugin)
        },
        _ => false,
    })
}

#[test]
#[serial(plugin_tests)]
fn a_launch_gives_a_pane_only_to_a_plugin_that_declares_one() {
    let session = Session::new();
    let declared = PluginId::new(0);
    let headless = PluginId::new(1);
    let declared_answer = answer(session.launch(session.write("declared.scm", DECLARED)));
    assert_eq!(
        declared_answer.affected_pane_id,
        Some(PaneId::Plugin(declared))
    );
    let headless_answer = answer(session.launch(session.write("headless.scm", UNDECLARED)));
    assert_eq!(
        headless_answer.affected_pane_id,
        Some(PaneId::Plugin(headless))
    );
    let stdout = headless_answer.stdout_message.unwrap_or_default();
    assert!(
        stdout.ends_with("runs headless: it declares no pane"),
        "{stdout}"
    );
    assert_eq!(headless_answer.error_message, None);

    wait_for_render_containing(&session.log, "declared pane");
    // The headless plugin runs: it asks for a permission, through the screen's own prompt.
    session.wait_for(
        "the headless plugin's permission prompt",
        |i| matches!(i, ScreenInstruction::RequestPluginPermissions(id, _) if *id == headless),
    );
    // Its own pane is refused, as an error its launcher hears about once.
    session.message(headless, "hide");
    session.wait_for("the own-pane refusal", |i| {
        toast(i, headless, "crashed:")
            && toast(i, headless, "'HideSelf' needs the plugin's own pane")
    });

    let log = session.log.lock().clone();
    // The declared plugin's pane reached the screen before what its `load` sent.
    let add = log
        .iter()
        .position(|i| !added(std::slice::from_ref(i), declared).is_empty())
        .expect("the declared plugin's pane");
    let selectable = log
        .iter()
        .position(|i| matches!(i, ScreenInstruction::SetSelectable(PaneId::Plugin(id), false) if *id == declared))
        .expect("SetSelectable from load");
    assert!(add < selectable, "{:?}", session.logged());
    assert!(added(&log, headless).is_empty(), "{:?}", session.logged());
    assert!(log.iter().any(|i| paneless(i, headless)));
    assert!(!rendered_by(&log, headless), "{:?}", session.logged());
}

#[test]
#[serial(plugin_tests)]
fn a_background_start_parks_only_a_plugin_that_declares_a_pane() {
    let session = Session::new();
    let undeclared = path_source(false);
    for (name, source) in [("declared.scm", DECLARED), ("headless.scm", &undeclared)] {
        session
            .sender
            .send(PluginInstruction::LoadBackgroundPlugin(
                session.write(name, source),
                CLIENT,
            ))
            .unwrap();
    }
    let (declared, headless) = (PluginId::new(0), PluginId::new(1));
    session.wait_for(
        "the headless background plugin runs",
        |i| matches!(i, ScreenInstruction::RequestPluginPermissions(id, _) if *id == headless),
    );
    session.wait_for("the declared plugin's parked record", |i| {
        matches!(i, ScreenInstruction::AddPlugin(.., id, _, _, true, _, _, _, _, _) if *id == declared)
    });
    render_barrier(&session, headless, "the headless background plugin");
    let log = session.log.lock().clone();
    assert!(added(&log, headless).is_empty(), "{:?}", session.logged());
    assert!(log.iter().any(|i| paneless(i, headless)));
    assert!(!rendered_by(&log, headless));
}

#[test]
#[serial(plugin_tests)]
fn a_slot_held_for_an_undeclared_plugin_is_removed_and_the_plugin_unloaded() {
    let session = Session::new();
    let slot = PaneId::Terminal(TerminalId::new(5));
    for (name, source) in [("declared.scm", DECLARED), ("headless.scm", UNDECLARED)] {
        session
            .sender
            .send(PluginInstruction::LoadIntoSlot {
                run: session.write(name, source),
                slot,
                client_id: CLIENT,
                cwd: session.directory.path().to_owned(),
            })
            .unwrap();
    }
    let (declared, headless) = (PluginId::new(0), PluginId::new(1));
    session.wait_for("the undeclared child unloads", |i| unloaded(i, headless));
    let log = session.log.lock().clone();
    assert!(
        matches!(added(&log, declared).as_slice(),
            [ScreenInstruction::AddPlugin(_, true, _, _, _, _, _, Some(replaced), ..)] if *replaced == slot),
        "{:?}",
        session.logged()
    );
    assert!(added(&log, headless).is_empty());
    assert!(log
        .iter()
        .any(|i| matches!(i, ScreenInstruction::GiveUpSlot(closed) if *closed == slot)));
    assert!(log
        .iter()
        .any(|i| toast(i, headless, "declares no pane; its pane was removed")));
    // It never ran.
    assert!(!log.iter().any(
        |i| matches!(i, ScreenInstruction::RequestPluginPermissions(id, _) if *id == headless)
    ));
}

#[test]
#[serial(plugin_tests)]
fn a_pane_tab_of_an_undeclared_plugin_is_given_up_before_it_is_placed() {
    let session = Session::new();
    let path = session.directory.path().join("headless.scm");
    std::fs::write(&path, UNDECLARED).unwrap();
    for index in 0..2 {
        session
            .sender
            .send(PluginInstruction::LoadPaneTab {
                skip_cache: false,
                id: PaneTabsId(1),
                index,
                generation: 1,
                descriptor: PaneTabDescriptor {
                    label: "headless".into(),
                    location: format!("file:{}", path.display()),
                    configuration: BTreeMap::new(),
                    initial_cwd: None,
                },
                client_id: CLIENT,
                tab_id: TabId::new(1),
                size: SIZE,
            })
            .unwrap();
    }
    let plugin = PluginId::new(1);
    session.wait_for("the second pane tab's plugin unloads", |i| {
        unloaded(i, plugin)
    });
    let log = session.log.lock().clone();
    for plugin in [PluginId::new(0), plugin] {
        let paneless_at = log.iter().position(|i| paneless(i, plugin)).unwrap();
        let loaded_at = log
            .iter()
            .position(|i| matches!(i, ScreenInstruction::PaneTabLoaded { result: Ok((id, _)), .. } if *id == plugin))
            .unwrap();
        // The screen hears the plugin has no pane before it would place it.
        assert!(paneless_at < loaded_at, "{:?}", session.logged());
        assert!(added(&log, plugin).is_empty());
    }
    // One notice per location, however many slots it lost.
    let notices = log
        .iter()
        .filter(|i| matches!(i, ScreenInstruction::PluginNotify { text, .. } if text.contains("its layout pane was removed")))
        .count();
    assert_eq!(notices, 1, "{:?}", session.logged());
}

#[test]
#[serial(plugin_tests)]
fn a_reload_moves_a_plugin_between_its_pane_and_headless() {
    let session = Session::new();
    let run = session.write("flip.scm", DECLARED);
    let plugin = PluginId::new(0);
    answer(session.launch(run.clone()));
    wait_for_render_containing(&session.log, "declared pane");
    session.write("flip.scm", UNDECLARED);
    session
        .sender
        .send(PluginInstruction::ReloadPluginWithId(plugin))
        .unwrap();
    session.wait_for("the pane is dropped", |i| paneless(i, plugin));
    session.wait_for(
        "the headless reload runs",
        |i| matches!(i, ScreenInstruction::RequestPluginPermissions(id, _) if *id == plugin),
    );
    session.write("flip.scm", DECLARED);
    session
        .sender
        .send(PluginInstruction::ReloadPluginWithId(plugin))
        .unwrap();
    session
        .log
        .wait_until("a pane again", default_timeout(), |log| {
            (added(log, plugin).len() == 2).then_some(())
        });
    let log = session.log.lock().clone();
    assert!(
        matches!(
            added(&log, plugin)[1],
            ScreenInstruction::AddPlugin(
                Some(true),
                false,
                _,
                _,
                _,
                _,
                _,
                None,
                _,
                false,
                _,
                Some(false),
                ..
            )
        ),
        "{:?}",
        session.logged()
    );
}

#[test]
#[serial(plugin_tests)]
fn a_client_attaching_to_a_headless_plugin_shows_no_loading_for_it() {
    let session = Session::new();
    let plugin = PluginId::new(0);
    answer(session.launch(session.write("headless.scm", UNDECLARED)));
    session.wait_for(
        "the plugin runs",
        |i| matches!(i, ScreenInstruction::RequestPluginPermissions(id, _) if *id == plugin),
    );
    let attached_at = session.log.lock().len();
    session
        .sender
        .send(PluginInstruction::AddClient(ClientId::new(2)))
        .unwrap();
    // The new client's instance is loaded and the screen asked for state for it.
    session
        .log
        .wait_until("the new client's instance", default_timeout(), |log| {
            log[attached_at..]
                .iter()
                .any(|i| matches!(i, ScreenInstruction::RequestStateUpdateForPlugins))
                .then_some(())
        });
    let log = session.log.lock().clone();
    assert!(
        !log.iter().any(|i| matches!(
            i,
            ScreenInstruction::StartPluginLoadingIndication(id, _) if *id == plugin
        )),
        "{:?}",
        session.logged()
    );
    assert!(added(&log, plugin).is_empty());
}

#[test]
#[serial(plugin_tests)]
fn a_declaration_the_scan_cannot_see_fails_the_load() {
    let session = Session::new();
    let run = session.write(
        "hidden.scm",
        "(begin (yinz-declare-pane)) (define (render rows cols) \"hidden\")",
    );
    let plugin = PluginId::new(0);
    let launched = answer(session.launch(run));
    assert_eq!(launched.affected_pane_id, Some(PaneId::Plugin(plugin)));
    session.wait_for("the mismatch is reported", |i| {
        toast(
            i,
            plugin,
            "(yinz-declare-pane) must appear literally at the top level of the entry file",
        )
    });
    assert!(added(&session.log.lock(), plugin).is_empty());
}

#[test]
#[serial(plugin_tests)]
fn closing_a_headless_plugin_unloads_it_without_a_pane() {
    let session = Session::new();
    let plugin = PluginId::new(0);
    answer(session.launch(session.write("headless.scm", UNDECLARED)));
    session.wait_for(
        "the plugin runs",
        |i| matches!(i, ScreenInstruction::RequestPluginPermissions(id, _) if *id == plugin),
    );
    session.message(plugin, "close");
    session.wait_for("the plugin unloads", |i| unloaded(i, plugin));
    assert!(!session
        .log
        .lock()
        .iter()
        .any(|i| matches!(i, ScreenInstruction::CloseSelf(..))));
}

#[test]
#[serial(plugin_tests)]
fn an_unresolved_alias_fails_its_launch_without_a_pane() {
    let session = Session::new();
    let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "no-such-alias",
        &None,
        None,
        None,
    )
    .unwrap();
    let launched = answer(session.launch(run));
    let error = launched.error_message.expect("the launch fails");
    assert!(error.contains("Failed to resolve plugin alias"), "{error}");
    let plugin = PluginId::new(0);
    session.wait_for("the client is told", |i| toast(i, plugin, "failed to load"));
    assert!(added(&session.log.lock(), plugin).is_empty());
}

/// A git plugin's declaration waits for its repository, here one that never answers. Its launch
/// is answered with its id at once, and the screen hears it is loading (`PluginResolving`) so
/// that a second launch finds it; nothing claims a pane or headlessness it does not know yet.
/// Unloading it cancels the clone.
#[test]
#[serial(plugin_tests)]
fn a_git_plugin_resolving_its_repository_is_announced_and_answered_at_once() {
    use yinz_types::input::git_plugin::GitPlugin;
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = silent.local_addr().unwrap().port();
    // Accepts and holds every connection, answering nothing, until the test ends.
    let held = std::thread::spawn(move || {
        let mut connections = vec![];
        for connection in silent.incoming() {
            match connection {
                Ok(connection) => connections.push(connection),
                Err(_) => break,
            }
            if connections.len() > 8 {
                break;
            }
        }
    });
    let session = Session::new();
    let run = RunPluginOrAlias::RunPlugin(RunPlugin {
        location: RunPluginLocation::Git(GitPlugin {
            repo: format!("git://127.0.0.1:{port}/"),
            entrypoint: "main.scm".into(),
            revision: None,
        }),
        ..Default::default()
    });
    let plugin = PluginId::new(0);
    let launched = answer(session.launch(run));
    assert_eq!(launched.error_message, None);
    assert_eq!(launched.affected_pane_id, Some(PaneId::Plugin(plugin)));
    {
        let log = session.log.lock();
        assert!(
            log.iter().any(|i| matches!(i, ScreenInstruction::PluginResolving { plugin_id, .. } if *plugin_id == plugin)),
            "{:?}",
            session.logged()
        );
        assert!(added(&log, plugin).is_empty());
        assert!(!log.iter().any(|i| paneless(i, plugin)));
    }
    session
        .sender
        .send(PluginInstruction::Unload(plugin))
        .unwrap();
    session.wait_for("the unload", |i| unloaded(i, plugin));
    drop(session);
    // Nothing connects any more once the session is gone; wake the listener so it ends.
    drop(std::net::TcpStream::connect(("127.0.0.1", port)));
    for _ in 0..8 {
        drop(std::net::TcpStream::connect(("127.0.0.1", port)));
    }
    held.join().unwrap();
}

/// A guest that waits for another plugin's pane must not wait for that plugin's load: with one
/// executor thread, the guest holds the only thread the load could be scheduled on.
#[test]
#[serial(plugin_tests)]
fn a_guest_opening_a_plugin_pane_returns_with_one_executor_thread() {
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            crate::plugin_bridge::TEST_EXECUTOR_THREADS.with(|fixed| fixed.set(0));
        }
    }
    let _restore = Restore;
    crate::plugin_bridge::TEST_EXECUTOR_THREADS.with(|fixed| fixed.set(1));
    // A target refused before its declaration is read (a Lua entry) fails the open, also
    // without the executor.
    for (name, target, opened) in [
        ("target.scm", DECLARED, "opened 1"),
        ("target.scm", UNDECLARED, "opened 1"),
        ("target.lua", "print('lua')", "returned"),
    ] {
        let session = Session::new();
        let target = session.write(name, target);
        let RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::File(target),
            ..
        }) = target
        else {
            unreachable!()
        };
        let opener = format!(
            r#"
(yinz-declare-pane)
(define state "waiting")
(define (load config)
  (yinz-command (hash "Subscribe" (list "PermissionRequestResult")))
  (yinz-command (hash "RequestPluginPermissions" (list "OpenTerminalsOrPlugins"))))
(define (update event)
  (when (and (hash? event) (hash-contains? event "PermissionRequestResult"))
    (define pane (yinz-command (hash "OpenPluginPaneFloating"
      (hash "plugin_url" "file:{}" "configuration" (hash)
            "floating_pane_coordinates" json-null "context" (hash)))))
    (set! state "returned")
    (when (and (hash? pane) (hash-contains? pane "Plugin"))
      (set! state (string-append "opened " (number->string (hash-ref pane "Plugin"))))))
  #t)
(define (render rows cols) state)
"#,
            target.display()
        );
        answer(session.launch(session.write("opener.scm", &opener)));
        wait_for_render_containing(&session.log, opened);
    }
}

/// The one-thread override above belongs to the test that sets it: bridges built on any other
/// thread, such as those of tests running beside it under a bare `cargo test`, keep their usual
/// executor size.
#[test]
fn a_fixed_executor_size_stays_on_the_thread_that_set_it() {
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            crate::plugin_bridge::TEST_EXECUTOR_THREADS.with(|fixed| fixed.set(0));
        }
    }
    let _restore = Restore;
    crate::plugin_bridge::TEST_EXECUTOR_THREADS.with(|fixed| fixed.set(1));
    assert_eq!(crate::plugin_bridge::executor_threads(), 1);
    let elsewhere = std::thread::spawn(crate::plugin_bridge::executor_threads)
        .join()
        .unwrap();
    assert!(
        elsewhere >= 4,
        "another thread's bridge must not get this test's executor size, got {elsewhere}"
    );
}

fn headless_env() -> crate::plugin_map::PluginEnv {
    let mut env = crate::test_env::test_env();
    env.surface = Surface::Headless;
    env
}

#[test]
fn plugin_quit_commands_are_immediate_with_or_without_a_pane() {
    for surface in [Surface::Headless, Surface::Pane] {
        for command in [
            PluginCommand::QuitYinz,
            PluginCommand::RunAction(Action::Quit, BTreeMap::new()),
        ] {
            let mut env = crate::test_env::test_env();
            env.surface = surface;
            env.set_permissions(std::collections::HashSet::from([
                PermissionType::ChangeApplicationState,
                PermissionType::RunActionsAsUser,
            ]));
            let client = env.client_id;
            let (to_server, server) = channels::unbounded();
            let (to_screen, screen) = channels::unbounded();
            env.senders.to_server = Some(SenderWithContext::new(to_server));
            env.senders.to_screen = Some(SenderWithContext::new(to_screen));
            let exited = std::thread::spawn(move || {
                crossbeam::select! {
                    recv(server) -> instruction => {
                        match instruction.unwrap().0 {
                            ServerInstruction::ClientExit(routed_client, completion) => {
                                assert_eq!(routed_client, client);
                                assert!(completion.is_some());
                            },
                            other => panic!("unexpected server instruction: {other:?}"),
                        }
                    },
                    recv(screen) -> instruction => {
                        panic!("{surface:?} quit must not prompt: {instruction:?}");
                    },
                    default(default_timeout()) => panic!("{surface:?} plugin did not quit"),
                }
            });
            crate::yinz_exports::run_plugin_command(&mut env, command).unwrap();
            exited.join().unwrap();
        }
    }
}

#[test]
fn own_pane_commands_are_refused_only_without_a_pane() {
    let mut env = headless_env();
    let error =
        crate::yinz_exports::run_plugin_command(&mut env, PluginCommand::HideSelf).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "'HideSelf' needs the plugin's own pane, but {} declares no pane; add \
             (yinz-declare-pane) at the top level of its entry file",
            env.name()
        )
    );
    // Commands that mean something without a pane still run.
    crate::yinz_exports::run_plugin_command(&mut env, PluginCommand::CloseSelf).unwrap();
    let mut env = crate::test_env::test_env();
    crate::yinz_exports::run_plugin_command(&mut env, PluginCommand::HideSelf).unwrap();
}

#[test]
fn plugin_info_reports_whether_each_plugin_has_a_pane() {
    let info = |env: &mut crate::plugin_map::PluginEnv, id: Option<PluginId>| {
        crate::yinz_exports::run_plugin_command(env, PluginCommand::GetPluginInfo(id)).unwrap();
        let response = <yinz_plugin_api::plugin_command::ProtobufGetPluginInfoResponse as prost::Message>::decode(
            env.response_sink.take().unwrap().as_slice(),
        )
        .unwrap();
        serde_json::from_str::<Option<PluginInstanceInfo>>(&response.json).unwrap()
    };
    let mut env = headless_env();
    // Asking about another plugin reads application state.
    env.set_permissions(std::collections::HashSet::from([
        PermissionType::ReadApplicationState,
    ]));
    let me = env.plugin_id;
    assert_eq!(info(&mut env, None).unwrap().pane, Some(false));
    let other = PluginId::new(9);
    assert_eq!(info(&mut env, Some(other)), None);
    {
        let mut panes = env.panes.lock().unwrap();
        panes.request(
            other,
            1,
            crate::plugin_panes::PaneRequest::Placed {
                run: RunPluginOrAlias::RunPlugin(RunPlugin::default()),
            },
        );
    }
    // Known, still deciding.
    assert_eq!(info(&mut env, Some(other)).unwrap().pane, None);
    let _ = env.panes.lock().unwrap().declare(other, 1, true);
    assert_eq!(info(&mut env, Some(other)).unwrap().pane, Some(true));
    env.surface = Surface::Pane;
    assert_eq!(info(&mut env, Some(me)).unwrap().pane, Some(true));
}

#[test]
fn a_headless_plugin_in_a_tab_hears_events_through_the_registry_and_acts_from_no_pane() {
    let (to_plugin, plugin_receiver) = channels::unbounded();
    let mut env = headless_env();
    assert!(env.tab_index.is_some());
    env.senders = yinz_bus::thread_bus::ThreadSenders {
        to_plugin: Some(SenderWithContext::new(to_plugin)),
        ..Default::default()
    }
    .silently_fail_on_send();
    crate::yinz_exports::run_plugin_command(
        &mut env,
        PluginCommand::Subscribe(std::collections::HashSet::from([
            yinz_types::data::EventType::PaneUpdate,
        ])),
    )
    .unwrap();
    let registered = plugin_receiver.try_iter().any(|(instruction, _)| {
        matches!(instruction, PluginInstruction::PluginSubscribedToEvents(_, _, _, Some(events), _)
            if events.contains(&yinz_types::data::EventType::PaneUpdate))
    });
    assert!(
        registered,
        "a plugin with no pane is in no tab's event routing"
    );
    assert_eq!(env.own_pane(), None);
    env.surface = Surface::Pane;
    assert_eq!(env.own_pane(), Some(PaneId::Plugin(env.plugin_id)));
    assert!(!env.routes_events_by_registry());
}

#[test]
#[serial(plugin_tests)]
fn a_headless_hook_that_declares_no_pane_hears_every_firing_with_no_pane_record() {
    let session = Session::new();
    // The same hook the declared hook tests use, minus its declaration.
    let source = super::script_plugins::HEADLESS_HOOK_SCM.replace("(yinz-declare-pane)\n", "");
    assert!(!source.contains("yinz-declare-pane"));
    super::script_plugins::bind_hook(
        &session.sender,
        yinz_types::input::hooks::HookPoint::PaneExit,
        session.write("hook.scm", &source),
    );
    for pane in [7, 8] {
        session
            .sender
            .send(PluginInstruction::RunHook {
                point: yinz_types::input::hooks::HookPoint::PaneExit,
                pane_id: Some(PaneId::Terminal(TerminalId::new(pane))),
                payload: super::script_plugins::pane_exit_payload(pane),
                client_id: CLIENT,
            })
            .unwrap();
        session.wait_for("the hook names the firing", |i| {
            matches!(i, ScreenInstruction::RenameTabWithId(_, name, _)
                if *name == format!("pane_exit:{pane}").into_bytes())
        });
    }
    let log = session.log.lock().clone();
    assert!(
        !log.iter()
            .any(|i| matches!(i, ScreenInstruction::AddPlugin(..))),
        "{:?}",
        session.logged()
    );
    let hook = PluginId::new(0);
    assert_eq!(
        log.iter().filter(|i| paneless(i, hook)).count(),
        1,
        "one headless instance for every firing: {:?}",
        session.logged()
    );
    assert!(!rendered_by(&log, hook));
}

/// The plugin every launch path below starts, declared or not: it asks for permissions as it
/// loads (the sign that it runs), asks for another when a pipe message reaches it (the sign the
/// message was delivered), and renders only if it has a pane. A `CustomMessage` asks it to
/// render, and asks for a permission named after the message (the sign it was handled).
fn path_source(declared: bool) -> String {
    format!(
        r#"
{}
(define (load config)
  (yinz-command (hash "Subscribe" (list "CustomMessage")))
  (yinz-command (hash "RequestPluginPermissions" (list "ReadApplicationState" "ReadCliPipes"))))
(define (pipe message)
  (yinz-command (hash "RequestPluginPermissions" (list "ChangeApplicationState")))
  #f)
(define (update event)
  (when (and (hash? event) (hash-contains? event "CustomMessage"))
    (yinz-command (hash "RequestPluginPermissions" (list (car (hash-ref event "CustomMessage"))))))
  #t)
(define (render rows cols) "path pane")
"#,
        if declared { "(yinz-declare-pane)" } else { "" }
    )
}

/// What a launch path asks for, which decides what a declared plugin gets.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Asked {
    /// A pane: a declared plugin gets one; an undeclared one runs headless.
    Show,
    /// A background start: a declared plugin gets a parked record; an undeclared one nothing.
    Parked,
    /// A slot the screen holds: a declared plugin keeps it; an undeclared one gives it up and
    /// is unloaded without running.
    Placed,
}

/// Every way the plugin thread starts a plugin, as the instruction that starts it.
///
/// Two more are covered elsewhere, because their plugin is not one a test can write: a slot
/// child (`LoadIntoSlot`) above, and the picker, whose driver is always the builtin file-finder,
/// in `plugin_bridge_tests::the_picker_driver_starts_headless_with_no_pane_record`.
#[derive(Clone, Copy, Debug)]
enum Path {
    Load,
    LoadBackgroundPlugin,
    ReloadStartsIt,
    CliPipe,
    KeybindPipe,
    MessageFromPlugin,
    HeadlessHook,
    ReplacePaneHook,
    PluginsStart,
    LoadPaneTab,
    InPlaceOfPaneId,
    NewTabLayout,
}

const HOOKED: PaneId = PaneId::Terminal(TerminalId::new(7));

impl Path {
    const ALL: [Path; 12] = [
        Path::Load,
        Path::LoadBackgroundPlugin,
        Path::ReloadStartsIt,
        Path::CliPipe,
        Path::KeybindPipe,
        Path::MessageFromPlugin,
        Path::HeadlessHook,
        Path::ReplacePaneHook,
        Path::PluginsStart,
        Path::LoadPaneTab,
        Path::InPlaceOfPaneId,
        Path::NewTabLayout,
    ];

    fn asked(self) -> Asked {
        match self {
            Path::LoadBackgroundPlugin | Path::HeadlessHook | Path::PluginsStart => Asked::Parked,
            Path::LoadPaneTab | Path::NewTabLayout => Asked::Placed,
            _ => Asked::Show,
        }
    }

    /// Whether the path delivers a pipe message to the plugin it starts.
    fn pipes(self) -> bool {
        matches!(
            self,
            Path::CliPipe | Path::KeybindPipe | Path::MessageFromPlugin
        )
    }

    /// Start the plugin at `path` this way; the launch's completion, when it has one.
    fn start(
        self,
        session: &Session,
        path: &std::path::Path,
    ) -> Option<tokio::sync::oneshot::Receiver<ActionCompletionResult>> {
        let url = format!("file:{}", path.display());
        let run = RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::File(path.to_owned()),
            ..Default::default()
        });
        let (completion, answer) = tokio::sync::oneshot::channel();
        let completion = NotificationEnd::new(completion);
        let send = |instruction| session.sender.send(instruction).unwrap();
        match self {
            Path::Load => return Some(session.launch(run)),
            Path::LoadBackgroundPlugin => {
                send(PluginInstruction::LoadBackgroundPlugin(run, CLIENT))
            },
            Path::ReloadStartsIt => send(PluginInstruction::Reload(
                Some(true),
                None,
                run,
                TabId::new(1),
                SIZE,
                Some(completion),
            )),
            Path::CliPipe => send(PluginInstruction::CliPipe {
                focus_plugin: false,
                pipe_id: "path_pipe".into(),
                name: "ping".into(),
                payload: Some("payload".into()),
                is_eof: false,
                plugin: Some(url),
                args: None,
                configuration: None,
                floating: None,
                pane_id_to_replace: None,
                pane_title: None,
                cwd: None,
                skip_cache: false,
                cli_client_id: ClientId::new(5),
                source_pane_id: None,
            }),
            Path::KeybindPipe => send(PluginInstruction::KeybindPipe {
                name: "ping".into(),
                payload: None,
                plugin: Some(url),
                args: None,
                configuration: None,
                floating: None,
                should_open_in_place: false,
                pane_id_to_replace: None,
                pane_title: None,
                cwd: None,
                skip_cache: false,
                cli_client_id: CLIENT,
                plugin_and_client_id: None,
                notification_end: Some(completion),
            }),
            Path::MessageFromPlugin => send(PluginInstruction::MessageFromPlugin {
                source_plugin_id: PluginId::new(99),
                message: yinz_types::data::MessageToPlugin {
                    plugin_url: Some(url),
                    destination_plugin_id: None,
                    plugin_config: BTreeMap::new(),
                    message_name: "ping".into(),
                    message_payload: None,
                    message_args: BTreeMap::new(),
                    new_plugin_args: None,
                    floating_pane_coordinates: None,
                },
            }),
            Path::HeadlessHook | Path::ReplacePaneHook => {
                use crate::HookFiring;
                use yinz_types::input::hooks::HookPoint;
                let (point, payload) = if matches!(self, Path::HeadlessHook) {
                    (
                        HookPoint::PaneExit,
                        super::script_plugins::pane_exit_payload(7),
                    )
                } else {
                    (
                        HookPoint::AgentExit,
                        HookFiring::ReplacePane(BTreeMap::new()),
                    )
                };
                super::script_plugins::bind_hook(&session.sender, point, run);
                send(PluginInstruction::RunHook {
                    point,
                    pane_id: Some(HOOKED),
                    payload,
                    client_id: CLIENT,
                });
            },
            Path::PluginsStart => {
                let (reply, replied) = std::sync::mpsc::channel();
                send(PluginInstruction::PluginRequest {
                    request: yinz_types::cli::PluginRequest {
                        operation: yinz_types::cli::PluginOperation::Start,
                        target: Some(url),
                        configuration: None,
                        fallback: run.get_run_plugin(),
                        candidates: vec![],
                        caller: None,
                    },
                    client_id: CLIENT,
                    reply,
                });
                replied
                    .recv_timeout(default_timeout())
                    .expect("plugins start answers")
                    .expect("plugins start succeeds");
            },
            Path::LoadPaneTab => send(PluginInstruction::LoadPaneTab {
                skip_cache: false,
                id: PaneTabsId(1),
                index: 0,
                generation: 1,
                descriptor: PaneTabDescriptor {
                    label: "path".into(),
                    location: url,
                    configuration: BTreeMap::new(),
                    initial_cwd: None,
                },
                client_id: CLIENT,
                tab_id: TabId::new(1),
                size: SIZE,
            }),
            Path::InPlaceOfPaneId => send(PluginInstruction::OpenPluginPaneInPlaceOfPaneId {
                pane_id: HOOKED,
                plugin_url: url,
                configuration: BTreeMap::new(),
                cwd: session.directory.path().to_owned(),
                tab_id: TabId::new(1),
                client_id: CLIENT,
                completion,
            }),
            Path::NewTabLayout => {
                let layout = yinz_config::kdl::layout::from_kdl(
                    &format!(r#"layout {{ pane {{ plugin location="{url}"; }}; }}"#),
                    None,
                    None,
                    None,
                )
                .unwrap();
                let (tiled, _) = layout.new_tab();
                send(PluginInstruction::NewTab(
                    None,
                    None,
                    Some(tiled),
                    Some(vec![]),
                    TabId::new(1),
                    None,
                    false,
                    false,
                    CLIENT,
                    None,
                ));
            },
        }
        // These hand their completion to the pane they ask for.
        matches!(self, Path::ReloadStartsIt | Path::InPlaceOfPaneId).then_some(answer)
    }
}

/// Waits until a render `plugin` (a [`path_source`] plugin) would send has had its chance to
/// reach the screen. The first message asks the plugin to render, and the second is sent only
/// once the first is being handled, so it runs after that render would have been sent. Without
/// it, "never renders" would be checked before the plugin had any reason to.
fn render_barrier(session: &Session, plugin: PluginId, case: &str) {
    session.message(plugin, "OpenFiles");
    session.wait_for(&format!("{case}: the render request is handled"), |i| {
        asked_for(i, plugin, PermissionType::OpenFiles)
    });
    session.message(plugin, "WriteToStdin");
    session.wait_for(&format!("{case}: the next message is handled"), |i| {
        asked_for(i, plugin, PermissionType::WriteToStdin)
    });
}

fn asked_for(
    instruction: &ScreenInstruction,
    plugin: PluginId,
    permission: PermissionType,
) -> bool {
    matches!(instruction, ScreenInstruction::RequestPluginPermissions(id, request)
        if *id == plugin && request.permissions.contains(&permission))
}

/// §12.1: every launch path, with a plugin that declares a pane and one that does not. Only the
/// declared plugin ever gets a pane record, of the kind its launcher asked for; the undeclared
/// one runs headless (never rendering, still hearing pipes), or - in a slot the screen holds -
/// gives the slot up and is unloaded without running.
#[test]
#[serial(plugin_tests)]
fn every_launch_path_gives_a_pane_only_to_a_plugin_that_declares_one() {
    let plugin = PluginId::new(0);
    for path in Path::ALL {
        for declared in [true, false] {
            let case = format!("{path:?}, declared={declared}");
            let session = Session::new();
            let file = session.directory.path().join("path.scm");
            std::fs::write(&file, path_source(declared)).unwrap();
            let answer = path.start(&session, &file);
            let gives_up_its_slot = !declared && path.asked() == Asked::Placed;
            if gives_up_its_slot {
                session.wait_for(&format!("{case}: the slot is given up"), |i| {
                    unloaded(i, plugin)
                });
            } else {
                session.wait_for(&format!("{case}: the plugin runs"), |i| {
                    asked_for(i, plugin, PermissionType::ReadApplicationState)
                });
            }
            if path.pipes() {
                session.wait_for(&format!("{case}: the message is delivered"), |i| {
                    asked_for(i, plugin, PermissionType::ChangeApplicationState)
                });
            }
            if !declared && !gives_up_its_slot {
                render_barrier(&session, plugin, &case);
            }
            if declared && path.asked() != Asked::Parked {
                wait_for_render_containing(&session.log, "path pane");
            }
            let answered = answer.map(self::answer);
            let log = session.log.lock().clone();
            let added = added(&log, plugin);
            let paneless = log.iter().any(|i| paneless(i, plugin));
            match (declared, path.asked()) {
                (true, Asked::Show) => {
                    assert!(
                        matches!(
                            added.as_slice(),
                            [ScreenInstruction::AddPlugin(.., false, _, _, _, _, _)]
                        ),
                        "{case}: {:?}",
                        session.logged()
                    );
                    if matches!(path, Path::ReplacePaneHook | Path::InPlaceOfPaneId) {
                        assert!(
                            matches!(
                                added[0],
                                ScreenInstruction::AddPlugin(
                                    _,
                                    true,
                                    ..,
                                    Some(HOOKED),
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _,
                                    _
                                )
                            ),
                            "{case}: opened in place of the pane: {:?}",
                            session.logged()
                        );
                    }
                },
                (true, Asked::Parked) => assert!(
                    matches!(
                        added.as_slice(),
                        [ScreenInstruction::AddPlugin(.., true, _, _, _, _, _)]
                    ),
                    "{case}: {:?}",
                    session.logged()
                ),
                (true, Asked::Placed) => {
                    assert!(added.is_empty(), "{case}: the screen holds the slot");
                },
                (false, asked) => {
                    assert!(added.is_empty(), "{case}: {:?}", session.logged());
                    assert!(paneless, "{case}: the screen hears it has no pane");
                    assert!(
                        !rendered_by(&log, plugin),
                        "{case}: a headless plugin renders"
                    );
                    if asked == Asked::Placed {
                        assert!(
                            !log.iter().any(|i| asked_for(
                                i,
                                plugin,
                                PermissionType::ReadApplicationState
                            )),
                            "{case}: a plugin without its slot never runs"
                        );
                    }
                    // A pane the launcher named is left alone.
                    assert!(
                        !log.iter()
                            .any(|i| matches!(i, ScreenInstruction::ClosePane(HOOKED, ..))),
                        "{case}: {:?}",
                        session.logged()
                    );
                },
            }
            if declared {
                assert!(!paneless, "{case}: {:?}", session.logged());
            }
            if let Some(answer) = answered {
                assert_eq!(answer.error_message, None, "{case}");
                assert_eq!(
                    answer.affected_pane_id,
                    Some(PaneId::Plugin(plugin)),
                    "{case}"
                );
                if !declared {
                    let stdout = answer.stdout_message.unwrap_or_default();
                    assert!(
                        stdout.ends_with("runs headless: it declares no pane"),
                        "{case}: {stdout}"
                    );
                }
            }
        }
    }
}

/// G5: a headless plugin has no pane for a new pane to go near, so one it opens without focus is
/// placed against its client rather than against a pane it does not have.
#[test]
fn a_headless_plugin_opens_panes_against_its_client() {
    use yinz_bus::pty::ClientTabIndexOrPaneId;
    for surface in [Surface::Headless, Surface::Pane] {
        let (to_pty, pty) = channels::unbounded();
        let mut env = crate::test_env::test_env();
        env.surface = surface;
        env.set_permissions(std::collections::HashSet::from([
            PermissionType::RunActionsAsUser,
        ]));
        env.senders = yinz_bus::thread_bus::ThreadSenders {
            to_pty: Some(SenderWithContext::new(to_pty)),
            ..Default::default()
        }
        .silently_fail_on_send();
        // The pty thread places the pane; dropping the request answers its completion.
        let placed = std::thread::spawn(move || {
            let (instruction, _) = pty.recv_timeout(default_timeout()).unwrap();
            match instruction {
                PtyInstruction::SpawnTerminal(_, _, _, _, placement, ..) => placement,
                other => panic!("expected a terminal, got {other:?}"),
            }
        });
        crate::yinz_exports::run_plugin_command(
            &mut env,
            PluginCommand::RunAction(
                Action::NewTiledPane {
                    direction: None,
                    command: None,
                    pane_name: None,
                    near_current_pane: false,
                    no_focus: true,
                    borderless: None,
                    tab_id: None,
                },
                BTreeMap::new(),
            ),
        )
        .unwrap();
        let placement = placed.join().unwrap();
        let expected = match surface {
            Surface::Headless => ClientTabIndexOrPaneId::ClientIdNoFocus(env.client_id),
            Surface::Pane => ClientTabIndexOrPaneId::PaneId(PaneId::Plugin(env.plugin_id)),
        };
        assert_eq!(placement, expected, "{surface:?}");
    }
}

/// A plugin launched on request that runs headless and fails in `load` tells its launcher once,
/// with a toast, since it has no pane to show the failure in.
#[test]
#[serial(plugin_tests)]
fn a_headless_load_failure_is_toasted_once() {
    let session = Session::new();
    let failing = PluginId::new(0);
    let launched = answer(session.launch(session.write(
        "failing.scm",
        "(define (load config) (error \"load exploded\"))",
    )));
    assert_eq!(launched.error_message, None);
    session.wait_for("the failure is toasted", |i| {
        toast(i, failing, "failed to load") && toast(i, failing, "load exploded")
    });
    // Anything more the failure sends is on the channel before a later launch's pane.
    answer(session.launch(session.write("declared.scm", DECLARED)));
    wait_for_render_containing(&session.log, "declared pane");
    let log = session.log.lock().clone();
    let toasts = log
        .iter()
        .filter(|i| matches!(i, ScreenInstruction::PluginNotify { plugin_id, .. } if *plugin_id == failing))
        .count();
    assert_eq!(toasts, 1, "{:?}", session.logged());
    assert!(added(&log, failing).is_empty());
}

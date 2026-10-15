//! Cancellation and cleanup through real Scheme workers.
use super::script_harness::{load, release_event, run_plugin, Harness};
use super::*;
use yinz_types::internal_ids::PluginId;
const CLIENT: ClientId = ClientId::new(1);
const PLUGIN: PluginId = PluginId::new(0);

#[test]
#[serial(plugin_tests)]
fn steel_disconnect_interrupts_attached_clients_infinite_load() {
    disconnect_attached_runaway(true);
}

#[test]
#[serial(plugin_tests)]
fn steel_disconnect_interrupts_attached_clients_infinite_callback() {
    disconnect_attached_runaway(false);
}

fn disconnect_attached_runaway(during_load: bool) {
    let source = r#"
(define keys 0)
(define (runaway)
  (when (= (hash-ref (yinz-command "GetPluginIds") "client_id") 2)
    (yinz-command (hash "SetSelectable" #f)) (let loop () (loop))))
(define (load configuration) (yinz-command (hash "Subscribe" '("Key"))) LOAD)
(define (update event)
  (if (and (hash? event) (hash-contains? event "Key"))
      (begin (runaway) (set! keys (+ keys 1)) #t) #f))
(define (render rows columns)
  (string-append "client=" (number->string (hash-ref (yinz-command "GetPluginIds") "client_id")) " keys=" (number->string keys)))
"#;
    let harness = Harness::new(
        "scm",
        &source.replace("LOAD", if during_load { "(runaway)" } else { "(void)" }),
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("client=1 keys=0");
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    if !during_load {
        harness.rendered("client=2 keys=0");
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(other),
                Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
            )]))
            .unwrap();
    }
    harness.log.wait_until(
        "the attaching client's instance entered its loop",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    // A late startup update must not block cancellation behind the loader's map lock.
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            None,
            Event::AvailableLayoutInfo(vec![], vec![]),
        )]))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::RemoveClient(other))
        .unwrap();
    // This instance uses the same executor: a new frame proves cancellation freed it
    // and did not cancel the original client's token.
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('b'))));
    harness.rendered("client=1 keys=1");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_shared_infinite_load_can_unload_after_its_last_client_disconnects() {
    disconnect_shared_runaway(false);
}

#[test]
#[serial(plugin_tests)]
fn steel_shared_infinite_load_can_unload_after_its_owner_disconnects() {
    disconnect_shared_runaway(true);
}

fn disconnect_shared_runaway(another_client: bool) {
    let (finished, completion) = std::sync::mpsc::channel();
    let test = std::thread::spawn(move || {
        let harness = Harness::new(
            "scm",
            r#"
(define (load configuration) (yinz-command (hash "SetSelectable" #f)) (let loop () (loop)))
"#,
            PermissionStatus::Granted,
        );
        if another_client {
            harness
                .sender
                .send(PluginInstruction::AddClient(ClientId::new(2)))
                .unwrap();
        }
        harness.load_shared();
        harness.log.wait_until(
            "the shared loader entered guest code while holding the plugin map",
            default_timeout(),
            |log| {
                log.iter()
                    .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                    .then_some(())
            },
        );
        // Shared instances ignore client cancellation. Removal must return so the
        // next instruction can cancel the load and release its map lock.
        harness
            .sender
            .send(PluginInstruction::RemoveClient(CLIENT))
            .unwrap();
        harness
            .sender
            .send(PluginInstruction::Unload(PLUGIN))
            .unwrap();
        harness.log.wait_until(
            "the queued unload reached subscription cleanup",
            default_timeout(),
            |log| {
                log.iter()
                    .any(|i| matches!(i, ScreenInstruction::UnsubscribePaneOutput(id, None) if *id == PLUGIN))
                    .then_some(())
            },
        );
        harness.finish(); // Queues Exit and joins the plugin thread and its executors.
        finished.send(()).unwrap();
    });
    completion
        .recv_timeout(default_timeout() * 3)
        .expect("disconnect, queued Unload and Exit must complete during a shared infinite load");
    test.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn steel_shared_instance_survives_its_owners_disconnect() {
    let harness = Harness::new(
        "scm",
        r#"
(define keys 0) (define chunks 0)
(define (load configuration)
  (yinz-command (hash "Subscribe" '("Key" "PaneOutput")))
  (yinz-then (yinz-wait-for-permissions '("ReadPaneContents"))
    (lambda (_) (yinz-command (hash "SubscribePaneOutput" (hash "work" "one"))))))
(define (update event)
  (when (and (hash? event) (hash-contains? event "Key")) (set! keys (+ keys 1)))
  (when (and (hash? event) (hash-contains? event "PaneOutput")) (set! chunks (+ chunks 1))) #t)
(define (render rows columns) (string-append "shared keys=" (number->string keys) " chunks=" (number->string chunks)))
"#,
        PermissionStatus::Granted,
    );
    harness.load_shared();
    harness.rendered("shared keys=0");
    let subscription =
        harness
            .log
            .wait_until("shared output subscription", default_timeout(), |log| {
                log.iter().find_map(|instruction| match instruction {
                    ScreenInstruction::SubscribePaneOutput(plugin, client, Some(_), generation) => {
                        Some((*plugin, *client, *generation))
                    },
                    _ => None,
                })
            });
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            Some(other),
            Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
        )]))
        .unwrap();
    harness.rendered("shared keys=1");
    let output = || {
        PluginInstruction::for_plugin_event(
            subscription.0,
            subscription.1,
            subscription.2,
            Event::PaneOutput {
                pane_id: 7,
                data: vec![255],
                context: BTreeMap::new(),
            },
        )
    };
    harness.sender.send(output()).unwrap();
    harness.rendered("shared keys=1 chunks=1");
    harness
        .sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    harness.sender.send(output()).unwrap();
    harness.rendered("shared keys=1 chunks=2");
    let log = harness.log.lock();
    assert_eq!(
        log.iter()
            .filter(|instruction| matches!(instruction, ScreenInstruction::SubscribePaneOutput(..)))
            .count(),
        1
    );
    assert!(!log
        .iter()
        .any(|instruction| matches!(instruction, ScreenInstruction::UnsubscribePaneOutput(..))));
    drop(log);
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_disconnect_during_pending_async_load_discards_its_continuation() {
    let source = r#"
(define label "original")
(define state (string-append label " load pending"))
(define (load configuration)
  (yinz-then (yinz-wait-for-event "CustomMessage" (lambda (payload) (equal? (car payload) "acceptance-release")))
    (lambda (_) (yinz-command (hash "SetSelectable" (equal? label "replacement")))
      (set! state (string-append label " load completed")))))
(define (render rows columns) state)
"#;
    let harness = Harness::new("scm", source, PermissionStatus::Granted);
    harness.load();
    harness.rendered("original load pending");
    std::fs::write(
        &harness.path,
        source.replace("\"original\"", "\"replacement\""),
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    harness.rendered("replacement load pending");
    harness.event(release_event());
    harness.rendered("replacement load completed");
    let log = harness.finish();
    assert!(
        !log.iter()
            .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false))),
        "disconnect must revoke the suspended load's host access"
    );
    assert!(log
        .iter()
        .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true))));
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("original load completed")));
}

#[test]
#[serial(plugin_tests)]
fn steel_timer_before_unload_interrupts_initial_load() {
    cancel_load_after_timer(PluginInstruction::Unload(PLUGIN));
}

#[test]
#[serial(plugin_tests)]
fn steel_timer_before_disconnect_interrupts_initial_load() {
    cancel_load_after_timer(PluginInstruction::RemoveClient(CLIENT));
}

#[test]
#[serial(plugin_tests)]
fn steel_timer_before_shutdown_interrupts_initial_load() {
    cancel_load_after_timer(PluginInstruction::Exit);
}

fn cancel_load_after_timer(cancellation: PluginInstruction) {
    let (finished, completion) = std::sync::mpsc::channel();
    let test = std::thread::spawn(move || {
        let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
        let to_proxy = SenderWithContext::new(to_proxy);
        let harness = Harness::with_event_sender(
            "scm",
            r#"
(define (load configuration) (yinz-command (hash "SetTimeout" 0.0)) (let loop () (loop)))
(define (render rows columns) "runaway load completed")
"#,
            None,
            Some(to_proxy.clone()),
        );
        let completions = Watched::new(Vec::new());
        let proxy = std::thread::spawn({
            let completions = completions.clone();
            let sender = harness.sender.clone();
            move || loop {
                let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
                match instruction {
                    PluginInstruction::Exit => break,
                    timer @ PluginInstruction::PluginReply(_, _, _, Event::Timer(_), _) => {
                        completions.lock().push(timer);
                    },
                    other => {
                        let _ = sender.send(other);
                    },
                }
            }
        });
        harness.load();
        let timer = completions.wait_until(
            "the looping load produced a real host timer completion",
            default_timeout(),
            |events| events.first().cloned(),
        );
        // These share a FIFO: cancellation must be reachable after the real completion.
        harness.sender.send(timer).unwrap();
        let shutdown = matches!(cancellation, PluginInstruction::Exit);
        let disconnect = matches!(cancellation, PluginInstruction::RemoveClient(_));
        std::fs::write(&harness.path, "").unwrap();
        harness.sender.send(cancellation).unwrap();
        if !shutdown {
            if disconnect {
                harness
                    .sender
                    .send(PluginInstruction::AddClient(ClientId::new(2)))
                    .unwrap();
            }
            let survivor = harness.directory.path().join("survivor.scm");
            std::fs::write(
                &survivor,
                "(define (render rows columns) \"survived timer cancellation\")",
            )
            .unwrap();
            load(&harness.sender, run_plugin(&survivor));
            harness.rendered("survived timer cancellation");
        }
        let log = harness.finish();
        assert!(!rendered_assets(&log)
            .iter()
            .any(|(_, frame)| frame.contains("runaway load completed")));
        to_proxy.send(PluginInstruction::Exit).unwrap();
        proxy.join().unwrap();
        finished.send(()).unwrap();
    });
    completion
        .recv_timeout(default_timeout() * 3)
        .expect("timer cancellation and teardown must complete");
    test.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn steel_disconnect_interrupts_initial_load_before_the_instance_is_published() {
    let harness = Harness::new(
        "scm",
        r#"
(define (load configuration) (yinz-command (hash "SetSelectable" #f)) (let loop () (loop)))
(define (render rows columns) "disconnected initialization was published")
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.log.wait_until(
        "the loader entered guest code before publishing the instance",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    // Reattaching may provision this plugin for the new client. Its next load must
    // be safe, while the original isolate remains inside the already-running loop.
    std::fs::write(
        &harness.path,
        "(define (render rows columns) \"replacement after interrupted initialization\")",
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    // A new client and plugin can progress only once the old initialization released
    // the plugin map. No release event or second host call can free that infinite loop.
    harness
        .sender
        .send(PluginInstruction::AddClient(ClientId::new(2)))
        .unwrap();
    let survivor = harness.directory.path().join("survivor.scm");
    std::fs::write(
        &survivor,
        "(define (render rows columns) \"client survived pending initialization\")",
    )
    .unwrap();
    load(&harness.sender, run_plugin(&survivor));
    harness.rendered("client survived pending initialization");
    let log = harness.finish();
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("disconnected initialization was published")));
}

#[test]
#[serial(plugin_tests)]
fn steel_shutdown_delivers_before_close_to_an_idle_plugin_once() {
    let harness = Harness::new(
        "scm",
        r#"
(define (load configuration) (yinz-command (hash "Subscribe" '("BeforeClose"))))
(define (update event)
  (when (equal? event "BeforeClose") (yinz-command (hash "SetSelectable" #f))) #f)
(define (render rows columns) "idle plugin ready for shutdown")
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("idle plugin ready for shutdown");
    // Exercise session shutdown, not an explicit Unload that happens to close a plugin.
    harness.sender.send(PluginInstruction::Exit).unwrap();
    harness.log.wait_until_described(
        "the idle plugin receives BeforeClose during shutdown",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
        |log| describe_renders(log),
    );
    let log = harness.finish();
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
            .count(),
        1,
        "BeforeClose must be delivered exactly once"
    );
    assert!(
        !log.iter().any(
            |i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(id, stage)
        if *id == PLUGIN && stage.is_error())
        ),
        "normal shutdown must not report a guest crash"
    );
}

#[test]
#[serial(plugin_tests)]
fn steel_unload_disposes_pending_load_and_render_awaits() {
    let harness = Harness::new(
        "scm",
        r#"
(define (release) (yinz-wait-for-event "CustomMessage" (lambda (payload) (equal? (car payload) "acceptance-release"))))
(define (load configuration) (yinz-then (release) (lambda (_) (yinz-command (hash "SetSelectable" #f)))))
(define (render rows columns)
  (define ready (release)) (yinz-command (hash "SetSelectable" #t))
  (yinz-then ready (lambda (_) (yinz-command (hash "SetSelectable" #f)) "disposed render resumed")))
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.log.wait_until(
        "both guest callbacks have registered pending awaits",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true)))
                .then_some(())
        },
    );
    harness
        .sender
        .send(PluginInstruction::Unload(PLUGIN))
        .unwrap();
    let survivor = harness.directory.path().join("survivor.scm");
    std::fs::write(
        &survivor,
        r#"
(define state "survivor awaiting input")
(define (load configuration)
  (yinz-then (yinz-wait-for-event "CustomMessage" (lambda (payload) (equal? (car payload) "acceptance-release")))
    (lambda (_) (set! state "survivor received input"))))
(define (render rows columns) state)
"#,
    )
    .unwrap();
    load(&harness.sender, run_plugin(&survivor));
    harness.rendered("survivor awaiting input");
    // Broadcast the very event both discarded callbacks awaited, then observe its delivery.
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            None,
            Some(CLIENT),
            release_event(),
        )]))
        .unwrap();
    harness.rendered("survivor received input");
    let log = harness.finish();
    assert!(
        !log.iter()
            .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false))),
        "unloaded callbacks must not issue host commands"
    );
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("disposed render resumed")));
}

#[test]
#[serial(plugin_tests)]
fn steel_rejected_promise_reports_the_original_scheme_dependency_line() {
    let harness = Harness::new(
        "scm",
        r#"
(require "./failure.scm")
(define state "ready to reject")
(define (load configuration) (yinz-command (hash "Subscribe" '("Key"))))
(define (update event)
  (when (and (hash? event) (hash-contains? event "Key"))
    (set! state "awaiting rejection") (yinz-request-render) (reject-after-input)))
(define (render rows columns) state)
"#,
        PermissionStatus::Granted,
    );
    // Padding distinguishes the module error line from its entrypoint and SDK callback.
    let dependency = r#";; Dependency diagnostics must preserve the original source location.
;; These lines separate its error site from the entrypoint and SDK wrappers.
(provide reject-after-input)
(define (reject-after-input)
  (yinz-then
    (yinz-wait-for-event "CustomMessage" (lambda (payload) (equal? (car payload) "acceptance-release")))
    (lambda (_)
      (error "acceptance rejected promise"))))
"#;
    let expected_line = dependency
        .lines()
        .position(|line| line.contains("(error "))
        .unwrap()
        + 1;
    std::fs::write(harness.directory.path().join("failure.scm"), dependency).unwrap();
    harness.load();
    harness.rendered("ready to reject");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.rendered("awaiting rejection");
    harness.event(release_event());
    let error = harness.log.wait_until_described(
        "the asynchronous rejection reaches the plugin's error UI",
        default_timeout(),
        |log| {
            log.iter().find_map(|i| match i {
                ScreenInstruction::UpdatePluginLoadingStage(id, stage)
                    if *id == PLUGIN && stage.is_error() =>
                {
                    Some(format!("{stage:?}"))
                },
                _ => None,
            })
        },
        |log| describe_renders(log),
    );
    assert!(error.contains("acceptance rejected promise"), "{error}");
    assert!(
        error.contains(&format!("failure.scm:{expected_line}:")),
        "the error must name the original Scheme dependency and source line: {error}"
    );
    harness.finish();
}

fn unload_runaway(source: &str, start_with_key: bool) {
    let harness = Harness::new("scm", source, PermissionStatus::Granted);
    harness.load();
    if start_with_key {
        harness.rendered("ready to loop");
        harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    }
    harness.log.wait_until(
        "the guest entered the callback that loops",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    harness
        .sender
        .send(PluginInstruction::Unload(PLUGIN))
        .unwrap();
    // An unloaded generation no longer owns a pane to paint an error into. Observe
    // continued plugin-thread progress instead of waiting for its suppressed crash UI.
    let survivor = harness.directory.path().join("survivor.scm");
    std::fs::write(
        &survivor,
        "(define (render rows columns) \"survivor is alive\")",
    )
    .unwrap();
    load(&harness.sender, run_plugin(&survivor));
    harness.rendered("survivor is alive");
    // The survivor may get a different executor. finish also joins the screen logger,
    // which cannot disconnect until the old executor releases its screen senders.
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_reload_by_url_interrupts_an_infinite_update_and_runs_the_replacement() {
    let harness = Harness::new(
        "scm",
        r#"
(define (load configuration) (yinz-command (hash "Subscribe" '("Key" "BeforeClose"))))
(define (update event)
  (yinz-command (hash "SetSelectable" #f))
  (unless (equal? event "BeforeClose") (let loop () (loop))) #f)
(define (render rows columns) "original ready to loop")
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("original ready to loop");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.log.wait_until(
        "the old instance entered its infinite update",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    std::fs::write(
        &harness.path,
        r#"
(define keys 0)
(define (load configuration) (yinz-command (hash "Subscribe" '("Key"))) (yinz-command (hash "SetSelectable" #t)))
(define (update event) (set! keys (+ keys 1)) #t)
(define (render rows columns) (string-append "replacement alive keys=" (number->string keys)))
"#,
    )
    .unwrap();
    // Exercise location matching while the guest owns its instance lock. An ID-only
    // reload would bypass the lookup that formerly deadlocked before cancellation.
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
            &harness.path,
        )))
        .unwrap();
    harness.rendered("replacement alive keys=0");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('b'))));
    harness.rendered("replacement alive keys=1");
    let log = harness.finish();
    let selectable: Vec<_> = log
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::SetSelectable(_, value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(
        selectable,
        [false, true],
        "interrupted guests skip cleanup, and the replacement retains host access"
    );
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1,
        "reload by URL must replace the running instance in its existing pane"
    );
    assert!(log.iter().any(|i| matches!(i, ScreenInstruction::PluginBytes(assets)
        if assets.iter().any(|asset| asset.plugin_id == PLUGIN && frame_of(&asset.bytes).contains("replacement alive keys=1")))),
        "the original plugin ID must identify the responsive replacement");
}

#[test]
#[serial(plugin_tests)]
fn steel_failed_reload_disposes_every_client_and_recovers_in_the_same_pane() {
    let harness = Harness::new(
        "scm",
        r#"
(define (load configuration) (yinz-command (hash "Subscribe" '("BeforeClose" "CustomMessage"))))
(define (update event)
  (when (equal? event "BeforeClose") (yinz-command (hash "SetSelectable" #f)))
  (when (and (hash? event) (hash-contains? event "CustomMessage")) (yinz-command (hash "SetSelectable" #t))) #t)
(define (render rows columns) "old guest ready")
"#,
        PermissionStatus::Granted,
    );
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    harness.load();
    let wait_for_both = |needle: &str| {
        harness
            .log
            .wait_until("both client instances rendered", default_timeout(), |log| {
                let assets = rendered_assets(log);
                [CLIENT, other]
                    .iter()
                    .all(|client| {
                        assets
                            .iter()
                            .any(|(id, frame)| id == client && frame.contains(needle))
                    })
                    .then_some(())
            });
    };
    wait_for_both("old guest ready");
    std::fs::write(&harness.path, "(define (broken syntax").unwrap();
    harness.reload();
    harness.log.wait_until("replacement compilation failed", default_timeout(), |log| {
        log.iter().any(|i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(id, stage) if *id == PLUGIN && stage.is_error())).then_some(())
    });
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            None,
            release_event(),
        )]))
        .unwrap();
    std::fs::write(
        &harness.path,
        "(define (render rows columns) \"recovered guest\")",
    )
    .unwrap();
    harness.reload();
    wait_for_both("recovered guest");
    let log = harness.finish();
    let selectable: Vec<_> = log
        .iter()
        .filter_map(|i| match i {
            ScreenInstruction::SetSelectable(_, value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(
        selectable,
        [false, false],
        "each old guest cleans up once and cannot handle input after a failed reload"
    );
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1,
        "recovery retains the pane"
    );
}

/// A reload that arrives while a healthy guest is inside a call must wait that call out: a
/// guest stopped mid-call is not given `BeforeClose`.
#[test]
#[serial(plugin_tests)]
fn steel_reload_by_url_waits_out_a_short_update_before_delivering_before_close() {
    let harness = Harness::new(
        "scm",
        r#"
(define (load configuration) (yinz-command (hash "Subscribe" '("BeforeClose" "Key"))))
(define (update event)
  (if (equal? event "BeforeClose") (yinz-command (hash "SetSelectable" #f))
      (begin (yinz-command (hash "SetSelectable" #t))
        (define end (+ (yinz-now-ms) 100)) (let wait () (when (< (yinz-now-ms) end) (wait))))) #f)
(define (render rows columns) "original ready for a short update")
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("original ready for a short update");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.log.wait_until(
        "the old instance entered its update",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true)))
                .then_some(())
        },
    );
    std::fs::write(
        &harness.path,
        r#"(define (render rows columns) "replacement after a short update")"#,
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
            &harness.path,
        )))
        .unwrap();
    harness.rendered("replacement after a short update");
    let log = harness.finish();
    let selectable: Vec<bool> = log
        .iter()
        .filter_map(|i| match i {
            ScreenInstruction::SetSelectable(_, value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(
        selectable,
        [true, false],
        "the update returns on its own, then the original's BeforeClose runs once"
    );
}

#[test]
#[serial(plugin_tests)]
// Each synchronous call fits the three-second grace; together they cross the old
// call's deadline. The watched command establishes entry before requesting reload.
fn steel_reload_gives_before_close_its_own_grace_after_a_completed_call() {
    let harness = Harness::new(
        "scm",
        r#"
(define (load configuration) (yinz-command (hash "Subscribe" '("BeforeClose" "Key"))))
(define (update event)
  (define key? (and (hash? event) (hash-contains? event "Key")))
  (when (or key? (equal? event "BeforeClose"))
    (when key? (yinz-command (hash "SetSelectable" #t)))
    (define end (+ (yinz-now-ms) 2000)) (let wait () (when (< (yinz-now-ms) end) (wait)))
    (when (equal? event "BeforeClose") (yinz-command (hash "SetSelectable" #f)))) #f)
(define (render rows columns) "original ready for timed cleanup")
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("original ready for timed cleanup");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness
        .log
        .wait_until("original invocation entered", default_timeout(), |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true)))
                .then_some(())
        });
    std::fs::write(
        &harness.path,
        r#"
(define (load configuration) (yinz-command (hash "SetSelectable" #t)))
(define (render rows columns) "replacement after timed cleanup")
"#,
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
            &harness.path,
        )))
        .unwrap();
    harness.rendered("replacement after timed cleanup");
    let log = harness.finish();
    let selectable: Vec<_> = log
        .iter()
        .filter_map(|i| match i {
            ScreenInstruction::SetSelectable(_, value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(
        selectable,
        [true, false, true],
        "a completed invocation's watchdog interrupted eligible cleanup"
    );
}

#[test]
#[serial(plugin_tests)]
fn steel_reload_by_url_delivers_idle_before_close_before_loading_the_replacement() {
    let harness = Harness::new(
        "scm",
        r#"
(define (load configuration) (yinz-command (hash "Subscribe" '("BeforeClose"))))
(define (update event) (when (equal? event "BeforeClose") (yinz-command (hash "SetSelectable" #f))) #f)
(define (render rows columns) "idle original ready for reload")
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("idle original ready for reload");
    std::fs::write(
        &harness.path,
        r#"
(define (load configuration) (yinz-command (hash "SetSelectable" #t)))
(define (render rows columns) "idle replacement loaded")
"#,
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
            &harness.path,
        )))
        .unwrap();
    harness.rendered("idle replacement loaded");
    let log = harness.finish();
    let selectable: Vec<bool> = log
        .iter()
        .filter_map(|i| match i {
            ScreenInstruction::SetSelectable(_, value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(selectable, [false, true],
        "the idle original's BeforeClose must retain host access and run exactly once, before replacement load");
    assert!(
        !log.iter().any(
            |i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(id, stage)
        if *id == PLUGIN && stage.is_error())
        ),
        "reloading an idle plugin must not crash it"
    );
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1
    );
}

#[test]
#[serial(plugin_tests)]
fn steel_reload_at_a_host_call_boundary_cleans_up_once_with_queued_work() {
    for queued in [false, true] {
        let harness = Harness::new(
            "scm",
            r#"
(define ready #f)
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState"))
    (lambda (_) (yinz-command (hash "Subscribe" '("BeforeClose" "Key"))) (set! ready #t))))
(define (update event)
  (cond [(equal? event "BeforeClose") (yinz-command (hash "SetSelectable" #f))]
        [(and (hash? event) (hash-contains? event "Key")) (yinz-command (hash "DumpSessionLayout" (hash)))]) #f)
(define (render rows columns) (if ready "boundary original ready" "waiting for permission"))
"#,
            PermissionStatus::Granted,
        );
        harness.load();
        harness.rendered("boundary original ready");
        harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
        let reply =
            harness
                .log
                .wait_until("guest paused in a host call", default_timeout(), |log| {
                    log.iter().find_map(|instruction| match instruction {
                        ScreenInstruction::DumpLayoutToPlugin {
                            response_channel, ..
                        } => Some(response_channel.clone()),
                        _ => None,
                    })
                });
        std::fs::write(
            &harness.path,
            r#"
(define (load configuration) (yinz-command (hash "SetSelectable" #t)))
(define (render rows columns) "boundary replacement ready")
"#,
        )
        .unwrap();
        if queued {
            harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('b'))));
        }
        harness
            .sender
            .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
                &harness.path,
            )))
            .unwrap();
        harness.log.wait_until("reload received while guest is in its call", default_timeout(), |log| {
            log.iter().any(|instruction| matches!(instruction,
                ScreenInstruction::NotificationPolicyUnavailable { reason, .. } if reason == "policy_reloaded"
            )).then_some(())
        });
        reply
            .send(yinz_bus::plugins::DumpSessionLayoutResponse {
                layout_result: Ok("layout {}".into()),
                metadata: None,
            })
            .unwrap();
        harness.rendered("boundary replacement ready");
        let log = harness.finish();
        assert_eq!(
            log.iter()
                .filter(|instruction| matches!(
                    instruction,
                    ScreenInstruction::DumpLayoutToPlugin { .. }
                ))
                .count(),
            1,
            "queued={queued}: callbacks retired at the reload boundary must not enter the host"
        );
        let selectable: Vec<_> = log
            .iter()
            .filter_map(|instruction| match instruction {
                ScreenInstruction::SetSelectable(_, value) => Some(*value),
                _ => None,
            })
            .collect();
        assert_eq!(
            selectable,
            [false, true],
            "queued={queued}: cleanup precedes replacement exactly once"
        );
    }
}

#[test]
#[serial(plugin_tests)]
fn steel_unload_during_reload_cleanup_does_not_load_the_replacement() {
    let harness = Harness::new(
        "scm",
        r#"
(define ready #f)
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState"))
    (lambda (_) (yinz-command (hash "Subscribe" '("BeforeClose"))) (set! ready #t))))
(define (update event)
  (when (equal? event "BeforeClose")
    (yinz-command (hash "SetSelectable" #f)) (yinz-command (hash "DumpSessionLayout" (hash)))) #f)
(define (render rows columns) (if ready "ready for cleanup unload" "starting"))
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("ready for cleanup unload");
    std::fs::write(
        &harness.path,
        "(define (load configuration) (yinz-command (hash \"SetSelectable\" #t)))",
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
            &harness.path,
        )))
        .unwrap();
    let reply = harness.log.wait_until(
        "reload cleanup is in a host call",
        default_timeout(),
        |log| {
            log.iter().find_map(|instruction| match instruction {
                ScreenInstruction::DumpLayoutToPlugin {
                    response_channel, ..
                } => Some(response_channel.clone()),
                _ => None,
            })
        },
    );
    harness
        .sender
        .send(PluginInstruction::Unload(PLUGIN))
        .unwrap();
    harness.log.wait_until(
        "explicit unload removed the old identity",
        default_timeout(),
        |log| {
            log.iter()
                .any(|instruction| {
                    matches!(instruction,
                        ScreenInstruction::ClearKeyPressesIntercepts(id, _) if *id == PLUGIN
                    )
                })
                .then_some(())
        },
    );
    reply
        .send(yinz_bus::plugins::DumpSessionLayoutResponse {
            layout_result: Ok("layout {}".into()),
            metadata: None,
        })
        .unwrap();
    let log = harness.finish();
    let selectable: Vec<_> = log
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::SetSelectable(_, value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(
        selectable,
        [false],
        "cleanup runs once and explicit unload wins over replacement"
    );
}

#[test]
#[serial(plugin_tests)]
fn steel_unload_interrupts_an_infinite_update_without_host_calls() {
    unload_runaway(
        r#"
(define (load configuration) (yinz-command (hash "Subscribe" '("Key"))))
(define (update event) (yinz-command (hash "SetSelectable" #f)) (let loop () (loop)))
(define (render rows columns) "ready to loop")
"#,
        true,
    );
}

#[test]
#[serial(plugin_tests)]
fn steel_unload_interrupts_an_infinite_load_without_host_calls() {
    unload_runaway(
        r#"
(define (load configuration) (yinz-command (hash "SetSelectable" #f)) (let loop () (loop)))
"#,
        false,
    );
}

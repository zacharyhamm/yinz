use super::script_harness::Harness;
use super::*;

#[test]
#[serial(plugin_tests)]
fn steel_file_loads_dispatches_events_and_reloads() {
    let harness = Harness::new(
        "scm",
        r#"
        (define text "initial")
        (define (load configuration)
          (yinz-command (hash "Subscribe" (list "CustomMessage")))
          (set! text (hash-ref configuration "greeting")))
        (define (update event)
          (set! text (car (hash-ref event "CustomMessage")))
          #t)
        (define (render rows columns) text)
        "#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("configured");
    harness.event(Event::CustomMessage(
        "Steel event arrived".into(),
        String::new(),
    ));
    harness.rendered("Steel event arrived");
    std::fs::write(
        &harness.path,
        "(define (render rows columns) \"Steel replacement ready\")",
    )
    .unwrap();
    harness.reload();
    harness.rendered("Steel replacement ready");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_reload_interrupts_a_loop_without_host_calls() {
    let harness = Harness::new(
        "scm",
        r#"
        (define (load configuration)
          (yinz-command (hash "Subscribe" (list "CustomMessage"))))
        (define (update event)
          (yinz-command (hash "SetSelectable" #f))
          (let loop () (loop)))
        (define (render rows columns) "Steel ready to loop")
        (define (unload) (yinz-command (hash "SetSelectable" #t)))
        "#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("Steel ready to loop");
    harness.event(Event::CustomMessage("loop".into(), String::new()));
    harness.log.wait_until(
        "Steel callback entered the loop",
        default_timeout(),
        |log| {
            log.iter()
                .any(|instruction| {
                    matches!(instruction, ScreenInstruction::SetSelectable(_, false))
                })
                .then_some(())
        },
    );
    std::fs::write(
        &harness.path,
        "(define (render rows columns) \"Steel recovered from loop\")",
    )
    .unwrap();
    harness.reload();
    harness.rendered("Steel recovered from loop");
    let log = harness.finish();
    assert!(
        !log.iter()
            .any(|instruction| matches!(instruction, ScreenInstruction::SetSelectable(_, true))),
        "an interrupted instance must not run cleanup code"
    );
}

#[test]
#[serial(plugin_tests)]
fn steel_workers_keep_fifo_state_and_reload_their_confined_modules() {
    let harness = Harness::new(
        "scm",
        r#"
        (require "./label.scm")
        (define count 0)
        (define text "")
        (yinz-register-workers
          (hash "probe"
            (lambda (name payload)
              (when (equal? name "error") (error "expected worker error"))
              (yinz-then (yinz-sleep 0)
                (lambda (ignored)
                  (set! count (+ count 1))
                  (yinz-post-plugin "reply"
                    (string-append label payload (number->string count))))))))
        (define (load configuration)
          (yinz-command (hash "Subscribe" (list "CustomMessage")))
          (yinz-post-worker "probe" "job" "first")
          (yinz-post-worker "probe" "error" "")
          (yinz-post-worker "probe" "job" "second"))
        (define (update event)
          (set! text (string-append text (cadr (hash-ref event "CustomMessage")) " "))
          #t)
        (define (render rows columns)
          (string-append (number->string count) ":" text))
        "#,
        PermissionStatus::Granted,
    );
    let module = harness.path.parent().unwrap().join("label.scm");
    std::fs::write(&module, "(provide label) (define label \"old-\")").unwrap();
    harness.load();
    // The worker retains its counter, while the parent evaluates a separate module instance.
    harness.rendered("0:old-first1 old-second2");
    std::fs::write(&module, "(provide label) (define label \"new-\")").unwrap();
    harness.reload();
    harness.rendered("0:new-first1 new-second2");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_reload_interrupts_and_joins_a_looping_worker() {
    let harness = Harness::new(
        "scm",
        r#"
        (yinz-register-workers
          (hash "spin"
            (lambda (name payload)
              (yinz-command (hash "SetSelectable" #f))
              (let loop () (loop)))))
        (define (load configuration)
          (yinz-command (hash "Subscribe" (list "CustomMessage"))))
        (define (update event) (yinz-post-worker "spin" "start" "") #f)
        (define (render rows columns) "Steel worker ready")
        "#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("Steel worker ready");
    harness.event(Event::CustomMessage("start".into(), String::new()));
    harness
        .log
        .wait_until("Steel worker entered the loop", default_timeout(), |log| {
            log.iter()
                .any(|instruction| {
                    matches!(instruction, ScreenInstruction::SetSelectable(_, false))
                })
                .then_some(())
        });
    std::fs::write(
        &harness.path,
        "(define (render rows columns) \"Steel worker replacement ready\")",
    )
    .unwrap();
    harness.reload();
    // Parent disposal joins every old worker before the replacement can render.
    harness.rendered("Steel worker replacement ready");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_shared_builtin_keeps_per_client_state_in_one_instance() {
    let harness = Harness::new(
        "scm",
        yinz_steel_builtins::builtin_steel_plugin("shared-test").unwrap(),
        PermissionStatus::Granted,
    );
    let first = ClientId::new(1);
    let second = ClientId::new(2);
    harness.load_shared();
    harness.rendered("your presses: 0");
    harness
        .sender
        .send(PluginInstruction::AddClient(second))
        .unwrap();
    for client in [first, second, second] {
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(yinz_types::internal_ids::PluginId::new(0)),
                Some(client),
                Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
            )]))
            .unwrap();
    }
    harness.log.wait_until(
        "shared Steel state renders independently for both clients",
        default_timeout(),
        |log| {
            [first, second]
                .into_iter()
                .all(|client| {
                    rendered_assets(log).iter().any(|(rendered_client, frame)| {
                        *rendered_client == client
                            && frame.contains(&format!("you are client {client}"))
                            && frame.contains(&format!(
                                "your presses: {}",
                                if client == first { 1 } else { 2 }
                            ))
                            && frame.contains("total: 3")
                            && frame.contains("plugin_id=0")
                    })
                })
                .then_some(())
        },
    );
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(yinz_types::internal_ids::PluginId::new(0)),
            Some(second),
            Event::Key(KeyWithModifier::new(BareKey::Char('m'))),
        )]))
        .unwrap();
    harness.server_log.wait_until(
        "the shared Steel mode action uses its acting client",
        default_timeout(),
        |log| {
            log.iter()
                .any(|instruction| {
                    matches!(
                        instruction, ServerInstruction::ChangeMode(client, InputMode::Locked, _)
                            if *client == second
                    )
                })
                .then_some(())
        },
    );
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_shared_deferred_callbacks_and_render_keep_their_acting_clients() {
    let harness = Harness::new(
        "scm",
        r#"
      (define loads 0)
      (define waiting '())
      (define completed '())
      (define (origin) (number->string (hash-ref (yinz-command "GetPluginIds") "client_id")))
      (define (load config)
        (set! loads (+ loads 1))
        (yinz-command (hash "Subscribe" (list "Key"))))
      (define (update event)
        (when (and (hash? event) (hash-contains? event "Key"))
          (define started (origin))
          (set! waiting (append waiting (list started)))
          (yinz-request-render)
          (yinz-then (yinz-wait-for-event "CustomMessage")
            (lambda (message)
              (define after-event (origin))
              (yinz-then (yinz-sleep 0)
                (lambda (ignored)
                  (set! completed (append completed (list (string-append started "->" after-event "->" (origin)))))
                  #t))))))
      (define (render rows columns)
        (define started (origin))
        (yinz-then (yinz-sleep 0)
          (lambda (ignored)
            (string-append "loads=" (number->string loads)
              " waiting=" (string-join waiting ",")
              " done=" (string-join (sort completed string<?) ",")
              " render=" started "->" (origin)))))
    "#,
        PermissionStatus::Granted,
    );
    let first = ClientId::new(1);
    let second = ClientId::new(2);
    harness.load_shared();
    harness.rendered("loads=1 waiting= done= render=1->1");
    harness
        .sender
        .send(PluginInstruction::AddClient(second))
        .unwrap();
    for client in [first, second] {
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(yinz_types::internal_ids::PluginId::new(0)),
                Some(client),
                Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
            )]))
            .unwrap();
    }
    harness.rendered("loads=1 waiting=1,2");
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(yinz_types::internal_ids::PluginId::new(0)),
            Some(second),
            Event::CustomMessage("release".into(), String::new()),
        )]))
        .unwrap();
    harness.log.wait_until(
        "both shared Steel deferred chains and renders preserve the actor",
        default_timeout(),
        |log| {
            [first, second]
                .into_iter()
                .all(|client| {
                    rendered_assets(log).iter().any(|(rendered_client, frame)| {
                        *rendered_client == client
                            && frame.contains("done=1->1->1,2->2->2")
                            && frame.contains(&format!("render={client}->{client}"))
                    })
                })
                .then_some(())
        },
    );
    let log = harness.finish();
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("loads=2")));
}

#[test]
#[serial(plugin_tests)]
fn steel_deferred_permission_grant_resumes_the_load_hook() {
    let harness = Harness::new(
        "scm",
        r#"
      (define text "waiting")
      (define (load config)
        (yinz-then (yinz-wait-for-permissions (list "ChangeApplicationState"))
          (lambda (ignored)
            (yinz-command (hash "SwitchToMode" "Locked"))
            (set! text "Steel permission continuation completed"))))
      (define (render rows columns) text)
    "#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("Steel permission continuation completed");
    harness.server_log.wait_until(
        "the granted Steel continuation changes its client mode",
        default_timeout(),
        |log| {
            log.iter()
                .any(|instruction| {
                    matches!(
                        instruction,
                        ServerInstruction::ChangeMode(client, InputMode::Locked, _)
                            if *client == ClientId::new(1)
                    )
                })
                .then_some(())
        },
    );
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_fixture_roundtrip_uses_real_data_filesystem_worker_and_event_services() {
    let harness = Harness::new(
        "scm",
        include_str!("../../../../yinz-steel-builtins/fixtures/fixture-plugin-for-tests.scm"),
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("Rows: 10, Cols: 80");
    harness.event(Event::Key(
        KeyWithModifier::new(BareKey::Char('k'))
            .with_ctrl_modifier()
            .with_shift_modifier(),
    ));
    harness.rendered("Data store: created true, rows [\"fixing a rebase conflict\", \"writing the changelog\"], search hits 1, count 2, watch true");
    // A second write must use the revisions it read, preserving the same two records.
    harness.event(Event::Key(
        KeyWithModifier::new(BareKey::Char('k'))
            .with_ctrl_modifier()
            .with_shift_modifier(),
    ));
    harness.rendered("Data store: created false, rows [\"fixing a rebase conflict\", \"writing the changelog\"], search hits 1, count 2, watch true");
    harness.event(Event::Key(
        KeyWithModifier::new(BareKey::Char('8')).with_ctrl_modifier(),
    ));
    harness.event(Event::CustomMessage(
        "fixture write completed".into(),
        String::new(),
    ));
    harness.rendered("fixture write completed");
    assert_eq!(
        std::fs::read(harness.path.parent().unwrap().join("hi-from-plugin.txt")).unwrap(),
        b"Hi there!",
    );
    harness.event(Event::SystemClipboardFailure);
    harness.rendered("Payload from worker: \"gimme_back_my_payload, received 1 messages\"");
    harness.event(Event::SystemClipboardFailure);
    harness.rendered("Payload from worker: \"gimme_back_my_payload, received 2 messages\"");
    harness.finish();
}

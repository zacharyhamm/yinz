//! Workers execute the same permission, FIFO and reload matrix as the retained runtime tests.
use super::script_harness::{release_event, Harness};
use super::*;
use yinz_types::internal_ids::PluginId;
const CLIENT: ClientId = ClientId::new(1);
const PLUGIN: PluginId = PluginId::new(0);

#[test]
fn steel_worker_permission_grant_retains_the_parents_run_commands_grant() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("permissions.scm");
    std::fs::write(
        &path,
        r#"
(define state "starting")
(yinz-register-workers (hash "change" (lambda (name payload)
  (yinz-then (yinz-wait-for-permissions '("ChangeApplicationState")) (lambda (_)
    (yinz-command (hash "RenameTabWithId" '(1 "worker changed tab")))
    (yinz-post-plugin "changed" ""))))))
(define (load config)
  (yinz-then (yinz-wait-for-permissions '("RunCommands")) (lambda (_)
    (define changed (yinz-wait-for-event "CustomMessage" (lambda (message) (equal? (car message) "changed"))))
    (yinz-post-worker "change" "go" "")
    (yinz-then changed (lambda (_)
      (yinz-command (hash "RunCommand" (list '("/usr/bin/true") (hash) "." (hash) json-null json-null)))
      (set! state "parent and worker commands dispatched"))))))
(define (render rows columns) state)
"#,
    )
    .unwrap();
    let (sender, jobs_receiver, receiver, teardown) =
        create_plugin_thread_with_background_jobs_receiver(Some(directory.path().to_owned()), None);
    let log = Watched::new(vec![]);
    let logger = permissions_and_log_until_disconnect(
        &log,
        receiver,
        directory.path().join("permissions.kdl"),
        sender.clone(),
        CLIENT,
        PermissionStatus::Granted,
    );
    let jobs = Watched::new(vec![]);
    let job_logger = log_until_disconnect(&jobs, jobs_receiver);
    sender.send(PluginInstruction::AddClient(CLIENT)).unwrap();
    let harness = Harness {
        sender,
        log,
        server_log: Watched::new(vec![]),
        server_logger: None,
        directory,
        path,
        teardown: Some(teardown),
        logger: Some(logger),
    };
    harness.load();
    harness.rendered("parent and worker commands dispatched");
    jobs.wait_until(
        "the parent retained RunCommands",
        default_timeout(),
        |jobs| {
            jobs.iter()
                .any(|job| matches!(job, BackgroundJob::RunCommand(..)))
                .then_some(())
        },
    );
    let log = harness.finish();
    assert!(log.iter().any(|instruction| matches!(instruction,
        ScreenInstruction::RenameTabWithId(_, name, _) if name == b"worker changed tab")));
    let requests: Vec<_> = log
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::RequestPluginPermissions(_, request) => {
                Some(request.permissions.clone())
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        requests,
        vec![
            vec![PermissionType::RunCommands],
            vec![
                PermissionType::RunCommands,
                PermissionType::ChangeApplicationState
            ],
        ]
    );
    job_logger.join().unwrap();
}

#[test]
fn steel_parent_and_two_workers_share_one_permission_fifo_and_keep_callback_clients() {
    for second_status in [PermissionStatus::Granted, PermissionStatus::Denied] {
        let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
        let to_proxy = SenderWithContext::new(to_proxy);
        let harness = Harness::with_event_sender(
            "scm",
            r#"
(define state "ready")
(define (request name permission)
  (define result (yinz-wait-for-permissions (list permission)))
  (when (equal? name "parent") (yinz-post-worker "first" "go" ""))
  (when (equal? name "first") (yinz-post-worker "second" "go" ""))
  (yinz-post-plugin "queued" name)
  (define (settled status)
    (yinz-post-plugin "settled" (string-append name ":" status ":"
      (number->string (hash-ref (yinz-command "GetPluginIds") "client_id")))))
  (yinz-catch (yinz-then result (lambda (_) (settled "granted")))
    (lambda (_) (settled "denied"))))
(yinz-register-workers (hash
  "first" (lambda (name payload) (request "first" "ChangeApplicationState"))
  "second" (lambda (name payload) (request "second" "ReadFiles"))))
(define (load config) (yinz-command (hash "Subscribe" '("Key" "CustomMessage"))))
(define (update event) (when (hash-contains? event "Key") (request "parent" "RunCommands")))
(define (render rows columns) state)
"#,
            None,
            Some(to_proxy.clone()),
        );
        let messages = Watched::new(Vec::<(String, String, ClientId)>::new());
        let load_forwarded = Watched::new(false);
        let proxy = std::thread::spawn({
            let messages = messages.clone();
            let load_forwarded = load_forwarded.clone();
            let sender = harness.sender.clone();
            move || loop {
                let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
                if matches!(instruction, PluginInstruction::Exit) {
                    break;
                }
                if let PluginInstruction::PluginReply(
                    _,
                    client,
                    _,
                    Event::CustomMessage(name, payload),
                    _,
                ) = &instruction
                {
                    messages
                        .lock()
                        .push((name.clone(), payload.clone(), *client));
                }
                let finishes_loading = matches!(
                    &instruction,
                    PluginInstruction::ApplyCachedEvents { plugin_ids, .. }
                        if plugin_ids.contains(&PLUGIN)
                );
                if sender.send(instruction).is_ok() && finishes_loading {
                    *load_forwarded.lock() = true;
                }
            }
        });
        harness.load_shared();
        harness.rendered("ready");
        // A ready render can precede startup replay; enqueue replay before client 2's input.
        load_forwarded.wait_until("startup replay forwarded", default_timeout(), |forwarded| {
            forwarded.then_some(())
        });
        let other = ClientId::new(2);
        harness
            .sender
            .send(PluginInstruction::AddClient(other))
            .unwrap();
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(other),
                Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
            )]))
            .unwrap();
        messages.wait_until(
            "all three requests are queued",
            default_timeout(),
            |messages| {
                (messages
                    .iter()
                    .filter(|(name, _, _)| name == "queued")
                    .count()
                    == 3)
                    .then_some(())
            },
        );
        let settled = |count| {
            messages.wait_until(
                "only the answered requester completes",
                default_timeout(),
                |messages| {
                    let settled: Vec<_> = messages
                        .iter()
                        .filter(|(name, _, _)| name == "settled")
                        .map(|(_, payload, client)| (payload.clone(), *client))
                        .collect();
                    (settled.len() >= count).then_some(settled)
                },
            )
        };
        assert_eq!(settled(0), vec![]);
        assert_eq!(
            harness.permission_requests(1),
            vec![vec![PermissionType::RunCommands]]
        );
        harness.answer_permissions(vec![PermissionType::RunCommands], PermissionStatus::Granted);
        assert_eq!(settled(1), vec![("parent:granted:2".into(), other)]);
        let requests = harness.permission_requests(2);
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1],
            vec![
                PermissionType::RunCommands,
                PermissionType::ChangeApplicationState
            ]
        );
        harness.answer_permissions(requests[1].clone(), second_status);
        let status = if second_status == PermissionStatus::Granted {
            "granted"
        } else {
            "denied"
        };
        assert_eq!(
            settled(2),
            vec![
                ("parent:granted:2".into(), other),
                (format!("first:{status}:2"), other)
            ]
        );
        let requests = harness.permission_requests(3);
        assert_eq!(requests.len(), 3);
        let mut expected = vec![PermissionType::RunCommands];
        if second_status == PermissionStatus::Granted {
            expected.push(PermissionType::ChangeApplicationState);
        }
        expected.push(PermissionType::ReadFiles);
        assert_eq!(requests[2], expected);
        harness.answer_permissions(requests[2].clone(), PermissionStatus::Granted);
        assert_eq!(
            settled(3),
            vec![
                ("parent:granted:2".into(), other),
                (format!("first:{status}:2"), other),
                ("second:granted:2".into(), other)
            ]
        );
        harness.finish();
        to_proxy.send(PluginInstruction::Exit).unwrap();
        proxy.join().unwrap();
    }
}

#[test]
fn steel_reload_drops_a_pending_workers_permission_completion() {
    let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_proxy = SenderWithContext::new(to_proxy);
    let source = r#"
(define label "original")
(yinz-register-workers (hash "ask" (lambda (name payload)
  (define answer (yinz-wait-for-permissions '("RunCommands")))
  (yinz-post-plugin "queued" label)
  (yinz-catch (yinz-then answer (lambda (_) (yinz-post-plugin "settled" (string-append label ":granted"))))
    (lambda (_) (yinz-post-plugin "settled" (string-append label ":denied")))))))
(define (load config) (yinz-post-worker "ask" "go" ""))
(define (update event)
  (when (hash-contains? event "PermissionRequestResult") (yinz-post-plugin "observed" label)))
(define (render rows columns) label)
"#;
    let harness = Harness::with_event_sender("scm", source, None, Some(to_proxy.clone()));
    let messages = Watched::new(vec![]);
    let proxy = std::thread::spawn({
        let messages = messages.clone();
        let sender = harness.sender.clone();
        move || loop {
            let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
            if matches!(instruction, PluginInstruction::Exit) {
                break;
            }
            if matches!(
                instruction,
                PluginInstruction::PluginReply(_, _, _, Event::CustomMessage(..), _)
            ) {
                messages.lock().push(instruction.clone());
            }
            let _ = sender.send(instruction);
        }
    });
    let message = |name: &str, payload: &str| {
        messages.wait_until("the worker's message", default_timeout(), |messages| {
        messages.iter().find(|instruction| matches!(instruction,
            PluginInstruction::PluginReply(_, _, _, Event::CustomMessage(n, p), _) if n == name && p == payload)).cloned()
    })
    };
    harness.load();
    harness.permission_requests(1);
    let PluginInstruction::PluginReply(plugin, client, old_generation, _, _) =
        message("queued", "original")
    else {
        unreachable!()
    };
    std::fs::write(
        &harness.path,
        source.replace("\"original\"", "\"replacement\""),
    )
    .unwrap();
    harness.reload();
    let requests = harness.permission_requests(2);
    assert_eq!(requests.len(), 2);
    let PluginInstruction::PluginReply(_, _, new_generation, _, _) =
        message("queued", "replacement")
    else {
        unreachable!()
    };
    assert_ne!(old_generation, new_generation);
    // A late reply addressed to the old parent and an ordinary broadcast cannot complete
    // the replacement's identically numbered request for the same permission.
    harness
        .sender
        .send(PluginInstruction::PluginEvent(
            plugin,
            client,
            old_generation,
            Event::PermissionRequestResult(PermissionStatus::Granted),
        ))
        .unwrap();
    harness.event(Event::PermissionRequestResult(PermissionStatus::Granted));
    harness.answer_permissions(requests[1].clone(), PermissionStatus::Denied);
    message("settled", "replacement:denied");
    assert_eq!(
        messages
            .lock()
            .iter()
            .filter_map(|instruction| match instruction {
                PluginInstruction::PluginReply(_, _, _, Event::CustomMessage(name, payload), _)
                    if name == "settled" =>
                    Some(payload.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec!["replacement:denied"]
    );
    harness.finish();
    to_proxy.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

#[test]
fn steel_workers_keep_isolated_state_order_async_timers_and_shared_permissions() {
    let harness = Harness::new(
        "scm",
        r#"
(define calls 0)
(define state "starting")
(define results '())
(define (work name payload)
  (cond
    ((equal? name "denied")
      (define refused (with-handler (lambda (_) #t)
        (yinz-command (hash "RenameTabWithId" '(1 "not allowed"))) #f))
      (if refused (yinz-post-plugin "denied" "") (error "worker bypassed permissions")))
    ((equal? name "error") (error "expected worker error"))
    (else (yinz-then (yinz-sleep 1) (lambda (_)
      (yinz-command (hash "RenameTabWithId" '(1 "worker granted")))
      (set! calls (+ calls 1))
      (yinz-post-plugin "done" (string-append name ":" (number->string calls) ":" payload)))))))
(yinz-register-workers (hash "first" work "second" work))
(define (load config)
  (set! calls 99)
  (yinz-command (hash "Subscribe" '("CustomMessage")))
  (define refused (with-handler (lambda (failure) (string-contains? (error-object-message failure) "not registered"))
    (yinz-post-worker "missing" "x" "") #f))
  (unless refused (error "missing worker was accepted"))
  (define denied (yinz-wait-for-event "CustomMessage" (lambda (message) (equal? (car message) "denied"))))
  (yinz-post-worker "first" "denied" "")
  (yinz-then (yinz-then denied (lambda (_) (yinz-wait-for-permissions '("ChangeApplicationState"))))
    (lambda (_)
      (yinz-on-event "CustomMessage" (lambda (message)
        (when (equal? (car message) "done") (set! results (append results (list (cadr message)))))
        (when (= (length results) 4)
          (define first (filter (lambda (result) (starts-with? result "first")) results))
          (unless (and (equal? first '("first:1:hé🙂" "first:2:two" "first:3:three"))
                       (member "second:1:other" results) (= calls 99)) (error "worker state or order"))
          (set! state "workers verified") #t)))
      (yinz-post-worker "first" "first" "hé🙂")
      (yinz-post-worker "second" "second" "other")
      (yinz-command (hash "PostMessageTo" (hash "worker_name" "first" "name" "first" "payload" "two")))
      (yinz-post-worker "first" "error" "")
      (yinz-post-worker "first" "first" "three"))))
(define (render rows columns) state)
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("workers verified");
}

#[test]
fn steel_busy_workers_leave_the_ui_responsive_and_stop_with_the_owner() {
    for cleanup in ["reload", "unload", "disconnect", "shutdown"] {
        let harness = Harness::new(
            "scm",
            r#"
(define state "starting")
(yinz-register-workers (hash "spin" (lambda (name payload)
  (yinz-post-plugin "started" "") (let loop () (loop)))))
(define (load config) (yinz-command (hash "Subscribe" '("CustomMessage"))) (yinz-post-worker "spin" "go" ""))
(define (update event)
  (when (hash-contains? event "CustomMessage")
    (define name (car (hash-ref event "CustomMessage")))
    (when (equal? name "started") (set! state "worker is busy"))
    (when (equal? name "acceptance-release") (set! state "UI remains responsive"))) #t)
(define (render rows columns) state)
"#,
            PermissionStatus::Granted,
        );
        harness.load();
        harness.rendered("worker is busy");
        harness.event(release_event());
        harness.rendered("UI remains responsive");
        if cleanup == "reload" {
            std::fs::write(
                &harness.path,
                "(define (render rows columns) \"worker replacement loaded\")",
            )
            .unwrap();
            harness.reload();
            harness.rendered("worker replacement loaded");
        } else if cleanup == "unload" {
            harness
                .sender
                .send(PluginInstruction::Unload(PLUGIN))
                .unwrap();
        } else if cleanup == "disconnect" {
            harness
                .sender
                .send(PluginInstruction::RemoveClient(CLIENT))
                .unwrap();
        }
        harness.finish();
    }
}

#[test]
fn steel_worker_list_clients_replies_are_generation_scoped_and_release_its_fifo() {
    let source = r#"
(define label "original")
(define state "starting")
(define parent-replies 0)
(yinz-register-workers (hash "clients" (lambda (name payload)
  (if (equal? name "query")
    (let ((reply (yinz-wait-for-event "ListClients")))
      (yinz-command "ListClients")
      (yinz-then reply (lambda (clients)
        (unless (and (= (length clients) 1) (= (hash-ref (car clients) "client_id") 1)
                     (hash-ref (car clients) "is_current_client")) (error "wrong clients reply"))
        (yinz-post-plugin "done" ""))))
    (yinz-post-plugin "after" "")))))
(define (load config)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState")) (lambda (_)
    (yinz-on-event "ListClients" (lambda (_) (set! parent-replies (+ parent-replies 1))))
    (yinz-command (hash "Subscribe" '("ListClients" "CustomMessage")))
    (yinz-on-event "CustomMessage" (lambda (message) (set! state (string-append state ":" (car message))) #t))
    (yinz-post-worker "clients" "query" "") (yinz-post-worker "clients" "after" ""))))
(define (render rows columns) (string-append label " " state " parent=" (number->string parent-replies)))
"#;
    let harness = Harness::new("scm", source, PermissionStatus::Granted);
    let request = |index| {
        harness
            .log
            .wait_until("worker ListClients request", default_timeout(), |log| {
                log.iter()
                    .filter_map(|instruction| match instruction {
                        ScreenInstruction::ListClientsToPlugin(plugin, client, generation) => {
                            Some((*plugin, *client, *generation))
                        },
                        _ => None,
                    })
                    .nth(index)
            })
    };
    harness.load();
    let old = request(0);
    std::fs::write(
        &harness.path,
        source.replace("\"original\"", "\"replacement\""),
    )
    .unwrap();
    harness.reload();
    let current = request(1);
    assert!(old.2.is_some());
    assert!(current.2.is_some());
    assert_ne!(old.2, current.2);
    // An empty stale reply would fail the worker's assertion if it reached the replacement.
    for (origin, metadata) in [
        (old, Default::default()),
        (current, list_clients_metadata()),
    ] {
        harness
            .sender
            .send(PluginInstruction::ListClientsToPlugin(
                metadata, origin.0, origin.1, origin.2,
            ))
            .unwrap();
    }
    harness.rendered("replacement starting:done:after parent=0");
}

fn list_clients_metadata() -> yinz_bus::session_layout_metadata::SessionLayoutMetadata {
    use yinz_bus::session_layout_metadata::{PaneLayoutMetadata, SessionLayoutMetadata};
    let mut metadata = SessionLayoutMetadata::default();
    metadata.add_tab(
        "clients".into(),
        true,
        true,
        false,
        vec![PaneLayoutMetadata::new(
            PaneId::Terminal(crate::TerminalId::new(7)),
            Default::default(),
            false,
            None,
            None,
            true,
            None,
            vec![CLIENT],
            None,
            None,
        )],
        vec![],
    );
    metadata
}

#[test]
fn steel_worker_pane_replies_cannot_reach_a_reloaded_parent_or_worker() {
    let source = r#"
(define label "original")
(define state "starting")
(define replies 0)
(define events '("CommandPaneOpened" "CommandPaneReRun" "CommandPaneExited" "EditPaneOpened" "EditPaneExited"))
(define (observe) (for-each (lambda (event) (yinz-on-event event (lambda (_) (set! replies (+ replies 1))))) events))
(yinz-register-workers (hash "runner" (lambda (name payload)
  (observe) (yinz-command (hash "Subscribe" events))
  (define waiting (yinz-run-in-pane 7 (hash "path" "test")))
  (yinz-post-plugin "pending" "")
  (yinz-then waiting (lambda (result)
    (yinz-post-plugin "done" (string-append (number->string (hash-ref result "exitCode")) ":" (number->string replies))))))))
(define (load config)
  (yinz-then (yinz-wait-for-permissions '("RunCommands" "ReadApplicationState")) (lambda (_)
    (observe) (yinz-command (hash "Subscribe" (append events '("CustomMessage"))))
    (yinz-on-event "CustomMessage" (lambda (message) (set! state (string-append (car message) ":" (cadr message))) #t))
    (yinz-post-worker "runner" "go" ""))))
(define (render rows columns) (string-append label " " state " parent=" (number->string replies)))
"#;
    let harness = Harness::new("scm", source, PermissionStatus::Granted);
    let origin = |index| {
        harness
            .log
            .wait_until("worker dispatched runInPane", default_timeout(), |log| {
                log.iter()
                    .filter_map(|instruction| match instruction {
                        ScreenInstruction::RunCommandInPane(_, command, _) => {
                            command.originating_plugin.clone()
                        },
                        _ => None,
                    })
                    .nth(index)
            })
    };
    harness.load();
    let old = origin(0);
    harness.rendered("original pending: parent=0");
    std::fs::write(
        &harness.path,
        source.replace("\"original\"", "\"replacement\""),
    )
    .unwrap();
    harness.reload();
    let current = origin(1);
    assert!(old.generation.is_some());
    assert!(current.generation.is_some());
    assert_ne!(old.generation, current.generation);
    harness.rendered("replacement pending: parent=0");
    let terminal = crate::TerminalId::new(7);
    for event in [
        Event::CommandPaneOpened(terminal, old.context.clone()),
        Event::CommandPaneReRun(terminal, old.context.clone()),
        Event::CommandPaneExited(terminal, Some(99), old.context.clone()),
        Event::EditPaneOpened(terminal, old.context.clone()),
        Event::EditPaneExited(terminal, Some(99), old.context.clone()),
    ] {
        harness
            .sender
            .send(PluginInstruction::for_plugin_event(
                old.plugin_id,
                old.client_id,
                old.generation,
                event,
            ))
            .unwrap();
    }
    for event in [
        Event::CommandPaneReRun(terminal, current.context.clone()),
        Event::CommandPaneExited(terminal, Some(0), current.context.clone()),
    ] {
        harness
            .sender
            .send(PluginInstruction::for_plugin_event(
                current.plugin_id,
                current.client_id,
                current.generation,
                event,
            ))
            .unwrap();
    }
    harness.rendered("replacement done:0:2 parent=0");
    harness.finish();
}

#[test]
fn steel_workers_preserve_shared_clients_and_discard_replies_after_reload() {
    let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_proxy = SenderWithContext::new(to_proxy);
    let harness = Harness::with_event_sender(
        "scm",
        r#"
(define state "worker ready")
(yinz-register-workers (hash "echo" (lambda (name payload)
  (define before (hash-ref (yinz-command "GetPluginIds") "client_id"))
  (yinz-then (yinz-sleep 1) (lambda (_)
    (yinz-post-plugin "echo" (string-append (number->string before) ":"
      (number->string (hash-ref (yinz-command "GetPluginIds") "client_id")))))))))
(define (load config) (yinz-command (hash "Subscribe" '("Key" "CustomMessage"))))
(define (update event)
  (when (hash-contains? event "Key") (yinz-post-worker "echo" "go" ""))
  (when (hash-contains? event "CustomMessage")
    (set! state (string-append "worker client " (cadr (hash-ref event "CustomMessage")) ":"
      (number->string (hash-ref (yinz-command "GetPluginIds") "client_id"))))) #t)
(define (render rows columns) state)
"#,
        None,
        Some(to_proxy.clone()),
    );
    let captured = Watched::new(vec![]);
    let proxy = std::thread::spawn({
        let captured = captured.clone();
        let sender = harness.sender.clone();
        move || loop {
            let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
            if matches!(instruction, PluginInstruction::Exit) {
                break;
            }
            if matches!(instruction, PluginInstruction::PluginReply(..)) {
                captured.lock().push(instruction.clone());
            }
            let _ = sender.send(instruction);
        }
    });
    harness.load_shared();
    harness.rendered("worker ready");
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
    harness.rendered("worker client 2:2:2");
    let stale = captured.lock().clone();
    assert!(stale.iter().any(|message| matches!(
        message,
        PluginInstruction::PluginReply(_, _, _, Event::Timer(_), _)
    )));
    assert!(stale.iter().any(|message| matches!(
        message,
        PluginInstruction::PluginReply(_, _, _, Event::CustomMessage(..), _)
    )));
    harness
        .sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    std::fs::write(
        &harness.path,
        r#"
(define replies 0) (define keys 0)
(define (load config) (yinz-command (hash "Subscribe" '("CustomMessage" "Key"))))
(define (update event)
  (when (hash-contains? event "CustomMessage") (set! replies (+ replies 1)))
  (when (hash-contains? event "Key") (set! keys (+ keys 1))) #t)
(define (render rows columns) (string-append "replacement replies=" (number->string replies) " keys=" (number->string keys)))
"#,
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginWithId(PLUGIN))
        .unwrap();
    harness.rendered("replacement replies=0 keys=0");
    for message in stale {
        harness.sender.send(message).unwrap();
    }
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.rendered("replacement replies=0 keys=1");
    harness.finish();
    to_proxy.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

#[test]
fn steel_worker_host_folder_replies_release_fifo_and_stay_local_across_reload() {
    let source = r#"
(define label "original")
(define state "")
(define parent-replies 0)
(define (read-marker)
  (yinz-decode-utf8 (yinz-command (hash "FsRead" (hash "path" "/host/marker" "max_bytes" json-null "offset" json-null)))))
(yinz-register-workers (hash
  "move" (lambda (name path)
    (define (finish ignored) (yinz-post-plugin name (read-marker)))
    (if (or (equal? name "success") (equal? name "failure"))
      (let ((reply (yinz-wait-for-event (if (equal? name "success") "HostFolderChanged" "FailedToChangeHostFolder"))))
        (yinz-command (hash "ChangeHostFolder" path))
        (yinz-then reply (lambda (result)
          (unless (if (equal? name "success") (equal? result path) (string-contains? result "does not exist"))
            (error "wrong folder reply"))
          (finish result))))
      (finish json-null)))
  "sibling" (lambda (name payload) (yinz-post-plugin name (read-marker)))))
(define (load config)
  (yinz-then (yinz-wait-for-permissions '("ReadFiles" "FullHdAccess")) (lambda (_)
    (yinz-command (hash "Subscribe" '("CustomMessage" "HostFolderChanged" "FailedToChangeHostFolder")))
    (yinz-on-event "HostFolderChanged" (lambda (_) (set! parent-replies (+ parent-replies 1))))
    (yinz-on-event "FailedToChangeHostFolder" (lambda (_) (set! parent-replies (+ parent-replies 1))))
    (yinz-on-event "CustomMessage" (lambda (message)
      (define name (car message)) (define payload (cadr message))
      (cond ((equal? name "start")
             (yinz-post-worker "move" "success" payload)
             (yinz-post-worker "move" "failure" (string-append payload "/missing"))
             (yinz-post-worker "move" "after" ""))
            ((equal? name "probe") (yinz-post-worker "move" "checked" ""))
            (else
             (unless (equal? (if (equal? name "sibling") "parent" "moved") payload) (error "wrong host root"))
             (set! state (string-append state name ":" payload " "))
             (when (or (equal? name "after") (equal? name "checked")) (yinz-post-worker "sibling" "sibling" "")))) #t))
    (set! state "ready "))))
(define (render rows columns)
  (if (equal? state "") "starting"
    (string-append label " " state "parent=" (read-marker) " replies=" (number->string parent-replies))))
"#;
    let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_proxy = SenderWithContext::new(to_proxy);
    let harness = Harness::with_event_sender(
        "scm",
        source,
        Some(PermissionStatus::Granted),
        Some(to_proxy.clone()),
    );
    std::fs::write(harness.directory.path().join("marker"), "parent").unwrap();
    let moved = tempdir().unwrap();
    std::fs::write(moved.path().join("marker"), "moved").unwrap();
    let stale = tempdir().unwrap();
    std::fs::write(stale.path().join("marker"), "stale").unwrap();
    let captured = Watched::new(vec![]);
    let proxy = std::thread::spawn({
        let captured = captured.clone();
        let sender = harness.sender.clone();
        move || loop {
            let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
            if matches!(instruction, PluginInstruction::Exit) {
                break;
            }
            if matches!(
                instruction,
                PluginInstruction::ChangePluginHostDir(..)
                    | PluginInstruction::PluginReply(
                        _,
                        _,
                        _,
                        Event::HostFolderChanged(_) | Event::FailedToChangeHostFolder(_),
                        _
                    )
            ) {
                captured.lock().push(instruction.clone());
            }
            let _ = sender.send(instruction);
        }
    });
    let exercise = |label| {
        harness.rendered(&format!("{label} ready parent=parent replies=0"));
        harness.event(Event::CustomMessage(
            "start".into(),
            moved.path().display().to_string(),
        ));
        harness.rendered(&format!(
            "{label} ready success:moved failure:moved after:moved sibling:parent parent=parent replies=0"
        ));
    };
    harness.load();
    exercise("original");
    let old = captured.lock().clone();
    assert_eq!(
        old.len(),
        4,
        "two requests and their scoped replies: {old:?}"
    );
    let PluginInstruction::ChangePluginHostDir(_, _, _, Some(old_generation)) = old[0] else {
        panic!("worker request must carry a generation: {old:?}");
    };
    std::fs::write(
        &harness.path,
        source.replace("\"original\"", "\"replacement\""),
    )
    .unwrap();
    harness.reload();
    exercise("replacement");
    assert!(
        matches!(captured.lock()[4], PluginInstruction::ChangePluginHostDir(_, _, _, Some(generation)) if generation != old_generation)
    );
    // Replay real old requests and replies, with a root that would expose accidental delivery.
    for mut instruction in old {
        match &mut instruction {
            PluginInstruction::ChangePluginHostDir(path, _, _, _) if path == moved.path() => {
                *path = stale.path().to_path_buf();
            },
            PluginInstruction::PluginReply(_, _, _, Event::HostFolderChanged(path), _) => {
                *path = stale.path().to_path_buf();
            },
            _ => {},
        }
        harness.sender.send(instruction).unwrap();
    }
    harness.event(Event::CustomMessage("probe".into(), String::new()));
    harness.rendered("checked:moved sibling:parent parent=parent replies=0");
    harness.finish();
    to_proxy.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

#[test]
fn steel_failed_worker_removes_only_its_pane_output_registrations() {
    let harness = Harness::new(
        "scm",
        r#"
(yinz-register-workers (hash
  "failed" (lambda (name payload)
    (yinz-command (hash "Subscribe" '("CustomMessage")))
    (yinz-on-event "CustomMessage" (lambda (_) (error "expected worker runtime failure")))
    (yinz-command (hash "SubscribePaneOutput" (hash "work" "failed"))))
  "survivor" (lambda (name payload)
    (yinz-command (hash "SubscribePaneOutput" (hash "work" "survivor"))))))
(define (load config)
  (yinz-then (yinz-wait-for-permissions '("ReadPaneContents")) (lambda (_)
    (yinz-command (hash "SubscribePaneOutput" (hash "work" "parent")))
    (yinz-post-worker "failed" "go" "")
    (yinz-post-worker "survivor" "go" ""))))
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    let generations = harness.log.wait_until(
        "parent and worker registrations",
        default_timeout(),
        |log| {
            let generations: BTreeMap<_, _> = log
                .iter()
                .filter_map(|instruction| match instruction {
                    ScreenInstruction::SubscribePaneOutput(
                        plugin,
                        client,
                        Some(context),
                        Some(generation),
                    ) => {
                        assert_eq!((*plugin, *client), (PLUGIN, CLIENT));
                        Some((context["work"].clone(), *generation))
                    },
                    _ => None,
                })
                .collect();
            (generations.len() == 3).then_some(generations)
        },
    );
    harness
        .sender
        .send(PluginInstruction::PluginEvent(
            PLUGIN,
            CLIENT,
            generations["failed"],
            release_event(),
        ))
        .unwrap();
    harness
        .log
        .wait_until("failed worker disposal", default_timeout(), |log| {
            log.iter()
                .any(|instruction| {
                    matches!(instruction,
                        ScreenInstruction::UnsubscribePaneOutput(plugin, Some(generation))
                            if *plugin == PLUGIN && *generation == generations["failed"]
                    )
                })
                .then_some(())
        });
    assert_eq!(
        generations
            .values()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    for instruction in harness.log.lock().iter() {
        if let ScreenInstruction::UnsubscribePaneOutput(plugin, generation) = instruction {
            assert_eq!(
                (*plugin, *generation),
                (PLUGIN, Some(generations["failed"]))
            );
        }
    }
    let log = harness.finish();
    for generation in generations.values() {
        assert!(log.iter().any(|instruction| matches!(instruction,
            ScreenInstruction::UnsubscribePaneOutput(plugin, Some(removed))
                if *plugin == PLUGIN && removed == generation
        )));
    }
    assert!(log.iter().any(|instruction| matches!(instruction,
        ScreenInstruction::UnsubscribePaneOutput(plugin, None) if *plugin == PLUGIN
    )));
}

#[test]
fn steel_worker_binary_events_preserve_bytes_and_release_the_job_fifo() {
    let harness = Harness::new(
        "scm",
        r#"
(define state "starting")
(yinz-register-workers (hash "binary" (lambda (name payload)
  (if (equal? name "next")
    (yinz-post-plugin "fifo" "")
    (let ((reply (yinz-wait-for-event "RunCommandResult")))
      (yinz-command "ListClients")
      (yinz-then reply (lambda (result)
        (define stdout (cadr result))
        (define stderr (caddr result))
        (unless (and (= (car result) 7)
                     (bytevector? stdout) (= (bytes-length stdout) 3)
                     (= (bytes-ref stdout 0) 0) (= (bytes-ref stdout 1) 255) (= (bytes-ref stdout 2) 128)
                     (bytevector? stderr) (= (bytes-length stderr) 3)
                     (= (bytes-ref stderr 0) 226) (= (bytes-ref stderr 2) 166)
                     (equal? (hash-ref (list-ref result 3) "origin") "worker"))
          (error "worker binary event changed bytes or metadata"))
        (yinz-post-plugin "decoded" ""))))))))
(define (load config)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState" "RunCommands")) (lambda (_)
    (yinz-on-event "CustomMessage" (lambda (message)
      (set! state (string-append state " " (car message))) #t))
    (yinz-post-worker "binary" "wait" "")
    (yinz-post-worker "binary" "next" ""))))
(define (render rows columns) state)
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    let (plugin, client, generation) = harness.log.wait_until(
        "worker subscribed before exposing its reply route",
        default_timeout(),
        |log| {
            log.iter().find_map(|instruction| match instruction {
                ScreenInstruction::ListClientsToPlugin(plugin, client, Some(generation)) => {
                    Some((*plugin, *client, *generation))
                },
                _ => None,
            })
        },
    );
    harness
        .sender
        .send(PluginInstruction::PluginEvent(
            plugin,
            client,
            generation,
            Event::RunCommandResult(
                Some(7),
                vec![0, 255, 128],
                vec![226, 128, 166],
                BTreeMap::from([("origin".into(), "worker".into())]),
            ),
        ))
        .unwrap();
    harness.rendered("starting decoded fifo");
    harness.finish();
}

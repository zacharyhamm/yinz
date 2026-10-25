//! Deferred callbacks, permissions and replacement identity through real Scheme guests.
use super::script_harness::{load, release_event, Harness};
use super::*;
use yinz_types::internal_ids::{PluginId, TabId};
const CLIENT: ClientId = ClientId::new(1);
const PLUGIN: PluginId = PluginId::new(0);
const SIZE: Size = Size { cols: 80, rows: 10 };

#[test]
#[serial(plugin_tests)]
fn steel_scheme_awaits_an_event_and_timer_then_renders_asynchronously_after_resize() {
    let harness = Harness::new(
        "scm",
        r#"
(yinz-declare-pane)
(define state "starting")
(define (load config)
  (define key (yinz-wait-for-event "Key"))
  (set! state (string-append (hash-ref config "greeting") " awaiting key"))
  (yinz-request-render)
  (yinz-then (yinz-then key (lambda (event) (yinz-sleep 0)))
    (lambda (ignored) (set! state "key and timer completed"))))
(define (render rows columns)
  (yinz-then (yinz-sleep 0)
    (lambda (ignored) (string-append state " " (number->string rows) "x" (number->string columns)))))
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("configured awaiting key 10x80");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.rendered("key and timer completed 10x80");
    harness
        .sender
        .send(PluginInstruction::Resize(PLUGIN, 113, 27))
        .unwrap();
    harness.rendered("key and timer completed 27x113");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_resize_discards_an_in_flight_async_render_with_the_old_dimensions() {
    let harness = Harness::new(
        "scm",
        r#"
(yinz-declare-pane)
(define release #f)
(define (load config) (yinz-command (hash "Subscribe" (list "Key"))))
(define (update event)
  (yinz-then (yinz-sleep 0)
    (lambda (ignored) (when release (yinz-resolve! release #t)) #t)))
(define (render rows columns)
  (define ready
    (if (= columns 80)
        (begin (set! release (yinz-deferred))
               (yinz-command (hash "SetSelectable" #f)) release)
        #t))
  (yinz-then ready
    (lambda (ignored) (string-append "finished async render " (number->string rows) "x" (number->string columns)))))
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.log.wait_until(
        "the original render is awaiting a key",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    harness
        .sender
        .send(PluginInstruction::Resize(PLUGIN, 113, 27))
        .unwrap();
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.rendered("finished async render 27x113");
    let log = harness.finish();
    assert!(
        !rendered_assets(&log)
            .iter()
            .any(|(_, frame)| frame.contains("finished async render 10x80")),
        "the suspended render must not publish its obsolete dimensions"
    );
}

#[test]
#[serial(plugin_tests)]
fn steel_shared_promises_keep_their_origin_after_another_clients_event_and_timer() {
    let harness = Harness::new(
        "scm",
        r#"
(yinz-declare-pane)
(define (origin) (hash-ref (yinz-command "GetPluginIds") "client_id"))
(define waiting '())
(define completed '())
(define loads 0)
(define (load config)
  (set! loads (+ loads 1))
  (yinz-command (hash "Subscribe" (list "Key"))))
(define (update event)
  (if (and (hash? event) (hash-contains? event "Key"))
    (let* ((started (origin))
           (release (yinz-wait-for-event "CustomMessage" (lambda (message) (equal? (car message) "acceptance-release")))))
      (set! waiting (append waiting (list (number->string started))))
      (yinz-request-render)
      (yinz-then release
        (lambda (ignored)
          (define after-event (origin))
          (yinz-then (yinz-sleep 0)
            (lambda (ignored)
              (set! completed (cons (string-append (number->string started) "->" (number->string after-event) "->" (number->string (origin))) completed))
              #t)))))
    #f))
(define (render rows cols)
  (define started (origin))
  (yinz-then (yinz-sleep 0)
    (lambda (ignored)
      (string-append "loads=" (number->string loads) " waiting=" (string-join waiting ",")
        " done=" (string-join (sort completed string<?) ",") " render="
        (number->string started) "->" (number->string (origin))))))
"#,
        PermissionStatus::Granted,
    );
    harness.load_shared();
    harness.rendered("loads=1 waiting= done= render=1->1");
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    for client in [CLIENT, other] {
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(client),
                Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
            )]))
            .unwrap();
    }
    harness.rendered("loads=1 waiting=1,2");
    // Both tasks resume during client 2's event; each must retain its own origin.
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            Some(other),
            release_event(),
        )]))
        .unwrap();
    harness.log.wait_until_described(
        "both clients' async callbacks and frames retain their origin",
        default_timeout(),
        |log| {
            [CLIENT, other]
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
        |log| describe_renders(log),
    );
    let log = harness.finish();
    assert!(
        !rendered_assets(&log)
            .iter()
            .any(|(_, frame)| frame.contains("loads=2")),
        "a shared plugin must keep one instance"
    );
}

#[test]
#[serial(plugin_tests)]
fn steel_shared_permission_wait_survives_an_attach_and_replays_the_queued_event() {
    let harness = Harness::with_permission_reply(
        "scm",
        r#"
(yinz-declare-pane)
(define loads 0)
(define keys 0)
(define state "starting")
(define (load config)
  (set! loads (+ loads 1))
  (yinz-command (hash "Subscribe" (list "Key")))
  (yinz-then (yinz-wait-for-permissions (list "ChangeApplicationState"))
    (lambda (ignored) (set! state "granted"))))
(define (update event)
  (if (and (hash? event) (hash-contains? event "Key"))
    (begin (set! keys (+ keys 1)) #t) #f))
(define (render rows cols)
  (string-append "loads=" (number->string loads) " " state " keys=" (number->string keys)))
"#,
        None,
    );
    harness.load_shared();
    harness.log.wait_until(
        "the shared plugin is waiting on the permission prompt",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::RequestPluginPermissions(..)))
                .then_some(())
        },
    );
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
    harness
        .sender
        .send(PluginInstruction::PermissionAnswer {
            request_id: harness
                .log
                .lock()
                .iter()
                .rev()
                .find_map(|instruction| match instruction {
                    ScreenInstruction::RequestPluginPermissions(_, request) => {
                        Some(request.request_id)
                    },
                    _ => None,
                })
                .unwrap(),
            status: PermissionStatus::Granted,
            cache_path: Some(harness.directory.path().join("permissions.kdl")),
        })
        .unwrap();
    harness.log.wait_until_described(
        "the attached clients see the single granted instance's queued key",
        default_timeout(),
        |log| {
            [CLIENT, other]
                .into_iter()
                .all(|client| {
                    rendered_assets(log).iter().any(|(rendered_client, frame)| {
                        *rendered_client == client && frame.contains("loads=1 granted keys=1")
                    })
                })
                .then_some(())
        },
        |log| describe_renders(log),
    );
    let log = harness.finish();
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::RequestPluginPermissions(..)))
            .count(),
        1,
        "attaching must reuse the shared instance's permission wait"
    );
}

#[test]
#[serial(plugin_tests)]
fn steel_reload_rejects_an_old_host_timer_completion_but_accepts_the_current_one() {
    let source = r#"
(yinz-declare-pane)
(define label "original")
(define state "pending")
(define timers 0)
(define fences 0)
(define (load config)
  (yinz-command (hash "Subscribe" (list "CustomMessage")))
  (yinz-then (yinz-sleep 0) (lambda (ignored) (set! state "completed"))))
(define (update event)
  (cond
    ((and (hash? event) (hash-contains? event "Timer")) (set! timers (+ timers 1)) #t)
    ((and (hash? event) (hash-contains? event "CustomMessage")
          (equal? (car (hash-ref event "CustomMessage")) "acceptance-release"))
     (set! fences (+ fences 1)) #t)
    (else #f)))
(define (render rows cols)
  (string-append label " " state " timers=" (number->string timers) " fences=" (number->string fences)))
"#;
    let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_proxy = SenderWithContext::new(to_proxy);
    let harness = Harness::with_event_sender("scm", source, None, Some(to_proxy.clone()));
    let completions = Watched::new(Vec::<PluginInstruction>::new());
    let proxy = std::thread::spawn({
        let completions = completions.clone();
        let sender = harness.sender.clone();
        move || loop {
            let (instruction, _) = outgoing
                .recv_timeout(FAKE_SCREEN_RECV_TIMEOUT)
                .expect("the timer proxy must be stopped after teardown");
            match instruction {
                PluginInstruction::Exit => break,
                timer @ PluginInstruction::PluginReply(_, _, _, Event::Timer(_), _) => {
                    completions.lock().push(timer);
                },
                // Capture the legacy envelope too, so losing the generation at the
                // SetTimeout producer fails explicitly instead of accidentally delivering it.
                PluginInstruction::Update(ref events)
                    if events
                        .iter()
                        .any(|(_, _, event)| matches!(event, Event::Timer(_))) =>
                {
                    completions.lock().push(instruction);
                },
                other => {
                    let _ = sender.send(other);
                },
            }
        }
    });
    harness.load();
    let old = completions.wait_until(
        "the old SDK sleep produced a real host completion",
        default_timeout(),
        |events| events.first().cloned(),
    );
    harness.rendered("original pending timers=0 fences=0");
    std::fs::write(
        &harness.path,
        source.replace("\"original\"", "\"replacement\""),
    )
    .unwrap();
    harness.reload();
    let current = completions.wait_until(
        "the replacement SDK sleep produced its own host completion",
        default_timeout(),
        |events| events.get(1).cloned(),
    );
    harness.rendered("replacement pending timers=0 fences=0");
    match (&old, &current) {
        (
            PluginInstruction::PluginReply(old_id, old_client, old_generation, Event::Timer(_), _),
            PluginInstruction::PluginReply(new_id, new_client, new_generation, Event::Timer(_), _),
        ) => {
            assert_eq!((*old_id, *old_client), (PLUGIN, CLIENT));
            assert_eq!((*new_id, *new_client), (PLUGIN, CLIENT));
            assert_ne!(
                old_generation, new_generation,
                "reload must change the timer's generation"
            );
        },
        _ => panic!("host SetTimeout must tag its completions: old={old:?}, current={current:?}"),
    }
    // Both timers have already fired, but neither completion has been delivered.
    // Deliver the old one followed by a FIFO event the new lifecycle acknowledges.
    harness.sender.send(old).unwrap();
    harness.event(release_event());
    let frame = harness.rendered("fences=1");
    assert!(frame.contains("replacement pending timers=0 fences=1"),
        "an old host timer must neither reach update nor resolve the replacement's SDK sleep: {frame}");
    harness.sender.send(current).unwrap();
    harness.rendered("replacement completed timers=1 fences=1");
    harness.finish();
    // Stop explicitly: event-loop wakers may retain the plugin-only proxy sender
    // after isolate disposal, so channel disconnect is not the proxy's lifetime.
    to_proxy.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn steel_reload_rereads_imports_and_cancels_the_old_event_await() {
    let harness = Harness::new(
        "scm",
        r#"
(require "./dependency.scm")
(yinz-declare-pane)
(define state "starting")
(define (load config)
  (define event (yinz-wait-for-event "CustomMessage" (lambda (message) (equal? (car message) "acceptance-release"))))
  (yinz-command (hash "Unsubscribe" (list "CustomMessage")))
  (set! state (string-append label " awaiting input"))
  (yinz-request-render)
  (yinz-then event
    (lambda (ignored)
      (yinz-command (hash "SetSelectable" (equal? label "second")))
      (set! state (string-append label " completed")))))
(define (render rows cols) state)
"#,
        PermissionStatus::Granted,
    );
    let dependency = harness.directory.path().join("dependency.scm");
    std::fs::write(&dependency, r#"(provide label) (define label "first")"#).unwrap();
    harness.load();
    harness.rendered("first awaiting input");
    std::fs::write(&dependency, r#"(provide label) (define label "second")"#).unwrap();
    harness.reload();
    harness.rendered("second awaiting input");
    harness.event(release_event());
    harness.rendered("second completed");
    let log = harness.finish();
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("first completed")));
    assert!(!log
        .iter()
        .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false))));
    assert!(log
        .iter()
        .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true))));
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1,
        "reload must retain the existing pane"
    );
}

#[test]
#[serial(plugin_tests)]
#[cfg(unix)]
fn repository_file_url_reload_follows_retargeted_symlink() {
    let harness = Harness::new("scm", "(void)", PermissionStatus::Granted);
    let first = harness.directory.path().join("v1");
    let second = harness.directory.path().join("v2");
    for (path, version) in [(&first, "one"), (&second, "two")] {
        std::fs::create_dir(path).unwrap();
        std::fs::write(
            path.join("main.scm"),
            format!(r#"(yinz-declare-pane) (define (render rows cols) "repository {version}")"#),
        )
        .unwrap();
    }
    let current = harness.directory.path().join("current # %20?");
    std::os::unix::fs::symlink(&first, &current).unwrap();
    let url = url::Url::from_file_path(&current).unwrap();
    let config = yinz_config::config::Config::from_kdl(
        &format!("plugins {{ current repo=\"{url}\" entrypoint=\"main.scm\"; }}"),
        None,
    )
    .unwrap();
    let serialized = yinz_config::kdl::plugin_aliases::to_kdl(&config.plugins, false);
    let restored = yinz_config::kdl::plugin_aliases::from_kdl(&serialized).unwrap();
    let plugin = &restored.aliases["current"];
    let RunPluginLocation::Git(repository) = &plugin.location else {
        panic!("repository plugin")
    };
    assert_eq!(PathBuf::from(&repository.repo), current);
    load(&harness.sender, RunPluginOrAlias::RunPlugin(plugin.clone()));
    harness.rendered("repository one");
    std::fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink(&second, &current).unwrap();
    std::fs::remove_dir_all(first).unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginWithId(PLUGIN))
        .unwrap();
    harness.rendered("repository two");
    let log = harness.finish();
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1
    );
    assert!(log.iter().any(|i| matches!(i, ScreenInstruction::PluginBytes(assets)
        if assets.iter().any(|asset| asset.plugin_id == PLUGIN && frame_of(&asset.bytes).contains("repository two")))));
}

#[test]
#[serial(plugin_tests)]
#[cfg(unix)]
fn steel_reload_follows_retargeted_symlinks_by_id_and_url() {
    for by_id in [true, false] {
        let harness = Harness::new(
            "scm",
            r#"(yinz-declare-pane) (define (render rows cols) "version one")"#,
            PermissionStatus::Granted,
        );
        let first = harness.directory.path().join("v1.scm");
        std::fs::rename(&harness.path, &first).unwrap();
        let current = harness.directory.path().join("current.scm");
        std::os::unix::fs::symlink(&first, &current).unwrap();
        let url = url::Url::from_file_path(&current).unwrap().to_string();
        let parse = || {
            RunPluginOrAlias::RunPlugin(
                yinz_runtime::plugin_location::run_plugin_from_url(&url).unwrap(),
            )
        };
        load(&harness.sender, parse());
        harness.rendered("version one");

        let second = harness.directory.path().join("v2.scm");
        std::fs::write(
            &second,
            r#"(yinz-declare-pane) (define (render rows cols) "version two")"#,
        )
        .unwrap();
        std::fs::remove_file(&current).unwrap();
        std::os::unix::fs::symlink(&second, &current).unwrap();
        harness
            .sender
            .send(if by_id {
                PluginInstruction::ReloadPluginWithId(PLUGIN)
            } else {
                PluginInstruction::ReloadPluginByUrl(parse())
            })
            .unwrap();
        harness.rendered("version two");
        harness.log.wait_until("replacement identity published", default_timeout(), |log| {
            log.iter().any(|i| matches!(i, ScreenInstruction::UpdatePluginLocation(id, RunPluginLocation::File(path))
                if *id == PLUGIN && path == &second.canonicalize().unwrap())).then_some(())
        });
        // Updating the canonical identity must not discard the declared symlink for later reloads.
        std::fs::write(
            &harness.path,
            r#"(yinz-declare-pane) (define (render rows cols) "version three")"#,
        )
        .unwrap();
        std::fs::remove_file(&current).unwrap();
        std::os::unix::fs::symlink(&harness.path, &current).unwrap();
        harness
            .sender
            .send(PluginInstruction::ReloadPluginWithId(PLUGIN))
            .unwrap();
        harness.rendered("version three");
        // A reload naming the old canonical target must now start a separate plugin.
        harness
            .sender
            .send(PluginInstruction::Reload(
                Some(false),
                None,
                RunPluginOrAlias::RunPlugin(
                    yinz_runtime::plugin_location::run_plugin_from_url(
                        url::Url::from_file_path(&first).unwrap().as_ref(),
                    )
                    .unwrap(),
                ),
                TabId::new(1),
                SIZE,
                None,
            ))
            .unwrap();
        harness
            .log
            .wait_until("old target gets a new pane", default_timeout(), |log| {
                (log.iter()
                    .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
                    .count()
                    == 2)
                    .then_some(())
            });
        let log = harness.finish();
        assert_eq!(
            log.iter()
                .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
                .count(),
            2,
            "only the old target launch creates a second pane"
        );
        assert!(log.iter().any(|i| matches!(i, ScreenInstruction::PluginBytes(assets)
            if assets.iter().any(|asset| asset.plugin_id == PLUGIN && frame_of(&asset.bytes).contains("version two")))),
            "reload must retain the original plugin ID");
    }
}

#[test]
#[serial(plugin_tests)]
#[cfg(unix)]
fn steel_failed_symlink_reload_does_not_publish_a_replacement_identity() {
    let harness = Harness::new(
        "scm",
        r#"(yinz-declare-pane) (define (render rows cols) "original ready")"#,
        PermissionStatus::Granted,
    );
    let current = harness.directory.path().join("current.scm");
    std::os::unix::fs::symlink(&harness.path, &current).unwrap();
    let url = url::Url::from_file_path(&current).unwrap().to_string();
    load(
        &harness.sender,
        RunPluginOrAlias::RunPlugin(
            yinz_runtime::plugin_location::run_plugin_from_url(&url).unwrap(),
        ),
    );
    harness.rendered("original ready");
    let replacement = harness.directory.path().join("broken.scm");
    std::fs::write(
        &replacement,
        r#"(yinz-declare-pane) (define (load config) (error "replacement failed"))"#,
    )
    .unwrap();
    std::fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink(&replacement, &current).unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginWithId(PLUGIN))
        .unwrap();
    harness.log.wait_until("replacement load failed", default_timeout(), |log| {
        log.iter().any(|i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(id, stage) if *id == PLUGIN && stage.is_error())).then_some(())
    });
    assert!(!harness
        .finish()
        .iter()
        .any(|i| matches!(i, ScreenInstruction::UpdatePluginLocation(..))));
}

const PERMISSIONS: &str = r#"
(yinz-declare-pane)
(define state "starting")
(define (load config)
  (yinz-command (hash "Subscribe" (list "PermissionRequestResult")))
  (with-handler (lambda (ignored) (void))
    (yinz-command (hash "RenameTabWithId" (list 1 "before-grant"))))
  (define answer (yinz-wait-for-event "PermissionRequestResult"))
  (define helper "resolved")
  (define granted
    (yinz-catch (yinz-wait-for-permissions (list "ChangeApplicationState"))
      (lambda (ignored) (set! helper "rejected"))))
  (yinz-then granted
    (lambda (ignored)
      (yinz-then answer
        (lambda (status)
          (define ids (yinz-command "GetPluginIds"))
          (define object-ids (yinz-command (hash "GetPluginIds" json-null)))
          (unless (equal? ids object-ids) (error "command forms disagree"))
          (define outcome
            (with-handler (lambda (ignored) "rejected")
              (yinz-command (hash "RenameTabWithId" (list 1 "after-grant"))) "returned"))
          (set! state (string-append (hash-ref config "greeting") " " status
            " plugin=" (number->string (hash-ref ids "plugin_id"))
            " client=" (number->string (hash-ref ids "client_id"))
            " " outcome " helper=" helper)))))))
(define (render rows cols) state)
"#;

fn permission_round_trip(status: PermissionStatus, expected: &str) -> Vec<String> {
    let harness = Harness::new("scm", PERMISSIONS, status);
    harness.load();
    let outcome = if status == PermissionStatus::Granted {
        "returned helper=resolved"
    } else {
        "rejected helper=rejected"
    };
    harness.rendered(&format!(
        "configured {expected} plugin=0 client=1 {outcome}"
    ));
    harness
        .finish()
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::RenameTabWithId(_, name, _) => {
                Some(String::from_utf8(name.clone()).unwrap())
            },
            _ => None,
        })
        .collect()
}
#[test]
#[serial(plugin_tests)]
fn steel_permission_grant_resumes_load_and_decodes_replies() {
    assert_eq!(
        permission_round_trip(PermissionStatus::Granted, "Granted"),
        ["after-grant"]
    );
}
#[test]
#[serial(plugin_tests)]
fn steel_permission_denial_never_routes_protected_commands() {
    assert!(permission_round_trip(PermissionStatus::Denied, "Denied").is_empty());
}

#[test]
#[serial(plugin_tests)]
fn steel_deferred_commands_reject_without_permissions() {
    let harness = Harness::new(
        "scm",
        r#"
(yinz-declare-pane)
(define state "starting")
(define (denied name operation)
  (yinz-catch
    (yinz-then operation (lambda (ignored) (error "operation unexpectedly succeeded")))
    (lambda (failure)
      (unless (or (string-contains? failure "permission") (string-contains? failure "requires"))
        (error failure))
      (string-append name " denied"))))
(define (load config)
  (define run (denied "run" (yinz-run-command
    (hash "command" (list "/usr/bin/true") "on-output" (lambda (chunk) (error "denied command emitted output"))))))
  (define web (denied "web" (yinz-web-request (hash "url" "https://example.com"))))
  (yinz-then run
    (lambda (first)
      (yinz-then web
        (lambda (second)
          (yinz-command (hash "Unsubscribe" (list "RunCommandResult" "WebRequestResult")))
          (set! state (string-append first "," second)))))))
(define (render rows cols) state)
"#,
        PermissionStatus::Denied,
    );
    harness.load();
    harness.rendered("run denied,web denied");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_permission_errors_distinguish_pending_missing_and_incomplete_grants() {
    for state in ["pending", "missing", "incomplete"] {
        let request = match state {
            "pending" => "(yinz-command (hash \"RequestPluginPermissions\" (list \"PluginData\"))) (check-data)",
            "incomplete" => "(yinz-then (yinz-wait-for-permissions (list \"ReadFiles\")) (lambda (ignored) (check-data)))",
            _ => "(check-data)",
        };
        let source = format!(
            r#"
(yinz-declare-pane)
(define state "loading")
(define (check-data)
  (set! state (string-append "checked: "
    (with-handler (lambda (failure) (error-object-message failure))
      (yinz-data-fetch json-null (hash "entity" "note" "filter" json-null "search" json-null "sort" '() "select" '() "limit" 100 "offset" 0 "after_id" json-null)) "fetched")))
  (yinz-request-render))
(define (load config) {request})
(define (render rows cols) state)
"#
        );
        let harness = Harness::with_permission_reply(
            "scm",
            &source,
            if state == "incomplete" {
                Some(PermissionStatus::Granted)
            } else {
                None
            },
        );
        harness.load();
        let frame = harness.rendered("checked:");
        assert!(frame.contains("requires 'PluginData'"), "{frame}");
        if state == "pending" {
            assert!(
                frame.contains("PermissionRequestResult") && frame.contains("wait"),
                "{frame}"
            );
        } else {
            assert!(frame.contains("RequestPluginPermissions"), "{frame}");
            assert!(!frame.contains("PermissionRequestResult"), "{frame}");
        }
        harness.finish();
    }
}
#[test]
#[serial(plugin_tests)]
fn steel_command_helper_completes_and_reload_rejects_the_old_host_reply() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("command.scm");
    let source = r#"
(yinz-declare-pane)
(define label "original")
(define state "pending")
(define replies 0)
(define chunks 0)
(define fences 0)
(define (load config)
  (yinz-command (hash "Subscribe" (list "CustomMessage")))
  (yinz-then (yinz-wait-for-permissions (list "RunCommands"))
    (lambda (ignored)
      (define command (yinz-run-command
        (hash "command" (list "/usr/bin/true") "context" (hash "label" label)
          "on-output" (lambda (output)
            (unless (and (= (bytes-ref (hash-ref output "data") 0) 255)
                         (equal? (hash-ref (hash-ref output "context") "label") label))
              (error "incorrect chunk"))
            (set! chunks (+ chunks 1))
            (yinz-request-render)))))
      (yinz-command (hash "Unsubscribe" (list "RunCommandResult")))
      (yinz-then command
        (lambda (result)
          (unless (and (= (hash-ref result "exit-code") 7)
                       (equal? (yinz-decode-utf8 (hash-ref result "stdout")) "hé🙂")
                       (= (bytes-ref (hash-ref result "stderr") 0) 255)
                       (equal? (hash-ref (hash-ref result "context") "label") label))
            (error "incorrect result"))
          (set! state "completed"))))))
(define (update event)
  (when (and (hash? event) (hash-contains? event "RunCommandResult")) (set! replies (+ replies 1)))
  (when (and (hash? event) (hash-contains? event "CustomMessage")) (set! fences (+ fences 1)))
  #t)
(define (render rows cols)
  (string-append label " " state " replies=" (number->string replies)
    " fences=" (number->string fences) " chunks=" (number->string chunks)))
"#;
    std::fs::write(&path, source).unwrap();
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
    let completion = |index| {
        jobs.wait_until(
            "the helper dispatched its command",
            default_timeout(),
            |jobs| {
                jobs.iter()
                    .filter_map(|job| match job {
                        BackgroundJob::RunCommand(
                            plugin,
                            client,
                            _,
                            _,
                            _,
                            _,
                            context,
                            _,
                            _,
                            generation,
                        ) => Some(PluginInstruction::for_plugin_event(
                            *plugin,
                            *client,
                            *generation,
                            Event::RunCommandResult(
                                Some(7),
                                "hé🙂".as_bytes().to_vec(),
                                vec![255],
                                context.clone(),
                            ),
                        )),
                        _ => None,
                    })
                    .nth(index)
            },
        )
    };
    harness.load();
    let old = completion(0);
    harness.rendered("original pending replies=0 fences=0 chunks=0");
    std::fs::write(
        &harness.path,
        source.replace("\"original\"", "\"replacement\""),
    )
    .unwrap();
    harness.reload();
    let current = completion(1);
    harness.rendered("replacement pending replies=0 fences=0 chunks=0");
    match (&old, &current) {
        (
            PluginInstruction::PluginEvent(_, _, old_generation, _),
            PluginInstruction::PluginEvent(_, _, current_generation, _),
        ) => assert_ne!(old_generation, current_generation),
        _ => panic!("commands must carry their originating generation"),
    }
    let chunk = |instruction: &PluginInstruction| match instruction {
        PluginInstruction::PluginEvent(
            plugin,
            client,
            generation,
            Event::RunCommandResult(_, _, _, context),
        ) => PluginInstruction::for_plugin_event(
            *plugin,
            *client,
            Some(*generation),
            Event::RunCommandOutput {
                data: vec![255],
                stderr: true,
                context: context.clone(),
            },
        ),
        _ => panic!("expected generated command result"),
    };
    harness.sender.send(chunk(&old)).unwrap();
    harness.sender.send(old).unwrap();
    harness.event(release_event());
    harness.rendered("replacement pending replies=0 fences=1 chunks=0");
    harness.sender.send(chunk(&current)).unwrap();
    harness.sender.send(current).unwrap();
    harness.rendered("replacement completed replies=1 fences=1 chunks=1");
    harness.finish();
    job_logger.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn steel_pane_output_subscriptions_reload_with_new_generations() {
    let source = r#"
(yinz-declare-pane)
(define label "old")
(define chunks 0)
(define fences 0)
(define (load config)
  (yinz-command (hash "Subscribe" (list "CustomMessage" "PaneOutput")))
  (yinz-then (yinz-wait-for-permissions (list "ReadPaneContents"))
    (lambda (ignored) (yinz-command (hash "SubscribePaneOutput" (hash "work" "one"))))))
(define (update event)
  (when (and (hash? event) (hash-contains? event "PaneOutput"))
    (define output (hash-ref event "PaneOutput"))
    (unless (and (= (hash-ref output "pane_id") 7) (= (bytes-ref (hash-ref output "data") 0) 255))
      (error "bad output"))
    (set! chunks (+ chunks 1)))
  (when (and (hash? event) (hash-contains? event "CustomMessage")) (set! fences (+ fences 1)))
  #t)
(define (render rows cols)
  (string-append label " chunks=" (number->string chunks) " fences=" (number->string fences)))
"#;
    let harness = Harness::new("scm", source, PermissionStatus::Granted);
    let subscription = |index| {
        harness.log.wait_until(
            "pane output subscription",
            default_timeout(),
            |instructions| {
                instructions
                    .iter()
                    .filter_map(|instruction| match instruction {
                        ScreenInstruction::SubscribePaneOutput(
                            plugin,
                            client,
                            Some(context),
                            generation,
                        ) => {
                            assert_eq!(context.get("work").map(String::as_str), Some("one"));
                            Some((*plugin, *client, *generation))
                        },
                        _ => None,
                    })
                    .nth(index)
            },
        )
    };
    harness.load();
    let old = subscription(0);
    harness.rendered("old chunks=0 fences=0");
    std::fs::write(&harness.path, source.replace("\"old\"", "\"new\"")).unwrap();
    harness.reload();
    let current = subscription(1);
    assert!(old.2.is_some() && current.2.is_some());
    assert_ne!(old.2, current.2);
    assert!(harness.log.lock().iter().any(|instruction| matches!(instruction, ScreenInstruction::UnsubscribePaneOutput(plugin, generation) if *plugin==old.0 && *generation == old.2)));
    let chunk = |(plugin, client, generation)| {
        PluginInstruction::for_plugin_event(
            plugin,
            client,
            generation,
            Event::PaneOutput {
                pane_id: 7,
                data: vec![255],
                context: BTreeMap::from([("work".into(), "one".into())]),
            },
        )
    };
    harness.sender.send(chunk(old)).unwrap();
    harness.event(release_event());
    harness.rendered("new chunks=0 fences=1");
    harness.sender.send(chunk(current)).unwrap();
    harness.rendered("new chunks=1 fences=1");
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    let reattached = subscription(2);
    assert_ne!(current.2, reattached.2);
    assert!(harness.log.lock().iter().any(|instruction| matches!(instruction,
        ScreenInstruction::SubscribePaneOutput(plugin, client, None, _) if *plugin == current.0 && *client == CLIENT)));
    harness.sender.send(chunk(current)).unwrap();
    harness.sender.send(chunk(reattached)).unwrap();
    harness.event(release_event());
    harness.rendered("new chunks=1 fences=1");
    harness.event(release_event());
    harness.rendered("new chunks=1 fences=2");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn steel_removed_client_reattaches_with_fresh_state_and_no_old_callbacks() {
    client_reattaches(false);
}
#[test]
#[serial(plugin_tests)]
fn steel_reused_client_id_reconnects_without_waiting_for_disposal() {
    client_reattaches(true);
}
fn client_reattaches(immediate: bool) {
    let harness = Harness::new(
        "scm",
        r#"
(yinz-declare-pane)
(define keys 0)
(define completed 0)
(define (load config) (yinz-command (hash "Subscribe" (list "Key"))))
(define (update event)
  (if (and (hash? event) (hash-contains? event "Key"))
    (begin
      (set! keys (+ keys 1))
      (define ordinal keys)
      (define ready (yinz-wait-for-event "CustomMessage"
        (lambda (message) (equal? (car message) "acceptance-release"))))
      (yinz-request-render)
      (yinz-then ready
        (lambda (ignored)
          (yinz-command (hash "SetSelectable" (not (= ordinal 2))))
          (set! completed (+ completed 1)) #t)))
    #f))
(define (render rows cols)
  (string-append "client=" (number->string (hash-ref (yinz-command "GetPluginIds") "client_id"))
    " keys=" (number->string keys) " completed=" (number->string completed)))
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("client=1 keys=0 completed=0");
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    harness.rendered("client=2 keys=0 completed=0");
    for _ in 0..2 {
        harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    }
    harness.rendered("client=1 keys=2 completed=0");
    let before_reattach = harness.log.lock().len();
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    if !immediate {
        // The original case observes disposal before reattaching. The immediate
        // case deliberately sends RemoveClient/AddClient back to back.
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(other),
                Event::Key(KeyWithModifier::new(BareKey::Char('b'))),
            )]))
            .unwrap();
        harness.rendered("client=2 keys=1 completed=0");
    }
    harness
        .sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    let replacement_rendered = |expected: &str| {
        harness.log.wait_until_described(
            &format!("reattached client 1 renders {expected}"),
            default_timeout(),
            |log| {
                rendered_assets(&log[before_reattach..])
                    .iter()
                    .any(|(client, frame)| *client == CLIENT && frame.contains(expected))
                    .then_some(())
            },
            |log| describe_renders(log),
        );
    };
    replacement_rendered("client=1 keys=0 completed=0");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('c'))));
    // The removed instance also rendered keys=1. Only the replacement's frame
    // proves its event waiter exists before broadcasting the release below.
    replacement_rendered("client=1 keys=1 completed=0");
    if immediate {
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(other),
                Event::Key(KeyWithModifier::new(BareKey::Char('b'))),
            )]))
            .unwrap();
        harness.rendered("client=2 keys=1 completed=0");
    }
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            None,
            release_event(),
        )]))
        .unwrap();
    for client in [CLIENT, other] {
        harness.rendered(&format!("client={client} keys=1 completed=1"));
    }
    let log = harness.finish();
    assert!(
        !log.iter()
            .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false))),
        "the removed instance's pending callbacks must never run"
    );
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("completed=2")));
}

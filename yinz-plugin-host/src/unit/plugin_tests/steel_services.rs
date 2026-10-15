//! Correlated peer services exercise real host permission and generation routing.
use super::script_harness::{load, release_event, run_plugin, Harness};
use super::*;
use std::time::Duration;
use yinz_types::internal_ids::PluginId;

#[test]
fn steel_rpc_calls_peers_by_url_and_id() {
    let harness = Harness::new(
        "scm",
        r#"
(define state "starting")
(yinz-rpc-handle "back" (lambda (payload call)
  (hash "value" payload "from" (hash-ref (hash-ref call "source") "Plugin"))))
(define (load config)
  (yinz-catch
    (yinz-then (yinz-wait-for-permissions (list "MessageAndLaunchOtherPlugins"))
      (lambda (ignored)
        (yinz-then (yinz-rpc-call (hash "pluginUrl" (hash-ref config "peer") "pluginConfig" (hash "greeting" "Oslo")
          "name" "echo" "payload" "hello" "timeoutMs" 10000))
          (lambda (first)
            (unless (and (equal? (hash-ref first "greeting") "Oslo")
                         (equal? (hash-ref (hash-ref first "back") "value") "hello")
                         (= (hash-ref (hash-ref first "back") "from") (hash-ref first "id")))
              (error "URL/config/reverse RPC mismatch"))
            (yinz-then (yinz-rpc-call (hash "pluginId" (hash-ref first "id") "name" "echo" "payload" "again" "timeoutMs" 10000))
              (lambda (second)
                (unless (equal? (hash-ref (hash-ref second "back") "value") "again") (error "ID RPC mismatch"))
                (define (check-errors names)
                  (if (null? names) (set! state "RPC peer verified")
                    (yinz-then
                      (yinz-catch
                        (yinz-then (yinz-rpc-call (hash "pluginId" (hash-ref first "id") "name" (car names) "timeoutMs" 10000))
                          (lambda (ignored) (error "unexpected success")))
                        (lambda (failure)
                          (unless (string-contains? failure (if (equal? (car names) "missing") "no rpc handler" "peer failed"))
                            (error failure))))
                      (lambda (ignored) (check-errors (cdr names))))))
                (check-errors (list "missing" "failure"))))))))
    (lambda (failure) (set! state (string-append "RPC failed: " failure)))))
(define (render rows cols) state)
"#,
        PermissionStatus::Granted,
    );
    let peer = harness.directory.path().join("peer.scm");
    std::fs::write(&peer, r#"
(define ready #f)
(define greeting "")
(yinz-rpc-handle "echo" (lambda (payload call)
  (yinz-then ready
    (lambda (ignored)
      (yinz-then (yinz-rpc-call (hash "pluginId" (hash-ref (hash-ref call "source") "Plugin") "name" "back" "payload" payload "timeoutMs" 10000))
        (lambda (back) (hash "id" (hash-ref (yinz-command "GetPluginIds") "plugin_id") "greeting" greeting "back" back)))))))
(yinz-rpc-handle "failure" (lambda (payload call) (error "peer failed")))
(define (load config)
  (set! greeting (hash-ref config "greeting"))
  (set! ready (yinz-wait-for-permissions (list "MessageAndLaunchOtherPlugins"))) ready)
"#).unwrap();
    load(
        &harness.sender,
        RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::File(harness.path.clone()),
            configuration: PluginUserConfiguration::new(BTreeMap::from([(
                "peer".into(),
                format!("file:{}", peer.display()),
            )])),
            ..Default::default()
        }),
    );
    let frame = harness.rendered("RPC ");
    assert!(frame.contains("RPC peer verified"), "{frame}");
    harness.finish();
}

#[test]
fn steel_rpc_rejects_calls_without_message_permission() {
    let harness = Harness::new(
        "scm",
        r#"
(define state "starting")
(define denied 0)
(define (attempt)
  (yinz-catch (yinz-rpc-call (hash "pluginId" 7 "name" "echo" "timeoutMs" 100))
    (lambda (failure)
      (if (string-contains? failure "MessageAndLaunchOtherPlugins") (set! denied (+ denied 1)) (error failure)))))
(define (load config)
  (yinz-then (attempt)
    (lambda (ignored)
      (yinz-then (yinz-catch (yinz-wait-for-permissions (list "MessageAndLaunchOtherPlugins")) (lambda (failure) (void)))
        (lambda (ignored)
          (yinz-then (attempt)
            (lambda (ignored) (set! state (if (= denied 2) "RPC permission denied twice" "unexpected RPC permission")))))))))
(define (render rows cols) state)
"#,
        PermissionStatus::Denied,
    );
    harness.load();
    harness.rendered("RPC permission denied twice");
    harness.finish();
}

#[test]
fn steel_rpc_reload_ignores_old_replies_before_settling_the_new_call() {
    let source = r#"
(define label "old")
(define result "waiting")
(define fences 0)
(yinz-rpc-handle "echo" (lambda (payload call) label))
(define (load config)
  (yinz-command (hash "Subscribe" (list "CustomMessage")))
  (yinz-then (yinz-wait-for-permissions (list "MessageAndLaunchOtherPlugins"))
    (lambda (ignored)
      (yinz-then (yinz-rpc-call (hash "pluginId" (hash-ref (yinz-command "GetPluginIds") "plugin_id") "name" "echo"))
        (lambda (reply) (set! result reply))))))
(define (update event)
  (if (and (hash? event) (hash-contains? event "CustomMessage")) (begin (set! fences (+ fences 1)) #t) #f))
(define (render rows cols) (string-append label " result=" result " fences=" (number->string fences)))
"#;
    let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_proxy = SenderWithContext::new(to_proxy);
    let harness = Harness::with_event_sender(
        "scm",
        source,
        Some(PermissionStatus::Granted),
        Some(to_proxy.clone()),
    );
    let replies = Watched::new(Vec::<PluginInstruction>::new());
    let proxy = std::thread::spawn({
        let replies = replies.clone();
        let sender = harness.sender.clone();
        move || loop {
            let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
            match instruction {
                PluginInstruction::Exit => break,
                PluginInstruction::MessageFromPlugin { ref message, .. }
                    if message.message_name == "yinz_rpc_reply" =>
                {
                    replies.lock().push(instruction)
                },
                other => {
                    let _ = sender.send(other);
                },
            }
        }
    });
    harness.load();
    let old = replies.wait_until("old RPC reply", Duration::from_secs(20), |events| {
        events.first().cloned()
    });
    harness.rendered("old result=waiting fences=0");
    std::fs::write(&harness.path, source.replace("\"old\"", "\"new\"")).unwrap();
    harness.reload();
    let current = replies.wait_until("replacement RPC reply", Duration::from_secs(20), |events| {
        events.get(1).cloned()
    });
    harness.rendered("new result=waiting fences=0");
    harness.sender.send(old).unwrap();
    harness.event(release_event());
    harness.rendered("new result=waiting fences=1");
    harness.sender.send(current).unwrap();
    harness.rendered("new result=new fences=1");
    harness.finish();
    to_proxy.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn steel_completion_real_dispatch_permissions_and_reload_isolation() {
    // Initialize the real discovery snapshot without a background scanner racing this fixture.
    let (tx, rx): ChannelWithContext<BackgroundJob> = channels::unbounded();
    SenderWithContext::new(tx)
        .send(BackgroundJob::Exit)
        .unwrap();
    yinz_bus::background_jobs::background_jobs_main(
        Bus::new(vec![rx], None, None, None, None, None, None, None).should_silently_fail(),
        None,
        true,
    )
    .unwrap();
    let scan = yinz_bus::background_jobs::session_scan_state().unwrap();
    *scan.current_session_name.lock().unwrap() = "deno-completion-test".into();
    scan.current_session_info.lock().unwrap().name = "deno-completion-test".into();
    scan.current_session_plugin_list.lock().unwrap().insert(
        PluginId::new(7),
        RunPlugin {
            location: RunPluginLocation::Yinz(yinz_types::data::PluginTag::new("agent-service")),
            ..Default::default()
        },
    );
    let source = r#"
(define label "old")
(define state "starting")
(define fences 0)
(define (expect-denied permission)
  (yinz-catch
    (yinz-then (yinz-complete (hash "messages" (list (hash "role" "user"))))
      (lambda (ignored) (error "unprotected completion")))
    (lambda (failure) (unless (string-contains? failure permission) (error failure)))))
(define (load config)
  (yinz-command (hash "Subscribe" (list "CustomMessage")))
  (define ready
    (if (equal? label "old")
      (yinz-then (expect-denied "ReadApplicationState")
        (lambda (ignored)
          (yinz-then (yinz-wait-for-permissions (list "ReadApplicationState"))
            (lambda (ignored)
              (yinz-then (expect-denied "MessageAndLaunchOtherPlugins")
                (lambda (ignored) (yinz-wait-for-permissions (list "MessageAndLaunchOtherPlugins"))))))))
      (yinz-wait-for-permissions (list "ReadApplicationState" "MessageAndLaunchOtherPlugins"))))
  (yinz-then ready
    (lambda (ignored)
      (set! state "waiting") (yinz-request-render)
      (yinz-then (yinz-complete (hash "messages" (list (hash "role" "user" "content" label))))
        (lambda (result) (set! state (hash-ref result "content")))))))
(define (update event)
  (if (and (hash? event) (hash-contains? event "CustomMessage")) (begin (set! fences (+ fences 1)) #t) #f))
(define (render rows cols) (string-append label " completion=" state " fences=" (number->string fences)))
"#;
    let (tx, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let tx = SenderWithContext::new(tx);
    let harness = Harness::with_event_sender(
        "scm",
        source,
        Some(PermissionStatus::Granted),
        Some(tx.clone()),
    );
    let replies = Watched::new(Vec::<PluginInstruction>::new());
    let proxy = std::thread::spawn({
        let replies = replies.clone();
        let sender = harness.sender.clone();
        move || loop {
            match outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap().0 {
                PluginInstruction::Exit => break,
                PluginInstruction::MessageFromPlugin {
                    source_plugin_id,
                    mut message,
                } if message.message_name == "llm_complete" => {
                    assert_eq!(message.destination_plugin_id, Some(PluginId::new(7)));
                    let request: serde_json::Value =
                        serde_json::from_str(message.message_payload.as_ref().unwrap()).unwrap();
                    message.destination_plugin_id = Some(source_plugin_id);
                    message.message_name = "llm_complete_result".into();
                    message.message_payload = Some(serde_json::json!({"request_id":request["request_id"],"content":request["messages"][0]["content"],"tool_calls":[],"finish_reason":"stop","model":"deterministic-peer","usage":null,"error":""}).to_string());
                    replies.lock().push(PluginInstruction::MessageFromPlugin {
                        source_plugin_id: PluginId::new(7),
                        message,
                    });
                },
                other => {
                    let _ = sender.send(other);
                },
            }
        }
    });
    harness.load();
    let old = replies.wait_until("first completion request", Duration::from_secs(20), |r| {
        r.first().cloned()
    });
    harness.rendered("old completion=waiting fences=0");
    // Grants survive reload, so the replacement proceeds directly to its request.
    let replacement = source.replace("\"old\"", "\"new\"");
    std::fs::write(&harness.path, replacement).unwrap();
    harness.reload();
    let current = replies.wait_until(
        "replacement completion request",
        Duration::from_secs(20),
        |r| r.get(1).cloned(),
    );
    harness.rendered("new completion=waiting fences=0");
    harness.sender.send(old).unwrap();
    harness.event(release_event());
    harness.rendered("new completion=waiting fences=1");
    harness.sender.send(current).unwrap();
    harness.rendered("new completion=new fences=1");
    harness.finish();
    tx.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

#[test]
fn steel_agent_events_use_real_permissions_and_reloaded_waits() {
    use yinz_types::agent::{AgentEvent, AgentState};
    let source = r#"
(define label "old")
(define state "starting")
(define heard 0)
(define (load config)
  (yinz-then (yinz-wait-for-permissions (list "ReadApplicationState"))
    (lambda (ignored)
      (yinz-agent-on (lambda (event) (set! heard (+ heard 1))))
      (set! state "waiting") (yinz-request-render)
      (yinz-then (yinz-agent-wait-for-turn (hash "Terminal" 7))
        (lambda (turn) (set! state (string-append (hash-ref turn "hook") ":" (hash-ref turn "message"))))))))
(define (render rows cols) (string-append label " agent=" state " heard=" (number->string heard)))
"#;
    let harness = Harness::new("scm", source, PermissionStatus::Granted);
    let event = |hook: &str| {
        Event::AgentEvent(AgentEvent {
            pane_id: crate::TerminalId::new(7),
            kind: "codex".into(),
            hook: hook.into(),
            state: Some(AgentState::Idle),
            last_message: Some("done".into()),
            session_id: Some("session".into()),
            detail: None,
            prompt: None,
            tool_name: None,
            cwd: None,
            raw: String::new(),
        })
    };
    harness.load();
    harness.rendered("old agent=waiting heard=0");
    harness.event(event("SessionStart"));
    harness.event(release_event());
    std::fs::write(&harness.path, source.replace("\"old\"", "\"new\"")).unwrap();
    harness.reload();
    harness.rendered("new agent=waiting heard=0");
    harness.event(event("Stop"));
    harness.rendered("new agent=Stop:done heard=1");
    let log = harness.finish();
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, s)| s.contains("old agent=Stop")));
}

const CLIENT: ClientId = ClientId::new(1);
const PLUGIN: PluginId = PluginId::new(0);
const KEYBINDINGS_PLUGIN: &str = r#"
(define count 0)
(define state "loading")
(define (load config)
  (yinz-catch
    (yinz-then (yinz-wait-for-permissions (list "Reconfigure"))
      (lambda (ignored)
        (yinz-register-default-keybindings
          (list (hash "mode" "normal" "key" "Ctrl g" "handler"
                  (lambda (message)
                    (unless (equal? (hash-ref message "source") "Keybind") (error "source"))
                    (set! count (+ count 1)) #t))
                (hash "mode" "normal" "key" "Ctrl n" "actions" (list "NewPane"))))
        (set! state "registered")))
    (lambda (failure) (set! state (string-append "refused: " failure)))))
(define (pipe message) (set! state "leaked") #t)
(define (render rows cols) (string-append state " count=" (number->string count)))
"#;

fn keybind_pipe(name: &str) -> PluginInstruction {
    PluginInstruction::KeybindPipe {
        name: name.to_owned(),
        payload: None,
        plugin: None,
        args: None,
        configuration: None,
        floating: None,
        should_open_in_place: false,
        pane_id_to_replace: None,
        pane_title: None,
        cwd: None,
        skip_cache: false,
        cli_client_id: CLIENT,
        plugin_and_client_id: Some((PLUGIN, CLIENT)),
        notification_end: None,
    }
}

#[test]
fn steel_default_keybindings_register_after_the_reconfigure_grant_and_dispatch_their_reserved_pipe()
{
    use yinz_types::input::actions::Action;
    let directory = tempdir().unwrap();
    let path = directory.path().join("keybindings.scm");
    std::fs::write(&path, KEYBINDINGS_PLUGIN).unwrap();
    let (sender, server_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(Some(directory.path().to_owned()), None);
    let log = Watched::new(vec![]);
    let logger = permissions_and_log_until_disconnect(
        &log,
        screen_receiver,
        directory.path().join("permissions.kdl"),
        sender.clone(),
        CLIENT,
        PermissionStatus::Granted,
    );
    let rebinds = Watched::new(vec![]);
    let server_logger =
        log_actions_in_thread_struct!(rebinds, ServerInstruction::KillSession, server_receiver, 1);
    sender.send(PluginInstruction::AddClient(CLIENT)).unwrap();
    load(&sender, run_plugin(&path));
    wait_for_render_containing(&log, "registered count=0");
    let registered = rebinds.wait_until(
        "the Scheme plugin's default keybindings reached the server",
        default_timeout(),
        |instructions| {
            instructions
                .iter()
                .find_map(|instruction| match instruction {
                    ServerInstruction::RebindKeys {
                        keys_to_rebind,
                        keys_to_unbind,
                        write_config_to_disk,
                        plugin_id,
                        ..
                    } => Some((
                        keys_to_rebind.clone(),
                        keys_to_unbind.clone(),
                        *write_config_to_disk,
                        *plugin_id,
                    )),
                    _ => None,
                })
        },
    );
    let (keys_to_rebind, keys_to_unbind, write_config_to_disk, plugin_id) = registered;
    assert!(keys_to_unbind.is_empty() && !write_config_to_disk);
    assert_eq!(plugin_id, Some(PLUGIN));
    assert_eq!(keys_to_rebind.len(), 2);
    // The host rewrote the reserved MessagePlugin name to address this plugin.
    assert!(keys_to_rebind.iter().any(|(_, _, actions)| matches!(
        actions.as_slice(),
        [Action::KeybindPipe { name: Some(name), plugin_id: Some(id), .. }]
            if name == "__yinz_default_keybinding:1" && *id == PLUGIN
    )));
    assert!(keys_to_rebind
        .iter()
        .any(|(_, _, actions)| matches!(actions.as_slice(), [Action::NewPane { .. }])));
    sender
        .send(keybind_pipe("__yinz_default_keybinding:1"))
        .unwrap();
    wait_for_render_containing(&log, "registered count=1");
    sender
        .send(keybind_pipe("__yinz_default_keybinding:1"))
        .unwrap();
    wait_for_render_containing(&log, "registered count=2");
    // A stale index is consumed without reaching the plugin's own pipe entry point.
    sender
        .send(keybind_pipe("__yinz_default_keybinding:9"))
        .unwrap();
    sender
        .send(keybind_pipe("__yinz_default_keybinding:1"))
        .unwrap();
    let frame = wait_for_render_containing(&log, "count=3");
    assert!(frame.contains("registered count=3"), "{frame}");
    teardown();
    logger.join().unwrap();
    server_logger.join().unwrap();
}

#[test]
fn steel_default_keybindings_are_refused_without_the_reconfigure_grant() {
    let harness = Harness::new(
        "scm",
        r#"
(define state "loading")
(define (load config)
  (set! state (with-handler (lambda (failure) (error-object-message failure))
    (yinz-register-default-keybindings (list (hash "mode" "normal" "key" "Ctrl g" "actions" (list "NewPane"))))
    "registered")))
(define (render rows cols) state)
"#,
        PermissionStatus::Denied,
    );
    harness.load();
    let frame = harness.rendered("Reconfigure");
    assert!(frame.contains("requires 'Reconfigure'"), "{frame}");
}

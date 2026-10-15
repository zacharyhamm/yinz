use super::panes_tests::Harness;
use serde_json::{json, Value};

fn manifest(h: &mut Harness, panes: Value) {
    h.event(json!({"PaneUpdate":{"panes":{"0":panes}}}));
}
fn pane(id: u32, state: &str) -> Value {
    json!({"id":id,"is_plugin":false,"agent":{"kind":"codex","state":state,"session_id":"session"}})
}
fn hook(h: &mut Harness, id: u32, kind: &str) {
    h.event(json!({"AgentEvent":{"pane_id":id,"hook":kind,"state":"Idle","last_message":"done","session_id":"session"}}));
}

#[test]
fn agent_cache_state_turn_fallback_and_cancellation_preserve_client() {
    let mut h = Harness::new("(define results '())");
    h.eval(r#"
      (define missing (yinz-agent-info 7))
      (define waits (list (yinz-agent-wait-for-state 7 "idle" (hash "after" '("busy" "Waiting")))
        (yinz-agent-wait-for-event (hash "Terminal" 7) '("PreToolUse"))
        (yinz-agent-wait-for-turn 7) (yinz-agent-wait-for-turn 8 (hash "fallback" "idle"))
        (yinz-agent-wait-for-turn 9 (hash "fallback" "idle")) (yinz-agent-wait-for-turn 10 (hash "fallback" "idle"))))
      (for-each (lambda (i)
        (yinz-catch (yinz-then (list-ref waits i) (lambda (r) (yinz-command "GetPluginIds") (set! results (cons (list i r) results))))
          (lambda (e) (set! results (cons (list i e) results))))) (range 0 6))
      (yinz-cancel! (list-ref waits 4))
    "#);
    assert!(h.value("missing").is_null());
    manifest(&mut h, json!([pane(7, "Idle"), pane(8, "Idle")]));
    h.eval("(define saved (yinz-agent-all))");
    hook(&mut h, 7, "SessionStart");
    hook(&mut h, 8, "Stop");
    hook(&mut h, 99, "Stop");
    manifest(
        &mut h,
        json!([pane(7, "Busy"), pane(8, "Waiting"), pane(10, "Waiting")]),
    );
    manifest(
        &mut h,
        json!([pane(7, "Idle"), pane(8, "Idle"), pane(10, "Idle")]),
    );
    hook(&mut h, 7, "PreToolUse");
    hook(&mut h, 7, "StopFailure");
    hook(&mut h, 7, "Interrupt");
    manifest(&mut h, json!([]));
    h.eval("(define missing-after (yinz-agent-info 7)) (define empty (yinz-agent-all))");
    assert!(h.value("missing-after").is_null());
    assert_eq!(h.value("empty"), json!({}));
    assert_eq!(h.value("saved")["8"]["state"], "Idle");
    let results = h.value("results");
    let rows = results.as_array().unwrap();
    assert_eq!(rows.len(), 6);
    let get = |id| rows.iter().find(|r| r[0] == id).unwrap()[1].clone();
    assert_eq!(get(0)["agent"]["state"], "Idle");
    assert_eq!(get(1)["hook"], "PreToolUse");
    assert_eq!(
        get(2),
        json!({"hook":"StopFailure","message":"done","state":"Idle","sessionId":"session"})
    );
    assert_eq!(get(3)["hook"], "Stop");
    assert_eq!(get(4), "operation cancelled");
    assert!(get(5)["hook"].is_null());
    assert!(h
        .commands
        .lock()
        .unwrap()
        .iter()
        .filter(|(c, _)| c == "GetPluginIds")
        .all(|(_, client)| client == 2));
}

#[test]
fn agent_prompt_validation_bracketed_paste_order_and_unbounded_waits() {
    let mut h = Harness::new("(define errors '()) (define completed #f)");
    h.eval(r#"
      (for-each (lambda (pane) (yinz-catch (yinz-agent-wait-for-turn pane) (lambda (e) (set! errors (cons e errors)))))
        (list -1 1.5 4294967296 (hash "Plugin" 7) (hash "Terminal" 7 "Plugin" 8) "7" json-null))
      (for-each (lambda (op) (yinz-catch op (lambda (e) (set! errors (cons e errors)))))
        (list (yinz-agent-wait-for-state 7 "broken") (yinz-agent-wait-for-event 7 '(1))
          (yinz-agent-prompt 7 42 (hash "fallback" "idle"))
          (yinz-agent-prompt 7 "go" (hash "enter" "yes" "fallback" "idle"))))
    "#);
    assert_eq!(h.value("errors").as_array().unwrap().len(), 11);
    assert!(h.commands.lock().unwrap().is_empty());
    h.eval(r#"
      (yinz-agent-send 7 "draft" (hash "enter" #f)) (yinz-agent-send 7 "submit")
      (yinz-then (yinz-agent-prompt 7 "prompt") (lambda (reply) (set! completed (hash-ref reply "message"))))
      (define pending (yinz-agent-prompt 8 "cancel" (hash "enter" #f)))
      (yinz-catch pending (lambda (e) (set! errors (cons e errors)))) (yinz-cancel! pending)
    "#);
    h.advance(60000);
    assert_eq!(h.value("completed"), false);
    hook(&mut h, 8, "Stop");
    hook(&mut h, 7, "Stop");
    assert_eq!(h.value("completed"), "done");
    let writes = h.sent("WriteToPaneId");
    let text: Vec<String> = writes
        .iter()
        .map(|w| String::from_utf8(crate::binary::decode(&w[0]).unwrap()).unwrap())
        .collect();
    assert_eq!(
        text,
        [
            "\x1b[200~draft\x1b[201~",
            "\x1b[200~submit\x1b[201~",
            "\r",
            "\x1b[200~prompt\x1b[201~",
            "\r",
            "\x1b[200~cancel\x1b[201~"
        ]
    );
    assert!(h
        .commands
        .lock()
        .unwrap()
        .iter()
        .all(|(c, _)| c.get("Subscribe").is_some() || c.get("WriteToPaneId").is_some()));
}

#[test]
fn agent_control_correlates_responses_without_raw_input_and_spawn_keeps_spec() {
    let mut h = Harness::new("(define responses '())");
    h.eval(r#"
      (yinz-agent-register-provider (hash "kind" "fixture-agent" "executable" "fixture-agent" "input" (hash "submit_key" "Ctrl j")))
      (define providers (yinz-agent-providers))
      (for-each (lambda (wait)
        (yinz-then (yinz-agent-control (hash "Prompt" (hash "pane_id" (hash "Terminal" 7) "prompt" "work" "wait" wait "timeout_ms" 30000)))
          (lambda (value) (set! responses (cons value responses))))) '(#f #t))
      (define spawned (yinz-agent-spawn (hash "kind" "codex")))
      (yinz-agent-spawn-details (hash "kind" "claude" "permission_mode" "plan" "worktree_dir" ".trees"
        "sourceDirectory" "/host/project" "startSuppressed" #t "context" (hash "key" "value")))
    "#);
    assert_eq!(h.value("providers")[0]["kind"], "fixture-agent");
    assert_eq!(h.value("spawned"), json!({"Terminal":7}));
    assert!(h.sent("WriteToPaneId").is_empty());
    h.event(json!({"CustomMessage":["agent-response","{\"id\":\"stale\",\"response\":{}}"]}));
    assert_eq!(h.value("responses"), json!([]));
    for request in h.sent("AgentControl") {
        h.event(json!({"CustomMessage":["agent-response",json!({"id":request["id"],"response":{"outcome":"completed","message":"done","agents":[]}}).to_string()]}));
    }
    assert_eq!(h.value("responses").as_array().unwrap().len(), 2);
    let spawned = h.sent("SpawnAgent");
    assert_eq!(spawned[0]["spec"], json!({"kind":"codex"}));
    assert!(spawned[0]["source_directory"].is_null());
    assert_eq!(spawned[0]["start_suppressed"], false);
    assert_eq!(spawned[1]["spec"]["permission_mode"], "plan");
    assert_eq!(spawned[1]["source_directory"], "/host/project");
    assert_eq!(spawned[1]["context"], json!({"key":"value"}));
}

#[test]
fn agent_observers_release_on_cancel_and_dispatch_errors() {
    for command in ["Subscribe", "WriteToPaneId"] {
        let mut h = Harness::new("(define error #f)");
        *h.failure.lock().unwrap() = Some(command.into());
        h.eval("(yinz-catch (yinz-agent-prompt 7 \"fail\" (hash \"fallback\" \"idle\")) (lambda (message) (set! error message)))");
        assert_eq!(h.value("error"), format!("{command} dispatch failed"));
        *h.failure.lock().unwrap() = None;
        h.eval("(yinz-command (hash \"Unsubscribe\" '(\"AgentEvent\" \"PaneUpdate\")))");
        assert_eq!(
            h.sent("Unsubscribe").last().unwrap(),
            &json!(["AgentEvent", "PaneUpdate"])
        );
    }
    let mut h = Harness::new("(define calls 0)");
    h.eval("(define off (yinz-agent-on (lambda (_) (set! calls (+ calls 1))))) (off) (off)");
    hook(&mut h, 7, "Stop");
    assert_eq!(h.value("calls"), 0);
}

#[test]
fn agent_send_keeps_large_prompt_bytes_compact_and_exact() {
    let mut h = Harness::new("");
    h.eval("(yinz-agent-send 7 (make-string 200000 #\\x) (hash \"enter\" #f))");
    let writes = h.sent("WriteToPaneId");
    assert_eq!(writes.len(), 1);
    assert!(writes[0][0].is_object());
    let bytes = crate::binary::decode(&writes[0][0]).unwrap();
    assert_eq!(bytes.len(), 200012);
    assert!(bytes.starts_with(b"\x1b[200~"));
    assert!(bytes.ends_with(b"\x1b[201~"));
    assert!(bytes[6..bytes.len() - 6].iter().all(|byte| *byte == b'x'));
    assert_eq!(writes[0][1], json!({"Terminal": 7}));
}

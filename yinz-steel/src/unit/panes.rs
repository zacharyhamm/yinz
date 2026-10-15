use crate::Runtime;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

pub(super) struct Harness {
    pub runtime: Runtime,
    pub commands: Arc<Mutex<Vec<(Value, Value)>>>,
    pub failure: Arc<Mutex<Option<String>>>,
    pub info: Arc<Mutex<Value>>,
    now: Arc<AtomicU64>,
}
impl Harness {
    pub fn new(source: &str) -> Self {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let sent = commands.clone();
        let failure = Arc::new(Mutex::new(None::<String>));
        let failing = failure.clone();
        let info = Arc::new(Mutex::new(json!({"id":7,"is_plugin":false,"exited":false})));
        let pane_info = info.clone();
        let now = Arc::new(AtomicU64::new(0));
        let clock = now.clone();
        let mut runtime = Runtime::new_with_clock(
            move |envelope| {
                let actor = envelope
                    .pointer("/__yinz_runtime/client")
                    .cloned()
                    .unwrap_or(Value::Null);
                let command = envelope
                    .pointer("/__yinz_runtime/command")
                    .cloned()
                    .unwrap_or(envelope);
                let name = command
                    .as_str()
                    .or_else(|| command.as_object()?.keys().next().map(String::as_str))
                    .unwrap();
                crate::binary::decode_command(command.clone())
                    .map_err(|error| anyhow::anyhow!("invalid {name}: {error}"))?;
                sent.lock().unwrap().push((command.clone(), actor.clone()));
                if failing.lock().unwrap().as_deref() == Some(name) {
                    anyhow::bail!("{name} dispatch failed");
                }
                Ok(match name {
                    "OpenCommandPane" | "OpenCommandPaneFloating" => json!({"Terminal":7}),
                    "GetPaneInfo" => pane_info.lock().unwrap().clone(),
                    "GetPluginIds" => json!({"client_id":actor}),
                    "GetAgentProviders" => {
                        json!([{"kind":"fixture-agent","executable":"fixture-agent"}])
                    },
                    "SpawnAgent" => json!({"pane_id":{"Terminal":7}}),
                    _ => Value::Null,
                })
            },
            move || clock.load(Ordering::SeqCst),
        )
        .unwrap();
        // These files are also loaded by production Runtime initialization.
        runtime.invoke("yinz-async-enter", vec![json!(2)]).unwrap();
        runtime.eval(source).unwrap();
        Self {
            runtime,
            commands,
            failure,
            info,
            now,
        }
    }
    pub fn eval(&mut self, source: &str) {
        self.runtime.eval(source).unwrap();
        self.runtime.invoke("yinz-async-poll", vec![]).unwrap();
    }
    pub fn value(&self, name: &str) -> Value {
        self.runtime.value(name).unwrap()
    }
    pub fn sent(&self, name: &str) -> Vec<Value> {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(c, _)| c.get(name).cloned())
            .collect()
    }
    pub fn event(&mut self, value: Value) {
        self.runtime
            .invoke("yinz-async-enter", vec![json!(9)])
            .unwrap();
        self.runtime.update(value).unwrap();
    }
    pub fn context(&self) -> Value {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|(command, _)| {
                command
                    .pointer("/RunCommandInPane/context")
                    .or_else(|| command.pointer("/OpenCommandPane/1"))
                    .or_else(|| command.pointer("/OpenCommandPaneFloating/2"))
                    .cloned()
            })
            .unwrap()
    }
    pub fn pane_event(&mut self, kind: &str, code: Value, context: Option<Value>) {
        let payload = if kind == "PaneClosed" {
            json!({"Terminal":7})
        } else if kind == "CommandPaneExited" {
            json!([7, code, context.unwrap_or_else(|| self.context())])
        } else {
            json!([7, context.unwrap_or_else(|| self.context())])
        };
        self.event(json!({kind:payload}));
    }
    pub fn advance(&mut self, millis: u64) {
        self.now.fetch_add(millis, Ordering::SeqCst);
        self.event(json!({"Timer":millis as f64 / 1000.0}));
    }
}

#[test]
fn sequence_reuses_pane_correlates_stale_events_and_appends_from_callbacks() {
    let mut h = Harness::new(
        r#"
      (define result json-null) (define calls '()) (define sequence #f)
      (define (start)
        (set! sequence (yinz-pane-sequence (hash "steps" (list (hash "path" "true") (hash "path" "echo" "args" '("two") "stdin" "input" "timeoutMs" 10))
          "onStep" (lambda (n step code)
            (set! calls (append calls (list (list n (hash-ref step "path") code))))
            (when (= n 2) ((hash-ref sequence "push") (hash "path" "true")))
            (error "callback"))
          "onDone" (lambda (_) (error "done callback")))))
        (yinz-then (hash-ref sequence "done") (lambda (value) (set! result value))))
      (start)
    "#,
    );
    h.runtime
        .invoke("yinz-async-enter", vec![json!(2)])
        .unwrap();
    let old = h.context();
    h.event(json!({"CommandPaneExited":[8,0,old]}));
    h.pane_event("CommandPaneOpened", json!(0), None);
    h.pane_event("CommandPaneExited", json!(0), None);
    assert_eq!(h.sent("OpenCommandPane").len(), 1);
    assert_eq!(h.sent("RunCommandInPane")[0]["keep_scrollback"], true);
    assert!(h.sent("WriteCharsToPaneId").is_empty());
    h.pane_event("CommandPaneReRun", json!(0), Some(old.clone()));
    h.pane_event("CommandPaneExited", json!(7), Some(old));
    assert_eq!(h.value("calls").as_array().unwrap().len(), 1);
    h.pane_event("CommandPaneReRun", json!(0), None);
    h.pane_event("CommandPaneReRun", json!(0), None);
    h.pane_event("CommandPaneExited", json!(0), None);
    h.pane_event("CommandPaneExited", json!(0), None);
    assert_eq!(h.value("result")["status"], "success");
    assert_eq!(h.value("result")["exitCodes"], json!([0, 0, 0]));
    assert_eq!(
        h.sent("WriteCharsToPaneId"),
        vec![json!(["input",{"Terminal":7}])]
    );
    h.eval("((hash-ref sequence \"push\") json-null) ((hash-ref sequence \"cancel\"))");
    h.pane_event("CommandPaneExited", json!(0), None);
    assert_eq!(h.value("calls").as_array().unwrap().len(), 3);
}

#[test]
fn sequence_validates_all_steps_before_dispatch_and_preserves_failure_policy() {
    let mut h = Harness::new("(define errors '())");
    for options in [
        "(hash \"steps\" '())",
        "(hash \"steps\" (list (hash \"path\" \"true\") (hash \"path\" 42)))",
        "(hash \"steps\" (list (hash \"path\" \"x\" \"args\" '(1))))",
        "(hash \"steps\" (list (hash \"path\" \"x\" \"stdin\" 42)))",
        "(hash \"steps\" (list (hash \"path\" \"x\" \"timeoutMs\" -1)))",
        "(hash \"steps\" (list (hash \"path\" \"x\")) \"floating\" 1)",
        "(hash \"steps\" (list (hash \"path\" \"x\")) \"onDone\" 1)",
    ] {
        h.eval(&format!("(with-handler (lambda (error) (set! errors (cons #t errors))) (yinz-pane-sequence {options}))"));
    }
    assert_eq!(h.value("errors").as_array().unwrap().len(), 7);
    assert!(h.commands.lock().unwrap().is_empty());
    for stop in [true, false] {
        let mut h = Harness::new("(define result json-null) (define sequence #f)");
        h.eval(&format!("(set! sequence (yinz-pane-sequence (hash \"steps\" (list (hash \"path\" \"false\") (hash \"path\" \"true\")) \"stopOnFailure\" #{}))) (yinz-then (hash-ref sequence \"done\") (lambda (r) (set! result r)))",if stop {'t'} else {'f'}));
        h.pane_event("CommandPaneExited", json!(3), None);
        if !stop {
            h.pane_event("CommandPaneExited", json!(0), None);
        }
        assert_eq!(h.value("result")["status"], "failed");
        assert_eq!(h.value("result")["failedStep"], 1);
        assert_eq!(
            h.value("result")["exitCodes"],
            if stop { json!([3]) } else { json!([3, 0]) }
        );
    }
}

#[test]
fn sequence_timeout_cancel_grace_refusal_and_close_policies() {
    for timeout in [false, true] {
        let mut h = Harness::new("(define result json-null) (define sequence #f)");
        h.eval("(set! sequence (yinz-pane-sequence (hash \"steps\" (list (hash \"path\" \"cat\" \"timeoutMs\" 10)) \"killGraceMs\" 20 \"onCancel\" \"close\"))) (yinz-then (hash-ref sequence \"done\") (lambda (r) (set! result r)))");
        h.advance(100);
        assert!(h.sent("SendSigintToPaneId").is_empty());
        h.pane_event("CommandPaneOpened", json!(0), None);
        if timeout {
            h.advance(10);
        } else {
            *h.failure.lock().unwrap() = Some("SendSigintToPaneId".into());
            assert!(h.runtime.eval("((hash-ref sequence \"cancel\"))").is_err());
            h.eval("(define current-status ((hash-ref sequence \"status\")))");
            assert_eq!(h.value("current-status"), "running");
            *h.failure.lock().unwrap() = None;
            h.eval("((hash-ref sequence \"cancel\")) ((hash-ref sequence \"cancel\")) ((hash-ref sequence \"push\") json-null)");
        }
        h.advance(20);
        assert_eq!(h.sent("SendSigkillToPaneId"), vec![json!({"Terminal":7})]);
        h.advance(20);
        assert_eq!(
            h.value("result")["status"],
            if timeout { "failed" } else { "cancelled" }
        );
        assert_eq!(h.value("result")["exitCodes"], json!([null]));
        assert_eq!(h.sent("CloseTerminalPane").len(), usize::from(!timeout));
        let count = h.commands.lock().unwrap().len();
        h.pane_event("CommandPaneExited", json!(0), None);
        h.advance(100);
        assert_eq!(h.commands.lock().unwrap().len(), count);
    }
    for closed in [false, true] {
        let mut h = Harness::new("(define result json-null)");
        h.eval("(define sequence (yinz-pane-sequence (hash \"name\" \"named\" \"floating\" (hash \"width\" (hash \"Percent\" 50)) \"steps\" (list (hash \"path\" \"cat\" \"stdin\" \"ready\" \"timeoutMs\" 50)) \"closeOnSuccess\" #t))) (yinz-then (hash-ref sequence \"done\") (lambda (r) (set! result r)))");
        h.pane_event("CommandPaneOpened", json!(0), None);
        h.pane_event(
            if closed {
                "PaneClosed"
            } else {
                "CommandPaneExited"
            },
            json!(0),
            None,
        );
        h.advance(100);
        assert_eq!(
            h.value("result")["status"],
            if closed { "pane_closed" } else { "success" }
        );
        assert_eq!(h.sent("CloseTerminalPane").len(), usize::from(!closed));
        assert!(h.sent("SendSigkillToPaneId").is_empty());
        assert_eq!(h.sent("RenameTerminalPane"), vec![json!([7, "named"])]);
    }
}

#[test]
fn pane_exit_observes_metadata_initial_state_and_restores_originating_client() {
    for (info, expected) in [
        (
            json!({"id":7,"is_plugin":false,"exited":true,"exit_status":23}),
            json!({"paneId":{"Terminal":7},"exitCode":23,"closed":false}),
        ),
        (
            Value::Null,
            json!({"paneId":{"Terminal":7},"exitCode":null,"closed":true}),
        ),
    ] {
        let mut h = Harness::new("(define result json-null)");
        *h.info.lock().unwrap() = info;
        h.eval("(yinz-then (yinz-wait-for-pane-exit 7) (lambda (r) (set! result r)))");
        assert_eq!(h.value("result"), expected);
    }
    let mut h = Harness::new("(define result json-null)");
    h.eval("(define pending (yinz-wait-for-pane-exit (hash \"Terminal\" 7))) (yinz-then pending (lambda (r) (set! result r) (yinz-command \"GetPluginIds\")))");
    for panes in [
        json!([]),
        json!([{"id":8,"is_plugin":false,"exited":true},{"id":7,"is_plugin":true,"exited":true}]),
        json!([{"id":7,"is_plugin":false,"exited":false}]),
    ] {
        h.event(json!({"PaneUpdate":{"panes":{"0":panes}}}));
        assert!(h.value("result").is_null());
    }
    h.event(json!({"PaneUpdate":{"panes":{"0":[{"id":7,"is_plugin":false,"exited":true,"exit_status":null}]}}}));
    assert_eq!(
        h.value("result"),
        json!({"paneId":{"Terminal":7},"exitCode":null,"closed":false})
    );
    h.pane_event("PaneClosed", json!(0), None);
    assert!(h
        .commands
        .lock()
        .unwrap()
        .iter()
        .all(|(_, actor)| actor == 2));
    h.runtime
        .invoke("yinz-async-enter", vec![json!(2)])
        .unwrap();
    h.eval("(yinz-command (hash \"Unsubscribe\" '(\"CommandPaneOpened\" \"CommandPaneReRun\" \"CommandPaneExited\" \"PaneClosed\" \"PaneUpdate\")))");
    assert_eq!(
        h.sent("Unsubscribe")
            .last()
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        5
    );
}

#[test]
fn run_in_pane_cancellation_timeout_dispatch_failures_and_inherited_target_cwd() {
    let mut h = Harness::new("(define errors '()) (define result json-null)");
    h.eval("(define pending (yinz-run-in-pane 7 (hash \"path\" \"cat\" \"stdin\" \"hello\"))) (yinz-catch pending (lambda (e) (set! errors (cons e errors)))) (yinz-cancel! pending)");
    h.pane_event("CommandPaneReRun", json!(0), None);
    assert!(h.sent("WriteCharsToPaneId").is_empty());
    assert!(h.sent("SendSigintToPaneId").is_empty());
    h.eval("(define next (yinz-run-in-pane 7 (hash \"path\" \"cat\" \"timeoutMs\" 10))) (yinz-catch next (lambda (e) (set! errors (cons e errors))))");
    h.advance(20);
    assert_eq!(h.value("errors").as_array().unwrap().len(), 1);
    h.pane_event("CommandPaneReRun", json!(0), None);
    h.advance(10);
    assert!(h.value("errors")[0]
        .as_str()
        .unwrap()
        .contains("pane wait timed out"));
    *h.info.lock().unwrap() = json!({"cwd":"/latest","target":{"Ssh":{"host":"host"}}});
    h.runtime
        .invoke("yinz-async-enter", vec![json!(2)])
        .unwrap();
    h.eval("(yinz-then (yinz-run-in-pane 7 (hash \"path\" \"pwd\" \"cwd\" \"inherit\" \"target\" (hash \"Ssh\" (hash \"host\" \"host\")) \"remote_host\" \"legacy\")) (lambda (r) (set! result r)))");
    assert_eq!(
        h.sent("RunCommandInPane").last().unwrap()["command"]["cwd"],
        "/latest"
    );
    assert_eq!(
        h.sent("RunCommandInPane").last().unwrap()["command"]["remote_host"],
        "legacy"
    );
    h.pane_event(
        "CommandPaneExited",
        json!(4),
        Some(json!({"__yinz_pane_step":"old"})),
    );
    assert!(h.value("result").is_null());
    h.pane_event("CommandPaneExited", json!(4), None);
    assert_eq!(h.value("result")["exitCode"], 4);
    for command in [
        "Subscribe",
        "OpenCommandPane",
        "RunCommandInPane",
        "WriteCharsToPaneId",
        "SetTimeout",
    ] {
        let mut h = Harness::new("(define failure #f)");
        if matches!(command, "Subscribe" | "OpenCommandPane") {
            *h.failure.lock().unwrap() = Some(command.into());
        }
        h.eval("(define sequence (yinz-pane-sequence (hash \"steps\" (list (hash \"path\" \"cat\" \"stdin\" \"hello\" \"timeoutMs\" 10) (hash \"path\" \"true\"))))) (yinz-catch (hash-ref sequence \"done\") (lambda (error) (set! failure error)))");
        *h.failure.lock().unwrap() = Some(command.into());
        if command == "RunCommandInPane" {
            h.pane_event("CommandPaneExited", json!(0), None);
        } else if !matches!(command, "Subscribe" | "OpenCommandPane") {
            h.pane_event("CommandPaneOpened", json!(0), None);
        }
        assert_eq!(h.value("failure"), format!("{command} dispatch failed"));
    }
}

#[test]
fn pane_exit_cancel_query_failure_and_live_exit_release_all_observers() {
    for outcome in ["cancel", "query", "event"] {
        let mut h = Harness::new("(define result json-null) (define failure #f)");
        if outcome == "query" {
            *h.failure.lock().unwrap() = Some("GetPaneInfo".into());
        }
        h.eval("(define waiting (yinz-wait-for-pane-exit 7)) (yinz-catch (yinz-then waiting (lambda (r) (set! result r))) (lambda (e) (set! failure e)))");
        if outcome == "cancel" {
            h.eval("(yinz-cancel! waiting) (yinz-cancel! waiting)");
        }
        h.pane_event("CommandPaneExited", json!(31), Some(json!({})));
        h.event(json!({"PaneUpdate":{"panes":{"0":[{"id":7,"is_plugin":false,"exited":true,"exit_status":32}]}}}));
        h.pane_event("PaneClosed", json!(0), None);
        *h.failure.lock().unwrap() = None;
        h.eval("(yinz-command (hash \"Unsubscribe\" '(\"CommandPaneOpened\" \"CommandPaneReRun\" \"CommandPaneExited\" \"PaneClosed\" \"PaneUpdate\")))");
        assert_eq!(
            h.sent("Unsubscribe")
                .last()
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            5
        );
        match outcome {
            "cancel" => assert_eq!(h.value("failure"), "operation cancelled"),
            "query" => assert_eq!(h.value("failure"), "GetPaneInfo dispatch failed"),
            _ => assert_eq!(h.value("result")["exitCode"], 31),
        }
        assert!(h.sent("SendSigintToPaneId").is_empty());
    }
}

#[test]
fn sequence_exit_during_cancellation_stops_escalation_and_closes_once() {
    let mut h = Harness::new("(define result json-null)");
    h.eval("(define sequence (yinz-pane-sequence (hash \"steps\" (list (hash \"path\" \"cat\")) \"killGraceMs\" 10 \"onCancel\" \"close\"))) (yinz-then (hash-ref sequence \"done\") (lambda (r) (set! result r)))");
    h.pane_event("CommandPaneOpened", json!(0), None);
    h.eval("((hash-ref sequence \"cancel\"))");
    h.pane_event("CommandPaneExited", json!(130), None);
    h.advance(100);
    assert_eq!(h.value("result")["status"], "cancelled");
    assert_eq!(h.value("result")["exitCodes"], json!([130]));
    assert_eq!(h.sent("CloseTerminalPane"), vec![json!(7)]);
    assert!(h.sent("SendSigkillToPaneId").is_empty());
    assert!(h
        .commands
        .lock()
        .unwrap()
        .iter()
        .all(|(_, actor)| actor == 2));
}

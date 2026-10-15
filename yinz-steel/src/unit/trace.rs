use crate::Runtime;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

fn runtime(source: &str) -> (Runtime, Arc<Mutex<Vec<Value>>>) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let commands = sent.clone();
    let mut runtime = Runtime::new_with_clock(
        move |command| {
            commands.lock().unwrap().push(command);
            Ok(Value::Null)
        },
        || 0,
    )
    .unwrap();
    runtime.eval(source).unwrap();
    (runtime, sent)
}
fn actor(runtime: &mut Runtime, client: u32) {
    runtime
        .invoke("yinz-async-enter", vec![json!(client)])
        .unwrap();
}
#[test]
fn trace_configuration_and_explicit_toggles_are_instance_local() {
    let source = "(define (load config) (yinz-command \"GetPluginIds\"))";
    let (mut runtime, _) = runtime(source);
    actor(&mut runtime, 1);
    runtime.load(json!({})).unwrap();
    runtime.load(json!({"trace":"false"})).unwrap();
    assert!(runtime.take_logs().unwrap().is_empty());
    actor(&mut runtime, 2);
    runtime.load(json!({"trace":"true"})).unwrap();
    runtime.eval("(yinz-trace #f)").unwrap();
    actor(&mut runtime, 3);
    runtime.load(json!({"trace":"other"})).unwrap();
    runtime.eval("(yinz-trace)").unwrap();
    actor(&mut runtime, 4);
    runtime.load(json!({"trace":"other"})).unwrap();
    for invalid in ["json-null", "1", "\"true\"", "(hash)", "(lambda () #t)"] {
        assert!(runtime.eval(&format!("(yinz-trace {invalid})")).is_err());
    }
    let logs = runtime.take_logs().unwrap();
    let markers: Vec<_> = logs
        .lines()
        .map(|line| line.split(" client=").nth(1).unwrap())
        .collect();
    assert_eq!(
        markers,
        ["2 command \"GetPluginIds\"", "4 command \"GetPluginIds\""]
    );
    let id = runtime.invoke("yinz-instance-id", vec![]).unwrap();
    assert!(logs
        .lines()
        .all(|line| line.starts_with(&format!("trace instance={} ", id.as_str().unwrap()))));
    let (mut replacement, _) = self::runtime(source);
    actor(&mut replacement, 1);
    replacement.load(json!({})).unwrap();
    assert!(replacement.take_logs().unwrap().is_empty());
}
#[test]
fn trace_keeps_payloads_private_and_restores_async_actors() {
    let (mut runtime, _) = runtime(
        r#"
      (define outcomes '())
      (define (record outcome) (set! outcomes (append outcomes (list outcome))))
      (define (load config)
        (yinz-command "GetPluginIds")
        (yinz-command (hash "RenameTab" '(0 "SECRET")))
        (with-handler (lambda (_) (void)) (yinz-command (hash "SECRET" (lambda () #t))))
        (yinz-then (yinz-wait-for-pipe "done") (lambda (_) (record "finished")))
        (define cancelled (yinz-wait-for-event "Key"))
        (yinz-catch cancelled (lambda (_) (record "cancelled"))) (yinz-cancel! cancelled)
        (yinz-catch (yinz-wait-for-pipe "bad" (lambda (_) (error "SECRET"))) (lambda (_) (record "failed")))
        (yinz-after 0 (lambda () (yinz-command "GetPluginIds")))
        (void))
    "#,
    );
    actor(&mut runtime, 7);
    runtime.load(json!({"trace":"true"})).unwrap();
    actor(&mut runtime, 9);
    for name in ["done", "bad", "yinz_rpc_reply", "llm_complete_result"] {
        runtime
            .pipe(json!({"name":name,"payload":"SECRET","source":{"Plugin":7}}))
            .unwrap();
    }
    runtime
        .pipe(json!({"name":format!("a\n\u{1b}\u{85}{}", "x".repeat(250)),"payload":"SECRET"}))
        .unwrap();
    runtime.update(json!({"Timer":0.0})).unwrap();
    assert_eq!(
        runtime.value("outcomes").unwrap(),
        json!(["cancelled", "finished", "failed"])
    );
    let logs = runtime.take_logs().unwrap();
    assert!(!logs.contains("SECRET"), "{logs}");
    assert_eq!(logs.matches("client=7 command \"GetPluginIds\"").count(), 2);
    assert!(logs.contains("client=7 command invalid"));
    assert!(logs.contains("client=9 pipe \"yinz_rpc_reply\""));
    assert!(logs.contains("client=9 pipe \"llm_complete_result\""));
    assert!(logs.contains("client=9 event \"Timer\""));
    assert!(logs
        .lines()
        .any(|line| line.contains("client=7 operation ") && line.ends_with(" cancelled")));
    assert!(logs
        .lines()
        .any(|line| line.contains("client=7 operation ") && line.ends_with(" failed")));
    assert!(logs.contains("a\\n\\u001b\\u0085"));
    assert!(!logs.contains(&"x".repeat(200)));
    assert!(!logs.contains(['\u{1b}', '\u{85}']));
}
#[test]
fn logging_stays_out_of_render_frames_and_has_an_independent_budget() {
    let (mut runtime, _) = runtime(
        r#"
      (define (render rows columns) (yinz-log "diagnostic") (display "prefix ") "frame")
    "#,
    );
    assert_eq!(runtime.render(1, 20).unwrap(), "prefix frame");
    assert_eq!(runtime.take_logs().unwrap(), "diagnostic\n");
    assert!(runtime.take_logs().unwrap().is_empty());
    assert!(runtime
        .eval("(yinz-log (make-string 4194305 #\\x))")
        .is_err());
    assert!(runtime.take_logs().unwrap().len() <= 4 * 1024 * 1024);
    assert_eq!(
        runtime
            .eval("(display \"still available\")")
            .unwrap()
            .output,
        "still available"
    );
}
#[test]
fn trace_watch_disposal_and_disabled_completions_preserve_cleanup() {
    let mut runtime = Runtime::new(|command| {
        let command = command
            .pointer("/__yinz_runtime/command")
            .unwrap_or(&command);
        Ok(if command.get("DataWatch").is_some() {
            json!({"watch_id":17,"rows":{"columns":[],"rows":[]}})
        } else {
            Value::Null
        })
    })
    .unwrap();
    runtime.eval(r#"
      (define pending-wait #f) (define watch #f)
      (define (load config)
        (yinz-trace)
        (set! watch (yinz-data-watch-callback "shared:test"
          (hash "entity" "items" "sort" '() "select" '() "limit" 100 "offset" 0) (lambda (rows event) #f)))
        (set! pending-wait (yinz-wait-for-pipe "done")) (yinz-catch pending-wait (lambda (_) #f)) (void))
    "#).unwrap();
    actor(&mut runtime, 5);
    runtime.load(json!({})).unwrap();
    runtime.eval("((hash-ref watch \"dispose\")) ((hash-ref watch \"dispose\")) (yinz-trace #f) (yinz-cancel! pending-wait)").unwrap();
    runtime.invoke("yinz-async-poll", vec![]).unwrap();
    let logs = runtime.take_logs().unwrap();
    for command in ["DataWatch", "DataUnwatch", "Subscribe"] {
        assert_eq!(
            logs.matches(&format!("client=5 command \"{command}\""))
                .count(),
            1
        );
    }
    assert!(!logs.lines().any(|line| line.ends_with("cancelled")));
}

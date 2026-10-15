use crate::Runtime;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

fn runtime(source: &str) -> (Runtime, Arc<Mutex<Vec<Value>>>) {
    let commands = Arc::new(Mutex::new(Vec::new()));
    let sent = commands.clone();
    let mut runtime = Runtime::new(move |command| {
        sent.lock().unwrap().push(command);
        Ok(Value::Null)
    })
    .unwrap();
    runtime.eval(source).unwrap();
    runtime.invoke("yinz-async-enter", vec![json!(1)]).unwrap();
    (runtime, commands)
}

#[test]
fn deferred_command_replies_are_correlated_and_restore_the_originating_client() {
    let (mut runtime, commands) = runtime(
        r#"
      (define text "waiting")
      (define (load config)
        (yinz-then (yinz-run-command (hash "command" (list "example") "context" (hash "user" "value")))
          (lambda (reply)
            (set! text (yinz-decode-utf8 (hash-ref reply "stdout")))
            (yinz-command "GetPluginIds")
            #t)))
      (define (render rows columns) text)
    "#,
    );
    runtime.load(json!({})).unwrap();
    let token = commands
        .lock()
        .unwrap()
        .iter()
        .find_map(|command| {
            command
                .pointer("/__yinz_runtime/command/RunCommand/3/__yinz_async")
                .cloned()
        })
        .unwrap();
    runtime.invoke("yinz-async-enter", vec![json!(2)]).unwrap();
    runtime
        .update(json!({"RunCommandResult": [0, [88], [], {"__yinz_async": "wrong"}]}))
        .unwrap();
    assert_eq!(runtime.render(1, 40).unwrap(), "waiting");
    runtime.update(json!({"RunCommandResult": [0, [111, 107], [], {"__yinz_async": token, "user": "value"}]})).unwrap();
    assert_eq!(runtime.render(1, 40).unwrap(), "ok");
    let commands = commands.lock().unwrap();
    let last = commands.last().unwrap();
    assert_eq!(
        last.pointer("/__yinz_runtime/command"),
        Some(&json!("GetPluginIds"))
    );
    assert_eq!(last.pointer("/__yinz_runtime/client"), Some(&json!(1)));
}

#[test]
fn failed_cleanup_still_notifies_observers_and_restores_their_actor() {
    let (mut runtime, _) = runtime(
        r#"
        (define outcome json-null)
        (define pending (yinz-deferred))
        (vector-set! pending 4 (lambda () (error "cleanup failed")))
        (yinz-catch pending (lambda (failure) (set! outcome (list failure %async-client))))
        "#,
    );
    runtime.invoke("yinz-async-enter", vec![json!(9)]).unwrap();
    runtime.eval("(yinz-cancel! pending)").unwrap();
    assert!(runtime
        .invoke("yinz-async-poll", vec![])
        .unwrap_err()
        .to_string()
        .contains("cleanup failed"));
    assert_eq!(
        runtime.value("outcome").unwrap(),
        json!(["operation cancelled", false])
    );
    assert_eq!(runtime.value("%async-client").unwrap(), json!(9));
    assert!(!runtime
        .invoke("yinz-async-poll", vec![])
        .unwrap()
        .as_bool()
        .unwrap());
}

#[test]
fn rejected_imported_callback_keeps_source_location_through_deferred_chains() {
    let dependency =
        "(provide explode)\n(define (explode ignored)\n  (error \"dependency failure\"))\n";
    let mut root = crate::modules::SourceRoot::embedded(
        "main.scm",
        std::collections::BTreeMap::from([("dependency.scm".to_owned(), dependency.to_owned())]),
    )
    .unwrap();
    let mut runtime = Runtime::new(Ok).unwrap();
    runtime.eval_with_source_root(
        "(require \"./dependency.scm\") (define (load config) (yinz-then (yinz-then (yinz-sleep 0) explode) (lambda (value) value)))",
        &mut root,
    ).unwrap();
    runtime.load(json!({})).unwrap();
    let error = runtime
        .update(json!({"Timer": 0.0}))
        .unwrap_err()
        .to_string();
    assert!(error.contains("dependency.scm:3:"), "{error}");
    assert!(error.contains("dependency failure"), "{error}");
}

#[test]
fn named_entry_callbacks_keep_diagnostics_without_a_real_filesystem_path() {
    let source = "(define (load config)\n  (yinz-then (yinz-sleep 0)\n    (lambda (ignored) (error \"entry failure\"))))";
    for from_root in [false, true] {
        let mut runtime = Runtime::new(Ok).unwrap();
        if from_root {
            let mut root = crate::modules::SourceRoot::embedded(
                "virtual/main.scm",
                std::collections::BTreeMap::new(),
            )
            .unwrap();
            runtime.eval_with_source_root(source, &mut root).unwrap();
        } else {
            runtime
                .eval_with_path(
                    source,
                    Some(std::path::Path::new("/nonexistent/virtual/main.scm")),
                )
                .unwrap();
        }
        runtime.load(json!({})).unwrap();
        let error = runtime
            .update(json!({"Timer": 0.0}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("main.scm:3:"), "{error}");
        assert!(error.contains("entry failure"), "{error}");
    }
}

#[test]
fn permission_waits_require_private_completion_and_timers_resume_deferred_hooks() {
    let (mut runtime, commands) = runtime(
        r#"
      (define text "waiting")
      (define (load config)
        (yinz-then (yinz-wait-for-permissions (list "RunCommands"))
          (lambda (ignored)
            (set! text "granted")
            (yinz-then (yinz-sleep 0) (lambda (ignored) (set! text "awake") #t)))))
      (define (render rows columns) text)
    "#,
    );
    runtime.load(json!({})).unwrap();
    runtime
        .update(json!({"PermissionRequestResult": "Granted"}))
        .unwrap();
    assert_eq!(runtime.render(1, 40).unwrap(), "waiting");
    let id = commands
        .lock()
        .unwrap()
        .iter()
        .find_map(|command| {
            command
                .pointer("/__yinz_runtime/permission_id")
                .filter(|id| id.is_string())
                .cloned()
        })
        .unwrap();
    runtime
        .invoke("yinz-async-permission", vec![id, json!({"Ok": "Granted"})])
        .unwrap();
    assert_eq!(runtime.render(1, 40).unwrap(), "granted");
    assert!(runtime.update(json!({"Timer": 0.0})).unwrap());
    assert_eq!(runtime.render(1, 40).unwrap(), "awake");
}

#[test]
fn worker_fifo_remains_busy_until_deferred_settles_and_recovers_after_job_errors() {
    let (mut runtime, _) = runtime(
        r#"
      (define completed '())
      (yinz-register-workers (hash "probe"
        (lambda (name payload)
          (when (equal? name "error") (error "expected worker failure"))
          (yinz-then (yinz-wait-for-event "CustomMessage")
            (lambda (message) (set! completed (append completed (list payload))))))))
    "#,
    );
    runtime
        .invoke("yinz-worker-bind", vec![json!("probe")])
        .unwrap();
    runtime
        .invoke("yinz-worker-message", vec![json!("job"), json!("first")])
        .unwrap();
    assert_eq!(
        runtime.invoke("yinz-worker-ready", vec![]).unwrap(),
        json!(false)
    );
    runtime
        .invoke(
            "yinz-worker-event",
            vec![json!({"CustomMessage": ["reply", ""]})],
        )
        .unwrap();
    assert_eq!(
        runtime.invoke("yinz-worker-ready", vec![]).unwrap(),
        json!(true)
    );
    assert_eq!(runtime.value("completed").unwrap(), json!(["first"]));
    runtime
        .invoke("yinz-worker-message", vec![json!("error"), json!("")])
        .unwrap();
    assert!(runtime
        .take_logs()
        .unwrap()
        .contains("expected worker failure"));
    assert_eq!(
        runtime.invoke("yinz-worker-ready", vec![]).unwrap(),
        json!(true)
    );
    runtime
        .invoke("yinz-worker-message", vec![json!("job"), json!("second")])
        .unwrap();
    runtime
        .invoke(
            "yinz-worker-event",
            vec![json!({"CustomMessage": ["reply", ""]})],
        )
        .unwrap();
    assert_eq!(
        runtime.value("completed").unwrap(),
        json!(["first", "second"])
    );
}

#[test]
fn unload_waits_for_replies_and_stops_ordinary_lifecycle_callbacks() {
    let (mut runtime, _) = runtime(
        r#"
      (define updates 0)
      (define finished #f)
      (define (update event) (set! updates (+ updates 1)) #f)
      (define (unload)
        (yinz-then (yinz-wait-for-event "CustomMessage")
          (lambda (message) (set! finished #t))))
    "#,
    );
    runtime.unload().unwrap();
    assert_eq!(
        runtime.invoke("yinz-async-unloaded", vec![]).unwrap(),
        json!(false)
    );
    runtime
        .update(json!({"CustomMessage": ["cleanup", ""]}))
        .unwrap();
    assert_eq!(
        runtime.invoke("yinz-async-unloaded", vec![]).unwrap(),
        json!(true)
    );
    assert_eq!(runtime.value("finished").unwrap(), json!(true));
    assert_eq!(runtime.value("updates").unwrap(), json!(0));
}

#[test]
fn custom_cancel_waits_for_its_reply_but_teardown_always_releases_the_operation() {
    let (mut runtime, _) = runtime(
        r#"
      (define pending #f) (define cancellations '()) (define cleanups 0)
      (define (load config)
        (set! pending (yinz-deferred))
        (%async-set-cancel! pending (lambda () (set! cancellations (cons %async-client cancellations))))
        (vector-set! pending 4 (lambda () (set! cleanups (+ cleanups 1)))) (void))
    "#,
    );
    runtime.load(json!({})).unwrap();
    runtime.invoke("yinz-async-enter", vec![json!(9)]).unwrap();
    runtime.eval("(yinz-cancel! pending)").unwrap();
    assert_eq!(runtime.value("cancellations").unwrap(), json!([1]));
    assert_eq!(
        runtime.eval("(yinz-pending? pending)").unwrap().values,
        ["#true"]
    );
    runtime.invoke("yinz-async-cancel", vec![]).unwrap();
    assert_eq!(runtime.value("cancellations").unwrap(), json!([1]));
    assert_eq!(runtime.value("cleanups").unwrap(), json!(1));
    assert_eq!(
        runtime.eval("(yinz-pending? pending)").unwrap().values,
        ["#false"]
    );
}

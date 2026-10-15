use crate::Runtime;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

type Commands = Arc<Mutex<Vec<Value>>>;
fn runtime(source: &str) -> (Runtime, Commands, Arc<AtomicU64>) {
    let commands = Arc::new(Mutex::new(Vec::new()));
    let sent = commands.clone();
    let now = Arc::new(AtomicU64::new(0));
    let clock = now.clone();
    let mut runtime = Runtime::new_with_clock(
        move |command| {
            sent.lock().unwrap().push(command.clone());
            let wire = command.pointer("/__yinz_runtime/command").unwrap_or(&command);
            let _: yinz_types::data::PluginCommand = crate::binary::decode_command(wire.clone())
                .unwrap_or_else(|error| panic!("invalid command {wire}: {error}"));
            if wire.get("DataWatch").is_some() {
                Ok(json!({"watch_id":17,"rows":{"columns":[],"rows":[],"truncated":false,"next_cursor":null}}))
            } else {
                Ok(Value::Null)
            }
        },
        move || clock.load(Ordering::SeqCst),
    ).unwrap();
    runtime.eval(source).unwrap();
    runtime.invoke("yinz-async-enter", vec![json!(2)]).unwrap();
    (runtime, commands, now)
}
fn command(commands: &Commands, name: &str) -> Value {
    commands
        .lock()
        .unwrap()
        .iter()
        .find_map(|c| {
            c.pointer(&format!("/__yinz_runtime/command/{name}"))
                .cloned()
        })
        .unwrap()
}
#[test]
fn command_streams_restore_origin_and_cancel_the_underlying_subscription() {
    let (mut runtime, commands, _) = runtime(
        r#"
      (define output '()) (define result json-null) (define request #f)
      (define (load config)
        (set! request (yinz-run-command (hash "command" '("echo" "hello") "context" (hash "user" "value")
          "onOutput" (lambda (chunk) (set! output (append output (list (list chunk %async-client))))))))
        (yinz-catch (yinz-then request (lambda (reply) (set! result reply)))
          (lambda (failure) (set! result failure))))
    "#,
    );
    runtime.load(json!({})).unwrap();
    let call = command(&commands, "RunCommand");
    assert_eq!(call[5], true);
    let context = call[3].clone();
    runtime.invoke("yinz-async-enter", vec![json!(9)]).unwrap();
    runtime.update(json!({"RunCommandOutput":{"data":[120],"stderr":false,"context":{"__yinz_async":"wrong"}}})).unwrap();
    runtime
        .update(
            json!({"RunCommandOutput":{"data":[240,159,152,128],"stderr":false,"context":context}}),
        )
        .unwrap();
    assert_eq!(
        runtime.value("output").unwrap(),
        json!([[{"data":[240,159,152,128],"stderr":false,"context":{"user":"value"}},2]])
    );
    runtime.eval("(yinz-cancel! request)").unwrap();
    runtime.invoke("yinz-async-poll", vec![]).unwrap();
    runtime
        .update(json!({"RunCommandResult":[0,[111,107],[],context]}))
        .unwrap();
    runtime
        .update(json!({"RunCommandOutput":{"data":[120],"stderr":false,"context":context}}))
        .unwrap();
    assert_eq!(
        runtime.value("output").unwrap().as_array().unwrap().len(),
        1
    );
    assert_eq!(runtime.value("result").unwrap(), "operation cancelled");
    assert_eq!(runtime.eval("(list (length (hash-keys->list %async-events)) (length (hash-keys->list %async-handlers)))").unwrap().values,["(0 0)"]);
    runtime
        .eval("(yinz-command (hash \"Unsubscribe\" '(\"RunCommandResult\" \"RunCommandOutput\")))")
        .unwrap();
    assert_eq!(
        commands
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .pointer("/__yinz_runtime/command/Unsubscribe"),
        Some(&json!(["RunCommandResult", "RunCommandOutput"]))
    );
}
#[test]
fn command_and_web_replies_keep_context_and_nonzero_results() {
    let (mut runtime, commands, _) = runtime(
        r#"
      (define results '())
      (define (load config)
        (yinz-then (yinz-run-command (hash "command" '("false")))
          (lambda (reply) (set! results (append results (list reply)))))
        (yinz-then (yinz-web-request (hash "url" "https://example.invalid" "verb" "Post" "body" "日" "streamId" "stream" "proxy" "http://proxy"))
          (lambda (reply) (set! results (append results (list reply))))))
    "#,
    );
    runtime.load(json!({})).unwrap();
    let run = command(&commands, "RunCommand");
    let web = command(&commands, "WebRequest");
    assert_eq!(crate::binary::decode(&web[3]).unwrap(), "日".as_bytes());
    assert_eq!(web[5], "stream");
    assert_eq!(web[6], "http://proxy");
    runtime
        .update(json!({"RunCommandResult":[7,[1],[2],run[3]]}))
        .unwrap();
    runtime
        .update(json!({"WebRequestResult":[500,{"x":"y"},[3],web[4]]}))
        .unwrap();
    assert_eq!(
        runtime.value("results").unwrap(),
        json!([
            {"exit-code":7,"stdout":[1],"stderr":[2],"context":{}},
            {"status":500,"headers":{"x":"y"},"body":[3],"context":{}}
        ])
    );
}
#[test]
fn recurring_timers_wait_for_async_callbacks_and_retain_one_subscription() {
    let (mut runtime, commands, now) = runtime(
        r#"
      (define count 0) (define pending #f) (define timer #f)
      (define (load config)
        (set! timer (yinz-every 10 (lambda ()
          (set! count (+ count 1)) (set! pending (yinz-deferred)) pending))))
    "#,
    );
    runtime.load(json!({})).unwrap();
    now.store(10, Ordering::SeqCst);
    runtime.update(json!({"Timer":0.0})).unwrap();
    assert_eq!(runtime.value("count").unwrap(), 1);
    now.store(100, Ordering::SeqCst);
    runtime.update(json!({"Timer":0.0})).unwrap();
    assert_eq!(runtime.value("count").unwrap(), 1);
    runtime.eval("(yinz-resolve! pending #t)").unwrap();
    assert_eq!(runtime.invoke("yinz-async-poll", vec![]).unwrap(), true);
    now.store(109, Ordering::SeqCst);
    runtime.update(json!({"Timer":0.0})).unwrap();
    assert_eq!(runtime.value("count").unwrap(), 1);
    now.store(110, Ordering::SeqCst);
    runtime.update(json!({"Timer":0.0})).unwrap();
    assert_eq!(runtime.value("count").unwrap(), 2);
    runtime
        .eval("(yinz-cancel! timer) (yinz-resolve! pending #f)")
        .unwrap();
    runtime.invoke("yinz-async-poll", vec![]).unwrap();
    assert_eq!(
        runtime
            .eval("(length (hash-keys->list %async-timers))")
            .unwrap()
            .values,
        ["0"]
    );
    assert_eq!(
        commands
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.pointer("/__yinz_runtime/command/Subscribe") == Some(&json!(["Timer"])))
            .count(),
        1
    );
}
#[test]
fn data_watch_callbacks_and_disposal_keep_the_owner_and_ignore_late_events() {
    let (mut runtime, commands, _) = runtime(
        r#"
      (define changes '()) (define watch #f)
      (define (load config)
        (set! watch (yinz-data-watch-callback "shared:yinz"
          (hash "entity" "items" "sort" '() "select" '() "limit" 100 "offset" 0)
          (lambda (rows event) (set! changes (append changes (list %async-client))) #t))))
    "#,
    );
    runtime.load(json!({})).unwrap();
    runtime.invoke("yinz-async-enter", vec![json!(9)]).unwrap();
    runtime
        .update(json!({"PluginDataChanged":{"watch_id":18,"rows":{},"truncated":false}}))
        .unwrap();
    assert!(runtime
        .update(json!({"PluginDataChanged":{"watch_id":17,"rows":{},"truncated":false}}))
        .unwrap());
    assert_eq!(runtime.value("changes").unwrap(), json!([2]));
    runtime
        .eval("((hash-ref watch \"dispose\")) ((hash-ref watch \"dispose\"))")
        .unwrap();
    runtime
        .update(json!({"PluginDataChanged":{"watch_id":17,"rows":{},"truncated":false}}))
        .unwrap();
    assert_eq!(runtime.value("changes").unwrap(), json!([2]));
    let commands = commands.lock().unwrap();
    let calls: Vec<_> = commands
        .iter()
        .filter(|c| c.pointer("/__yinz_runtime/command/DataUnwatch").is_some())
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].pointer("/__yinz_runtime/client"), Some(&json!(2)));
}
#[test]
fn default_keybindings_replace_handlers_and_require_host_keybind_source() {
    let (mut runtime, commands, _) = runtime(
        r#"
      (define selected 0)
      (define (load config)
        (yinz-register-default-keybindings (list
          (hash "mode" "normal" "key" "Ctrl a" "handler" (lambda (message) (set! selected (+ selected 1)) #t))
          (hash "mode" "locked" "key" "Alt x" "actions"
            (list (hash "name" "NewPane" "args" '("right") "props" (hash "name" "a\"b" "z" #t)))))))
    "#,
    );
    runtime.load(json!({})).unwrap();
    assert_eq!(command(&commands,"RegisterDefaultKeybindings"),"keybinds clear-defaults=true { normal { bind \"Ctrl a\" { MessagePlugin { name \"__yinz_default_keybinding:1\"; }; }; }; locked { bind \"Alt x\" { NewPane \"right\" name=\"a\\\"b\" z=true; }; }; }");
    for source in [json!({"Plugin":7}), json!({"Cli":"pipe"})] {
        assert_eq!(
            runtime
                .invoke(
                    "yinz-keybinding-dispatch",
                    vec![json!({"name":"__yinz_default_keybinding:1","source":source})]
                )
                .unwrap(),
            true
        );
    }
    assert_eq!(runtime.value("selected").unwrap(), 0);
    runtime
        .invoke(
            "yinz-keybinding-dispatch",
            vec![json!({"name":"__yinz_default_keybinding:1","source":"Keybind"})],
        )
        .unwrap();
    assert_eq!(runtime.invoke("yinz-async-poll", vec![]).unwrap(), true);
    assert_eq!(runtime.value("selected").unwrap(), 1);
    runtime
        .eval("(yinz-register-default-keybindings '())")
        .unwrap();
    runtime
        .invoke(
            "yinz-keybinding-dispatch",
            vec![json!({"name":"__yinz_default_keybinding:1","source":"Keybind"})],
        )
        .unwrap();
    assert_eq!(runtime.value("selected").unwrap(), 1);
}

#[test]
fn integer_wire_fields_reject_inexact_scheme_numbers_before_dispatch() {
    let (mut runtime, commands, _) = runtime(
        r#"
      (define failures '())
      (define (load config)
        (for-each (lambda (operation)
          (yinz-catch operation (lambda (failure) (set! failures (cons failure failures)))))
          (list (yinz-run-command (hash "command" '("true") "maxBytesReturned" 1.0))
                (yinz-web-request (hash "url" "https://example.invalid" "body" '(1.0))))))
    "#,
    );
    runtime.load(json!({})).unwrap();
    assert_eq!(
        runtime.value("failures").unwrap().as_array().unwrap().len(),
        2
    );
    assert!(commands.lock().unwrap().is_empty());
}

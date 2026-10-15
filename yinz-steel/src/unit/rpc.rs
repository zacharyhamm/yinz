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
            sent.lock().unwrap().push(command);
            Ok(Value::Null)
        },
        move || clock.load(Ordering::SeqCst),
    )
    .unwrap();
    runtime.eval(source).unwrap();
    runtime.invoke("yinz-async-enter", vec![json!(2)]).unwrap();
    (runtime, commands, now)
}

fn sent(commands: &Commands, name: &str) -> Vec<(Value, Value, Value)> {
    commands
        .lock()
        .unwrap()
        .iter()
        .filter_map(|command| {
            let actor = command
                .pointer("/__yinz_runtime/client")
                .cloned()
                .unwrap_or(Value::Null);
            let command = command
                .pointer("/__yinz_runtime/command")
                .unwrap_or(command);
            let message = command.get("MessageToPlugin")?;
            (message["message_name"] == name).then(|| {
                (
                    serde_json::from_str(message["message_payload"].as_str().unwrap()).unwrap(),
                    actor,
                    message["destination_plugin_id"].clone(),
                )
            })
        })
        .collect()
}

fn pipe(runtime: &mut Runtime, name: &str, source: Value, body: Value) {
    runtime
        .pipe(json!({"name":name,"source":source,"payload":serde_json::to_string(&body).unwrap()}))
        .unwrap();
}

#[test]
fn handlers_validate_source_keep_public_pipes_and_restore_async_origin() {
    let (mut runtime, commands, _) = runtime(
        r#"
      (define pipes 0)
      (define waiting #f)
      (define report #f)
      (define (pipe message) (set! pipes (+ pipes 1)) #f)
      (yinz-rpc-handle "echo" (lambda (payload call) "replaced"))
      (yinz-rpc-handle "echo" (lambda (payload call) (hash "payload" payload "source" (hash-ref call "source"))))
      (yinz-rpc-handle "async" (lambda (payload call)
        (set! waiting (yinz-deferred)) (set! report (hash-ref call "progress"))
        (report "waiting") waiting))
      (yinz-rpc-handle "nil" (lambda (payload call) (void)))
      (yinz-rpc-handle "fail" (lambda (payload call) (error "thrown")))
      (yinz-rpc-handle "invalid" (lambda (payload call) (lambda () #t)))
    "#,
    );
    runtime.invoke("yinz-async-enter", vec![json!(3)]).unwrap();
    for (id, name) in ["echo", "async", "nil", "fail", "invalid", "unknown"]
        .into_iter()
        .enumerate()
    {
        pipe(
            &mut runtime,
            "yinz_rpc_call",
            json!({"Plugin":7}),
            json!({"version":1,"call_id":id.to_string(),"name":name,"payload":{"city":"Oslo"},"source":{"Plugin":99},"reply_to":99}),
        );
    }
    pipe(
        &mut runtime,
        "yinz_rpc_call",
        json!("Keybind"),
        json!({"version":1,"call_id":"forged","name":"echo"}),
    );
    pipe(
        &mut runtime,
        "yinz_rpc_call",
        json!({"Plugin":7}),
        json!({"version":2,"call_id":"bad","name":"echo"}),
    );
    pipe(&mut runtime, "ordinary", json!("Keybind"), json!({}));
    runtime.invoke("yinz-async-enter", vec![json!(9)]).unwrap();
    runtime.eval("(yinz-resolve! waiting \"done\")").unwrap();
    runtime.invoke("yinz-async-poll", vec![]).unwrap();
    runtime.eval("(report \"late\")").unwrap();
    let replies = sent(&commands, "yinz_rpc_reply");
    assert_eq!(replies.len(), 6);
    assert!(replies
        .iter()
        .all(|(_, actor, target)| actor == 3 && target == 7));
    let reply = |id: &str| {
        &replies
            .iter()
            .find(|(v, _, _)| v["call_id"] == id)
            .unwrap()
            .0
    };
    assert_eq!(
        reply("0")["result"],
        json!({"payload":{"city":"Oslo"},"source":{"Plugin":7}})
    );
    assert_eq!(reply("1")["result"], "done");
    assert!(reply("2")["result"].is_null());
    for id in ["3", "4", "5"] {
        assert!(reply(id)["error"].is_string());
    }
    assert_eq!(sent(&commands, "yinz_rpc_progress").len(), 1);
    assert_eq!(runtime.value("pipes").unwrap(), 1);
}

#[test]
fn calls_correlate_cancel_timeout_release_timers_and_restore_clients() {
    let (mut runtime, commands, now) = runtime(
        r#"
      (define results '())
      (define requests '())
      (define (record id value) (set! results (append results (list (list id value %async-client)))))
      (define (load config)
        (for-each (lambda (id)
          (define request (yinz-rpc-call (hash "pluginId" 7 "name" "echo" "timeoutMs" 100)))
          (set! requests (append requests (list request)))
          (yinz-catch (yinz-then request (lambda (value) (record id value)))
            (lambda (failure) (record id failure)))) (range 5))
        (yinz-then (yinz-rpc-call (hash "pluginUrl" "file:/plugin.scm" "pluginConfig" (hash "city" "Oslo") "name" "url"))
          (lambda (value) (record "url" value))))
    "#,
    );
    runtime.load(json!({})).unwrap();
    let calls = sent(&commands, "yinz_rpc_call");
    assert_eq!(calls.len(), 6);
    let id = |i: usize| calls[i].0["call_id"].clone();
    runtime.invoke("yinz-async-enter", vec![json!(9)]).unwrap();
    for (source, body) in [
        (json!({"Plugin":8}), json!({"result":"forged"})),
        (json!("Keybind"), json!({"result":"forged"})),
        (json!({"Plugin":7}), json!({"result":"bad","error":"bad"})),
        (json!({"Plugin":7}), json!({"error":7})),
    ] {
        let mut body = body;
        body["version"] = json!(1);
        body["call_id"] = id(0);
        pipe(&mut runtime, "yinz_rpc_reply", source, body);
    }
    for (index, result) in [
        (1, json!({"result":{"city":"Oslo"}})),
        (1, json!({"result":"duplicate"})),
        (0, json!({"error":"remote"})),
        (3, json!({})),
        (5, json!({"result":"url"})),
    ] {
        let mut body = result;
        body["version"] = json!(1);
        body["call_id"] = id(index);
        pipe(
            &mut runtime,
            "yinz_rpc_reply",
            json!({"Plugin": if index == 5 {123} else {7}}),
            body,
        );
    }
    runtime
        .eval("(yinz-cancel! (list-ref requests 2)) (yinz-cancel! (list-ref requests 2))")
        .unwrap();
    pipe(
        &mut runtime,
        "yinz_rpc_reply",
        json!({"Plugin":7}),
        json!({"version":1,"call_id":id(2),"result":"late"}),
    );
    now.store(100, Ordering::SeqCst);
    runtime.update(json!({"Timer":0.0})).unwrap();
    let results = runtime.value("results").unwrap();
    let results = results.as_array().unwrap();
    assert_eq!(results.len(), 6);
    assert!(results.iter().all(|row| row[2] == 2));
    let result = |id: Value| &results.iter().find(|row| row[0] == id).unwrap()[1];
    assert_eq!(result(json!(0)), "remote");
    assert_eq!(result(json!(1)), &json!({"city":"Oslo"}));
    assert!(result(json!(2)).as_str().unwrap().contains("cancelled"));
    assert!(result(json!(3)).is_null());
    assert!(result(json!(4))
        .as_str()
        .unwrap()
        .contains("RPC timed out: echo"));
    assert_eq!(result(json!("url")), "url");
    runtime
        .eval("(yinz-command (hash \"Unsubscribe\" (list \"Timer\")))")
        .unwrap();
    assert_eq!(
        commands
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .pointer("/__yinz_runtime/command/Unsubscribe"),
        Some(&json!(["Timer"]))
    );
}

#[test]
fn progress_is_nonterminal_bounded_and_keeps_the_original_deadline() {
    let (mut runtime, commands, now) = runtime(
        r#"
      (define progress '()) (define result json-null)
      (define (load config)
        (yinz-catch (yinz-rpc-call (hash "pluginId" 7 "name" "work" "timeoutMs" 100)
          (lambda (payload) (set! progress (append progress (list (list payload %async-client))))))
          (lambda (error) (set! result error))))
    "#,
    );
    runtime.load(json!({})).unwrap();
    let id = sent(&commands, "yinz_rpc_call")[0].0["call_id"].clone();
    runtime.invoke("yinz-async-enter", vec![json!(9)]).unwrap();
    for (source, body) in [
        (json!({"Plugin":8}), json!({"payload":"forged"})),
        (
            json!({"Plugin":7}),
            json!({"payload":"invalid","result":"invalid"}),
        ),
        (json!({"Plugin":7}), json!({})),
        (json!({"Plugin":7}), json!({"payload":"x".repeat(1048576)})),
    ] {
        let mut body = body;
        body["version"] = json!(1);
        body["call_id"] = id.clone();
        pipe(&mut runtime, "yinz_rpc_progress", source, body);
    }
    assert_eq!(runtime.value("progress").unwrap(), json!([]));
    now.store(99, Ordering::SeqCst);
    pipe(
        &mut runtime,
        "yinz_rpc_progress",
        json!({"Plugin":7}),
        json!({"version":1,"call_id":id,"payload":"waiting"}),
    );
    assert_eq!(runtime.value("progress").unwrap(), json!([["waiting", 2]]));
    now.store(100, Ordering::SeqCst);
    runtime.update(json!({"Timer":0.0})).unwrap();
    assert!(runtime
        .value("result")
        .unwrap()
        .as_str()
        .unwrap()
        .contains("timed out"));
}

#[test]
fn rpc_rejects_invalid_options_and_replacement_runtimes_have_distinct_ids() {
    let source = r#"
      (define errors '())
      (define (load config)
        (for-each (lambda (options)
          (yinz-catch (yinz-rpc-call options)
            (lambda (failure) (set! errors (cons failure errors)))))
          (list (hash) (hash "pluginId" 7 "pluginUrl" "x" "name" "x")
            (hash "pluginId" -1 "name" "x") (hash "pluginUrl" "" "name" "x")
            (hash "pluginId" 1.0 "name" "x")
            (hash "pluginId" 7 "name" "") (hash "pluginId" 7 "name" "x" "timeoutMs" -1)
            (hash "pluginId" 7 "name" "x" "typo" 1)
            (hash "pluginId" 7 "pluginConfig" (hash) "name" "x")
            (hash "pluginUrl" "x" "pluginConfig" (hash "x" 1) "name" "x")
            (hash "pluginId" 7 "name" "x" "onProgress" 1)
            (hash "pluginId" 7 "name" "x" "payload" (lambda () 1)))))
    "#;
    let (mut runtime, commands, _) = runtime(source);
    runtime.load(json!({})).unwrap();
    assert_eq!(
        runtime.value("errors").unwrap().as_array().unwrap().len(),
        12
    );
    assert!(sent(&commands, "yinz_rpc_call").is_empty());
    let prefix = runtime.value("%rpc-prefix").unwrap();
    let other = Runtime::new(Ok).unwrap();
    assert_ne!(prefix, other.value("%rpc-prefix").unwrap());
}

#[test]
fn error_replies_fit_even_when_call_id_nearly_fills_the_envelope() {
    let (mut runtime, commands, _) =
        runtime("(yinz-rpc-handle \"registered\" (lambda (payload call) payload))");
    let mut body = json!({"version":1,"call_id":"","name":"x"});
    let count = 1048576 - serde_json::to_string(&body).unwrap().len();
    body["call_id"] = json!("x".repeat(count));
    pipe(&mut runtime, "yinz_rpc_call", json!({"Plugin":7}), body);
    let replies = sent(&commands, "yinz_rpc_reply");
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].0["call_id"].as_str().unwrap().len(), count);
    assert!(serde_json::to_vec(&replies[0].0).unwrap().len() <= 1048576);
    assert!(replies[0].0["error"].is_string());
}

#[test]
fn large_unicode_and_escaped_payloads_roundtrip_without_agent_message_limits() {
    let (mut runtime, commands, _) = runtime(
        r#"
      (define results '())
      (yinz-rpc-handle "report" (lambda (payload call) payload))
      (define (load config)
        (for-each (lambda (payload)
          (yinz-then (yinz-rpc-call (hash "pluginId" 7 "name" "report" "payload" payload))
            (lambda (value) (set! results (append results (list value))))))
          (list (make-string 16384 #\😀) (make-string 32768 (integer->char 0)) (make-string 160000 #\x))))
    "#,
    );
    runtime.load(json!({})).unwrap();
    let calls = sent(&commands, "yinz_rpc_call");
    assert_eq!(calls.len(), 3);
    for (call, _, _) in &calls {
        assert!(serde_json::to_vec(call).unwrap().len() > 64000);
        pipe(
            &mut runtime,
            "yinz_rpc_call",
            json!({"Plugin":7}),
            call.clone(),
        );
    }
    for (reply, _, _) in sent(&commands, "yinz_rpc_reply") {
        pipe(&mut runtime, "yinz_rpc_reply", json!({"Plugin":7}), reply);
    }
    assert_eq!(
        runtime.value("results").unwrap(),
        Value::Array(
            calls
                .into_iter()
                .map(|(v, _, _)| v["payload"].clone())
                .collect()
        )
    );
}

#[test]
fn progress_callback_failure_and_dispatch_failure_release_waiters_and_timers() {
    let (mut runtime, commands, _) = runtime(
        r#"
      (define failed json-null)
      (define (load config)
        (yinz-catch (yinz-rpc-call (hash "pluginId" 7 "name" "work" "timeoutMs" 100)
          (lambda (payload) (error "progress failed")))
          (lambda (failure) (set! failed failure))))
    "#,
    );
    runtime.load(json!({})).unwrap();
    let id = sent(&commands, "yinz_rpc_call")[0].0["call_id"].clone();
    pipe(
        &mut runtime,
        "yinz_rpc_progress",
        json!({"Plugin":7}),
        json!({"version":1,"call_id":id,"payload":null}),
    );
    assert!(runtime
        .value("failed")
        .unwrap()
        .as_str()
        .unwrap()
        .contains("progress failed"));
    assert_eq!(runtime.eval("(list (length (hash-keys->list %rpc-calls)) (length (hash-keys->list %async-timers)))").unwrap().values,["(0 0)"]);

    let mut runtime = Runtime::new(|command| {
        if command.get("MessageToPlugin").is_some() {
            anyhow::bail!("dispatch failed");
        }
        Ok(Value::Null)
    })
    .unwrap();
    runtime
        .eval(
            r#"(define failure json-null) (define (load config)
      (yinz-catch (yinz-rpc-call (hash "pluginId" 7 "name" "work" "timeoutMs" 100))
        (lambda (error) (set! failure error))))"#,
        )
        .unwrap();
    runtime.load(json!({})).unwrap();
    assert!(runtime
        .value("failure")
        .unwrap()
        .as_str()
        .unwrap()
        .contains("dispatch failed"));
    assert_eq!(runtime.eval("(list (length (hash-keys->list %rpc-calls)) (length (hash-keys->list %async-timers)))").unwrap().values,["(0 0)"]);
}

#[test]
fn raw_protocol_plugins_keep_their_explicit_pipe_handler_until_sdk_registration() {
    let (mut runtime, _, _) =
        runtime("(define count 0) (define (pipe message) (set! count (+ count 1)) #t)");
    assert!(runtime
        .pipe(json!({"name":"yinz_rpc_call","source":"Keybind","payload":"{}"}))
        .unwrap());
    assert_eq!(runtime.value("count").unwrap(), 1);
    runtime
        .eval("(yinz-rpc-handle \"echo\" (lambda (payload call) payload))")
        .unwrap();
    assert!(!runtime
        .pipe(json!({"name":"yinz_rpc_call","source":"Keybind","payload":"{}"}))
        .unwrap());
    assert_eq!(runtime.value("count").unwrap(), 1);
}

#[test]
fn failed_deadline_registration_does_not_send_an_rpc_request() {
    for rejected in ["Subscribe", "SetTimeout"] {
        let commands = Arc::new(Mutex::new(Vec::<Value>::new()));
        let sent = commands.clone();
        let mut runtime = Runtime::new(move |command| {
            sent.lock().unwrap().push(command.clone());
            if command.get(rejected).is_some() {
                anyhow::bail!("{rejected} failed");
            }
            Ok(Value::Null)
        })
        .unwrap();
        runtime
            .eval(
                r#"(define failure #f) (define (load config)
            (yinz-catch (yinz-rpc-call (hash "pluginId" 7 "name" "work" "timeoutMs" 100))
                (lambda (error) (set! failure error))))"#,
            )
            .unwrap();
        runtime.load(json!({})).unwrap();
        assert_eq!(
            runtime.value("failure").unwrap(),
            json!(format!("{rejected} failed"))
        );
        assert!(!commands
            .lock()
            .unwrap()
            .iter()
            .any(|command| command.get("MessageToPlugin").is_some()));
        assert_eq!(runtime.eval("(list (length (hash-keys->list %rpc-calls)) (length (hash-keys->list %async-timers)))").unwrap().values, ["(0 0)"]);
    }
}

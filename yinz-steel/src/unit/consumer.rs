use crate::Runtime;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

type Commands = Arc<Mutex<Vec<Value>>>;
struct Host {
    session: Value,
    fail: Option<String>,
}
fn runtime(source: &str) -> (Runtime, Commands, Arc<AtomicU64>, Arc<Mutex<Host>>) {
    let commands = Arc::new(Mutex::new(Vec::new()));
    let recorded = commands.clone();
    let now = Arc::new(AtomicU64::new(0));
    let clock = now.clone();
    let host = Arc::new(Mutex::new(Host {
        session: json!({"live_sessions":[{"is_current_session":true,"plugins":{"7":{"location":"yinz:agent-service"}}}]}),
        fail: None,
    }));
    let answers = host.clone();
    let mut runtime = Runtime::new_with_clock(
        move |value| {
            recorded.lock().unwrap().push(value.clone());
            let command = value.pointer("/__yinz_runtime/command").unwrap_or(&value);
            let _: yinz_types::data::PluginCommand = serde_json::from_value(command.clone())
                .unwrap_or_else(|error| panic!("invalid command {command}: {error}"));
            let name = command
                .as_str()
                .unwrap_or_else(|| command.as_object().unwrap().keys().next().unwrap());
            let host = answers.lock().unwrap();
            if host.fail.as_deref() == Some(name) {
                anyhow::bail!("{name} dispatch failed");
            }
            Ok(if name == "GetSessionList" {
                host.session.clone()
            } else {
                Value::Null
            })
        },
        move || clock.load(Ordering::SeqCst),
    )
    .unwrap();
    runtime.eval(source).unwrap();
    runtime.invoke("yinz-async-enter", vec![json!(2)]).unwrap();
    (runtime, commands, now, host)
}
fn commands(sent: &Commands, kind: &str) -> Vec<(Value, Value)> {
    sent.lock()
        .unwrap()
        .iter()
        .filter_map(|value| {
            let actor = value
                .pointer("/__yinz_runtime/client")
                .cloned()
                .unwrap_or(Value::Null);
            let command = value.pointer("/__yinz_runtime/command").unwrap_or(value);
            command.get(kind).cloned().map(|command| (command, actor))
        })
        .collect()
}
fn calls(sent: &Commands) -> Vec<Value> {
    commands(sent, "MessageToPlugin")
        .into_iter()
        .map(|(message, _)| {
            assert_eq!(message["message_name"], "llm_complete");
            serde_json::from_str(message["message_payload"].as_str().unwrap()).unwrap()
        })
        .collect()
}
fn pipe(runtime: &mut Runtime, source: Value, body: Value) {
    runtime
        .pipe(json!({"name":"llm_complete_result","source":source,"payload":body.to_string()}))
        .unwrap();
}
fn response(id: Value) -> Value {
    json!({"request_id":id,"content":"answer","tool_calls":[{"id":"tool-1","name":"lookup","arguments":"{invalid json"}],"finish_reason":"tool_calls","usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3},"model":"model","error":""})
}

#[test]
fn completions_validate_origin_correlate_and_cancel_under_the_initiating_client() {
    let (mut runtime, sent, now, _) = runtime(
        r#"
      (define requests '()) (define results '())
      (define (record id value) (set! results (append results (list (list id value %async-client)))))
      (define (load config)
        (for-each (lambda (id)
          (define request (yinz-complete (hash "messages" (list (hash "role" "user" "content" "hello")) "timeoutMs" 100)))
          (set! requests (append requests (list request)))
          (yinz-catch (yinz-then request (lambda (value) (record id value)))
            (lambda (failure) (record id failure)))) (range 5)))
    "#,
    );
    runtime.load(json!({})).unwrap();
    let requests = calls(&sent);
    assert_eq!(requests.len(), 5);
    assert!(commands(&sent, "MessageToPlugin")
        .iter()
        .all(|(call, actor)| call["destination_plugin_id"] == 7 && actor == 2));
    runtime.invoke("yinz-async-enter", vec![json!(9)]).unwrap();
    let id = |index: usize| requests[index]["request_id"].clone();
    for source in [
        json!({"Plugin":8}),
        json!("Keybind"),
        json!({"Cli":"forged"}),
    ] {
        pipe(&mut runtime, source, response(id(0)));
    }
    for field in ["content", "error", "usage", "tool_calls"] {
        let mut invalid = response(id(0));
        invalid[field] = json!(17);
        pipe(&mut runtime, json!({"Plugin":7}), invalid);
    }
    assert_eq!(runtime.value("results").unwrap(), json!([]));
    let mut failure = response(id(0));
    failure["error"] = json!("daemon refused");
    pipe(&mut runtime, json!({"Plugin":7}), failure);
    pipe(&mut runtime, json!({"Plugin":7}), response(id(1)));
    pipe(&mut runtime, json!({"Plugin":7}), response(id(1)));
    runtime
        .eval("(yinz-cancel! (list-ref requests 2))")
        .unwrap();
    pipe(&mut runtime, json!({"Plugin":7}), response(id(2)));
    let mut large = response(id(3));
    large["content"] = json!("x".repeat(64001));
    pipe(&mut runtime, json!({"Plugin":7}), large);
    now.store(100, Ordering::SeqCst);
    runtime.update(json!({"Timer":0.0})).unwrap();
    let results = runtime.value("results").unwrap();
    let results = results.as_array().unwrap();
    assert_eq!(results.len(), 5);
    assert!(results.iter().all(|row| row[2] == 2));
    let result = |id: i32| &results.iter().find(|row| row[0] == id).unwrap()[1];
    assert_eq!(result(0), "daemon refused");
    assert_eq!(result(1)["toolCalls"][0]["arguments"], "{invalid json");
    assert_eq!(result(1)["usage"]["total_tokens"], 3);
    assert!(result(2).as_str().unwrap().contains("cancelled"));
    assert_eq!(result(3)["content"].as_str().unwrap().len(), 64001);
    assert_eq!(result(4), "LLM completion timed out");
    assert_eq!(runtime.value("%completion-calls").unwrap(), json!({}));
}

#[test]
fn completion_validation_precedes_dispatch_and_failures_release_waiters() {
    let (mut runtime, sent, _, host) = runtime(
        r#"
      (define errors '())
      (define (request options)
        (yinz-catch (yinz-complete options) (lambda (failure) (set! errors (cons failure errors)))) (void))
    "#,
    );
    for options in [
        json!({}),
        json!({"messages":[]}),
        json!({"messages":[{"role":"invalid"}]}),
        json!({"messages":[{"role":"tool"}]}),
        json!({"messages":[{"role":"user","content":3}]}),
        json!({"messages":[],"system":7}),
        json!({"messages":[],"system":"ok","maxTokens":-1}),
        json!({"messages":[],"system":"ok","maxTokens":1.0}),
        json!({"messages":[],"system":"ok","unknown":1}),
        json!({"messages":[],"system":"ok","timeoutMs":-1}),
        json!({"messages":[],"system":"ok","timeoutMs":2147483648u64}),
        json!({"messages":[],"system":"é".repeat(32000)}),
        json!({"messages":vec![json!({"role":"user"});201]}),
        json!({"messages":[],"system":"ok","tools":vec![json!({});65]}),
    ] {
        runtime.invoke("request", vec![options]).unwrap();
    }
    runtime.invoke("yinz-async-poll", vec![]).unwrap();
    assert_eq!(
        runtime.value("errors").unwrap().as_array().unwrap().len(),
        14
    );
    assert!(sent.lock().unwrap().is_empty());
    for session in [
        json!({"live_sessions":[]}),
        json!({"live_sessions":[{"is_current_session":true,"plugins":{"7":{"location":"yinz:agent-service"},"8":{"location":"yinz:agent-service"}}}]}),
    ] {
        host.lock().unwrap().session = session;
        runtime
            .invoke("request", vec![json!({"messages":[],"system":"ok"})])
            .unwrap();
        runtime.invoke("yinz-async-poll", vec![]).unwrap();
    }
    assert!(runtime.value("errors").unwrap()[0]
        .as_str()
        .unwrap()
        .contains("multiple agent-service"));
    assert!(runtime.value("errors").unwrap()[1]
        .as_str()
        .unwrap()
        .contains("no agent-service"));
    host.lock().unwrap().session = json!({"live_sessions":[{"is_current_session":true,"plugins":{"7":{"location":"yinz:agent-service"}}}]});
    for name in [
        "GetSessionList",
        "Subscribe",
        "SetTimeout",
        "MessageToPlugin",
    ] {
        host.lock().unwrap().fail = Some(name.into());
        runtime
            .invoke("request", vec![json!({"messages":[],"system":"ok"})])
            .unwrap();
        runtime.invoke("yinz-async-poll", vec![]).unwrap();
        assert_eq!(
            runtime.value("errors").unwrap()[0],
            format!("{name} dispatch failed")
        );
        assert_eq!(runtime.value("%completion-calls").unwrap(), json!({}));
    }
}

#[test]
fn completion_request_budget_counts_utf8_envelope_and_instances_have_distinct_ids() {
    let make = || {
        let (mut runtime, sent, _, _) =
            runtime("(define (request options) (yinz-complete options) (void))");
        runtime
            .invoke("request", vec![json!({"messages":[],"system":""})])
            .unwrap();
        let seed = calls(&sent)[0].clone();
        (runtime, sent, seed)
    };
    let (mut runtime, sent, seed) = make();
    assert_ne!(seed["request_id"], make().2["request_id"]);
    let remaining = 64000 - seed.to_string().len();
    runtime
        .invoke(
            "request",
            vec![json!({"messages":[],"system":"x".repeat(remaining)})],
        )
        .unwrap();
    assert_eq!(calls(&sent).last().unwrap().to_string().len(), 64000);
    runtime
        .invoke(
            "request",
            vec![json!({"messages":[],"system":format!("{}é", "x".repeat(remaining-1))})],
        )
        .unwrap();
    assert_eq!(calls(&sent).len(), 2);
}

#[test]
fn picker_correlates_results_cancels_the_host_and_keeps_deadline_origin() {
    let (mut runtime, sent, now, _) = runtime(
        r#"
      (define results '()) (define requests '())
      (define (load config)
        (for-each (lambda (id)
          (define request (yinz-pick (hash "kind" "file" "roots" '("/work") "extensions" '("txt" "rs") "hidden" #f "timeoutMs" 100)))
          (set! requests (append requests (list request)))
          (yinz-catch (yinz-then request (lambda (value) (set! results (cons (list id value %async-client) results))))
            (lambda (failure) (set! results (cons (list id failure %async-client) results))))) (range 3)))
    "#,
    );
    runtime.load(json!({})).unwrap();
    let starts = commands(&sent, "Pick")
        .into_iter()
        .map(|(call, _)| call["Start"]["request_id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 3);
    runtime.invoke("yinz-async-enter", vec![json!(9)]).unwrap();
    runtime
        .update(json!({"PickResult":{"request_id":"unrelated","result":{"Ok":"wrong"}}}))
        .unwrap();
    assert_eq!(runtime.value("results").unwrap(), json!([]));
    runtime
        .update(json!({"PickResult":{"request_id":starts[0],"result":{"Ok":"/work/é.txt"}}}))
        .unwrap();
    runtime
        .eval("(yinz-cancel! (list-ref requests 1))")
        .unwrap();
    runtime.invoke("yinz-async-poll", vec![]).unwrap();
    assert_eq!(
        runtime.value("results").unwrap().as_array().unwrap().len(),
        1
    );
    runtime
        .update(json!({"PickResult":{"request_id":starts[1],"result":{"Err":"AbortError"}}}))
        .unwrap();
    assert_eq!(runtime.value("results").unwrap()[0][1], "AbortError");
    now.store(100, Ordering::SeqCst);
    runtime.update(json!({"Timer":0.0})).unwrap();
    let cancels = commands(&sent, "Pick")
        .into_iter()
        .filter(|(call, _)| call.get("Cancel").is_some())
        .collect::<Vec<_>>();
    assert_eq!(cancels.len(), 2);
    assert!(cancels.iter().all(|(_, actor)| actor == 2));
    assert_eq!(
        cancels[0].0["Cancel"],
        json!({"request_id":starts[1],"timeout":false})
    );
    assert_eq!(
        cancels[1].0["Cancel"],
        json!({"request_id":starts[2],"timeout":true})
    );
    runtime
        .update(json!({"PickResult":{"request_id":starts[2],"result":{"Err":"TimeoutError"}}}))
        .unwrap();
    assert_eq!(
        runtime.value("results").unwrap().as_array().unwrap().len(),
        3
    );
    assert_eq!(commands(&sent, "Pick").len(), 5);
}

#[test]
fn picker_rejects_invalid_options_without_dispatch() {
    let (mut runtime,sent,_,_) = runtime("(define errors '()) (define (request options) (yinz-catch (yinz-pick options) (lambda (failure) (set! errors (cons failure errors)))) (void))");
    for options in [
        json!({}),
        json!({"kind":"files"}),
        json!({"kind":"file","roots":[]}),
        json!({"kind":"file","roots":["\0"]}),
        json!({"kind":"directory","extensions":[]}),
        json!({"kind":"file","extensions":[".txt"]}),
        json!({"kind":"file","extensions":["a*b"]}),
        json!({"kind":"file","hidden":1}),
        json!({"kind":"file","timeoutMs":-1}),
        json!({"kind":"file","timeoutMs":1.0}),
        json!({"kind":"file","timeoutMs":false}),
        json!({"kind":"file","timeoutMs":9007199254740992u64}),
        json!({"kind":"file","unknown":true}),
    ] {
        runtime.invoke("request", vec![options]).unwrap();
    }
    runtime.invoke("yinz-async-poll", vec![]).unwrap();
    assert_eq!(
        runtime.value("errors").unwrap().as_array().unwrap().len(),
        13
    );
    assert!(sent.lock().unwrap().is_empty());
}

#[test]
fn picker_cancel_failure_keeps_the_request_live_and_selection_can_win_the_race() {
    let (mut runtime, sent, _, host) = runtime(
        r#"
      (define request #f) (define result #f)
      (define (pending) (yinz-pending? request))
      (define (load config)
        (set! request (yinz-pick (hash "kind" "file")))
        (yinz-catch (yinz-then request (lambda (value) (set! result value)))
          (lambda (failure) (set! result failure))))
    "#,
    );
    runtime.load(json!({})).unwrap();
    let id = commands(&sent, "Pick")[0].0["Start"]["request_id"].clone();
    runtime.invoke("yinz-async-enter", vec![json!(9)]).unwrap();
    host.lock().unwrap().fail = Some("Pick".into());
    runtime
        .eval("(define failure (with-handler (lambda (error) (error-object-message error)) (yinz-cancel! request)))")
        .unwrap();
    assert_eq!(runtime.value("failure").unwrap(), "Pick dispatch failed");
    assert_eq!(runtime.invoke("pending", vec![]).unwrap(), true);
    assert_eq!(runtime.value("result").unwrap(), false);
    host.lock().unwrap().fail = None;
    runtime.eval("(yinz-cancel! request)").unwrap();
    assert_eq!(runtime.invoke("pending", vec![]).unwrap(), true);
    runtime
        .update(json!({"PickResult":{"request_id":id,"result":{"Ok":"/work/selected.txt"}}}))
        .unwrap();
    assert_eq!(runtime.value("result").unwrap(), "/work/selected.txt");
    assert_eq!(runtime.invoke("pending", vec![]).unwrap(), false);
    runtime.eval("(yinz-cancel! request)").unwrap();
    let calls = commands(&sent, "Pick");
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|(_, actor)| actor == 2));
    assert_eq!(runtime.value("%async-handlers").unwrap(), json!({}));
}

#[test]
fn picker_teardown_releases_the_waiter_without_waiting_for_host_acknowledgment() {
    let (mut runtime, sent, _, _) = runtime(
        r#"
      (define request #f)
      (define (pending) (yinz-pending? request))
      (define (load config)
        (set! request (yinz-pick (hash "kind" "directory" "timeoutMs" 1000))))
    "#,
    );
    runtime.load(json!({})).unwrap();
    runtime.invoke("yinz-async-enter", vec![json!(9)]).unwrap();
    runtime.invoke("yinz-async-cancel", vec![]).unwrap();
    assert_eq!(runtime.invoke("pending", vec![]).unwrap(), false);
    assert_eq!(runtime.value("%async-operations").unwrap(), json!({}));
    assert_eq!(runtime.value("%async-handlers").unwrap(), json!({}));
    assert_eq!(runtime.value("%async-timers").unwrap(), json!({}));
    let calls = commands(&sent, "Pick");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].0["Cancel"]["timeout"], false);
    assert!(calls.iter().all(|(_, actor)| actor == 2));
}

#[test]
fn plugin_test_requests_correlate_cancel_and_release_after_bad_json() {
    let (mut runtime, sent, _, _) = runtime(
        r#"
      (define requests '()) (define results '())
      (define (load config)
        (for-each (lambda (id)
          (define request (yinz-test-plugin "fixture"))
          (set! requests (append requests (list request)))
          (yinz-catch (yinz-then request (lambda (value) (set! results (cons (list id value %async-client) results))))
            (lambda (failure) (set! results (cons (list id failure %async-client) results))))) (range 3)))
    "#,
    );
    runtime.load(json!({})).unwrap();
    let calls = commands(&sent, "TestPlugin");
    let event = |id: usize, body: &str| json!({"CustomMessage":[format!("__yinz_plugin_test:{}",calls[id].0["request_id"].as_str().unwrap()),body]});
    runtime.invoke("yinz-async-enter", vec![json!(9)]).unwrap();
    runtime
        .update(json!({"CustomMessage":["__yinz_plugin_test:wrong","{\"Ok\":1}"]}))
        .unwrap();
    runtime.update(event(0, "{\"Ok\":{\"tests\":[]}}")).unwrap();
    runtime.update(event(0, "{\"Err\":\"duplicate\"}")).unwrap();
    runtime
        .eval("(yinz-cancel! (list-ref requests 1))")
        .unwrap();
    runtime.update(event(2, "invalid json")).unwrap();
    let canceled = commands(&sent, "CancelPluginTest");
    assert_eq!(canceled, vec![(calls[1].0["request_id"].clone(), json!(2))]);
    let results = runtime.value("results").unwrap();
    assert_eq!(results.as_array().unwrap().len(), 3);
    assert!(results.as_array().unwrap().iter().all(|row| row[2] == 2));
}

#[test]
fn failed_test_cancel_dispatch_still_notifies_observers() {
    let (mut runtime, _, _, host) = runtime(
        r#"
      (define request #f) (define failure #f)
      (define (load config)
        (set! request (yinz-test-plugin "fixture"))
        (yinz-catch request (lambda (error) (set! failure error))))
    "#,
    );
    runtime.load(json!({})).unwrap();
    host.lock().unwrap().fail = Some("CancelPluginTest".into());
    runtime.eval("(yinz-cancel! request)").unwrap();
    let error = runtime.invoke("yinz-async-poll", vec![]).unwrap_err();
    assert!(error
        .to_string()
        .contains("CancelPluginTest dispatch failed"));
    assert_eq!(runtime.value("failure").unwrap(), "operation cancelled");
}

use super::*;
use serde_json::json;

fn runtime() -> Runtime {
    Runtime::new(Ok).unwrap()
}

#[test]
fn evaluation_persists_and_captures_output() {
    let mut runtime = runtime();
    assert_eq!(
        runtime.eval("(define answer 40)").unwrap().values,
        Vec::<String>::new()
    );
    assert_eq!(
        runtime.eval("(display \"hello\") (+ answer 2)").unwrap(),
        EvalOutput {
            output: "hello".into(),
            values: vec!["42".into()]
        }
    );
    assert!(runtime
        .eval("(display \"before\") (error \"bad\")")
        .is_err());
    assert_eq!(runtime.take_output().unwrap(), "before");
}

#[test]
fn later_repl_mutations_are_visible_to_existing_closures() {
    let mut runtime = runtime();
    runtime
        .eval("(define counter 0) (define (read-counter) counter)")
        .unwrap();
    runtime.eval("(set! counter 42)").unwrap();
    assert_eq!(runtime.eval("(read-counter)").unwrap().values, ["42"]);
}

#[test]
fn integer_immediates_preserve_values_above_the_instruction_payload_width() {
    let mut runtime = runtime();
    for value in [
        16_777_215_i64,
        16_777_216,
        2_147_483_647,
        9_007_199_254_740_991,
    ] {
        runtime.eval(&format!("(define (add x) (+ x {value})) (define (subtract x) (- x {value})) (define (compare x) (<= x {value}))")).unwrap();
        assert_eq!(
            runtime.eval("(add 1)").unwrap().values,
            [(value + 1).to_string()]
        );
        assert_eq!(
            runtime.eval("(subtract 0)").unwrap().values,
            [(-value).to_string()]
        );
        assert_eq!(
            runtime
                .eval(&format!("(list (compare {value}) (compare {}))", value + 1))
                .unwrap()
                .values,
            ["(#true #false)"]
        );
    }
}

#[test]
fn bridge_preserves_json_types_and_integer_extremes() {
    let value = json!({"null": null, "false": false, "array": [[], {}, "é"], "min": i64::MIN, "max": u64::MAX, "fraction": 1.5});
    assert_eq!(from_scheme(&to_scheme(&value).unwrap()).unwrap(), value);
    let mut runtime = runtime();
    let result = runtime
        .eval("(yinz-json-string (yinz-command (hash \"Subscribe\" '(\"Key\"))))")
        .unwrap();
    assert_eq!(result.values.len(), 1);
    assert!(result.values[0].contains("Subscribe"));
    assert!(runtime.eval("(yinz-command (hash 'key 1))").is_err());
    assert!(runtime.eval("(yinz-command 18446744073709551616)").is_err());
}

#[test]
fn host_errors_are_scheme_errors() {
    let mut runtime = Runtime::new(|_| Err(anyhow!("permission denied"))).unwrap();
    assert!(runtime
        .eval("(yinz-command \"GetPluginIds\")")
        .unwrap_err()
        .to_string()
        .contains("permission denied"));
}

#[test]
fn lifecycle_hooks_use_values_and_capture_render() {
    let mut runtime = runtime();
    runtime.load(json!({})).unwrap();
    assert!(!runtime.update(json!({})).unwrap());
    assert_eq!(runtime.render(1, 2).unwrap(), "");
    runtime.eval("(define state 0) (define (load config) (set! state (hash-ref config \"value\"))) (define (update event) (set! state (+ state event)) #t) (define (pipe msg) #f) (define (render rows cols) (display state) (display \"x\") (write cols)) (define (unload) (set! state 0))").unwrap();
    runtime.load(json!({"value": 3})).unwrap();
    assert!(runtime.update(json!(4)).unwrap());
    assert!(!runtime.pipe(Value::Null).unwrap());
    assert_eq!(runtime.render(10, 20).unwrap(), "7x20");
    runtime.unload().unwrap();
    assert_eq!(runtime.eval("state").unwrap().values, ["0"]);
}

#[test]
fn filesystem_process_network_and_engine_escape_are_unavailable() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("secret.scm");
    std::fs::write(&file, "(provide secret) (define secret 77)").unwrap();
    let mut runtime = runtime();
    for source in [
        "(open-input-file \"/etc/passwd\")".to_owned(),
        "(require-builtin steel/process)".to_owned(),
        "(require-builtin steel/meta) (Engine::new)".to_owned(),
        "(require-builtin steel/tcp)".to_owned(),
        "(Engine::new)".to_owned(),
        "(load-dylib \"bad\")".to_owned(),
        "(env-var \"HOME\")".to_owned(),
        "(eval \"(+ 1 2)\")".to_owned(),
        format!("(require {:?}) secret", file.to_str().unwrap()),
        "(stdin)".to_owned(),
        "(stdout)".to_owned(),
        "(yinz-raw-display \"escape\")".to_owned(),
        "(require-builtin steel/ports) (#%raw-display \"escape\")".to_owned(),
        "(require-builtin steel/ports) (#%read-char)".to_owned(),
        "(require-builtin steel/ports) (#%default-output-port)".to_owned(),
    ] {
        assert!(runtime.eval(&source).is_err(), "escape succeeded: {source}");
    }
}

#[test]
fn conversion_limits_reject_deep_and_large_values() {
    let mut value = Value::Null;
    for _ in 0..66 {
        value = json!([value]);
    }
    assert!(to_scheme(&value).is_err());
    assert!(to_scheme(&json!("x".repeat(bridge::MAX_INPUT_BYTES + 1))).is_err());
    assert!(from_scheme(&steel::rvals::SteelVal::StringV(
        "x".repeat(64 * 1024 * 1024 + 1).into()
    ))
    .is_err());
    let mut runtime = runtime();
    assert!(runtime.eval("(yinz-command (lambda () 1))").is_err());
}

#[test]
fn repl_parser_recognizes_multiline_forms() {
    assert!(!source_complete("(+ 1"));
    assert!(!source_complete("'"));
    assert!(source_complete("(+ 1 2)"));
    assert!(source_complete(")"));
    assert!(source_complete("; comment"));
    assert!(!source_complete("\"unfinished string"));
    assert!(source_complete("\"finished string\""));
    assert!(!source_complete("#| outer #| nested |#"));
    assert!(source_complete("#| outer #| nested |# |#"));
    assert!(!source_complete("|unfinished identifier"));
}

#[test]
fn source_nesting_is_bounded_before_recursive_parsing() {
    let mut runtime = runtime();
    for source in ["(".repeat(129), "'".repeat(129)] {
        assert!(source_complete(&source));
        assert!(runtime
            .eval(&source)
            .unwrap_err()
            .to_string()
            .contains("nesting"));
    }
    assert!(runtime.eval(&format!("\"{}\"", "(".repeat(1000))).is_ok());
}

#[test]
fn comment_chains_do_not_recurse_in_the_source_guard() {
    let comments = "; comment\n".repeat(30_000);
    assert!(source_complete(&comments));
    assert_eq!(runtime().eval(&(comments + "42")).unwrap().values, ["42"]);
    assert!(validate_source(&format!("{}1", "'; comment\n".repeat(129))).is_err());
}

#[test]
fn native_plugin_helpers_preserve_text_and_validate_bytes() {
    let mut runtime = runtime();
    assert_eq!(
        runtime
            .eval("(yinz-text-sanitize \"a\\tb\\n\\u{1b}[31mc\")")
            .unwrap()
            .values,
        ["\"a    b\\nc\""]
    );
    assert_eq!(
        runtime
            .eval("(yinz-decode-utf8 '(97 255 98))")
            .unwrap()
            .values,
        ["\"a�b\""]
    );
    assert!(runtime.eval("(yinz-decode-utf8 '(256))").is_err());
    assert!(runtime.eval("(yinz-decode-utf8 '(-1))").is_err());
    assert_eq!(
        runtime.eval("(<= 0 (yinz-now-ms))").unwrap().values,
        ["#true"]
    );
}

#[test]
fn utc_date_parts_use_explicit_timestamps_and_offsets() {
    let mut runtime = runtime();
    assert_eq!(
        runtime.invoke("yinz-utc-date", vec![json!(0)]).unwrap(),
        json!({"year":1970,"month":1,"day":1,"weekday":4,"hour":0,"minute":0,"second":0})
    );
    assert_eq!(
        runtime
            .invoke("yinz-utc-date", vec![json!(-3600000)])
            .unwrap()["hour"],
        23
    );
    assert!(
        runtime
            .invoke("yinz-wall-clock-ms", vec![])
            .unwrap()
            .as_i64()
            .unwrap()
            > 0
    );
}

#[test]
fn interruption_stops_running_bytecode() {
    use std::sync::mpsc;
    use std::time::Duration;
    let (ready, started) = mpsc::channel();
    let mut runtime = Runtime::new(move |_| {
        ready.send(()).context("signaling bytecode start")?;
        Ok(Value::Null)
    })
    .unwrap();
    let interrupted = runtime.interruption_handle();
    let worker = std::thread::spawn(move || {
        runtime.eval("(define (spin) (spin)) (yinz-command \"ready\") (spin)")
    });
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    interrupted.interrupt();
    assert!(worker.join().unwrap().is_err());
}

#[test]
fn scheme_sort_and_exception_handler_support_plugin_callbacks() {
    let mut runtime = Runtime::new(|_| Err(anyhow!("permission denied"))).unwrap();
    assert_eq!(
        runtime
            .eval("(sort '(3 1 2) (lambda (a b) (< a b)))")
            .unwrap()
            .values,
        ["(1 2 3)"]
    );
    assert_eq!(runtime.eval("(with-handler (lambda (error) (error-object-message error)) (yinz-command \"GetPluginIds\"))").unwrap().values, ["\"permission denied\""]);
}

#[test]
fn checking_compiles_without_executing_host_commands_or_output() {
    let mut runtime = Runtime::new(|_| panic!("check executed a host command")).unwrap();
    runtime.check("(yinz-command \"GetPluginIds\") (display \"must not execute\") (error \"not during check\")").unwrap();
    assert!(runtime.check("(missing-command)").is_err());
    assert!(runtime.check("(define)").is_err());
    assert_eq!(runtime.take_output().unwrap(), "");
}

#[test]
fn injected_monotonic_clock_drives_the_precompiled_async_sdk() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let now = Arc::new(AtomicU64::new(0));
    let clock = now.clone();
    let mut runtime =
        Runtime::new_with_clock(|_| Ok(Value::Null), move || clock.load(Ordering::Relaxed))
            .unwrap();
    runtime
        .eval(
            r#"
            (define text "waiting")
            (define (load configuration)
              (yinz-then (yinz-sleep 100) (lambda (ignored) (set! text "awake"))))
            (define (render rows columns) text)
            "#,
        )
        .unwrap();
    runtime.load(json!({})).unwrap();
    assert_eq!(runtime.render(1, 80).unwrap(), "waiting");
    now.store(99, Ordering::Relaxed);
    runtime.update(json!({"Timer":0})).unwrap();
    assert_eq!(runtime.render(1, 80).unwrap(), "waiting");
    now.store(100, Ordering::Relaxed);
    runtime.update(json!({"Timer":0})).unwrap();
    assert_eq!(runtime.render(1, 80).unwrap(), "awake");
    now.store(u64::MAX, Ordering::Relaxed);
    assert_eq!(
        runtime.invoke("yinz-now-ms", vec![]).unwrap(),
        json!(isize::MAX)
    );
}

#[test]
fn internal_definitions_preserve_effect_order_in_bodies_with_local_functions() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        (define offset 0)
        (define (compute now value)
          (define (two x) (+ x 1))
          (when (> now 0) (set! offset value))
          (define date (+ now (* offset 1000)))
          (list date (two 1)))
        (define (recursive limit)
          (define (even? n) (if (= n 0) #t (odd? (- n 1))))
          (define (odd? n) (if (= n 0) #f (even? (- n 1))))
          (define answer (even? limit))
          answer)
        "#,
        )
        .unwrap();
    assert_eq!(
        runtime.invoke("compute", vec![json!(0), json!(0)]).unwrap(),
        json!([0, 2])
    );
    assert_eq!(
        runtime
            .invoke("compute", vec![json!(3600000), json!(19800)])
            .unwrap(),
        json!([23400000, 2])
    );
    assert_eq!(
        runtime.invoke("recursive", vec![json!(24)]).unwrap(),
        json!(true)
    );
    assert_eq!(
        runtime.invoke("recursive", vec![json!(25)]).unwrap(),
        json!(false)
    );
}

#[test]
fn bridge_accepts_a_large_host_tab_inventory_within_the_ipc_budget() {
    let tabs: Vec<_> = (0..10_002)
        .map(|position| yinz_types::data::TabInfo {
            position: yinz_types::internal_ids::TabPosition::new(position),
            name: format!("tab-{position}"),
            ..Default::default()
        })
        .collect();
    let event = serde_json::json!({"TabUpdate": tabs});
    assert!(serde_json::to_vec(&event).unwrap().len() < bridge::MAX_INPUT_BYTES);
    let mut runtime = runtime();
    runtime.eval("(define tab-count 0) (define (update event) (set! tab-count (length (hash-ref event \"TabUpdate\"))) #t)").unwrap();
    assert!(runtime.update(event).unwrap());
    assert_eq!(runtime.value("tab-count").unwrap(), 10_002);
}

#[test]
fn compile_only_diagnostics_include_the_failing_source() {
    let mut runtime = Runtime::new(|_| anyhow::bail!("check must not execute")).unwrap();
    let error = runtime
        .check("(define valid 1)\n(lambda (x))")
        .unwrap_err()
        .to_string();
    assert!(error.contains("(lambda (x))"), "{error}");
}

#[test]
fn agent_models_use_explicit_provider_snapshots_without_cross_runtime_leaks() {
    use yinz_types::agent::provider::{self, AgentProvider};
    let provider = AgentProvider {
        kind: "testprovider".into(),
        executable: "testprovider".into(),
        ..Default::default()
    };
    let mut a = runtime();
    let mut b = runtime();
    a.eval(&format!(
        "(define providers (yinz-json-parse {:?}))",
        serde_json::to_string(&vec![provider.clone()]).unwrap()
    ))
    .unwrap();
    assert_eq!(a.eval("(yinz-agent-model \"providers\" \"WithSnapshot\" (list providers \"agents\" \"KindFromCommand\" (list \"testprovider --run\")))").unwrap().values, vec!["\"testprovider\""]);
    assert_eq!(
        b.eval("(yinz-agent-model \"agents\" \"KindFromCommand\" (list \"testprovider --run\"))")
            .unwrap()
            .values,
        vec!["json-null"]
    );
    assert_eq!(
        a.eval("(yinz-agent-model \"agents\" \"KindFromCommand\" (list \"testprovider --run\"))")
            .unwrap()
            .values,
        vec!["json-null"]
    );
    provider::with_snapshot(&[provider], || {
        assert!(provider::provider("testprovider").is_some());
        let _ = std::panic::catch_unwind(|| provider::with_snapshot(&[], || panic!("restore")));
        assert!(provider::provider("testprovider").is_some());
    });
}

#[test]
fn injected_wall_clock_tracks_the_same_replay_time_as_monotonic_clock() {
    let clock = Arc::new(std::sync::atomic::AtomicU64::new(1_700_000_000_000));
    let mono = clock.clone();
    let wall = clock.clone();
    let mut runtime = Runtime::new_with_clocks(
        Ok,
        move || mono.load(std::sync::atomic::Ordering::SeqCst),
        move || wall.load(std::sync::atomic::Ordering::SeqCst) as i64,
    )
    .unwrap();
    assert_eq!(
        runtime
            .eval("(= (yinz-now-ms) (yinz-wall-clock-ms))")
            .unwrap()
            .values,
        vec!["#true"]
    );
    clock.fetch_add(1_000, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        runtime.eval("(yinz-wall-clock-ms)").unwrap().values,
        vec!["1700000001000"]
    );
}

#[test]
fn data_sdk_pages_through_host_calls_and_rejects_stalled_cursors() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let calls = seen.clone();
    let mut runtime = Runtime::new(move |command| {
        calls.lock().unwrap().push(command.clone());
        let page = command["DataFetch"]["request"]["after_id"].as_i64();
        Ok(if page.is_none() {
            json!({"columns":["id"],"rows":[[{"Int":1}]],"truncated":true,"next_cursor":1})
        } else {
            json!({"columns":["id"],"rows":[[{"Int":2}]],"truncated":false,"next_cursor":null})
        })
    })
    .unwrap();
    let result = runtime.eval("(define paged (yinz-data-fetch-all \"shared:yinz\" (yinz-agent-model \"store\" \"EntityRequest\" \"todo\"))) (length (hash-ref paged \"rows\"))").unwrap();
    assert_eq!(result.values, vec!["2"]);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1]["DataFetch"]["request"]["namespace"], "shared:yinz");
    assert_eq!(seen[1]["DataFetch"]["request"]["after_id"], 1);
    let mut runtime =
        Runtime::new(|_| Ok(json!({"columns":[],"rows":[],"truncated":true,"next_cursor":1})))
            .unwrap();
    assert!(runtime.eval("(yinz-data-fetch-all \"shared:yinz\" (yinz-agent-model \"store\" \"EntityRequest\" \"todo\"))").unwrap_err().to_string().contains("cursor did not advance"));
}

#[test]
fn errors_preserve_literal_strings_when_caught_and_rethrown() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"(define message "a \"quoted\" line\nwith\\path")
      (define first (with-handler (lambda (error) (error-object-message error)) (error message)))
      (define second (with-handler (lambda (error) (error-object-message error)) (error first)))"#,
        )
        .unwrap();
    assert_eq!(
        runtime.value("first").unwrap(),
        json!(" a \"quoted\" line\nwith\\path")
    );
    assert_eq!(
        runtime.value("second").unwrap(),
        json!("  a \"quoted\" line\nwith\\path")
    );
}

#[test]
fn character_classification_preserves_unicode_letters_and_numbers() {
    let mut runtime = runtime();
    assert_eq!(
        runtime
            .eval(r#"(map char-alphanumeric? (string->list "Aé日٣Ⅷ— !"))"#)
            .unwrap()
            .values,
        ["(#true #true #true #true #true #false #false #false)"]
    );
}

#[test]
fn json_encoding_sorts_object_keys_recursively_and_preserves_array_order() {
    let mut runtime = runtime();
    runtime.eval("(define encoded (yinz-json-string (hash \"z\" (list 2 1) \"a\" (hash \"b\" 1 \"a\" 2 \"é\" 3))))").unwrap();
    assert_eq!(
        runtime.value("encoded").unwrap(),
        json!(r#"{"a":{"a":2,"b":1,"é":3},"z":[2,1]}"#)
    );
}

#[test]
fn json_unicode_keys_sort_by_codepoint() {
    let mut runtime = runtime();
    runtime
        .eval(r#"(define encoded (yinz-json-string (hash "日" 1 "é" 2 "a" 3)))"#)
        .unwrap();
    assert_eq!(
        runtime.value("encoded").unwrap(),
        json!(r#"{"a":3,"é":2,"日":1}"#)
    );
}

#[test]
fn captured_mutable_locals_are_values_before_and_after_mutation() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
      (define (capture-check)
        (define n 8)
        (define seen (vector n))
        (for-each (lambda (ignored) (vector-push! seen n)) '(1))
        (set! n 9)
        (vector-push! seen n)
        (for-each (lambda (ignored) (vector-push! seen n)) '(1))
        (map (lambda (index) (vector-ref seen index)) (range (vector-length seen))))
      (define captured-values (capture-check))
    "#,
        )
        .unwrap();
    assert_eq!(
        runtime.value("captured-values").unwrap(),
        json!([8, 8, 9, 9])
    );
}

#[test]
fn server_metadata_is_host_supplied_once_and_does_not_depend_on_guest_config() {
    let mut runtime = runtime();
    assert_eq!(
        runtime.invoke("yinz-server-session", vec![]).unwrap(),
        serde_json::Value::Null
    );
    runtime
        .set_server_metadata(crate::ServerMetadata {
            binary: "/opt/yinz/bin/yinz".into(),
            session: Some("real-session".into()),
            incarnation: Some("server-generation".into()),
        })
        .unwrap();
    runtime.eval("(define (load config) (void))").unwrap();
    runtime
        .load(json!({"session": "forged-session", "binary": "/tmp/guest"}))
        .unwrap();
    assert_eq!(
        runtime.invoke("yinz-server-binary", vec![]).unwrap(),
        json!("/opt/yinz/bin/yinz")
    );
    assert_eq!(
        runtime.invoke("yinz-server-session", vec![]).unwrap(),
        json!("real-session")
    );
    assert_eq!(
        runtime.invoke("yinz-server-incarnation", vec![]).unwrap(),
        json!("server-generation")
    );
    assert!(runtime
        .set_server_metadata(crate::ServerMetadata {
            binary: "replacement".into(),
            session: None,
            incarnation: None,
        })
        .is_err());
}

#[test]
fn explicit_mock_callbacks_preserve_lexical_state_and_reentrant_controls() {
    let stored = std::sync::Arc::new(std::sync::Mutex::new(None::<crate::JsonCallback>));
    let callbacks = stored.clone();
    let mut runtime = Runtime::new(move |value| {
        let callback = callbacks.lock().unwrap().clone().unwrap();
        callback.call(value)
    })
    .unwrap();
    let callbacks = stored.clone();
    runtime.register_mock_callbacks(move |_, callback| {
        *callbacks.lock().unwrap() = Some(callback);
        Ok(())
    });
    runtime.register_json_function("control", |value| Ok(json!({"echo":value})));
    runtime
        .eval(
            r#"
      (define count 0)
      (define observed json-null)
      (let ((local 10))
        (%yinz-register-callback "mock" (%yinz-capture-callback
          (lambda (argument)
            (set! count (+ count 1))
            (set! local (+ local 1))
            (set! observed (control argument))
            (list count local observed)))))
    "#,
        )
        .unwrap();
    runtime
        .eval(r#"(define first-reply (yinz-command "one"))"#)
        .unwrap();
    assert_eq!(
        runtime.value("first-reply").unwrap(),
        json!([1, 11, {"echo":"one"}])
    );
    runtime
        .eval(r#"(set! count 8) (yinz-command "two")"#)
        .unwrap();
    assert_eq!(runtime.value("count").unwrap(), json!(9));
    assert_eq!(runtime.value("observed").unwrap(), json!({"echo":"two"}));
}

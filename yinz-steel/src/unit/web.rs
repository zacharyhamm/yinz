use super::panes_tests::Harness;
use serde_json::{json, Value};
fn request(id: u64, method: &str, path: &str) -> Value {
    json!({"id":id,"method":method,"path":path,"segments":path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>(),"headers":{},"query":{},"query_pairs":[],"body":null,"auth":{"session":"test","label":"test"},"client":2,"attached":true,"server":{"session":"test"},"deadline_ms":9_000_000_000_000u64})
}
fn dispatch(h: &mut Harness, id: u64, method: &str, path: &str) -> Value {
    h.runtime.web(request(id, method, path)).unwrap();
    let responses = h.sent("HttpRespond");
    serde_json::from_str(responses.last().unwrap()["response"].as_str().unwrap()).unwrap()
}
#[test]
fn route_specificity_head_fallback_replacement_and_helpers() {
    let mut h = Harness::new(
        r#"
      (yinz-web-route "*" "/items/*rest" (lambda (r) (yinz-web-text (yinz-web-param r "rest"))))
      (yinz-web-route "HEAD" "/items/:id" (lambda (r) (yinz-web-text "parameter-head")))
      (yinz-web-route "GET" "/items/fixed" (lambda (r) (yinz-web-text (hash-ref r "method"))))
      (define old (yinz-web-route "GET" "/same" (lambda (r) (yinz-web-text "old"))))
      (yinz-web-route "GET" "/same" (lambda (r) (yinz-web-text "new"))) (old)
      (yinz-web-route "HEAD" "/only" (lambda (r) (yinz-web-text "only-head")))
    "#,
    );
    assert_eq!(dispatch(&mut h, 1, "HEAD", "/items/fixed")["body"], "HEAD");
    assert_eq!(
        dispatch(&mut h, 2, "HEAD", "/items/other")["body"],
        "parameter-head"
    );
    assert_eq!(dispatch(&mut h, 3, "GET", "/items/a/b")["body"], "a/b");
    assert_eq!(dispatch(&mut h, 4, "GET", "/same")["body"], "new");
    assert_eq!(dispatch(&mut h, 5, "GET", "/only")["status"], 404);
    assert_eq!(dispatch(&mut h, 6, "POST", "/only")["status"], 404);
    h.eval(r#"(yinz-web-route "HEAD" "/items/fixed" (lambda (r) (yinz-web-text "explicit")))"#);
    assert_eq!(
        dispatch(&mut h, 7, "HEAD", "/items/fixed")["body"],
        "explicit"
    );
    assert_eq!(dispatch(&mut h, 8, "GET", "/items/fixed/")["body"], "GET");
    for source in [
        r#"(yinz-web-route "GET" "/:a/:a" (lambda (_) #f))"#,
        r#"(yinz-web-route "GET" "/*a/b" (lambda (_) #f))"#,
    ] {
        assert!(h.runtime.eval(source).is_err());
    }
}
#[test]
fn failures_are_sanitized_json_null_is_valid_and_responses_send_once() {
    let mut h = Harness::new(
        r#"
      (define count 0)
      (yinz-web-route "GET" "/bad" (lambda (_) (error "secret exception")))
      (yinz-web-route "POST" "/json" (lambda (r) (yinz-web-json (yinz-web-body-json r))))
      (yinz-web-route "GET" "/invalid" (lambda (_) (hash "status" 200 "headers" (hash) "body" "bad")))
      (yinz-web-route "GET" "/later" (lambda (_) (set! count (+ count 1)) (yinz-then (yinz-wait-for-event "CustomMessage") (lambda (_) (yinz-web-text "done")))))
    "#,
    );
    assert_eq!(
        dispatch(&mut h, 1, "GET", "/bad")["body"],
        r#"{"error":{"code":"internal"}}"#
    );
    assert_eq!(dispatch(&mut h, 2, "GET", "/invalid")["status"], 500);
    assert_eq!(dispatch(&mut h, 3, "POST", "/json")["status"], 400);
    let mut req = request(4, "POST", "/json");
    req["headers"] = json!({"content-type":"application/json; charset=utf-8"});
    req["body"] = json!("null");
    h.runtime.web(req).unwrap();
    assert_eq!(h.sent("HttpRespond").last().unwrap()["request_id"], 4);
    h.runtime.web(request(5, "HEAD", "/later")).unwrap();
    h.runtime.web(request(5, "HEAD", "/later")).unwrap();
    assert_eq!(h.value("count"), 1);
    h.event(json!({"CustomMessage":["go",""]}));
    h.event(json!({"CustomMessage":["go",""]}));
    assert_eq!(h.sent("HttpRespond").len(), 5);
    assert_eq!(h.value("%async-operations"), json!({}));
    assert_eq!(h.value("%web-active"), json!({}));
}
#[test]
fn deadlines_retire_owned_chains_but_preserve_borrowed_operations() {
    let mut h = Harness::new(
        r#"
      (define callbacks 0)
      (define borrowed (yinz-wait-for-event "CustomMessage"))
      (define baseline (length (hash-keys->list %async-operations)))
      (yinz-web-route "GET" "/owned" (lambda (_) (yinz-then (yinz-wait-for-event "CustomMessage") (lambda (_) (set! callbacks (+ callbacks 1)) (yinz-web-text "late")))))
      (yinz-web-route "GET" "/borrowed" (lambda (_) (yinz-then borrowed (lambda (_) (set! callbacks (+ callbacks 1)) (yinz-web-text "late")))))
      (yinz-web-route "GET" "/ok" (lambda (_) (yinz-web-text "ok")))
    "#,
    );
    // Use immediate expiry after admission via the injected monotonic clock.
    let mut req = request(1, "GET", "/owned");
    // Wall time is real in this harness; calculate a short future absolute deadline in Scheme.
    h.eval("(define deadline (+ (yinz-wall-clock-ms) 100000))");
    req["deadline_ms"] = h.value("deadline");
    h.runtime.web(req.clone()).unwrap();
    req["id"] = json!(2);
    req["path"] = json!("/borrowed");
    req["segments"] = json!(["borrowed"]);
    h.runtime.web(req).unwrap();
    h.advance(100001);
    h.eval("(define pending-count (length (hash-keys->list %async-operations))) (define observers (length (vector-ref borrowed 3)))");
    assert_eq!(h.value("pending-count"), h.value("baseline"));
    assert_eq!(h.value("observers"), 0);
    h.event(json!({"CustomMessage":["late",""]}));
    assert_eq!(h.value("callbacks"), 0);
    assert!(h.sent("HttpRespond").is_empty());
    assert_eq!(dispatch(&mut h, 3, "GET", "/ok")["body"], "ok");
}
#[test]
fn hooks_pass_next_arguments_republish_and_preserve_wrappers_on_default_change() {
    let mut h = Harness::new(
        r#"
      (yinz-web-hook-define "row" (lambda (x) (yinz-h "p" (hash) x)))
      (yinz-web-page "/" "text/html; charset=utf-8" (lambda () (yinz-web-hook "row" "<first>")))
      (yinz-web-hook-wrap! "row" (lambda (next x) (yinz-h "div" (hash) (next (string-append x "!")))))
      (yinz-web-hook-wrap! "row" (lambda (next x) (next (string-append x "?"))))
      (yinz-web-hook-define "row" (lambda (x) (yinz-h "b" (hash) x)))
    "#,
    );
    assert_eq!(h.sent("HttpPublish").len(), 4);
    assert_eq!(
        h.sent("HttpPublish").last().unwrap()["body"],
        "<div><b>&lt;first&gt;?!</b></div>"
    );
    assert!(h.sent("HttpAssets").is_empty());
    // The standard app installs only on request; its defaults keep existing wrappers.
    h.eval(r#"(yinz-web-agent-app (hash "title" "</title><script>x</script>"))"#);
    assert_eq!(
        h.sent("HttpAssets"),
        vec![json!({"prefix":"/static/","directory":"builtin:web/static"})]
    );
    let pages = h.sent("HttpPublish");
    let page = |path: &str| {
        pages
            .iter()
            .rev()
            .find(|page| page["path"] == path)
            .unwrap()["body"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let login = page("/login");
    assert!(login.contains("<title>&lt;/title&gt;&lt;script&gt;x&lt;/script&gt;</title>"));
    assert!(login.contains(r#"\u003c/title\u003e\u003cscript\u003ex\u003c/script\u003e"#));
    assert!(!login.contains("<script>x"));
    assert!(
        login.contains(r#"<script src="/static/login.js" type="module"></script>"#),
        "{login}"
    );
    assert!(page("/").contains(r#"<div id="app"></div>"#));
    h.eval(r#"(yinz-web-hook-wrap! "shell" (lambda (next context) (cons (yinz-h "p" (hash "class" "wrapped") "w") (next context))))"#);
    let republished = h.sent("HttpPublish");
    let shell = republished
        .iter()
        .rev()
        .find(|page| page["path"] == "/")
        .unwrap();
    assert!(shell["body"]
        .as_str()
        .unwrap()
        .contains(r#"<p class="wrapped">w</p><div id="app">"#));
    h.eval("(define routes (length %web-routes)) (define hooks (yinz-web-hook-names))");
    let commands = h.commands.lock().unwrap().len();
    h.eval(r#"(yinz-web-agent-app (hash "title" "replacement"))"#);
    h.eval("(define routes-again (length %web-routes)) (define hooks-again (yinz-web-hook-names))");
    assert_eq!(h.commands.lock().unwrap().len(), commands);
    assert_eq!(h.value("routes"), h.value("routes-again"));
    assert_eq!(h.value("routes"), 22);
    assert_eq!(h.value("hooks"), h.value("hooks-again"));
}

#[test]
fn standard_app_failed_installation_raises_and_retry_reinstalls_once() {
    let mut h = Harness::new("");
    *h.failure.lock().unwrap() = Some("HttpPublish".into());
    assert!(h
        .runtime
        .eval("(yinz-web-agent-app (hash))")
        .unwrap_err()
        .to_string()
        .contains("HttpPublish"));
    assert_eq!(h.value("%web-app-installed"), false);
    *h.failure.lock().unwrap() = None;
    h.eval("(define routes (length %web-routes))");
    h.eval("(yinz-web-agent-app (hash))");
    h.eval("(define routes-after (length %web-routes))");
    assert_eq!(h.value("routes"), h.value("routes-after"));
    assert_eq!(h.value("%web-app-installed"), true);
    assert_eq!(h.sent("HttpAssets").len(), 2);
    let published: Vec<_> = h
        .sent("HttpPublish")
        .iter()
        .map(|page| page["path"].clone())
        .collect();
    // The failed attempt's "/" was dispatched before the injected failure.
    assert_eq!(published, vec![json!("/"), json!("/"), json!("/login")]);
    h.eval("(yinz-web-agent-app (hash))");
    assert_eq!(h.sent("HttpPublish").len(), 3);
}

#[test]
fn duplicate_json_keys_are_detected_at_every_depth() {
    let mut h = Harness::new("");
    h.eval(
        r#"(define checks (map %web-json-unique-keys? (list "{\"a\":1,\"b\":[{\"a\":1}]}" "{\"a\":1,\"a\":2}" "[{\"x\":{\"y\":1,\"y\":1}}]" "null" "{" "{} {}")))"#,
    );
    assert_eq!(
        h.value("checks"),
        json!([true, false, false, true, false, false])
    );
}

#[test]
fn request_scopes_release_rpc_sources_nested_work_and_borrowed_observers() {
    let mut h = Harness::new(
        r#"
      (define effects 0)
      (define shared (yinz-rpc-call (hash "pluginId" 8 "name" "shared")))
      (define shared-observed #f)
      (yinz-then shared (lambda (value) (set! shared-observed value)))
      (yinz-web-route "GET" "/rpc" (lambda (_) (yinz-then
        (yinz-rpc-call (hash "pluginId" 8 "name" "owned" "onProgress" (lambda (_) (set! effects (+ effects 1)))))
        (lambda (_) (set! effects (+ effects 1)) (yinz-web-text "late")))))
      (yinz-web-route "GET" "/view" (lambda (_) (yinz-then (yinz-agent-view (hash "op" "list"))
        (lambda (_) (set! effects (+ effects 1)) (yinz-web-text "late")))))
      (yinz-web-route "GET" "/nested" (lambda (_) (yinz-then (yinz-sleep 1)
        (lambda (_) (yinz-catch (yinz-then (yinz-wait-for-event "CustomMessage")
          (lambda (_) (set! effects (+ effects 1)))) (lambda (_) (set! effects (+ effects 1))))))))
      (yinz-web-route "GET" "/borrow" (lambda (_) (yinz-then shared (lambda (_) (set! effects (+ effects 1))))))
    "#,
    );
    h.eval("(define deadline (+ (yinz-wall-clock-ms) 100000))");
    for (id, path) in [(1, "/rpc"), (2, "/view"), (3, "/nested"), (4, "/borrow")] {
        let mut req = request(id, "GET", path);
        req["deadline_ms"] = h.value("deadline");
        h.runtime.web(req).unwrap();
    }
    h.advance(1);
    h.advance(100001);
    h.eval("(define calls (length (hash-keys->list %rpc-calls))) (define ops (length (hash-keys->list %async-operations))) (define observers (length (vector-ref shared 3))) (define handlers (length (hash-keys->list %async-handlers))) (define timers (length (hash-keys->list %async-timers)))");
    assert_eq!(h.value("calls"), 1);
    assert_eq!(h.value("ops"), 2);
    assert_eq!(h.value("observers"), 1);
    assert_eq!(h.value("handlers"), 0);
    assert_eq!(h.value("timers"), 0);
    for cmd in h.sent("MessageToPlugin") {
        let call: Value = serde_json::from_str(cmd["message_payload"].as_str().unwrap()).unwrap();
        let id = call["call_id"].as_str().unwrap();
        h.runtime.pipe(json!({"name":"yinz_rpc_progress","source":{"Plugin":8},"payload":json!({"version":1,"call_id":id,"payload":1}).to_string()})).unwrap();
        h.runtime.pipe(json!({"name":"yinz_rpc_reply","source":{"Plugin":8},"payload":json!({"version":1,"call_id":id,"result":"shared-result"}).to_string()})).unwrap();
    }
    for cmd in h.sent("AgentView") {
        h.event(json!({"CustomMessage":["agent-view-response",json!({"id":cmd["id"],"reply":{"ok":true}}).to_string()]}));
    }
    h.event(json!({"CustomMessage":["late",""]}));
    assert_eq!(h.value("effects"), 0);
    assert_eq!(h.value("shared-observed"), "shared-result");
    assert!(h.sent("HttpRespond").is_empty());
}
#[test]
fn scoped_expiry_churn_and_cleanup_failures_do_not_exhaust_the_shared_budget() {
    let mut h = Harness::new(
        r#"
      (define effects 0)
      (define (spawn-owned)
        (define owned (yinz-deferred))
        (%async-set-cancel! owned (lambda () (void)))
        (vector-set! owned 4 (lambda () (error "cleanup-failed")))
        (yinz-on-event "CustomMessage" (lambda (_) (set! effects (+ effects 1))))
        (yinz-then (yinz-wait-for-event "CustomMessage") (lambda (_) (set! effects (+ effects 1)))))
      (define (exercise-scopes n)
        (unless (= n 0)
          (define scope (%async-scope-new))
          (%async-with-scope scope spawn-owned)
          (%async-scope-close scope)
          (exercise-scopes (- n 1))))
    "#,
    );
    h.eval("(exercise-scopes 1400)");
    h.event(json!({"CustomMessage":["late",""]}));
    h.eval("(define pending (length (hash-keys->list %async-operations))) (define owners (length (hash-keys->list %async-owners))) (define waiters (length (hash-keys->list %async-events))) (define handlers (length (hash-keys->list %async-handlers)))");
    for name in ["pending", "owners", "waiters", "handlers", "effects"] {
        assert_eq!(h.value(name), 0, "{name}");
    }
}
#[test]
fn chained_cancellation_detaches_without_claiming_its_source_and_fences_queued_callbacks() {
    let mut h = Harness::new(
        r#"
      (define effects 0)
      (define source (yinz-deferred))
      (define child (yinz-then source (lambda (_) (set! effects (+ effects 1)))))
      (yinz-cancel! child)
      (yinz-resolve! source #t)
      (define scope (%async-scope-new))
      (%async-with-scope scope (lambda () (yinz-then #t (lambda (_) (set! effects (+ effects 1))))))
      (%async-scope-close scope)
    "#,
    );
    h.eval("(void)");
    assert_eq!(h.value("effects"), 0);
}

#[test]
fn response_page_prefix_and_registration_limits_fail_before_publication() {
    let mut h = Harness::new("");
    for source in [
        r#"(%web-validate-response (hash "status" 199 "headers" (hash) "body" ""))"#,
        r#"(%web-validate-response (hash "status" 304 "headers" (hash "content-type" "text/plain") "body" "x"))"#,
        r#"(%web-validate-response (hash "status" 200 "headers" (hash "set-cookie" "bad") "body" ""))"#,
        r#"(%web-validate-response (hash "status" 200 "headers" (hash "x-yinz-x" "a\r\nb") "body" ""))"#,
        r#"(yinz-web-redirect "//evil")"#,
        r#"(yinz-web-redirect "/%2f%2fevil")"#,
        r#"(yinz-web-page "/api/bad" "text/plain; charset=utf-8" (lambda () "bad"))"#,
        r#"(yinz-web-page "/bad" "application/javascript" (lambda () "bad"))"#,
        r#"(yinz-web-page "/large" "text/plain; charset=utf-8" (lambda () (make-string 524289 #\x)))"#,
        r#"(yinz-web-assets "/api/static/" "static")"#,
    ] {
        assert!(h.runtime.eval(source).is_err(), "accepted {source}");
    }
    assert!(h.sent("HttpPublish").is_empty());
    assert!(h.sent("HttpAssets").is_empty());
    h.eval(r#"(for-each (lambda (i) (yinz-web-assets (string-append "/s" (number->string i) "/") "static")) (range 0 4))"#);
    assert!(h
        .runtime
        .eval(r#"(yinz-web-assets "/extra/" "static")"#)
        .is_err());
    h.eval(r#"(for-each (lambda (i) (yinz-web-page (string-append "/p" (number->string i)) "text/plain; charset=utf-8" (lambda () "ok"))) (range 0 16))"#);
    h.eval(r#"(yinz-web-page "/p0" "text/plain; charset=utf-8" (lambda () "replacement"))"#);
    assert!(h
        .runtime
        .eval(r#"(yinz-web-page "/extra" "text/plain; charset=utf-8" (lambda () "bad"))"#)
        .is_err());
}
#[test]
fn web_expiry_churn_recent_fifo_and_send_failure_leave_healthy_dispatch() {
    let mut h = Harness::new(
        r#"
      (define callbacks 0)
      (yinz-web-route "GET" "/expire" (lambda (_) (yinz-catch (yinz-then (yinz-wait-for-event "CustomMessage") (lambda (_) (set! callbacks (+ callbacks 1)))) (lambda (_) (set! callbacks (+ callbacks 1))))))
      (yinz-web-route "GET" "/ok" (lambda (_) (yinz-web-text "ok")))
      (define deadline (+ (yinz-wall-clock-ms) 100000))
    "#,
    );
    for id in 0..1400 {
        let mut req = request(id, "GET", "/expire");
        req["deadline_ms"] = h.value("deadline");
        h.runtime.web(req).unwrap();
        h.advance(100001);
    }
    h.event(json!({"CustomMessage":["late",""]}));
    h.eval("(define pending (length (hash-keys->list %async-operations))) (define recent (length %web-recent))");
    assert_eq!(h.value("pending"), 0);
    assert_eq!(h.value("recent"), 1024);
    assert_eq!(h.value("callbacks"), 0);
    *h.failure.lock().unwrap() = Some("HttpRespond".into());
    dispatch(&mut h, 1401, "GET", "/ok");
    assert_eq!(h.sent("HttpRespond").len(), 1);
    *h.failure.lock().unwrap() = None;
    assert_eq!(dispatch(&mut h, 1402, "GET", "/ok")["body"], "ok");
}

#[test]
fn concurrent_requests_keep_owned_callbacks_and_actors_isolated_and_unload_closes_all() {
    let mut h = Harness::new(
        r#"
      (define effects '())
      (yinz-web-route "GET" "/wait/:name" (lambda (r)
        (yinz-then (yinz-wait-for-event "CustomMessage")
          (lambda (_) (set! effects (cons (yinz-web-param r "name") effects))
            (yinz-web-text (yinz-web-param r "name"))))))
      (define deadline (+ (yinz-wall-clock-ms) 100000))
    "#,
    );
    let mut first = request(1, "GET", "/wait/expired");
    first["deadline_ms"] = h.value("deadline");
    h.runtime.web(first).unwrap();
    h.runtime
        .invoke("yinz-async-enter", vec![json!(3)])
        .unwrap();
    h.runtime.web(request(2, "HEAD", "/wait/healthy")).unwrap();
    h.advance(100001);
    h.event(json!({"CustomMessage":["go",""]}));
    assert_eq!(h.value("effects"), json!(["healthy"]));
    let replies = h.sent("HttpRespond");
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["request_id"], 2);
    let commands = h.commands.lock().unwrap();
    assert_eq!(
        commands
            .iter()
            .find(|(c, _)| c.get("HttpRespond").is_some())
            .unwrap()
            .1,
        3
    );
    drop(commands);
    h.runtime.web(request(3, "GET", "/wait/unload")).unwrap();
    h.runtime.unload().unwrap();
    h.event(json!({"CustomMessage":["late",""]}));
    assert_eq!(h.value("effects"), json!(["healthy"]));
    h.eval("(define pending (length (hash-keys->list %async-operations)))");
    assert_eq!(h.value("pending"), 0);
}
#[test]
fn deadline_and_agent_view_timer_setup_failures_do_not_start_handler_work() {
    let mut h = Harness::new(
        r#"(define effects 0) (yinz-web-route "GET" "/" (lambda (_) (set! effects (+ effects 1)) (yinz-web-text "unexpected")))"#,
    );
    *h.failure.lock().unwrap() = Some("SetTimeout".into());
    assert_eq!(dispatch(&mut h, 1, "GET", "/")["status"], 500);
    assert_eq!(h.value("effects"), 0);
    h.eval("(define failure #f) (yinz-catch (yinz-agent-view (hash \"op\" \"list\")) (lambda (error) (set! failure (yinz-agent-view-error error))))");
    assert!(h.sent("AgentView").is_empty());
    assert_eq!(h.value("failure")["origin"], "sdk");
}

#[test]
fn false_sync_or_deferred_results_are_invalid_responses_not_silent_timeouts() {
    let mut h = Harness::new(
        r#"
      (yinz-web-route "GET" "/false" (lambda (_) #f))
      (yinz-web-route "GET" "/deferred-false" (lambda (_) (yinz-then #t (lambda (_) #f))))
      (yinz-web-route "GET" "/rejected" (lambda (_) (define result (yinz-deferred)) (yinz-reject! result "private") result))
    "#,
    );
    for (id, path) in [(1, "/false"), (2, "/deferred-false"), (3, "/rejected")] {
        assert_eq!(dispatch(&mut h, id, "HEAD", path)["status"], 500);
    }
}

#[test]
fn request_owned_event_handler_failure_completes_once_and_preserves_other_work() {
    let mut h = Harness::new(
        r#"
      (yinz-web-route "GET" "/listener" (lambda (_)
        (yinz-on-event "CustomMessage" (lambda (_) (error "private listener failure")))
        (yinz-deferred)))
      (yinz-web-route "GET" "/ok" (lambda (_) (yinz-web-text "ok")))
    "#,
    );
    h.runtime.web(request(1, "GET", "/listener")).unwrap();
    h.event(json!({"CustomMessage":["go",""]}));
    let response: Value =
        serde_json::from_str(h.sent("HttpRespond")[0]["response"].as_str().unwrap()).unwrap();
    assert_eq!(response["status"], 500);
    assert!(!response.to_string().contains("private"));
    h.event(json!({"CustomMessage":["late",""]}));
    assert_eq!(h.sent("HttpRespond").len(), 1);
    assert_eq!(dispatch(&mut h, 2, "GET", "/ok")["body"], "ok");
}

#[test]
fn service_timer_failures_stay_request_local_and_release_owned_work() {
    for timer in ["yinz-after", "yinz-every"] {
        for callback in [
            r#"(error "private-timer-failure")"#,
            r#"(define rejected (yinz-deferred)) (yinz-reject! rejected "private-rejection") rejected"#,
        ] {
            let mut h = Harness::new(&format!(
                r#"
              (define borrowed (yinz-deferred))
              (define closed-scope #f)
              (yinz-web-route "GET" "/bad" (lambda (_)
                (yinz-then ({timer} 0 (lambda () {callback})) (lambda (_) (yinz-web-text "unexpected")))))
              (yinz-web-route "GET" "/background" (lambda (_)
                (set! closed-scope %async-scope)
                ({timer} 0 (lambda () {callback})) (yinz-deferred)))
              (yinz-web-route "GET" "/wait" (lambda (_) (yinz-then borrowed (lambda (_) (yinz-web-text "concurrent")))))
              (yinz-web-route "GET" "/ok" (lambda (_) (yinz-web-text "healthy")))
            "#
            ));
            for (id, path) in [(1, "/bad"), (2, "/background")] {
                h.runtime.web(request(id, "GET", path)).unwrap();
            }
            h.runtime
                .invoke("yinz-async-enter", vec![json!(3)])
                .unwrap();
            h.runtime.web(request(3, "GET", "/wait")).unwrap();
            h.advance(1); // Also asserts Timer update succeeds, rather than merely observing 500.
            h.advance(1);
            h.eval("(%async-with-scope closed-scope (lambda () (%async-fail \"late failure\")))");
            let replies = h.sent("HttpRespond");
            assert_eq!(replies.len(), 2);
            for reply in replies {
                let response: Value =
                    serde_json::from_str(reply["response"].as_str().unwrap()).unwrap();
                assert_eq!(response["status"], 500);
                assert_eq!(response["body"], r#"{"error":{"code":"internal"}}"#);
            }
            h.eval("(yinz-resolve! borrowed #t)");
            assert_eq!(h.sent("HttpRespond").last().unwrap()["request_id"], 3);
            assert_eq!(
                h.commands
                    .lock()
                    .unwrap()
                    .iter()
                    .rev()
                    .find(|(c, _)| c.get("HttpRespond").is_some())
                    .unwrap()
                    .1,
                3
            );
            assert_eq!(dispatch(&mut h, 4, "GET", "/ok")["body"], "healthy");
            h.eval("(define counts (list (length (hash-keys->list %async-operations)) (length (hash-keys->list %async-timers)) (length (hash-keys->list %async-owners)) (length (hash-keys->list %async-events)) (length %async-jobs) (hash-ref %async-required \"Timer\")))");
            assert_eq!(h.value("counts"), json!([0, 0, 0, 0, 0, 0]));
        }
    }
}

#[test]
fn handled_failures_log_the_original_named_source_and_line() {
    let mut h = Harness::new("");
    h.runtime.eval_with_path(
        "(define (private-sync)\n  (error \"private-sync-failure\"))\n(define (private-chain _)\n  (error \"private-chain-failure\"))\n(yinz-web-route \"GET\" \"/sync\" (lambda (_) (private-sync)))\n(yinz-web-route \"GET\" \"/chain\" (lambda (_) (yinz-then (yinz-sleep 0) private-chain)))",
        Some(std::path::Path::new("/virtual/web-helper.scm")),
    ).unwrap();
    assert_eq!(dispatch(&mut h, 1, "GET", "/sync")["status"], 500);
    h.runtime.web(request(2, "GET", "/chain")).unwrap();
    h.advance(1);
    let logs = h.runtime.take_logs().unwrap();
    for expected in [
        "private-sync-failure",
        "private-chain-failure",
        "web-helper.scm:2",
        "web-helper.scm:4",
    ] {
        assert!(logs.contains(expected), "missing {expected}: {logs}");
    }
    assert!(h.runtime.take_logs().unwrap().is_empty());
    assert!(h.runtime.take_output().unwrap().is_empty());
    for reply in h.sent("HttpRespond") {
        let response = reply["response"].as_str().unwrap();
        assert!(!response.contains("private"));
        assert!(!response.contains("web-helper"));
    }
}

#[test]
fn service_registration_and_cleanup_failures_use_the_saved_owner() {
    let mut h = Harness::new(
        r#"
      (yinz-web-route "GET" "/registration" (lambda (_)
        (define arm %async-arm-timer)
        (set! %async-arm-timer (lambda () (error "private-registration")))
        (define handle (yinz-after 0 (lambda () (error "must not run"))))
        (set! %async-arm-timer arm)
        handle))
      (define cleanup-resource #f)
      (yinz-web-route "GET" "/cleanup" (lambda (_)
        (define handle (yinz-deferred))
        (vector-set! handle 4 (lambda () (error "private-cleanup")))
        (set! cleanup-resource handle)
        (yinz-then handle (lambda (_) (yinz-web-text "unexpected")))))
      (yinz-web-route "GET" "/ok" (lambda (_) (yinz-web-text "ok")))
    "#,
    );
    assert_eq!(dispatch(&mut h, 1, "GET", "/registration")["status"], 500);
    h.runtime.web(request(2, "GET", "/cleanup")).unwrap();
    h.runtime
        .invoke("yinz-async-enter", vec![json!(9)])
        .unwrap();
    h.eval("(yinz-resolve! cleanup-resource #t)");
    assert_eq!(
        h.commands
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(c, _)| c.get("HttpRespond").is_some())
            .unwrap()
            .1,
        2
    );
    let responses = h.sent("HttpRespond");
    assert_eq!(responses.len(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(responses[1]["response"].as_str().unwrap()).unwrap()
            ["status"],
        500
    );
    assert_eq!(h.value("%async-owners"), json!({}));
    assert_eq!(h.value("%async-operations"), json!({}));
    assert_eq!(dispatch(&mut h, 3, "GET", "/ok")["body"], "ok");
}

#[test]
fn scoped_timer_failure_never_erases_an_unrelated_global_failure() {
    let mut h = Harness::new(
        r#"
      (define (unscoped-failure) (error "unrelated-global-failure"))
      (yinz-after 0 unscoped-failure)
      (yinz-web-route "GET" "/scoped" (lambda (_)
        (yinz-after 0 (lambda () (error "request-local-failure")))
        (define resource (yinz-deferred))
        (vector-set! resource 4 (lambda () (error "request-cleanup-failure")))
        resource))
    "#,
    );
    h.runtime.web(request(1, "GET", "/scoped")).unwrap();
    let failure = h
        .runtime
        .update(json!({"Timer":0.0}))
        .unwrap_err()
        .to_string();
    assert!(failure.contains("unrelated-global-failure"), "{failure}");
    assert!(!failure.contains("request-local-failure"), "{failure}");
    let responses = h.sent("HttpRespond");
    assert_eq!(responses.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(responses[0]["response"].as_str().unwrap()).unwrap()
            ["status"],
        500
    );
    assert_eq!(h.value("%async-operations"), json!({}));
}

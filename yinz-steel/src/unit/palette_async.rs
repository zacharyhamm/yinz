use crate::panes_tests::Harness;
use serde_json::{json, Value};

fn scope(registration: Value, revision: u32) -> Value {
    json!({"registration":registration,"opening":1,"view":0,"revision":revision})
}
fn registered(harness: &Harness) -> Value {
    harness
        .sent("CommandPalette")
        .into_iter()
        .find_map(|value| (value["type"] == "register").then(|| value["registration"].clone()))
        .unwrap()
}
fn send(harness: &mut Harness, client: u32, mut message: Value) {
    harness
        .runtime
        .invoke("yinz-async-enter", vec![json!(client)])
        .unwrap();
    message["context"] =
        json!({"clientId":client,"paneId":{"Terminal":7},"tabId":1,"currentProject":null});
    harness.runtime.palette(message).unwrap();
}
fn replies(harness: &Harness) -> Vec<Value> {
    harness
        .sent("CommandPalette")
        .into_iter()
        .filter(|v| v["type"] == "reply")
        .collect()
}

#[test]
fn deferred_discovery_preserves_clients_and_discards_cancelled_replies() {
    let mut h = Harness::new(
        r#"
      (define pending (hash)) (define cancelled '())
      (define (load config)
        (yinz-palette-register "async" "Async" (lambda (query context)
          (define result (yinz-deferred))
          ((hash-ref context "onCancel") (lambda ()
            (set! cancelled (append cancelled (list (list query %async-client ((hash-ref context "cancelled"))))))))
          (set! pending (hash-insert pending query result))
          (yinz-then result (lambda (_)
            (yinz-command (hash "SetSelectable" #t))
            (list (hash "id" query "label" query "run" (lambda (_ context) json-null))))))))
    "#,
    );
    h.runtime.load(json!({})).unwrap();
    let registration = registered(&h);
    let first = scope(registration.clone(), 1);
    let second = scope(registration, 2);
    send(
        &mut h,
        2,
        json!({"type":"discover","source":null,"query":"first","scope":first}),
    );
    send(
        &mut h,
        3,
        json!({"type":"discover","source":null,"query":"second","scope":second}),
    );
    assert!(replies(&h).is_empty());
    // A peer cannot cancel a task belonging to another acting client.
    send(&mut h, 3, json!({"type":"cancel","scope":first}));
    assert_eq!(h.value("cancelled"), json!([]));
    send(&mut h, 2, json!({"type":"cancel","scope":first}));
    send(&mut h, 2, json!({"type":"cancel","scope":first}));
    assert_eq!(h.value("cancelled"), json!([["first", 2, true]]));
    h.runtime
        .invoke("yinz-async-enter", vec![json!(9)])
        .unwrap();
    h.eval(r#"(yinz-resolve! (hash-ref pending "second") #t) (yinz-resolve! (hash-ref pending "first") #t)"#);
    let replies = replies(&h);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["scope"], second);
    assert_eq!(replies[0]["result"]["Ok"]["actions"][0]["label"], "second");
    assert!(h
        .commands
        .lock()
        .unwrap()
        .iter()
        .filter(|(v, _)| v.pointer("/CommandPalette/type") == Some(&json!("reply")))
        .all(|(_, actor)| actor == 3));
    h.eval("(define tasks-left (length (hash-keys->list %palette-tasks)))");
    assert_eq!(h.value("tasks-left"), json!(0));
}

#[test]
fn an_explicit_close_survives_dismissal_and_finishes_with_the_original_client() {
    let mut h = Harness::new(
        r#"
      (define pending #f) (define cancelled 0) (define finished #f)
      (define (load config)
        (yinz-palette-register "close" "Close" (list
          (hash "id" "close" "label" "Close" "run" (lambda (_ context)
            (set! pending (yinz-deferred))
            ((hash-ref context "onCancel") (lambda () (set! cancelled (+ cancelled 1))))
            ((hash-ref context "suspend") #t)
            ((hash-ref context "close")) ((hash-ref context "close"))
            (yinz-then pending (lambda (_)
              (set! finished %async-client)
              (hash "type" "complete" "notification" "Done" "view" (hash "type" "text" "title" "Late" "text" "hidden")))))))))
    "#,
    );
    h.runtime.load(json!({})).unwrap();
    let registration = registered(&h);
    send(
        &mut h,
        2,
        json!({"type":"discover","source":null,"query":"","scope":scope(registration.clone(),1)}),
    );
    let handle = replies(&h)[0]["result"]["Ok"]["actions"][0]["handle"].clone();
    send(
        &mut h,
        2,
        json!({"type":"execute","handle":handle,"values":null,"scope":scope(registration,2)}),
    );
    assert_eq!(
        h.sent("CommandPalette")
            .iter()
            .filter(|v| v["type"] == "close")
            .count(),
        1
    );
    send(&mut h, 2, json!({"type":"dismiss","opening":1}));
    assert_eq!(h.value("cancelled"), json!(0));
    h.runtime
        .invoke("yinz-async-enter", vec![json!(9)])
        .unwrap();
    h.eval("(yinz-resolve! pending #t)");
    assert_eq!(h.value("finished"), json!(2));
    assert_eq!(
        replies(&h).last().unwrap()["result"]["Ok"],
        json!({"type":"complete","notification":"Done","view":null})
    );
}

#[test]
fn disconnect_and_unregister_cancel_pending_actions_once_and_suppress_late_results() {
    for disconnect in [true, false] {
        let mut h = Harness::new(
            r#"
          (define pending #f) (define cancelled '()) (define unregister #f)
          (define (load config)
            (set! unregister (yinz-palette-register "actions" "Actions" (list
              (hash "id" "work" "label" "Work" "run" (lambda (_ context)
                (set! pending (yinz-deferred))
                ((hash-ref context "onCancel") (lambda () (set! cancelled (append cancelled (list %async-client)))))
                pending))))))
        "#,
        );
        h.runtime.load(json!({})).unwrap();
        let registration = registered(&h);
        send(
            &mut h,
            2,
            json!({"type":"discover","source":null,"query":"","scope":scope(registration.clone(),1)}),
        );
        let handle = replies(&h)[0]["result"]["Ok"]["actions"][0]["handle"].clone();
        send(
            &mut h,
            2,
            json!({"type":"execute","handle":handle,"values":null,"scope":scope(registration,2)}),
        );
        if disconnect {
            send(&mut h, 2, json!({"type":"disconnect"}));
            send(&mut h, 2, json!({"type":"disconnect"}));
        } else {
            h.eval("(unregister) (unregister)");
        }
        assert_eq!(h.value("cancelled"), json!([2]));
        h.eval(r#"(yinz-resolve! pending (hash "type" "text" "title" "Late" "text" "obsolete"))"#);
        assert_eq!(replies(&h).len(), 1);
    }
}

#[test]
fn deferred_form_completion_serializes_callbacks_and_errors_stay_in_the_palette() {
    let mut h = Harness::new(
        r#"
      (define (load config)
        (yinz-palette-register "forms" "Forms" (list
          (hash "id" "form" "label" "Form" "run" (lambda (_ context)
            (yinz-then json-null (lambda (_)
              (hash "type" "complete" "notification" "Ready" "view"
                (hash "type" "form" "title" "Details" "fields" (list (hash "type" "text" "id" "name" "label" "Name"))
                  "onSubmit" (lambda (values context)
                    (yinz-then json-null (lambda (_) (error "Name required\ntry again")))))))))))))
    "#,
    );
    h.runtime.load(json!({})).unwrap();
    let registration = registered(&h);
    send(
        &mut h,
        2,
        json!({"type":"discover","source":null,"query":"","scope":scope(registration.clone(),1)}),
    );
    let handle = replies(&h)[0]["result"]["Ok"]["actions"][0]["handle"].clone();
    send(
        &mut h,
        2,
        json!({"type":"execute","handle":handle,"values":null,"scope":scope(registration.clone(),2)}),
    );
    let form = replies(&h).last().unwrap()["result"]["Ok"]["view"].clone();
    assert_eq!(form["type"], "form");
    assert!(form["handle"].is_string());
    send(
        &mut h,
        2,
        json!({"type":"execute","handle":form["handle"],"values":{"name":""},"scope":scope(registration,3)}),
    );
    assert_eq!(
        replies(&h).last().unwrap()["result"]["Err"],
        "Name required try again"
    );
    // Catching a palette failure must keep ordinary plugin callbacks healthy.
    h.runtime
        .update(json!({"CustomMessage":["alive",""]}))
        .unwrap();
}

#[test]
fn source_cleanup_retains_the_acting_client_and_runs_once() {
    let mut h = Harness::new(
        r#"
      (define cleaned '())
      (define (load config)
        (yinz-palette-register "cleanup" "Cleanup" (list
          (hash "id" "open" "label" "Open" "run" (lambda (_ context)
            (hash "type" "actions" "title" "Inventory"
              "source" (lambda (query context) '())
              "onClose" (lambda () (set! cleaned (cons %async-client cleaned)))))))))
    "#,
    );
    h.runtime.load(json!({})).unwrap();
    let registration = registered(&h);
    send(
        &mut h,
        2,
        json!({"type":"discover","source":null,"query":"","scope":scope(registration.clone(),1)}),
    );
    let handle = replies(&h)[0]["result"]["Ok"]["actions"][0]["handle"].clone();
    send(
        &mut h,
        2,
        json!({"type":"execute","handle":handle,"values":null,"scope":scope(registration,2)}),
    );
    let source = replies(&h).last().unwrap()["result"]["Ok"]["view"]["source"].clone();
    send(&mut h, 3, json!({"type":"release","handles":[source]}));
    assert_eq!(h.value("cleaned"), json!([]));
    send(&mut h, 2, json!({"type":"release","handles":[source]}));
    send(&mut h, 2, json!({"type":"dismiss","opening":1}));
    assert_eq!(h.value("cleaned"), json!([2]));
}

#[test]
fn table_sdk_round_trip_preserves_cells_columns_and_callbacks() {
    let mut h = Harness::new("");
    h.eval(
        r#"
      (define wired (%palette-wire-view
        (hash "type" "actions" "title" "Records"
          "columns" (list (hash "header" "Number" "width" 8)
                          (hash "header" "Subject" "maxLines" 3))
          "actions" (list
            (hash "id" "42" "label" "Record 雪" "aliases" '("Zoë")
              "cells" '("42" "Unicode 雪 🦀 subject")
              "run" (lambda (values context) "original callback"))
            (hash "id" "jump" "label" "Jump" "run" (lambda (values context) json-null))))
        (hash "registration" "1" "opening" 1 "view" 1 "revision" 1) 7))
    "#,
    );
    let wired = h.value("wired");
    assert_eq!(
        wired["columns"],
        json!([{"header":"Number","width":8},{"header":"Subject","maxLines":3}])
    );
    assert_eq!(
        wired["actions"][0]["cells"],
        json!(["42", "Unicode 雪 🦀 subject"])
    );
    assert!(wired["actions"][1].get("cells").is_none());
    let view: yinz_types::command_palette::View = serde_json::from_value(wired).unwrap();
    assert!(yinz_types::command_palette::Reply::View { view }
        .validate()
        .is_ok());
    h.eval(r#"(define callback-result ((hash-ref (hash-ref %palette-handles (hash-ref (car (hash-ref wired "actions")) "handle")) "callback") (hash) (hash)))"#);
    assert_eq!(h.value("callback-result"), json!("original callback"));
    h.eval(
        r#"(define legacy (%palette-wire-view (hash "type" "actions" "title" "Legacy") (hash) 7))"#,
    );
    assert!(h.value("legacy").get("columns").is_none());
}

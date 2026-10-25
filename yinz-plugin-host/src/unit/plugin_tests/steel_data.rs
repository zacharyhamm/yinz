//! Real SQLite effects through the Scheme command and watch helpers.
use super::script_harness::{load, release_event, run_plugin, Harness};
use super::*;
use std::time::Duration;
const HELPERS: &str = include_str!("fixtures/data_helpers.scm");

#[test]
fn steel_data_values_conflict_rollback_watch_disposal_and_namespace_persistence() {
    let source = format!(
        "{HELPERS}{}",
        r#"
(yinz-declare-pane)
(define state "starting")
(define (load config)
  (define denied (with-handler (lambda (failure) (error-object-message failure))
    (yinz-data-apply json-null (list (yinz-data-put "items" (hash "text" "ungranted"))))
    "unexpected success"))
  (assert-data (string-contains? denied "PluginData"))
  (yinz-catch
    (yinz-then (yinz-wait-for-permissions (list "PluginData"))
      (lambda (ignored)
        (define shared (data-definition "shared_item" (list (list "text" "Text"))))
        (define denial (with-handler (lambda (failure) (error-object-message failure))
          (yinz-data-define "shared:steel_sdk" shared) "unexpected success"))
        (assert-data (string-contains? denial "SharedPluginData"))
        (yinz-then (yinz-wait-for-permissions (list "SharedPluginData"))
          (lambda (ignored)
            (yinz-data-define "shared:steel_sdk" shared)
            (yinz-data-apply "shared:steel_sdk" (list
              (yinz-data-put "shared_item" (hash "text" "shared from Scheme") (hash "uid" "shared"))))
            (yinz-data-define json-null (data-definition "items"
              (list (list "n" "Int") (list "r" "Real") (list "text" "Text") (list "json" "Json")
                (list "blob" "Blob") (list "flag" "Bool") (list "nullable" "Text")
                (list "host_id" "Text") (list "yinz_session_name" "Text"))))
            (define values (list
              (list "n" (hash "Int" 9223372036854775807)) (list "r" (hash "Real" 1.0))
              (list "text" (hash "Text" "hello")) (list "json" (hash "Text" "{\"nested\":[1,true]}"))
              (list "blob" (hash "Blob" (list 0 128 255))) (list "flag" (hash "Bool" #f))
              (list "nullable" json-null) (list "host_id" (hash "Text" "author-host"))
              (list "yinz_session_name" (hash "Text" "author-session"))))
            (yinz-data-apply json-null (list (yinz-data-put "items" values (hash "uid" "row" "expect_rev" 0))))
            (define row-filter (yinz-data-eq "uid" "row"))
            (define query (yinz-data-query "items" (hash "filter" row-filter)))
            (define first (yinz-data-fetch json-null query))
            (define second (yinz-data-fetch json-null query))
            (assert-data (= (hash-ref (yinz-data-row first (car (hash-ref first "rows"))) "rev") (data-value first "rev")))
            (assert-data (= (data-value first "n") 9223372036854775807))
            (assert-data (= (data-value first "r") 1))
            (assert-data (equal? (data-field first "r") (hash "Real" 1.0)))
            (assert-data (equal? (data-value first "flag") #f))
            (assert-data (json-null? (data-value first "nullable")))
            (assert-data (equal? (data-value first "json") "{\"nested\":[1,true]}"))
            (assert-data (equal? (data-value first "blob") (bytes 0 128 255)))
            (assert-data (equal? (data-value first "host_id") "author-host"))
            (assert-data (equal? (data-value first "yinz_session_name") "author-session"))
            (assert-data (not (null? (filter (lambda (entry) (equal? (hash-ref (hash-ref entry "def") "name") "items"))
              (yinz-command (hash "DataCatalog" (hash "namespace" json-null)))))))
            (define (rename text revision)
              (data-update "items" row-filter (list (list "text" (hash "Text" text))) revision))
            (yinz-data-apply json-null (list (rename "second author" (data-value second "rev"))))
            (define conflict (with-handler (lambda (failure) (string-contains? (error-object-message failure) "conflict"))
              (yinz-data-apply json-null (list (yinz-data-put "items" values (hash "uid" "rollback"))
                (yinz-data-put "items" (hash "text" "stale author")
                  (hash "uid" "row" "expect_rev" (data-value first "rev"))))) #f))
            (assert-data conflict)
            (assert-data (= (yinz-data-count json-null (data-query "items")) 1))
            (assert-data (equal? (data-value (yinz-data-fetch json-null query) "text") "second author"))
            (define combined (hash "And" (list (hash "Gt" (list "n" (hash "Int" 0)))
              (hash "Not" (data-eq "flag" (hash "Bool" #t))) (hash "Contains" (list "text" "author")))))
            (assert-data (= (length (hash-ref (yinz-data-fetch json-null (data-query "items" combined)) "rows")) 1))
            (define changes 0)
            (define watch (yinz-data-watch-callback json-null query
              (lambda (rows event) (set! changes (+ changes 1))
                (assert-data (equal? (data-value rows "text") "watched")) #t)))
            (assert-data (equal? (data-value (hash-ref watch "initial") "text") "second author"))
            (define changed (yinz-wait-for-event "PluginDataChanged"
              (lambda (event) (equal? (hash-ref event "watch_id") (hash-ref watch "watch-id")))))
            (yinz-data-apply json-null (list (rename "watched" json-null)))
            (yinz-then changed
              (lambda (ignored)
                (assert-data (= changes 1))
                ((hash-ref watch "dispose")) ((hash-ref watch "dispose"))
                (yinz-data-apply json-null (list (rename "persisted" json-null)))
                (set! state "data helpers: watch disposed") (yinz-request-render)
                (yinz-then (yinz-wait-for-event "CustomMessage")
                  (lambda (ignored)
                    (assert-data (= changes 1))
                    (assert-data (equal? (data-value (yinz-data-fetch json-null query) "text") "persisted"))
                    (set! state "data helpers verified")))))))))
    (lambda (failure) (set! state (string-append "data helpers failed: " failure)))))
(define (render rows cols) state)
"#
    );
    let harness =
        Harness::with_services("scm", &source, Some(PermissionStatus::Granted), None, None);
    harness.load();
    let frame = harness.rendered("data helpers");
    assert!(frame.contains("watch disposed"), "{frame}");
    harness.event(release_event());
    harness.rendered("data helpers verified");
    std::fs::write(
        &harness.path,
        format!(
            "{HELPERS}{}",
            r#"
(yinz-declare-pane)
(define state "reloading")
(define (load config)
  (yinz-then (yinz-wait-for-permissions (list "PluginData"))
    (lambda (ignored)
      (define rows (yinz-data-fetch json-null (data-query "items")))
      (assert-data (equal? (data-value rows "text") "persisted"))
      (assert-data (= (data-value rows "n") 9223372036854775807))
      (set! state "same URL persistence verified"))))
(define (render rows cols) state)
"#
        ),
    )
    .unwrap();
    harness.reload();
    harness.rendered("same URL persistence verified");
    for (filename, verify_private) in [("other.scm", true), ("shared-reader.scm", false)] {
        let private = if verify_private {
            r#"
(yinz-data-define json-null (data-definition "items" (list (list "text" "Text"))))
(assert-data (= (yinz-data-count json-null (data-query "items")) 0))
"#
        } else {
            ""
        };
        let source = format!(
            r#"(yinz-declare-pane) {HELPERS}
(define state "starting")
(define (load config)
  (yinz-then (yinz-wait-for-permissions (list "PluginData" "SharedPluginData"))
    (lambda (ignored)
      {private}
      (assert-data (equal? (data-value (yinz-data-fetch "shared:steel_sdk" (data-query "shared_item")) "text") "shared from Scheme"))
      (set! state "{filename} namespace verified"))))
(define (render rows cols) state)
"#
        );
        let path = harness.directory.path().join(filename);
        std::fs::write(&path, source).unwrap();
        load(&harness.sender, run_plugin(&path));
        harness.rendered(&format!("{filename} namespace verified"));
    }
    harness.finish();
}

#[test]
fn steel_integer_commands_preserve_exact_values_and_revision_guards_in_sqlite() {
    let source = format!(
        "{HELPERS}{}",
        r#"
(yinz-declare-pane)
(define state "starting")
(define (load config)
  (yinz-then (yinz-wait-for-permissions (list "PluginData"))
    (lambda (ignored)
      (yinz-data-define json-null (data-definition "numbers" (list (list "n" "Int") (list "r" "Real"))))
      (define (put n r revision)
        (yinz-data-apply json-null (list (data-put "numbers" "row"
          (list (list "n" (hash "Int" n)) (list "r" (hash "Real" r))) revision))))
      (define (read-row) (yinz-data-fetch json-null (data-query "numbers")))
      (for-each (lambda (n)
        (put n 1.5 json-null)
        (assert-data (= (data-value (read-row) "n") n))
        (assert-data (= (data-value (read-row) "r") 1.5)))
        (list -9223372036854775808 9223372036854775807 9007199254740991 -9007199254740991))
      (define rev (data-value (read-row) "rev"))
      (for-each (lambda (invalid)
        (define refused (with-handler (lambda (failure) #t) (put 10 2 invalid) #f))
        (assert-data refused)
        (assert-data (= (data-value (read-row) "rev") rev)))
        (list 0.5 -0.5 -1 18446744073709551616))
      (put 42 2 rev)
      (define conflict (with-handler (lambda (failure) (string-contains? (error-object-message failure) "conflict"))
        (put 99 3 rev) #f))
      (assert-data conflict)
      (assert-data (= (data-value (read-row) "n") 42))
      (set! state "integer host path verified"))))
(define (render rows cols) state)
"#
    );
    let harness = Harness::new("scm", &source, PermissionStatus::Granted);
    harness.load();
    harness.rendered("integer host path verified");
    harness.finish();
}

#[test]
fn steel_reload_disposes_host_watches_and_rejects_their_queued_deliveries() {
    let source = format!(
        "{HELPERS}{}",
        r#"
(yinz-declare-pane)
(define label "old")
(define changes 0)
(define fences 0)
(define (load config)
  (yinz-command (hash "Subscribe" (list "CustomMessage")))
  (yinz-then (yinz-wait-for-permissions (list "PluginData"))
    (lambda (ignored)
      (yinz-data-define json-null (data-definition "watched" (list (list "value" "Text"))))
      (yinz-data-watch-callback json-null (data-query "watched")
        (lambda (rows event) (set! changes (+ changes 1)) (yinz-request-render)))
      (yinz-data-apply json-null (list (data-put "watched" "one"
        (list (list "value" (hash "Text" label))) json-null))))))
(define (update event)
  (if (and (hash? event) (hash-contains? event "CustomMessage"))
    (begin (set! fences (+ fences 1)) #t) #f))
(define (render rows cols)
  (string-append label " changes=" (number->string changes) " fences=" (number->string fences)))
"#
    );
    let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_proxy = SenderWithContext::new(to_proxy);
    let harness = Harness::with_event_sender(
        "scm",
        &source,
        Some(PermissionStatus::Granted),
        Some(to_proxy.clone()),
    );
    let replies = Watched::new(Vec::<PluginInstruction>::new());
    let proxy = std::thread::spawn({
        let replies = replies.clone();
        let sender = harness.sender.clone();
        move || loop {
            let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
            match instruction {
                PluginInstruction::Exit => break,
                changed @ PluginInstruction::PluginReply(
                    _,
                    _,
                    _,
                    Event::PluginDataChanged { .. },
                    _,
                ) => replies.lock().push(changed),
                PluginInstruction::Update(ref events)
                    if events
                        .iter()
                        .any(|(_, _, event)| matches!(event, Event::PluginDataChanged { .. })) =>
                {
                    replies.lock().push(instruction)
                },
                other => {
                    let _ = sender.send(other);
                },
            }
        }
    });
    harness.load();
    let old = replies.wait_until("old host watch reply", Duration::from_secs(20), |events| {
        events.first().cloned()
    });
    harness.rendered("old changes=0 fences=0");
    std::fs::write(&harness.path, source.replace("\"old\"", "\"new\"")).unwrap();
    harness.reload();
    let current = replies.wait_until("new host watch reply", Duration::from_secs(20), |events| {
        events.get(1).cloned()
    });
    harness.rendered("new changes=0 fences=0");
    match (&old, &current) {
        (
            PluginInstruction::PluginReply(_, _, old, _, _),
            PluginInstruction::PluginReply(_, _, new, _, _),
        ) => assert_ne!(old, new),
        _ => panic!("data replies require generations: {old:?}, {current:?}"),
    }
    // Each synchronous apply emits one change before its following render tick.
    // A surviving old watch would have emitted a third change from the second apply.
    assert_eq!(replies.lock().len(), 2);
    harness.sender.send(old).unwrap();
    harness.event(release_event());
    harness.rendered("new changes=0 fences=1");
    harness.sender.send(current).unwrap();
    harness.rendered("new changes=1 fences=1");
    harness.finish();
    to_proxy.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

use super::*;

#[test]
fn scheme_installed_render_settles_callbacks_before_capturing_visible_text() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("render.scm"),
        r#"
(define text "before")
(define (render rows columns)
  (define snapshot text)
  (yinz-then json-null (lambda (_) (set! text "after") (yinz-request-render)))
  snapshot)
"#,
    )
    .unwrap();
    let path = root.path().join("render.test.scm");
    std::fs::write(
        &path,
        r#"
(test "render settles callbacks" (lambda ()
  (test-load-plugin "./render.scm")
  (assert-equal (test-render) "after")
  (assert-equal (test-renders) '("after"))))
"#,
    )
    .unwrap();
    let report = run(
        crate::testing::fixture_data().path(),
        &path,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn scheme_installed_picker_harness_uses_results_cancellation_and_deadlines() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("picker.scm"),
        r#"
(define text "")
(define pending #f)
(define (load config) (void))
(define (pipe message)
  (define name (hash-ref message "name"))
  (if (equal? name "cancel")
      (begin (when pending (yinz-cancel! pending)) #f)
      (begin
        (define options (hash "kind" "directory" "roots" '("/home")))
        (when (equal? name "timeout")
          (set! options (hash-insert options "timeoutMs" 10)))
        (set! pending (yinz-pick options))
        (yinz-catch
          (yinz-then pending (lambda (folder)
            (if (json-null? folder)
                (set! text "cancelled")
                (begin
                  (set! text (string-append "folder:" folder))
                  (set! pending (yinz-pick
                    (hash "kind" "file" "roots" (list folder) "extensions" '("pdf"))))
                  (yinz-then pending (lambda (file)
                    (set! text (string-append "file:" (if (json-null? file) "null" file)))))))))
          (lambda (failure) (set! text failure))))))
(define (render rows columns) text)
"#,
    )
    .unwrap();
    let path = root.path().join("picker.test.scm");
    std::fs::write(
        &path,
        r#"
(define (pick-requests)
  (map (lambda (command) (hash-ref (hash-ref command "Pick") "Start"))
    (filter (lambda (command)
      (and (hash? command) (hash-contains? command "Pick")
           (hash-contains? (hash-ref command "Pick") "Start"))) (test-commands))))
(define (request-id index) (hash-ref (list-ref (pick-requests) index) "request_id"))
(define (complete-pick index result)
  (test-event (hash "PickResult" (hash "request_id" (request-id index)
                                      "result" (hash "Ok" result)))))
(define (fail-pick index failure)
  (test-event (hash "PickResult" (hash "request_id" (request-id index)
                                      "result" (hash "Err" failure)))))
(define (start)
  (test-load-plugin "./picker.scm")
  (test-pipe (hash "name" "start")))

(test "sequential results and stale replies" (lambda ()
  (start)
  (assert-equal (length (pick-requests)) 1)
  (complete-pick 0 "/home/Papers.pdf")
  (assert-equal (length (pick-requests)) 2)
  (assert-equal (hash-ref (hash-ref (list-ref (pick-requests) 1) "options") "roots")
                '("/home/Papers.pdf"))
  (complete-pick 0 "/stale")
  (complete-pick 1 "/home/Papers.pdf/a.pdf")
  (assert-equal (test-render) "file:/home/Papers.pdf/a.pdf")))
(test "escape" (lambda ()
  (start)
  (complete-pick 0 json-null)
  (assert-equal (test-render) "cancelled")
  (assert-equal (length (pick-requests)) 1)))
(test "failure" (lambda ()
  (start)
  (fail-pick 0 "fzf is not installed")
  (assert (string-contains? (test-render) "fzf is not installed"))))
(test "client disconnect settles the pending deferred value" (lambda ()
  (start)
  (fail-pick 0 "picker client disconnected")
  (assert-equal (test-render) "picker client disconnected")
  (test-pipe (hash "name" "cancel"))
  (complete-pick 0 "/stale")
  (assert-equal (test-render) "picker client disconnected")
  (assert-equal (length (pick-requests)) 1)))
(test "escape at the second step" (lambda ()
  (start)
  (complete-pick 0 "/home/folder")
  (complete-pick 1 json-null)
  (assert-equal (test-render) "file:null")
  (assert-equal (length (pick-requests)) 2)))
(test "cancel" (lambda ()
  (start)
  (test-pipe (hash "name" "cancel"))
  (assert (string-contains? (test-render) "AbortError"))))
(test "deadline" (lambda ()
  (test-load-plugin "./picker.scm")
  (test-pipe (hash "name" "timeout"))
  (test-advance 9)
  (assert-equal (test-render) "")
  (test-advance 1)
  (assert (string-contains? (test-render) "TimeoutError"))))
"#,
    )
    .unwrap();
    let report = run(
        crate::testing::fixture_data().path(),
        &path,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
    assert_eq!(report.tests.len(), 7);
}

use super::*;

#[test]
fn scheme_installed_assert_rejects_checks_deferred_and_synchronous_errors() {
    let root = tempfile::tempdir().unwrap();
    let entry = root.path().join("assertions.test.scm");
    std::fs::write(
        &entry,
        r#"
(test "rejected deferred" (lambda ()
  (yinz-then
    (assert-rejects (lambda ()
      (yinz-then json-null (lambda (_) (error "delayed failure")))) "delayed failure")
    (lambda (failure) (assert (string-contains? failure "delayed failure"))))))
(test "synchronous error" (lambda ()
  (assert-rejects (lambda () (error "synchronous failure")) "synchronous failure")))
(test "resolved deferred is a failure" (lambda ()
  (assert-rejects (lambda ()
    (assert-rejects (lambda () (yinz-then #f (lambda (value) value)))))
    "expected deferred to reject")))
(test "ordinary return is a failure" (lambda ()
  (assert-rejects (lambda () (assert-rejects (lambda () json-null)))
    "expected deferred to reject")))
(test "wrong error is a failure" (lambda ()
  (assert-rejects (lambda ()
    (assert-rejects (lambda () (error "actual failure")) "different failure"))
    "unexpected error: ")))
"#,
    )
    .unwrap();
    let report = run(&entry, &Default::default(), None, &AtomicBool::new(false)).unwrap();
    assert!(report.passed(), "{report:#?}");
    assert_eq!(report.tests.len(), 5);
}

#[test]
fn scheme_installed_wait_for_awaits_predicates_and_bounds_pending_deferreds() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("context.scm"),
        r#"(define (load configuration) (yinz-log "wait diagnostic log"))
           (define (render rows columns) "wait diagnostic screen")"#,
    )
    .unwrap();
    let entry = root.path().join("wait.integration.test.scm");
    std::fs::write(
        &entry,
        r#"
(test "deferred predicates" (lambda ()
  (test-session "start")
  (test-session "load" (hash "path" "./context.scm"))
  (test-wait-for-text "wait diagnostic screen")
  (define attempts 0)
  (test-wait-for "second deferred result" (lambda ()
    (set! attempts (+ attempts 1))
    (yinz-then json-null (lambda (_) (= attempts 2)))))
  (assert-equal attempts 2)
  (assert-throws (lambda ()
    (test-wait-for "rejected predicate" (lambda ()
      (yinz-then json-null (lambda (_) (error "predicate failed"))))))
    "predicate failed")
  (define pending (yinz-deferred))
  (define failure (assert-throws (lambda ()
    (test-wait-for "pending predicate" (lambda () pending) 20))
    "timed out waiting for pending predicate"))
  (assert (string-contains? failure "wait diagnostic screen"))
  (assert (string-contains? failure "wait diagnostic log"))
  (yinz-cancel! pending)
  (test-wait-for "controller survives" (lambda () #t))
  (assert-equal (yinz-now-ms) 0)))
"#,
    )
    .unwrap();
    let report = run(&entry, &Default::default(), None, &AtomicBool::new(false)).unwrap();
    assert!(report.passed(), "{report:#?}");
    assert_eq!(report.tests.len(), 1);
    assert_eq!(report.tests[0].kind, PluginTestKind::Integration);
}

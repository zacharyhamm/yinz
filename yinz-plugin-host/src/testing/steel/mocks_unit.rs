use super::*;

const PERMISSIONS_PLUGIN: &str = r#"
(define settled '())
(define observed #f)
(define (record name suffix) (set! settled (append settled (list (string-append name ":" suffix)))))
(define (request name permission)
  (yinz-catch
    (yinz-then (yinz-wait-for-permissions (list permission))
      (lambda (_) (record name "granted")))
    (lambda (failure) (record name (trim failure)))))
(define (load configuration)
  (unless (equal? (yinz-instance-id) "test") (error "mock instance ID must be test"))
  (when (equal? (hash-ref configuration "mode") "errors") (request "immediate" "WriteFiles"))
  (request "first" "ReadApplicationState")
  (when (equal? (hash-ref configuration "mode") "errors") (request "queued" "RunCommands"))
  (request "last" "ChangeApplicationState"))
(define (update event)
  (when (and (hash? event) (hash-contains? event "PermissionRequestResult") (not observed))
    (set! observed #t) (request "observer" "OpenFiles")) #f)
(define (render rows cols) (string-join settled ","))
"#;

fn permission_cases(source: &str) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("permissions.scm"), PERMISSIONS_PLUGIN).unwrap();
    let entry = root.path().join("permissions.test.scm");
    std::fs::write(&entry, source).unwrap();
    let report = run(
        crate::testing::fixture_data().path(),
        &entry,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn scheme_installed_callback_mocks_preserve_permission_fifo_grants_and_observer_ownership() {
    permission_cases(
        r#"
(test "FIFO permission answers" (lambda ()
  (define dispatched '())
  (test-load-plugin "./permissions.scm" (hash "configuration" (hash "mode" "fifo")
    "replies" (hash "RequestPluginPermissions" json-null)))
  (define (requests) (filter (lambda (command) (and (hash? command) (hash-contains? command "RequestPluginPermissions"))) (test-commands)))
  (assert-equal (requests) (list (hash "RequestPluginPermissions" '("ReadApplicationState"))))
  (test-stub "RequestPluginPermissions" (lambda (permissions)
    (set! dispatched (append dispatched (list permissions)))
    (assert-equal (hash-ref (last (requests)) "RequestPluginPermissions") permissions)
    (displayln "mock callback observed")
    (assert-throws (lambda () (yinz-command "GetPluginIds")) "host commands are unavailable")
    json-null))
  (assert-equal dispatched '())
  (test-event (hash "PermissionRequestResult" "Granted"))
  (assert-equal (test-render) "first:granted")
  (assert-equal dispatched '(("ReadApplicationState" "ChangeApplicationState")))
  (assert-equal (length (requests)) 2)
  (test-event (hash "PermissionRequestResult" "Denied"))
  (assert-equal (test-render) "first:granted,last:plugin permissions denied")
  (assert-equal dispatched '(("ReadApplicationState" "ChangeApplicationState") ("ReadApplicationState" "OpenFiles")))
  (test-event (hash "PermissionRequestResult" "Granted"))
  (assert-equal (test-render) "first:granted,last:plugin permissions denied,observer:granted")
  (assert-equal (length (requests)) 3)
  (assert (member "mock callback observed\n" (test-logs)))))
"#,
    );
}

#[test]
fn scheme_installed_callback_dispatch_failures_advance_without_granting_or_stealing_answers() {
    permission_cases(
        r#"
(test "dispatch failures retain FIFO" (lambda ()
  (define initial '())
  (test-load-plugin "./permissions.scm" (hash "configuration" (hash "mode" "errors")
    "replies" (hash "RequestPluginPermissions" (lambda (permissions)
      (set! initial (append initial (list permissions)))
      (when (member "WriteFiles" permissions) (error "immediate failure")) json-null))))
  (assert-equal initial '(("WriteFiles") ("ReadApplicationState")))
  (assert (string-contains? (test-render) "immediate failure"))
  (define later '())
  (test-stub "RequestPluginPermissions" (lambda (permissions)
    (set! later (append later (list permissions)))
    (when (member "RunCommands" permissions) (error "queued failure")) json-null))
  (test-event (hash "PermissionRequestResult" "Granted"))
  (assert (string-contains? (test-render) "first:granted"))
  (assert (string-contains? (test-render) "queued failure"))
  (assert-equal later '(("ReadApplicationState" "RunCommands") ("ReadApplicationState" "ChangeApplicationState")))
  (test-event (hash "PermissionRequestResult" "Granted"))
  (assert-equal (last later) '("ReadApplicationState" "ChangeApplicationState" "OpenFiles"))
  (test-event (hash "PermissionRequestResult" "Granted"))
  (assert (string-contains? (test-render) "last:granted,observer:granted"))
  (assert-equal (length (filter (lambda (c) (and (hash? c) (hash-contains? c "RequestPluginPermissions"))) (test-commands))) 5)))
(test "unsolicited answer cannot settle observer request" (lambda ()
  (test-load-plugin "./permissions.scm" (hash "configuration" (hash "mode" "errors")
    "replies" (hash "RequestPluginPermissions" (lambda (_) (error "offline")))))
  (test-stub "RequestPluginPermissions" json-null)
  (define before (test-render))
  (test-event (hash "PermissionRequestResult" "Granted"))
  (assert-equal (test-render) before)
  (assert-equal (last (test-commands)) (hash "RequestPluginPermissions" '("OpenFiles")))
  (test-event (hash "PermissionRequestResult" "Granted"))
  (assert-equal (test-render) (string-append before ",observer:granted"))))
"#,
    );
}

#[test]
fn scheme_installed_disposal_cancels_only_the_owning_instance_without_the_parent_token() {
    use crate::{plugin_map::RunningPlugin, steel::SteelPlugin, test_env::test_env};
    use std::sync::Arc;

    for disposal in ["shared steel", "direct steel"] {
        let mut steel = SteelPlugin::new(
            "",
            Path::new("/tmp/disposal.scm"),
            test_env(),
            Arc::default(),
        )
        .unwrap();
        steel.env_mut().generation = Some(steel.generation());
        let mut steel = Some(steel);
        let mut plugin = if disposal == "shared steel" {
            Some(RunningPlugin::new(steel.take().unwrap(), 24, 80))
        } else {
            None
        };
        let env = plugin
            .as_ref()
            .map(RunningPlugin::env)
            .unwrap_or_else(|| steel.as_ref().unwrap().env());
        let parent_cancel = env.cancel.clone();
        let keys = [
            (env.plugin_id, env.client_id, env.generation, "one".into()),
            (env.plugin_id, env.client_id, env.generation, "two".into()),
            (
                env.plugin_id,
                crate::ClientId::new(2),
                env.generation,
                "one".into(),
            ),
            (env.plugin_id, env.client_id, Some(u64::MAX), "one".into()),
            (
                crate::PluginId::new(2),
                env.client_id,
                env.generation,
                "one".into(),
            ),
        ];
        let tokens = keys.each_ref().map(|_| Arc::new(AtomicBool::new(false)));
        RUNS.lock()
            .unwrap()
            .extend(keys.iter().cloned().zip(tokens.iter().cloned()));
        assert!(!parent_cancel.load(Ordering::Acquire));
        if disposal == "direct steel" {
            steel.as_mut().unwrap().dispose_with_cleanup(true);
        } else {
            plugin.as_mut().unwrap().dispose();
        }
        assert!(!parent_cancel.load(Ordering::Acquire));
        for (index, token) in tokens.iter().enumerate() {
            assert_eq!(
                token.load(Ordering::Acquire),
                index < 2,
                "{disposal}: {index}"
            );
        }
        let mut runs = RUNS.lock().unwrap();
        for key in keys {
            assert!(
                runs.remove(&key).is_some(),
                "cleanup still belongs to the worker"
            );
        }
    }
}

#[test]
fn scheme_installed_instance_cancellation_stops_a_running_worker_and_reaps_its_session() {
    use std::sync::{mpsc, Arc};

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cancel.integration.test.scm");
    std::fs::write(&path, r#"(test "cancel" (lambda () (test-session "start") (displayln "worker is running") (let loop () (loop))))"#).unwrap();
    let mut owned = session::OwnedSession::new(crate::testing::fixture_data().path()).unwrap();
    let session_root = owned.root().to_owned();
    let pid: i32 = std::fs::read_to_string(session_root.join("server.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let request = Request {
        path,
        root: root.path().to_owned(),
        index: Some(0),
        case: Some(Case {
            name: "cancel".into(),
            options: Default::default(),
        }),
        session: Some(session_root.clone()),
        journal: None,
    };
    let mut env = crate::test_env::test_env();
    env.plugin_id = crate::PluginId::new(3);
    let key = (
        env.plugin_id,
        env.client_id,
        env.generation,
        "worker".into(),
    );
    let cancel = Arc::new(AtomicBool::new(false));
    RUNS.lock().unwrap().insert(key.clone(), cancel.clone());
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = invoke(&request, Duration::from_secs(60), &cancel);
        let cleanup = owned.shutdown();
        drop(owned);
        sender.send((result, cleanup)).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    // Observe the worker's journal, while also detecting an early worker exit.
    while !std::fs::read_to_string(session_root.join("diagnostics"))
        .unwrap_or_default()
        .contains("worker is running")
    {
        assert!(
            Instant::now() < deadline,
            "worker never reached the running case"
        );
        match receiver.recv_timeout(Duration::from_millis(10)) {
            Err(mpsc::RecvTimeoutError::Timeout) => {},
            result => panic!("worker exited before cancellation: {result:?}"),
        }
    }
    assert!(!env.cancel.load(Ordering::Acquire));
    cancel_instance(&env);
    let (result, cleanup) = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    cleanup.unwrap();
    let report: PluginTestResult = serde_json::from_value(result.unwrap()).unwrap();
    assert_eq!(report.status, "failed");
    assert!(report.error.unwrap().contains("cancelled"));
    assert!(report.output.contains("worker is running"));
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH),
        "the disposable server must be reaped"
    );
    assert!(!session_root.exists());
    RUNS.lock().unwrap().remove(&key);
}

#[test]
fn scheme_installed_runner_retains_source_errors_filters_and_import_confinement() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("errors.test.scm");
    for (source, expected) in [
        (
            "(test \"missing\" (lambda () (unknown-test-command)))",
            "unknown-test-command",
        ),
        (
            "(test \"mapped\" (lambda ()\n  (void)\n  (error \"mapped failure\")))",
            "errors.test.scm:3:",
        ),
        ("(define no-tests #t)", "no matching tests"),
        (
            "(require \"../outside.scm\") (test \"escape\" (lambda () (void)))",
            "escapes",
        ),
    ] {
        std::fs::write(&path, source).unwrap();
        let report = run(
            crate::testing::fixture_data().path(),
            &path,
            &Default::default(),
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!report.passed(), "{report:#?}");
        assert!(
            report.tests.iter().any(|test| test
                .error
                .as_deref()
                .is_some_and(|error| error.contains(expected))),
            "expected {expected}: {report:#?}"
        );
    }
    std::fs::write(&path, "(test \"one\" (lambda () (void)))").unwrap();
    let report = run(
        crate::testing::fixture_data().path(),
        &path,
        &PluginTestOptions {
            filter: Some("absent".into()),
            ..Default::default()
        },
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!report.passed(), "{report:#?}");
    let report = run(
        crate::testing::fixture_data().path(),
        &path,
        &Default::default(),
        None,
        &AtomicBool::new(true),
    )
    .unwrap();
    assert!(report.tests[0]
        .error
        .as_deref()
        .unwrap()
        .contains("cancelled"));
    assert!(yinz_steel::Runtime::new(Ok)
        .unwrap()
        .check("(test \"forbidden production test privilege\" (lambda () #t))")
        .is_err());
}

#[test]
fn scheme_installed_adjacent_tests_preserve_selected_root_and_script_extensions() {
    let root = tempfile::tempdir().unwrap();
    for extension in ["scm", "ss", "sld", "ts", "js", "wasm", "txt"] {
        let entry = root.path().join(format!("plugin.{extension}"));
        let adjacent = [
            format!("plugin.test.{extension}"),
            format!("plugin.integration.test.{extension}"),
        ]
        .map(|name| root.path().join(name));
        std::fs::write(&entry, "").unwrap();
        for path in &adjacent {
            std::fs::write(path, "").unwrap();
        }
        let mut found = vec![];
        discover(&entry, root.path(), &mut found).unwrap();
        if matches!(extension, "scm" | "ss" | "sld") {
            assert_eq!(found, adjacent);
        } else {
            assert!(found.is_empty(), "{extension}: {found:?}");
        }
        found.clear();
        discover(&entry, root.path().parent().unwrap(), &mut found).unwrap();
        assert!(found.is_empty(), "adjacency stays in selected root");
    }
}

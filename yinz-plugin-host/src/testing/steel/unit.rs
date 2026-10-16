use super::*;

#[test]
fn scheme_installed_tests_load_modules_mock_commands_and_advance_deferred_callbacks() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("math.scm"),
        "(provide twice) (define (twice n) (* 2 n))",
    )
    .unwrap();
    std::fs::write(
        root.path().join("plugin.scm"),
        r#"
        (define text "waiting")
        (define (load config)
          (displayln "plugin loaded")
          (yinz-then (yinz-sleep 0.1) (lambda (ignored) (set! text "awake") #t)))
        (define (render rows cols) (display "printed: ") (displayln text))
        (define (update event)
          (when (equal? event "InputReceived") (yinz-command "GetPluginIds")) #t)
    "#,
    )
    .unwrap();
    let path = root.path().join("plugin.test.scm");
    std::fs::write(&path, r#"
        (require "./math.scm")
        (test "module and assertions" (lambda ()
          (assert-equal (twice 21) 42)
          (assert-throws (lambda () (error "expected failure")) "expected failure")))
        (test "mocked deferred callbacks" (lambda ()
          (assert-equal (yinz-now-ms) 0)
          (assert-equal (yinz-wall-clock-ms) 0)
          (test-load-plugin "./plugin.scm")
          (assert (member "plugin loaded\n" (test-logs)))
          (assert (string-contains? (test-render) "printed: waiting"))
          (test-advance 100)
          (assert-equal (yinz-now-ms) 100)
          (assert-equal (yinz-wall-clock-ms) 100)
          (assert (string-contains? (test-render) "printed: awake"))
          (assert (> (length (test-commands)) 0))
          (test-stub "GetPluginIds" (hash "plugin_id" 7))
          (test-event "InputReceived")
          (test-stub-error "GetPluginIds" "fixture host failure")
          (assert-throws (lambda () (test-event "InputReceived")) "fixture host failure")
          (test-stub "GetPluginIds" (hash "plugin_id" 8))
          (test-event "InputReceived")
          (displayln "captured test output")))
        (test "unstubbed host operation" (lambda ()
          (test-load-plugin "./plugin.scm")
          (assert-throws (lambda () (test-event "InputReceived")) "unstubbed host operation GetPluginIds")))
    "#).unwrap();
    let report = run(
        &path,
        &PluginTestOptions::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
    assert_eq!(report.tests.len(), 3);
    assert!(
        report.tests[1].output.contains("captured test output"),
        "{report:#?}"
    );
}

#[test]
fn scheme_installed_integration_tests_use_the_disposable_session() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("plugin.scm"),
        r#"(define (render rows columns) "scheme integration ready")"#,
    )
    .unwrap();
    let path = root.path().join("plugin.integration.test.scm");
    std::fs::write(
        &path,
        r#"
        (test "real session" (lambda ()
          (test-session "start")
          (test-session "load" (hash "path" "./plugin.scm"))
          (test-wait-for-text "scheme integration ready")))
    "#,
    )
    .unwrap();
    let report = run(
        &path,
        &PluginTestOptions::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
    assert_eq!(report.tests[0].kind, PluginTestKind::Integration);
}

#[test]
fn scheme_installed_locations_work_before_load_and_copy_dependencies() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("lib")).unwrap();
    std::fs::write(
        root.path().join("lib/text.scm"),
        "(provide label) (define label \"copied graph ready\")",
    )
    .unwrap();
    std::fs::write(root.path().join("reader 日本.scm"), r#"
        (require "./lib/text.scm")
        (define own-location (yinz-plugin-location))
        (define helper (yinz-plugin-resolve "./lib/text.scm"))
        (define (render rows cols)
          (if (and (string-contains? own-location "reader") (string-contains? helper "lib/text.scm"))
              label "identity unavailable"))
    "#).unwrap();
    let unit = root.path().join("locations.test.scm");
    std::fs::write(
        &unit,
        r#"
        (define location-before-case (test-location))
        (define target-before-case (test-resolve "./reader 日本.scm"))
        (test "mock identity" (lambda ()
          (assert (string-contains? location-before-case "locations.test.scm"))
          (assert-equal target-before-case (test-resolve target-before-case))
          (assert-throws (lambda () (test-resolve "../escape.scm")))
          (test-load-plugin target-before-case)
          (assert-equal (test-render) "copied graph ready")))
    "#,
    )
    .unwrap();
    let report = run(&unit, &Default::default(), None, &AtomicBool::new(false)).unwrap();
    assert!(report.passed(), "{report:#?}");
    let integration = root.path().join("locations.integration.test.scm");
    std::fs::write(
        &integration,
        r#"
        (test "copy graph" (lambda ()
          (define copy (test-copy-plugin "./reader 日本.scm" "日本 copy"))
          (assert (string-contains? copy "reader"))
          (assert-throws (lambda () (test-copy-plugin "./reader 日本.scm" "日本 copy")))
          (assert-throws (lambda () (test-copy-plugin "./reader 日本.scm" "../escape")))
          (test-session "start")
          (test-session "load" (hash "path" copy))
          (test-wait-for-text "copied graph ready")))
    "#,
    )
    .unwrap();
    let report = run(
        &integration,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn scheme_installed_rpc_uses_authenticated_plugins_and_releases_errors_and_timeouts() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("rpc-target.scm"),
        r#"
      (define ready #f)
      (define calls 0)
      (define (load configuration)
        (yinz-then (yinz-wait-for-permissions '("ReadCliPipes" "MessageAndLaunchOtherPlugins"))
          (lambda (_)
            (yinz-rpc-handle "echo" (lambda (payload call)
              (set! calls (+ calls 1))
              (yinz-request-render)
              (hash "payload" payload "source" (hash-ref call "source"))))
            (yinz-rpc-handle "fail" (lambda (payload call) (error "expected remote failure")))
            (yinz-rpc-handle "wait" (lambda (payload call) (yinz-deferred)))
            (yinz-rpc-handle "large" (lambda (payload call) (make-string 33000 #\x)))
            (set! ready #t) (yinz-request-render))))
      (define (render rows columns)
        (if ready (string-append "rpc ready, calls:" (number->string calls)) "waiting for grants"))
    "#,
    )
    .unwrap();
    let path = root.path().join("rpc.integration.test.scm");
    std::fs::write(&path, r#"
      (test "authenticated RPC" (hash "grants" '("ReadCliPipes" "MessageAndLaunchOtherPlugins"))
        (lambda ()
          (test-session "start")
          (test-session "load" (hash "path" "./rpc-target.scm"))
          (test-wait-for-text "rpc ready, calls:0")
          (define (call name payload timeout)
            (test-session "rpc" (hash "plugin" "./rpc-target.scm" "name" name
              "payload" payload "timeoutMs" timeout)))
          (define payload (hash "unicode" "日本語" "integer" 18446744073709551615))
          (define reply (call "echo" payload 3000))
          (assert-equal (hash-ref reply "payload") payload)
          (assert (integer? (hash-ref (hash-ref reply "source") "Plugin")))
          (test-wait-for-text "rpc ready, calls:1")
          (test-session "pipe" (hash "plugin" "./rpc-target.scm" "name" "yinz_rpc_call"
            "payload" (yinz-json-string (hash "version" 1 "call_id" "forged" "name" "echo" "payload" "forged"))))
          (assert-throws (lambda () (call "fail" json-null 3000)) "expected remote failure")
          (assert-throws (lambda () (call "wait" json-null 25)) "RPC timed out")
          (assert-throws (lambda () (call "large" json-null 3000)) "exceeds 32 KiB")
          (call "echo" "still live" 3000)
          (test-wait-for-text "rpc ready, calls:2")))
    "#).unwrap();
    let report = run(
        &path,
        &PluginTestOptions::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn scheme_installed_tests_bound_execution_and_retain_pre_timeout_logs() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("loop.test.scm");
    std::fs::write(
        &path,
        r#"(test "loop" (lambda () (displayln "before deadline") (let loop () (loop))))"#,
    )
    .unwrap();
    let report = run(
        &path,
        &PluginTestOptions {
            timeout_ms: 10000,
            ..Default::default()
        },
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!report.passed(), "{report:#?}");
    assert!(
        report.tests[0]
            .error
            .as_deref()
            .unwrap()
            .contains("deadline"),
        "{report:#?}"
    );
    assert!(
        report.tests[0].output.contains("before deadline"),
        "{report:#?}"
    );
}

#[test]
fn scheme_test_targets_are_confined_and_preserve_escaped_names() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let entry = root.path().join("case.test.scm");
    std::fs::write(&entry, "").unwrap();
    let file = root.path().join("reader #?%23.scm");
    std::fs::write(&file, "").unwrap();
    let external = outside.path().join("external.scm");
    std::fs::write(&external, "").unwrap();
    std::os::unix::fs::symlink(&external, root.path().join("escape.scm")).unwrap();
    let sources = Sources {
        entry,
        root: root.path().canonicalize().unwrap(),
    };
    let expected = url::Url::from_file_path(file.canonicalize().unwrap())
        .unwrap()
        .to_string();
    assert_eq!(
        sources
            .resolve_test_path("./reader%20%23%3F%2523.scm")
            .unwrap(),
        expected
    );
    assert_eq!(sources.resolve_test_path(&expected).unwrap(), expected);
    for path in [
        "./escape.scm",
        "../external.scm",
        "./case.test.scm?query",
        "https://example.com/plugin.scm",
        "yinz:plugin",
        "./case.ts",
    ] {
        assert!(sources.resolve_test_path(path).is_err(), "{path}");
    }
    let path = root.path().join("grants.test.scm");
    std::fs::write(&path, "(test \"grants\" (hash \"grants\" '(\"RunCommands\")) (lambda () (error \"must not run\")))").unwrap();
    let report = run(
        &path,
        &PluginTestOptions::default(),
        Some(&[]),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!report.passed(), "{report:#?}");
    assert!(
        report.tests[0]
            .error
            .as_deref()
            .unwrap()
            .contains("exceed the calling plugin"),
        "{report:#?}"
    );
}

#[test]
fn test_resolution_does_not_anchor_to_a_colliding_fixture_filename() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    std::fs::create_dir(directory.join("nested")).unwrap();
    for name in [
        "case.test.scm",
        "actual.scm",
        "nested/anchor.scm",
        "nested/link.scm",
    ] {
        std::fs::write(directory.join(name), "").unwrap();
    }
    std::os::unix::fs::symlink(
        directory.join("nested/anchor.scm"),
        directory.join("__test_resolution__.scm"),
    )
    .unwrap();
    std::os::unix::fs::symlink(directory.join("actual.scm"), directory.join("link.scm")).unwrap();
    let sources = Sources {
        entry: directory.join("case.test.scm"),
        root: directory.clone(),
    };
    assert_eq!(
        sources.resolve_test_path("./actual.scm").unwrap(),
        url::Url::from_file_path(directory.join("actual.scm"))
            .unwrap()
            .as_str()
    );
    assert!(sources.resolve_test_path("./link.scm").is_err());
}

macro_rules! builtin_scheme_suites {
    ($kind:ident; $($name:ident => $tag:literal),* $(,)?) => { $(
        #[test]
        fn $name() {
            let report = super::super::run_named(
                Path::new(""), $tag,
                &PluginTestOptions { kind: PluginTestKind::$kind, timeout_ms: 30000, ..Default::default() },
                None, &AtomicBool::new(false),
            ).unwrap();
            assert!(report.passed(), "{report:#?}");
            assert!(!report.tests.is_empty());
        }
    )* };
}
builtin_scheme_suites! {
    Unit;
    scheme_builtin_agent_policy_unit => "agent-policy",
    scheme_builtin_agent_opencode_unit => "agent-opencode",
    scheme_builtin_context_menu_unit => "context-menu",
    scheme_builtin_tab_bar_unit => "tab-bar",
    scheme_builtin_compact_bar_unit => "compact-bar",
    scheme_builtin_status_bar_unit => "status-bar",
    scheme_builtin_session_palette_unit => "session-palette",
    scheme_builtin_configuration_palette_unit => "configuration-palette",
    scheme_builtin_agent_exit_unit => "agent-exit",
}
builtin_scheme_suites! {
    Integration;
    scheme_builtin_agent_policy_integration => "agent-policy",
    scheme_builtin_agent_opencode_integration => "agent-opencode",
    scheme_builtin_context_menu_integration => "context-menu",
    scheme_builtin_tab_bar_integration => "tab-bar",
    scheme_builtin_compact_bar_integration => "compact-bar",
    scheme_builtin_status_bar_integration => "status-bar",
    scheme_builtin_session_palette_integration => "session-palette",
    scheme_builtin_configuration_palette_integration => "configuration-palette",
    scheme_builtin_agent_exit_integration => "agent-exit",
}

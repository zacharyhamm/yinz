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
        (yinz-declare-pane)
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
        super::super::fixture_data().path(),
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
fn scheme_test_wall_clock_steps_leave_monotonic_time_and_events_unchanged() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("plugin.scm"),
        r#"
        (yinz-declare-pane)
        (define events 0)
        (define (update event) (set! events (+ events 1)) #t)
        (define (render rows cols)
          (string-append (number->string (yinz-now-ms)) "/"
            (number->string (yinz-wall-clock-ms)) "/" (number->string events)))
    "#,
    )
    .unwrap();
    let path = root.path().join("plugin.test.scm");
    std::fs::write(
        &path,
        r#"
        (test "independent clocks" (lambda ()
          (test-load-plugin "./plugin.scm")
          (test-step-wall-clock -474)
          (assert-equal (yinz-now-ms) 0)
          (assert-equal (yinz-wall-clock-ms) -474)
          (assert (string-contains? (test-render) "0/-474/0"))
          (test-advance 60000)
          (assert-equal (yinz-now-ms) 60000)
          (assert-equal (yinz-wall-clock-ms) 59526)
          (assert (string-contains? (test-render) "60000/59526/1"))
          (test-step-wall-clock 474)
          (assert (string-contains? (test-render) "60000/60000/1"))
          (assert-throws (lambda () (test-step-wall-clock 0.5)) "signed integer")
          (assert-throws (lambda () (test-step-wall-clock 9223372036854775807)) "overflow")
          (assert-throws (lambda () (test-advance 9223372036854775807)) "overflow")
          (assert-equal (yinz-now-ms) 60000)
          (assert-equal (yinz-wall-clock-ms) 60000)
          (assert (string-contains? (test-render) "60000/60000/1"))))
    "#,
    )
    .unwrap();
    let report = run(
        super::super::fixture_data().path(),
        &path,
        &PluginTestOptions::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn scheme_installed_tests_check_the_pane_declaration_as_the_host_does() {
    let root = tempfile::tempdir().unwrap();
    for (name, source) in [
        (
            "headless.scm",
            r#"(define (load config) #t) (define (update event) (yinz-command "HideSelf") #t)"#,
        ),
        (
            "pane.scm",
            r#"(yinz-declare-pane-unless "duplicate" "true")
               (define (load config)
                 (unless (and (hash-contains? config "duplicate")
                              (equal? (hash-ref config "duplicate") "true"))
                   (yinz-command "HideSelf")))
               (define (update event) (yinz-command "HideSelf") #t)"#,
        ),
        (
            "hidden.scm",
            "(begin (yinz-declare-pane)) (define (load config) #t)",
        ),
    ] {
        std::fs::write(root.path().join(name), source).unwrap();
    }
    let path = root.path().join("plugin.test.scm");
    std::fs::write(
        &path,
        r#"
        (test "a headless plugin cannot use its own pane" (lambda ()
          (test-load-plugin "./headless.scm")
          (assert-equal (test-pane-declared?) #f)
          (assert-throws (lambda () (test-event "InputReceived"))
            "'HideSelf' needs the plugin's own pane")))
        (test "a declared pane follows the configuration" (lambda ()
          (test-load-plugin "./pane.scm" (hash "replies" (hash "HideSelf" json-null)))
          (assert-equal (test-pane-declared?) #t)
          (test-event "InputReceived")))
        (test "the declaration is undone by its configuration" (lambda ()
          (test-load-plugin "./pane.scm" (hash "configuration" (hash "duplicate" "true")))
          (assert-equal (test-pane-declared?) #f)))
        (test "a configuration value is read as the host spells it" (lambda ()
          (test-load-plugin "./pane.scm" (hash "configuration" (hash "duplicate" #t)))
          (assert-equal (test-pane-declared?) #f)))
        (test "a declaration the scan cannot see fails the load" (lambda ()
          (assert-throws (lambda () (test-load-plugin "./hidden.scm"))
            "(yinz-declare-pane) must appear literally at the top level of the entry file")))
    "#,
    )
    .unwrap();
    let report = run(
        super::super::fixture_data().path(),
        &path,
        &PluginTestOptions::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
    assert_eq!(report.tests.len(), 5);
}

#[test]
fn scheme_installed_integration_tests_use_the_disposable_session() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("plugin.scm"),
        r#"(yinz-declare-pane) (define (render rows columns) "scheme integration ready")"#,
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
        super::super::fixture_data().path(),
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
    std::fs::write(
        root.path().join("reader 日本.scm"),
        r#"
        (require "./lib/text.scm")
        (yinz-declare-pane)
        (define own-location (yinz-plugin-location))
        (define helper (yinz-plugin-resolve "./lib/text.scm"))
        (define (render rows cols)
          (if (and (string-contains? own-location "reader") (string-contains? helper "text.scm"))
              label "identity unavailable"))
    "#,
    )
    .unwrap();
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
    let report = run(
        super::super::fixture_data().path(),
        &unit,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
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
        super::super::fixture_data().path(),
        &integration,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn scheme_installed_locations_keep_parent_imports_through_reload_workers_and_aliases() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(
        root.path().join("shared.scm"),
        "(provide label) (define label \"parent import ready\")",
    )
    .unwrap();
    std::fs::write(
        root.path().join("src/main.scm"),
        r#"
        (require "../shared.scm")
        (yinz-declare-pane)
        (define own-location (yinz-plugin-location))
        (define sibling-location (yinz-plugin-resolve "../shared.scm"))
        (define phase "unloaded")
        (define ready #f)
        (yinz-register-workers (hash "identity" (lambda (name payload)
          (yinz-post-plugin "identity" (yinz-json-string
            (list label (yinz-plugin-location) (yinz-plugin-resolve "../shared.scm")))))))
        (define (load config)
          (unless (equal? own-location (hash-ref config "expected"))
            (error "copy identity changed"))
          (set! phase (hash-ref config "phase"))
          (yinz-command (hash "Subscribe" '("CustomMessage" "Key")))
          (yinz-post-worker "identity" "probe" ""))
        (define (update event)
          (when (and (hash? event) (hash-contains? event "Key"))
            (set! phase "await reload"))
          (when (and (hash? event) (hash-contains? event "CustomMessage"))
            (define reply (yinz-json-parse (cadr (hash-ref event "CustomMessage"))))
            (unless (equal? reply (list label own-location sibling-location))
              (error "worker lost copy identity"))
            (set! ready #t))
          #t)
        (define (render rows cols)
          (if ready (string-append phase " " label) "waiting for worker identity"))
    "#,
    )
    .unwrap();
    let binary = super::super::binary()
        .unwrap()
        .to_string_lossy()
        .replace('\'', "'\\''");
    let command = format!(
        "'{binary}' --session plugin-test action reconfigure --file \"$HOME/aliases.kdl\" && exec '{binary}' --session plugin-test action launch-or-focus-plugin copied-reader"
    );
    let path = root.path().join("copy.integration.test.scm");
    let script = r#"
        (test "nested copy" (hash "grants" '("RunCommands")) (lambda ()
          (define copy (test-copy-plugin "./src/main.scm" "日本 copy #?%23"))
          (assert (string-contains? copy "git://plugin?"))
          (test-session "fixture" (hash
            "files" (hash "aliases.kdl" (string-append
              "plugins { copied-reader location=" (yinz-json-string copy)
              " { expected " (yinz-json-string copy) "; phase \"alias\"; }; }"))
            "commands" (hash "launch-copy-alias" COMMAND)))
          (test-session "start")
          (test-session "load" (hash "path" copy
            "configuration" (hash "expected" copy "phase" "loaded")))
          (test-wait-for-text "loaded parent import ready")
          (test-session "key" "r")
          (test-wait-for-text "await reload parent import ready")
          (test-session "reload" (hash "path" copy))
          (test-wait-for-text "loaded parent import ready")
          (test-session "stop" (hash "path" copy))
          (test-wait-for "terminal restored" (lambda ()
            (define screen (test-session "screen"))
            (and (string-contains? screen "yinz-test>")
                 (not (string-contains? screen "parent import ready")))))
          (test-session "text" "launch-copy-alias")
          (test-session "key" "Enter")
          (test-wait-for-text "alias parent import ready")))
    "#
    .replace("COMMAND", &serde_json::to_string(&command).unwrap());
    std::fs::write(&path, script).unwrap();
    let report = run(
        super::super::fixture_data().path(),
        &path,
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
      (yinz-declare-pane)
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
        super::super::fixture_data().path(),
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
        super::super::fixture_data().path(),
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
        super::super::fixture_data().path(),
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
fn scheme_test_targets_allow_root_aliases_but_reject_internal_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().canonicalize().unwrap();
    let root = directory.join("root");
    std::fs::create_dir(&root).unwrap();
    let file = root.join("reader #?%23.scm");
    let external = directory.join("external.scm");
    for path in [root.join("case.test.scm"), file.clone(), external.clone()] {
        std::fs::write(path, "").unwrap();
    }
    let alias = directory.join("alias");
    let root_alias = directory.join("root-alias");
    for (target, link) in [
        (&directory, alias.clone()),
        (&root, root_alias.clone()),
        (&file, root.join("link.scm")),
        (&root, root.join("loop")),
        (&external, root.join("escape.scm")),
        (&directory, root.join("outside")),
    ] {
        std::os::unix::fs::symlink(target, link).unwrap();
    }
    let expected = url::Url::from_file_path(&file).unwrap().to_string();
    for aliased_root in [alias.join("root"), root_alias] {
        for source_root in [&root, &aliased_root] {
            let sources = Sources {
                entry: aliased_root.join("case.test.scm"),
                root: source_root.clone(),
            };
            let base = url::Url::from_file_path(&sources.entry).unwrap();
            let aliased_url = base.join("./reader%20%23%3F%2523.scm").unwrap();
            for path in [
                "./reader%20%23%3F%2523.scm",
                aliased_url.as_str(),
                &expected,
            ] {
                assert_eq!(sources.resolve_test_path(path).unwrap(), expected, "{path}");
            }
            for path in [
                "./link.scm",
                "./loop/reader%20%23%3F%2523.scm",
                "./escape.scm",
                "./outside/external.scm",
                "../external.scm",
            ] {
                assert!(sources.resolve_test_path(path).is_err(), "{path}");
                let absolute = base.join(path).unwrap();
                assert!(
                    sources.resolve_test_path(absolute.as_str()).is_err(),
                    "{absolute}"
                );
            }
        }
    }
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
            let data = tempfile::tempdir().unwrap();
            yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), data.path()).unwrap();
            let report = super::super::run_named(
                data.path(), Path::new(""), $tag,
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
    scheme_builtin_layout_palette_unit => "layout-palette",
    scheme_builtin_plugin_palette_unit => "plugin-palette",
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
    scheme_builtin_layout_palette_integration => "layout-palette",
    scheme_builtin_plugin_palette_integration => "plugin-palette",
    scheme_builtin_configuration_palette_integration => "configuration-palette",
    scheme_builtin_agent_exit_integration => "agent-exit",
}

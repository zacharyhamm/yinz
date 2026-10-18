use std::{path::Path, sync::atomic::AtomicBool};
use yinz_types::authoring::{PluginTestKind, PluginTestOptions};

#[test]
fn installed_builtin_scheme_integration_suites() {
    let data = tempfile::tempdir().unwrap();
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), data.path()).unwrap();
    for name in [
        "compact-bar",
        "tab-bar",
        "status-bar",
        "context-menu",
        "agent-policy",
        "agent-opencode",
        "agent-exit",
        "session-palette",
        "configuration-palette",
    ] {
        assert!(
            yinz_steel_builtins::asset_dir()
                .join(format!("{name}.integration.test.scm"))
                .exists(),
            "missing migrated integration suite for {name}"
        );
    }
    for name in yinz_runtime::consts::BUILTIN_PLUGIN_NAMES {
        if !yinz_steel_builtins::asset_dir()
            .join(format!("{name}.integration.test.scm"))
            .exists()
        {
            continue;
        }
        let report = yinz_server::plugin_testing::run_named(
            data.path(),
            Path::new("/unused"),
            name,
            &PluginTestOptions {
                kind: PluginTestKind::Integration,
                ..Default::default()
            },
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(report.passed(), "{name}: {report:#?}");
    }
}

#[test]
fn installed_integration_failure_and_timeout_reap_the_owned_session() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cleanup.integration.test.scm");
    for (body, failure) in [
        (r#"(error "intentional failure")"#, "intentional failure"),
        ("(let loop () (loop))", "deadline"),
        (
            r#"(yinz-log (make-string 65536 #\x)) (error "large output failure")"#,
            "large output failure",
        ),
    ] {
        std::fs::write(
            &path,
            format!(r#"(test "cleanup" (lambda () (test-session "start") {body}))"#),
        )
        .unwrap();
        let report = yinz_server::plugin_testing::run(
            &yinz_integration_tests::test_env::init().join("data"),
            &path,
            &PluginTestOptions {
                // Discovery and server startup must finish under parallel suite load.
                timeout_ms: 30_000,
                ..Default::default()
            },
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!report.passed(), "{report:#?}");
        assert_eq!(report.tests.len(), 1, "{report:#?}");
        assert_eq!(
            report.tests[0].name, "cleanup",
            "case execution must follow discovery: {report:#?}"
        );
        assert!(
            report.tests[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains(failure)),
            "expected case failure {failure}: {report:#?}"
        );
        assert!(
            report.tests[0].output.contains("owned server pid: "),
            "{report:#?}"
        );
        assert!(
            report.tests[0].output.len() <= 32 * 1024,
            "output stays bounded: {report:#?}"
        );
        assert_server_gone(&report.tests[0].output);
        // The runner's result follows shutdown and waitpid. A teardown failure is reported
        // separately and cannot turn the original failure into a successful cleanup.
        assert!(
            !report.tests[0].error.as_ref().unwrap().contains("cleanup:"),
            "{report:#?}"
        );
    }
}

#[test]
fn installed_integration_discovery_failure_has_no_owned_server() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("discovery.integration.test.scm");
    std::fs::write(&path, r#"(error "controlled discovery failure")"#).unwrap();
    let report = yinz_server::plugin_testing::run(
        &yinz_integration_tests::test_env::init().join("data"),
        &path,
        &PluginTestOptions::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(report.tests.len(), 1, "{report:#?}");
    let test = &report.tests[0];
    assert_eq!(
        Path::new(&test.name),
        path.canonicalize().unwrap(),
        "{report:#?}"
    );
    assert!(
        test.error
            .as_deref()
            .is_some_and(|error| error.contains("controlled discovery failure")),
        "{report:#?}"
    );
    assert!(
        !test.output.contains("owned server pid:"),
        "discovery must not claim to own a server: {report:#?}"
    );
}

#[test]
fn installed_controller_routes_input_enforces_permissions_and_isolates_storage() {
    let root = tempfile::tempdir().unwrap();
    let plugin = root
        .path()
        .canonicalize()
        .unwrap()
        .join("echo 日本 #?%23%2523.scm");
    let target = url::Url::from_file_path(&plugin).unwrap().to_string();
    std::fs::write(&plugin, r#"
(define target "TARGET_URL")
(define text "ready")
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadCliPipes" "PluginData")) (lambda (_)
    (yinz-command (hash "Subscribe" '("Key")))
    (yinz-data-define json-null (hash "name" "fixture" "version" 1 "attrs"
      (list (hash "name" "value" "kind" "Text" "required" #t "indexed" #f "full_text" #f "unique" #f "default" json-null))))
    (yinz-data-apply json-null (list (hash "Put" (hash "entity" "fixture" "uid" "url"
      "values" (list (list "value" (hash "Text" target))) "expect_rev" json-null))))
    (define refusal (with-handler %async-error
      (yinz-command (hash "RunCommand" (list '("/usr/bin/true") (hash) "" (hash) 64 json-null))) #f))
    (unless (and (string? refusal) (string-contains? refusal "'RunCommand' requires 'RunCommands'"))
      (error "permission bypass"))
    (yinz-log "permission denied as expected")
    (yinz-request-render))))
(define (update event)
  (if (and (hash? event) (hash-contains? event "Key")
           (hash? (hash-ref (hash-ref event "Key") "bare_key")))
      (begin (set! text (string-append text (hash-ref (hash-ref (hash-ref event "Key") "bare_key") "Char"))) #t) #f))
(define (pipe message)
  (define source (hash-ref message "source"))
  (when (and (hash? source) (hash-contains? source "Cli"))
    (define id (hash-ref source "Cli"))
    (yinz-command (hash "CliPipeOutput" (list id (yinz-json-string (hash "echo" (hash-ref message "payload") "url" target)))))
    (yinz-command (hash "UnblockCliPipeInput" id))) #f)
(define (render rows columns) (string-append text " size=" (number->string rows) "x" (number->string columns)))
"#.replace("TARGET_URL", &target)).unwrap();
    let path = root.path().join("echo.integration.test.scm");
    std::fs::write(&path, r#"
(define target "TARGET_URL")
(define encoded-path "./echo%20%E6%97%A5%E6%9C%AC%20%23%3F%2523%252523.scm")
(define pane-target "PANE_TARGET")
(define (size)
  (define parts (split-many (test-session "screen") "size="))
  (and (pair? (cdr parts))
    (let scan ([characters (string->list (cadr parts))] [result '()])
      (if (and (pair? characters) (member (car characters) (string->list "0123456789x")))
          (scan (cdr characters) (cons (car characters) result))
          (and (pair? result) (list->string (reverse result)))))))
(for-each (lambda (name)
  (test name (hash "grants" '("ReadCliPipes" "PluginData")) (lambda ()
    (assert-throws (lambda () (test-session "fixture" (hash "files" (hash) "commands" (hash "forbidden" "exit 0")))) "RunCommands")
    (test-session "start")
    (assert-throws (lambda () (test-wait-for "bounded frozen clock" (lambda () #f) 20)) "timed out")
    (assert-equal (yinz-now-ms) 0) (assert-equal (yinz-wall-clock-ms) 0)
    (test-wait-for-text "yinz-test>")
    (assert-equal (test-session "catalog") '())
    (test-session "text" "printf 'terminal-%s' 'input' ") (test-session "key" "Enter")
    (test-wait-for-text "terminal-input")
    (test-session "load" (hash "path" encoded-path)) (test-wait-for-text "ready size=")
    (test-wait-for "encoded URL identifies the loaded pane" (lambda ()
      (not (null? (filter (lambda (pane) (equal? (%async-get pane "plugin_url" #f) pane-target)) (test-session "panes"))))))
    (test-session "text" "xy") (test-wait-for-text "readyxy")
    (define before (size))
    (test-session "resize" (hash "rows" 35 "columns" 100))
    (test-wait-for "resize reaches plugin" (lambda () (define after (size)) (and after (not (equal? after before)))))
    (assert-equal (yinz-json-parse (test-session "pipe" (hash "name" "echo" "payload" "payload" "plugin" target)))
                  (hash "echo" "payload" "url" target))
    (define data (test-session "data" (hash "plugin" target "entity" "fixture")))
    (assert-equal (length (hash-ref data "rows")) 1)
    (define value-index (- (length (hash-ref data "columns")) (length (member "value" (hash-ref data "columns")))))
    (assert-equal (list-ref (car (hash-ref data "rows")) value-index) (hash "Text" target))
    (test-wait-for "permission denial logged" (lambda () (string-contains? (test-session "logs") "permission denied as expected")))
    (assert (not (null? (filter (lambda (entity) (equal? (hash-ref (hash-ref entity "def") "name") "fixture")) (test-session "catalog")))))
    (test-session "reload" target) (test-wait-for-text "ready size=")
    (assert (not (string-contains? (test-session "screen") "readyxy"))))))
  '("first isolated case" "second isolated case"))
"#.replace("TARGET_URL", &target)
    .replace("\"PANE_TARGET\"", &serde_json::to_string(&format!("file:{}", plugin.display())).unwrap())).unwrap();
    let report = yinz_server::plugin_testing::run(
        &yinz_integration_tests::test_env::init().join("data"),
        &path,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn installed_async_plugin_api_returns_results_and_cancels_while_input_continues() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("hanging.test.scm"),
        r#"(test "loop" (lambda () (let loop () (loop))))"#,
    )
    .unwrap();
    std::fs::write(root.path().join("caller.scm"), r#"
(define status "starting") (define pending #f)
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("AuthorPlugins")) (lambda (_)
    (yinz-command (hash "Subscribe" '("Key")))
    (yinz-then (yinz-test-plugin "agent-policy" (hash "kind" "unit" "filter" "session")) (lambda (report)
      (yinz-log (string-append "API_REPORT " (yinz-json-string report)))
      (set! pending (yinz-test-plugin "hanging.test.scm" (hash "kind" "unit")))
      (set! status "test running; press c to cancel") (yinz-request-render)
      (yinz-then (yinz-catch (yinz-then pending (lambda (_) (set! status "unexpected completion")))
          (lambda (failure) (set! status (string-append "cancelled: " failure))))
        (lambda (_) (yinz-request-render))))))))
(define (update event)
  (if (and (hash? event) (hash-contains? event "Key"))
      (begin (when pending (yinz-cancel! pending)) #t) #f))
(define (render rows columns) status)
"#).unwrap();
    let path = root.path().join("api.integration.test.scm");
    std::fs::write(
        &path,
        r#"
(test "async API agrees and stays responsive" (hash "grants" '("AuthorPlugins")) (lambda ()
  (test-session "start") (test-session "load" (hash "path" "./caller.scm"))
  (test-wait-for-text "test running; press c")
  (define line (trim (car (split-many (car (reverse (split-many (test-session "logs") "API_REPORT "))) "\n"))))
  (define report (yinz-json-parse line))
  (assert-equal (length (hash-ref report "tests")) 1)
  (define result (car (hash-ref report "tests")))
  (assert-equal (hash-ref result "kind") "unit") (assert-equal (hash-ref result "status") "passed")
  (assert (string-contains? (hash-ref result "name") "session"))
  (test-session "key" "c") (test-wait-for-text "cancelled:")))
"#,
    )
    .unwrap();
    let report = yinz_server::plugin_testing::run(
        &yinz_integration_tests::test_env::init().join("data"),
        &path,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

fn assert_server_gone(output: &str) {
    let pid = output
        .split("owned server pid: ")
        .nth(1)
        .unwrap_or_else(|| panic!("owned server diagnostic missing from full output:\n{output}"))
        .lines()
        .next()
        .unwrap();
    assert!(
        !std::process::Command::new("/bin/sh")
            .args(["-c", "kill -0 \"$1\" 2>/dev/null", "test-server", pid])
            .status()
            .unwrap()
            .success(),
        "owned server {pid} still exists"
    );
}

#[test]
fn installed_integration_server_crash_is_a_failure_and_is_reaped() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("crash.scm"),
        r#"
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("RunCommands")) (lambda (_)
    (yinz-run-command (hash "command" '("crash-server"))))))
"#,
    )
    .unwrap();
    let path = root.path().join("crash.integration.test.scm");
    std::fs::write(
        &path,
        r#"
(test "server crash" (hash "grants" '("RunCommands")) (lambda ()
  (test-session "fixture" (hash "files" (hash) "commands" (hash "crash-server" "kill -KILL \"$PPID\"")))
  (test-session "start") (test-session "load" (hash "path" "./crash.scm"))
  (test-wait-for-text "never rendered")))
"#,
    )
    .unwrap();
    let report = yinz_server::plugin_testing::run(
        &yinz_integration_tests::test_env::init().join("data"),
        &path,
        &PluginTestOptions {
            timeout_ms: 5000,
            ..Default::default()
        },
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!report.passed(), "{report:#?}");
    assert_eq!(report.tests.len(), 1, "{report:#?}");
    assert_eq!(report.tests[0].name, "server crash", "{report:#?}");
    assert!(
        report.tests[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cleanup:")),
        "server death must be reported during cleanup: {report:#?}"
    );
    assert!(
        report.tests[0].error.as_ref().unwrap().contains("signal"),
        "{report:#?}"
    );
    assert_server_gone(&report.tests[0].output);
}

#[test]
fn installed_cli_runs_builtin_tests_without_a_configured_plugin_directory() {
    let root = tempfile::tempdir().unwrap();
    yinz_steel_builtins::install(
        &yinz_steel_builtins::asset_dir(),
        &root.path().join("data/yinz"),
    )
    .unwrap();
    let binary = root.path().join("yinz");
    std::fs::copy(yinz_server::plugin_testing::binary().unwrap(), &binary).unwrap();
    let result = std::process::Command::new(binary)
        .arg("--data-dir")
        .arg(root.path().join("data/yinz"))
        .args([
            "plugins",
            "test",
            "agent-policy",
            "--kind",
            "unit",
            "--json",
        ])
        .current_dir(root.path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("TMPDIR", root.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: yinz_types::authoring::PluginTestReport =
        serde_json::from_slice(&result.stdout).unwrap();
    assert!(report.passed(), "{report:#?}");
    assert_eq!(report.tests.len(), 2);
}

#[test]
fn installed_session_rpc_uses_real_headless_routing_storage_and_caller_identity() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("service.scm"),
        r#"
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("PluginData" "MessageAndLaunchOtherPlugins")) (lambda (_)
    (yinz-data-define json-null (hash "name" "rpc_rows" "version" 1 "attrs"
      (list (hash "name" "value" "kind" "Text" "required" #t "indexed" #f "full_text" #f "unique" #f "default" json-null))))
    (yinz-rpc-handle "echo" (lambda (payload call)
      (when (equal? payload "fail") (error "fixture RPC failure"))
      (yinz-data-apply json-null (list (hash "Put" (hash "entity" "rpc_rows" "uid" "last"
        "values" (list (list "value" (hash "Text" payload))) "expect_rev" json-null))))
      (hash "echo" payload "source" (%async-get (hash-ref call "source") "Plugin" json-null))))
    (yinz-log "RPC_SERVICE_READY"))))
"#,
    )
    .unwrap();
    let path = root.path().join("rpc.integration.test.scm");
    std::fs::write(&path,r#"
(test "headless RPC uses authenticated callers and persists its result"
  (hash "grants" '("ReadCliPipes" "MessageAndLaunchOtherPlugins" "PluginData")) (lambda ()
    (test-session "start" (hash "backgroundPlugins" '("./service.scm")))
    (test-wait-for "headless RPC ready" (lambda () (string-contains? (test-session "logs") "RPC_SERVICE_READY")))
    (define reply (test-session "rpc" (hash "plugin" "./service.scm" "name" "echo" "payload" "hello" "timeoutMs" 10000)))
    (assert-equal (hash-ref reply "echo") "hello") (assert (number? (hash-ref reply "source")))
    (assert-equal (length (hash-ref (test-session "data" (hash "plugin" "./service.scm" "entity" "rpc_rows")) "rows")) 1)
    (assert-throws (lambda () (test-session "rpc" (hash "plugin" "./service.scm" "name" "echo" "payload" "fail" "timeoutMs" 10000))) "fixture RPC failure")))
"#).unwrap();
    let report = yinz_server::plugin_testing::run(
        &yinz_integration_tests::test_env::init().join("data"),
        &path,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn installed_palette_uses_real_keyboard_discovery_and_execution() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("palette.scm"),
        r#"
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState")) (lambda (_)
    (yinz-palette-register "static-fixture" "Static fixture"
      (list (hash "id" "static" "label" "Unique static fixture action" "aliases" '("static-keyword")
        "run" (lambda (values context) (yinz-log "STATIC_EXECUTED") json-null))))
    (yinz-palette-register "function-fixture" "Function fixture" (lambda (query context)
      (if (string-contains? query "dynamic-keyword")
          (list (hash "id" "dynamic" "label" "Unique dynamic fixture action"
            "run" (lambda (values context) (yinz-log "DYNAMIC_EXECUTED") json-null))) '())))
    (yinz-log "PALETTE_REGISTERED"))))
"#,
    )
    .unwrap();
    let path = root.path().join("palette.integration.test.scm");
    std::fs::write(
        &path,
        r#"
(test "real palette input" (hash "grants" '("ReadApplicationState")) (lambda ()
  (test-session "start" (hash "backgroundPlugins" '("./palette.scm")))
  (test-wait-for "palette registration" (lambda () (string-contains? (test-session "logs") "PALETTE_REGISTERED")))
  (for-each (lambda (entry)
    (define query (car entry)) (define label (cadr entry)) (define marker (caddr entry))
    (test-session "key" "Ctrl Alt p") (test-session "text" query)
    (test-wait-for-text label) (assert (not (string-contains? (test-session "logs") marker)))
    (test-session "key" "Enter")
    (test-wait-for "palette action executed" (lambda () (string-contains? (test-session "logs") marker)))
    ;; The callback logs before the screen receives its close reply.
    (test-wait-for "palette closed" (lambda () (not (string-contains? (test-session "screen") query)))))
    '(("static-keyword" "Unique static fixture action" "STATIC_EXECUTED")
      ("dynamic-keyword" "Unique dynamic fixture action" "DYNAMIC_EXECUTED")))))
"#,
    )
    .unwrap();
    let report = yinz_server::plugin_testing::run(
        &yinz_integration_tests::test_env::init().join("data"),
        &path,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn installed_headless_lifecycle_exercises_the_permission_modal() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("background.scm"),
        r#"
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState")) (lambda (_) (yinz-log "HEADLESS_READY"))))
"#,
    )
    .unwrap();
    let path = root.path().join("background.integration.test.scm");
    std::fs::write(&path, r#"
(define (focused-terminals)
  (filter (lambda (pane) (and (not (hash-ref pane "is_plugin")) (hash-ref pane "is_focused"))) (test-session "panes")))
(define (plugin-status path) (car (hash-ref (test-session "status" path) "plugins")))
(test "headless start and stop" (hash "grants" '("ReadApplicationState")) (lambda ()
  (test-session "start" (hash "autoGrantPermissions" #f))
  (test-wait-for-text "yinz-test>")
  (test-wait-for "focused terminal metadata" (lambda () (pair? (focused-terminals))))
  (define terminal (car (focused-terminals))) (assert terminal)
  (define first (test-session "load" (hash "path" "./background.scm" "headless" #t "configuration" (hash "shared" "true"))))
  (assert first) (define id (hash-ref (car (hash-ref first "plugins")) "id"))
  (assert-equal (hash-ref (car (hash-ref (test-session "load" (hash "path" "./background.scm" "headless" #t "configuration" (hash "shared" "true"))) "plugins")) "id") id)
  (test-wait-for-text "Plugin permissions")
  (assert-equal (hash-ref (plugin-status "./background.scm") "state") "awaiting_permission")
  (test-session "key" "Tab") (test-session "key" "Enter")
  (test-wait-for "permission promise settles" (lambda () (string-contains? (test-session "logs") "HEADLESS_READY")))
  (assert-equal (hash-ref (plugin-status "./background.scm") "state") "running")
  (define target (string-append "plugin_" (number->string id)))
  (assert-equal (hash-ref (car (hash-ref (test-session "reload" target) "plugins")) "id") id)
  (test-wait-for "replacement available" (lambda () (equal? (hash-ref (plugin-status target) "state") "running")))
  (assert-equal (hash-ref (car (focused-terminals)) "id") (hash-ref terminal "id"))
  (assert-equal (length (filter (lambda (pane) (not (hash-ref pane "is_plugin"))) (test-session "panes"))) 1)
  (assert-equal (hash-ref (car (hash-ref (test-session "stop" target) "plugins")) "state") "stopped")
  (assert-equal (hash-ref (test-session "status" "./background.scm") "plugins") '())
  (assert-throws (lambda () (test-session "reload" "./background.scm")) "not loaded; use plugins start")))
"#).unwrap();
    let report = yinz_server::plugin_testing::run(
        &yinz_integration_tests::test_env::init().join("data"),
        &path,
        &Default::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.passed(), "{report:#?}");
}

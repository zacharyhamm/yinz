#![cfg(unix)]

//! Scheme lifecycle, composition, and asynchronous palette behavior in whole sessions.

use std::fs;
use std::io::{Read, Write};
use yinz_cli::args::CliAction;
use yinz_integration_tests::{LayoutInfo, TestRunner, TERMINAL_SIZE};

fn lifecycle(
    session: &yinz_integration_tests::TestSession,
    operation: yinz_types::cli::PluginOperation,
    plugin: &std::path::Path,
    configuration: Option<std::collections::BTreeMap<String, String>>,
) -> yinz_types::cli::PluginResponse {
    yinz_client::store_client::plugin_request(
        &session.session_name().into(),
        yinz_types::cli::PluginRequest {
            operation,
            target: Some(plugin.display().to_string()),
            configuration,
            fallback: Some(yinz_types::input::layout::RunPlugin {
                location: yinz_types::input::layout::RunPluginLocation::File(plugin.to_owned()),
                ..Default::default()
            }),
            candidates: vec![],
            caller: None,
        },
    )
    .unwrap()
}

fn wait_for_plugin_state(
    session: &yinz_integration_tests::TestSession,
    plugin: &std::path::Path,
    state: yinz_types::cli::PluginState,
) -> yinz_types::cli::PluginStatus {
    let deadline = std::time::Instant::now() + yinz_test_support::default_timeout();
    loop {
        // Each correlated IPC reply observes the plugin thread after its preceding work.
        let response = lifecycle(
            session,
            yinz_types::cli::PluginOperation::Inspect,
            plugin,
            None,
        );
        if let Some(status) = response
            .plugins
            .first()
            .filter(|status| status.state == state)
        {
            return status.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "waiting for {state:?}: {response:?}"
        );
        std::thread::yield_now();
    }
}

fn receive_http(listener: &std::net::TcpListener) -> std::net::TcpStream {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut socket = loop {
        match listener.accept() {
            Ok((socket, _)) => break socket,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "HTTP guest did not connect"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            },
            Err(error) => panic!("HTTP accept: {error}"),
        }
    };
    // macOS inherits the listener's nonblocking mode.
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    socket
        .set_write_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        request.push(byte[0]);
        assert!(request.len() < 8192);
    }
    socket
}

#[test]
fn steel_native_compilation_failure_keeps_the_session_alive() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("recursive.scm");
    fs::write(
        &plugin,
        format!("{}0{}", "(".repeat(10_000), ")".repeat(10_000)),
    )
    .unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(&format!(
            "user_plugin_dir \"{}\"",
            directory.path().display()
        ))
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n tab {{\n pane {{ plugin location=\"file:{}\"; }}\n }}\n}}",
            plugin.display()
        )))
        .start();
    yinz.wait_until("compilation failure appears in the live session", |grid| {
        grid.contains("nesting")
    });
    fs::write(
        &plugin,
        r#"
(define state "checking")
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("AuthorPlugins" "ReadFiles"))
    (lambda (_)
      (define source (string-append (make-string 10000 #\() "0" (make-string 10000 #\))))
      (for-each (lambda (mode)
        (define failed (yinz-command (hash "CheckPlugin" (hash "name" "check.scm" "source" source "mode" mode))))
        (unless (member #t (map (lambda (d) (string-contains? d "nesting")) (hash-ref failed "diagnostics")))
          (error "missing compilation diagnostic"))
        (define healthy (yinz-command (hash "CheckPlugin" (hash "name" "check.scm" "source" "(void)" "mode" mode))))
        (unless (null? (hash-ref healthy "diagnostics")) (error (hash-ref healthy "diagnostics")))) '("Syntax" "Semantic"))
      (set! state "healthy after compilation failure"))))
(define (render rows columns) state)
"#,
    )
    .unwrap();
    let (code, output) = yinz.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: format!("file:{}", plugin.display()),
        configuration: None,
        configuration_json: None,
    });
    assert_eq!(code, 0, "{output}");
    yinz.wait_until("authoring permission prompt", |grid| {
        grid.contains("Plugin permissions")
    });
    // Failed initial loads are absent from the runtime map, so reload opens a new pane.
    let (code, output) = yinz.run_cli_action_reporting(CliAction::FocusNextPane);
    assert_eq!(code, 0, "{output}");
    yinz.send_stdin(b"\t\r");
    yinz.wait_until(
        "a healthy replacement renders in the same session",
        |grid| grid.contains("healthy after compilation failure"),
    );
    yinz.quit();
}

#[test]
fn embedded_steel_context_menu_hook_filters_and_dispatches_selection() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            r#"
mouse_mode true
load_plugins clear-defaults=true { "yinz:context-menu"; }
hooks { context_menu "yinz:context-menu"; }
context_menu {
    "Copy" { Copy; }
    separator
    "Scheme menu action" { NewTab; }
}
"#,
        )
        .start();
    yinz_integration_tests::claim_first_terminal_and_wait_for_prompt(&yinz);
    // Pipe completion waits for this background instance to finish loading before the
    // right-click starts the production menu fallback deadline.
    let (code, output) = yinz.run_cli_action_reporting(CliAction::Pipe {
        focus_plugin: false,
        name: Some("ready".into()),
        payload: Some(String::new()),
        args: None,
        plugin: Some("yinz:context-menu".into()),
        plugin_configuration: None,
        configuration_json: None,
        force_launch_plugin: false,
        skip_plugin_cache: false,
        floating_plugin: None,
        in_place_plugin: None,
        plugin_cwd: None,
        plugin_title: None,
    });
    assert_eq!(code, 0, "{output}");
    yinz.send_stdin(b"\x1b[<2;5;5M\x1b[<2;5;5m");
    yinz.wait_until("embedded hook filters Copy from the menu", |grid| {
        grid.contains("Scheme menu action") && !grid.contains("Copy")
    });
    yinz.send_stdin(b"\x1b[B\r");
    yinz.wait_until("configured menu action opens a second tab", |grid| {
        grid.contains("Tab #2")
    });
    yinz.quit();
}

#[test]
fn steel_pane_sequence_reuses_one_host_pane_and_cancels_its_child() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("sequence.scm");
    fs::write(&plugin, r#"
(define state "starting")
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("RunCommands" "ReadApplicationState" "ChangeApplicationState" "WriteToStdin"))
    (lambda (_)
      (define sequence (yinz-pane-sequence (hash "steps" (list (hash "path" "first" "stdin" "first-ready") (hash "path" "second" "stdin" "second-ready")))))
      (yinz-then (hash-ref sequence "done") (lambda (result)
        (unless (and (equal? (hash-ref result "status") "success") (equal? (hash-ref result "exitCodes") '(0 0))) (error "sequence result"))
        (set! state "sequence success") (yinz-request-render)
        (define cancelled (yinz-pane-sequence (hash "steps" (list (hash "path" "cancel" "stdin" "cancel-ready")) "killGraceMs" 0)))
        (define started (yinz-wait-for-event "CommandPaneOpened" (lambda (payload) (= (car payload) (hash-ref ((hash-ref cancelled "paneId")) "Terminal")))))
        (yinz-then started (lambda (_)
          ((hash-ref cancelled "cancel")) ((hash-ref cancelled "cancel"))
          (yinz-then (hash-ref cancelled "done") (lambda (stopped)
            (unless (equal? (hash-ref stopped "status") "cancelled") (error "cancel result"))
            (set! state "sequence and cancellation verified") (yinz-request-render))))))))))
(define (render rows columns) state)
"#).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n tab {{\n pane {{plugin location=\"file:{}\";}}\n }}\n}}",
            plugin.display()
        )))
        .start();
    yinz.wait_until("sequence permissions", |screen| {
        screen.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    let pane = yinz.expect_pty_spawn();
    pane.wait_for_stdin("first command starts before its stdin", |bytes| {
        bytes.ends_with(b"first-ready")
    });
    pane.exit(Some(0));
    pane.wait_for_stdin("second command reuses the first pane", |bytes| {
        bytes.ends_with(b"second-ready")
    });
    pane.exit(Some(0));
    let cancelled = yinz.expect_pty_spawn();
    cancelled.wait_for_stdin("cancellable command starts", |bytes| {
        bytes.ends_with(b"cancel-ready")
    });
    yinz.wait_until("sequence and cancellation finish", |screen| {
        screen.contains("sequence and cancellation verified")
    });
    assert!(cancelled.is_exited());
    yinz.quit();
}

#[test]
fn steel_pane_sequence_reload_does_not_advance_the_old_steps() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("reload-sequence.scm");
    fs::write(&plugin, r#"
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("RunCommands" "ReadApplicationState" "WriteToStdin"))
    (lambda (_)
      (hash-ref (yinz-pane-sequence (hash "steps" (list (hash "path" "old" "stdin" "old-ready") (hash "path" "must-not-start" "stdin" "old-late")))) "done"))))
(define (render rows columns) "old sequence")
"#).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n tab {{\n pane {{plugin location=\"file:{}\";}}\n }}\n}}",
            plugin.display()
        )))
        .start();
    yinz.wait_until("old sequence permissions", |screen| {
        screen.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    let pane = yinz.expect_pty_spawn();
    pane.wait_for_stdin("old step started", |bytes| bytes.ends_with(b"old-ready"));
    fs::write(
        &plugin,
        format!(
            r#"
(define state "starting replacement")
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("RunCommands" "ReadApplicationState" "WriteToStdin"))
    (lambda (_)
      (define waiting (yinz-run-in-pane {} (hash "path" "replacement" "stdin" "replacement-ready")))
      (set! state "replacement loaded") (yinz-request-render)
      (yinz-then waiting (lambda (result)
        (unless (= (hash-ref result "exitCode") 0) (error "replacement exit"))
        (set! state "replacement exit verified") (yinz-request-render))))))
(define (render rows columns) state)
"#,
            pane.terminal_id()
        ),
    )
    .unwrap();
    let (code, output) = yinz.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: format!("file:{}", plugin.display()),
        configuration: None,
        configuration_json: None,
    });
    assert_eq!(code, 0, "{output}");
    yinz.wait_until("replacement loaded", |screen| {
        screen.contains("replacement loaded")
    });
    pane.exit(Some(0));
    pane.wait_for_stdin("replacement starts on old pane", |bytes| {
        bytes.ends_with(b"replacement-ready")
    });
    pane.exit(Some(0));
    yinz.wait_until("replacement observes its own exit", |screen| {
        screen.contains("replacement exit verified")
    });
    assert!(!String::from_utf8_lossy(&pane.stdin_bytes()).contains("old-late"));
    yinz.quit();
}

#[test]
fn steel_pane_exit_observes_a_held_cli_pane() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("observer.scm");
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config("load_plugins clear-defaults=true {}")
        .start();
    yinz_integration_tests::claim_first_terminal_and_wait_for_prompt(&yinz);
    let (code, output) = yinz.run_cli_action_reporting(CliAction::NewPane {
        direction: None,
        command: vec!["controlled-command".to_owned()],
        remote_host: None,

        plugin: None,
        cwd: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
    });
    assert_eq!(code, 0, "{output}");
    let pane = yinz.expect_pty_spawn();
    fs::write(&plugin, format!(r#"
(define state "starting observer")
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState"))
    (lambda (_)
      (define waiting (yinz-wait-for-pane-exit {id}))
      (set! state "observer ready") (yinz-request-render)
      (yinz-then waiting (lambda (result)
        (define info (yinz-command (hash "GetPaneInfo" (hash "Terminal" {id}))))
        (unless (and (= (hash-ref (hash-ref result "paneId") "Terminal") {id})
                     (= (hash-ref result "exitCode") 23) (not (hash-ref result "closed"))
                     (hash-ref info "is_held") (hash-ref info "exited") (= (hash-ref info "exit_status") 23))
          (error "held exit mismatch"))
        (set! state "held CLI exit verified") (yinz-request-render))))))
(define (render rows columns) state)
"#, id=pane.terminal_id())).unwrap();
    let (code, output) = yinz.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: format!("file:{}", plugin.display()),
        configuration: None,
        configuration_json: None,
    });
    assert_eq!(code, 0, "{output}");
    yinz.wait_until("observer permission prompt", |screen| {
        screen.contains("Plugin permissions")
    });
    assert_eq!(yinz.run_cli_action(CliAction::FocusNextPane), 0);
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("observer registered before the exit", |screen| {
        screen.contains("observer ready")
    });
    pane.exit(Some(23));
    yinz.wait_until("separate observer sees held CLI exit status", |screen| {
        screen.contains("held CLI exit verified")
    });
    yinz.quit();
}

#[test]
fn steel_multipane_reserves_launch_slots_and_keeps_exit_and_failed_plugin_in_place() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("multipane.scm");
    let permission_child = directory.path().join("permission-child.scm");
    fs::write(
        &permission_child,
        r#"
(define state "CHILD LOADING")
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("WebAccess")) (lambda (_) (set! state "CHILD READY"))))
(define (render rows columns) state)
"#,
    )
    .unwrap();
    fs::write(&plugin, r#"
(define state "requesting composition") (define container #f)
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ChangeApplicationState" "RunCommands" "OpenTerminalsOrPlugins" "ReadApplicationState"))
    (lambda (_)
      (define own (hash "Plugin" (hash-ref (yinz-command "GetPluginIds") "plugin_id")))
      (define placement (hash "kind" "Dismissible" "coordinates"
        (hash "x" (hash "Fixed" 3) "y" (hash "Fixed" 3) "width" (hash "Fixed" 90) "height" (hash "Fixed" 18))))
      (define split (hash "direction" "LeftRight" "ratio" 5000))
      (with-handler (lambda (failure) (unless (string-contains? (error-object-message failure) "distinct") (raise-error failure)))
        (yinz-command (hash "CreateMultipane" (hash "children" (list (hash "Existing" own) (hash "Existing" own)) "placement" placement "split" split "focus" json-null)))
        (error "duplicate adoption succeeded"))
      (set! container (yinz-command (hash "CreateMultipane" (hash "children" (list (hash "Existing" own) (hash "Terminal" (hash "path" "multipane-command"))) "placement" placement "split" split "focus" 0))))
      (unless (and (equal? (car (hash-ref container "children")) own) (hash-contains? (cadr (hash-ref container "children")) "Terminal")) (error "child order"))
      (yinz-command (hash "UpdateMultipane" (hash "id" (hash-ref container "id") "coordinates" json-null "split" (hash "direction" "TopBottom" "ratio" 4000) "override_user_placement" #f)))
      (yinz-command (hash "Subscribe" '("Key"))) (set! state "COMPOSITION OWNER"))))
(define (update event)
  (define key (and (hash? event) (hash-contains? event "Key") (let ([bare (hash-ref (hash-ref event "Key") "bare_key")]) (and (hash? bare) (and (hash-contains? bare "Char") (hash-ref bare "Char"))))))
  (define (replace child) (yinz-command (hash "ReplaceMultipaneChild" (hash "id" (hash-ref container "id") "slot" 1 "child" child))))
  (cond [(equal? key "t") (replace (hash "Terminal" (hash "path" "multipane-replacement"))) #t]
        [(equal? key "p") (replace (hash "Plugin" (hash "url" "PERMISSION_CHILD" "configuration" (hash)))) #t]
        [(equal? key "q") (yinz-command "CloseSelf") #f]
        [(equal? key "r") (replace (hash "Plugin" (hash "url" "file:/missing-multipane-child.scm" "configuration" (hash)))) (set! state "SIBLING SURVIVED") #t]
        [else #f]))
(define (render rows columns) state)
"#.replace("PERMISSION_CHILD", &format!("file:{}", permission_child.display()))).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n pane\n pane focus=true {{plugin location=\"file:{}\";}}\n}}",
            plugin.display()
        )))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(b"BASE SHELL");
    yinz.wait_until("composition permission prompt", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    let command = yinz.expect_pty_spawn();
    command.wait_for_command("multipane-command");
    command.output(b"LAUNCHED IN SLOT");
    yinz.wait_until("both owned slots render", |grid| {
        grid.contains("COMPOSITION OWNER") && grid.contains("LAUNCHED IN SLOT")
    });
    let list = || {
        let (code, output) = yinz.run_cli_action_reporting(CliAction::ListPanes {
            tab: false,
            command: false,
            state: true,
            geometry: true,
            all: true,
            json: true,
        });
        assert_eq!(code, 0, "{output}");
        serde_json::from_str::<Vec<serde_json::Value>>(&output).unwrap()
    };
    let panes = list();
    let members: Vec<_> = panes
        .iter()
        .filter(|pane| pane.get("multipane").is_some())
        .collect();
    assert_eq!(members.len(), 2);
    let info = members[0]["multipane"].clone();
    assert_eq!(members[1]["multipane"], info);
    assert_eq!(info["split"]["direction"], "TopBottom");
    command.exit(Some(17));
    yinz.wait_until("exited command stays in its slot", |_| {
        list().iter().any(|pane| {
            pane["multipane"]["id"] == info["id"]
                && pane["is_held"] == true
                && pane["exit_status"] == 17
        })
    });
    yinz.run_cli_action(CliAction::FocusPaneId {
        pane_id: format!("terminal_{}", command.terminal_id()),
    });
    yinz.wait_until("focused exit status uses the outer frame", |grid| {
        grid.contains("COMPOSITION OWNER") && grid.contains("17")
    });
    let owner = members
        .iter()
        .find(|pane| pane["is_plugin"] == true)
        .unwrap();
    yinz.run_cli_action(CliAction::FocusPaneId {
        pane_id: format!("plugin_{}", owner["id"]),
    });
    assert_eq!(
        list()
            .iter()
            .filter(|pane| pane["multipane"]["id"] == info["id"])
            .count(),
        2
    );
    // Replacement must start immediately even when the current child is still running.
    for message in ["FIRST REPLACEMENT", "RUNNING CHILD REPLACED"] {
        yinz.send_stdin(b"t");
        let replacement = yinz.expect_pty_spawn();
        replacement.wait_for_command("multipane-replacement");
        replacement.output(message.as_bytes());
        yinz.wait_until("terminal replacement stays beside its owner", |grid| {
            grid.contains(message) && grid.contains("COMPOSITION OWNER")
        });
    }
    yinz.send_stdin(b"r");
    yinz.wait_until("failed child launch stays beside its sibling", |grid| {
        grid.contains("SIBLING SURVIVED")
            && (grid.contains("Failed") || grid.contains("failed") || grid.contains("ERROR"))
    });
    let members: Vec<_> = list()
        .into_iter()
        .filter(|pane| pane["multipane"]["id"] == info["id"])
        .collect();
    assert_eq!(members.len(), 2);
    assert!(members.iter().all(|pane| pane["is_plugin"] == true));
    assert_eq!(members[0]["multipane"], members[1]["multipane"]);
    yinz.send_stdin(b"p");
    yinz.wait_until("child permission prompt stays in its slot", |grid| {
        grid.contains("SIBLING SURVIVED") && grid.contains("Plugin permissions")
    });
    assert_eq!(
        list()
            .iter()
            .filter(|pane| pane["multipane"]["id"] == info["id"])
            .count(),
        2
    );
    let child = list()
        .into_iter()
        .find(|pane| pane["multipane"]["id"] == info["id"] && pane["id"] != owner["id"])
        .unwrap();
    yinz.run_cli_action(CliAction::FocusPaneId {
        pane_id: format!("plugin_{}", child["id"]),
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("delayed child finishes in its slot", |grid| {
        grid.contains("SIBLING SURVIVED") && grid.contains("CHILD READY")
    });
    yinz.run_cli_action(CliAction::FocusPaneId {
        pane_id: format!("plugin_{}", owner["id"]),
    });
    yinz.send_stdin(b"q");
    yinz.wait_until("close_self closes the container", |grid| {
        grid.contains("BASE SHELL")
            && !grid.contains("SIBLING SURVIVED")
            && !grid.contains("CHILD READY")
    });
    assert!(list().iter().all(|pane| pane.get("multipane").is_none()));
    yinz.quit();
}

#[test]
fn native_pane_tabs_mix_fixture_and_scheme_and_retain_hidden_state() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("tab-child.scm");
    let failing = directory.path().join("later.scm");
    fs::write(
        &plugin,
        r#"
(define count 0) (define hides 0) (define hidden #f) (define updates 0) (define ready #f)
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState"))
    (lambda (_) (yinz-command (hash "Subscribe" '("Key" "Visible" "PaneUpdate")))
      (set! ready #t) (yinz-request-render))))
(define (update event)
  (when (hash? event)
    (when (hash-contains? event "Key")
      (define bare (hash-ref (hash-ref event "Key") "bare_key"))
      (if (and (hash? bare) (equal? (and (hash-contains? bare "Char") (hash-ref bare "Char")) "c")) (yinz-command "CloseSelf")
          (set! count (+ count 1))))
    (when (hash-contains? event "Visible")
      (set! hidden (not (hash-ref event "Visible"))) (when hidden (set! hides (+ hides 1))))
    (when (and (hash-contains? event "PaneUpdate") hidden) (set! updates (+ updates 1)))) #t)
(define (render rows columns)
  (if ready
    (string-append "SCHEME keys=" (number->string count) " hides=" (number->string hides) " hidden_updates=" (number->string updates))
    "Waiting for permissions"))
"#,
    )
    .unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            r#"
layout {{
    pane tabbed=true {{
        pane name="Fixture" {{ plugin location="file:{}"; }}
        pane name="Scheme" {{ plugin location="file:{}"; }}
        pane name="Retry" {{ plugin location="file:{}"; }}
    }}
}}
"#,
            yinz_integration_tests::test_env::fixture_plugin_path().display(),
            plugin.display(),
            failing.display()
        )))
        .start();
    yinz.wait_until("Fixture requests its own permissions", |grid| {
        grid.contains(" Fixture │") && grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("native bar over Fixture content", |grid| {
        grid.contains(" Fixture │") && grid.contains("Received events")
    });
    yinz.run_cli_action(CliAction::NextPaneTab);
    yinz.wait_until("selected child requests its own permissions", |grid| {
        grid.contains(" Scheme │") && grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("Scheme loaded in the selected slot", |grid| {
        grid.contains("SCHEME keys=0")
    });
    yinz.send_stdin(b"x");
    yinz.wait_until("content input reaches Scheme", |grid| {
        grid.contains("SCHEME keys=1")
    });
    yinz.run_cli_action(CliAction::PreviousPaneTab);
    yinz.wait_until("Fixture retained", |grid| {
        grid.contains(" Fixture │") && grid.contains("Received events")
    });
    yinz.run_cli_action(CliAction::NextPaneTab);
    yinz.wait_until(
        "hidden child retained its state and received visibility",
        |grid| grid.contains("SCHEME keys=1 hides=1") && !grid.contains("hidden_updates=0"),
    );
    yinz.run_cli_action(CliAction::FocusPaneTabs);
    yinz.send_stdin(b"\x1b[Dj?\x1b[C\rx");
    yinz.wait_until("only the key after Enter reaches content", |grid| {
        grid.contains("SCHEME keys=2 hides=1")
    });
    yinz.send_stdin(b"c");
    yinz.wait_until(
        "a child can close without removing its fixed slot",
        |grid| grid.contains(" Scheme │") && grid.contains("Closed (r: reopen)"),
    );
    yinz.send_stdin(b"r");
    yinz.wait_until("closed child reopens as a fresh instance", |grid| {
        grid.contains("SCHEME keys=0")
    });
    yinz.run_cli_action(CliAction::NextPaneTab);
    yinz.wait_until(
        "a failed child exposes retry without losing the bar",
        |grid| grid.contains(" Retry │") && grid.contains("r: retry"),
    );
    fs::write(
        &failing,
        "(define (render rows columns) \"RETRY SUCCEEDED\")",
    )
    .unwrap();
    yinz.send_stdin(b"r");
    yinz.wait_until("retry replaces the failed load attempt", |grid| {
        grid.contains("RETRY SUCCEEDED")
    });
    yinz.run_cli_action(CliAction::PreviousPaneTab);
    let peer = yinz.attach_client(TERMINAL_SIZE);
    peer.wait_until("new client sees the shared selection", |grid| {
        grid.contains(" Scheme │") && grid.contains("SCHEME keys=0")
    });
    yinz.run_cli_action(CliAction::PreviousPaneTab);
    peer.wait_until("selection changes for both clients", |grid| {
        grid.contains(" Fixture │") && grid.contains("Received events")
    });
    peer.quit();
    yinz.quit();
}

#[test]
fn steel_palette_network_discovery_is_async_and_obsolete_replies_are_ignored() {
    use yinz_integration_tests::keys;
    const TOGGLE: &[u8] = b"\x1b[112;7u";
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("palette.scm");
    fs::write(&plugin, format!(r#"
(define state "starting") (define received 0)
(define (result-view response)
  (list (hash "id" "result" "label" (bytes->string/utf8 (hash-ref response "body"))
    "run" (lambda (_ context)
      (hash "type" "actions" "title" "Nested" "actions" (list
        (hash "id" "form" "label" "Form" "run" (lambda (_ context)
          (hash "type" "form" "title" "Input" "fields" (list (hash "type" "text" "id" "name" "label" "Name"))
            "onSubmit" (lambda (values context)
              (when (equal? (hash-ref values "name") "") (error "Name required"))
              (hash "type" "text" "title" "Output" "text" (hash-ref values "name"))))))
        (hash "id" "close" "label" "Close now" "run" (lambda (_ execution)
          ((hash-ref execution "close"))
          (yinz-then (yinz-sleep 10) (lambda (_) (set! state "closed operation finished") (yinz-request-render)))))))))))
(define (discover query context)
  (set! state (string-append "request:" query)) (yinz-request-render)
  (cond [(equal? query "") '()]
        [(not (starts-with? query "http")) (yinz-deferred)]
        [else
          (define request (yinz-web-request (hash "url" (string-append "http://{address}/?q=" (string-replace query " " "%20")))))
          (define remove ((hash-ref context "onCancel") (lambda () (yinz-cancel! request))))
          (yinz-catch (yinz-then request (lambda (response)
            (remove)
            (unless (= (hash-ref (yinz-command "GetPluginIds") "client_id") (hash-ref context "clientId")) (error "wrong client"))
            (result-view response)))
            (lambda (failure) (remove) (error failure)))]))
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState" "WebAccess"))
    (lambda (_)
      (yinz-on-event "WebRequestResult" (lambda (_) (set! received (+ received 1)) #t))
      (yinz-palette-register "remote" "Remote" discover)
      (set! state "palette ready") (yinz-request-render))))
(define (render rows columns) (string-append state "\nreceived:" (number->string received)))
"#)).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{ pane {{ plugin location=\"file:{}\"; }}; }}",
            plugin.display()
        )))
        .start();
    yinz.wait_until("palette permission prompt", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("provider registered", |grid| grid.contains("palette ready"));
    yinz.send_stdin(TOGGLE);
    yinz.send_stdin(b"Rename pane");
    yinz.wait_until("delayed provider leaves native match usable", |grid| {
        grid.contains("request:Rename pane") && grid.contains("Rename pane")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("native form while discovery awaits", |grid| {
        grid.contains("Name:")
    });
    yinz.send_stdin(b"native rename");
    yinz.send_stdin(&keys::ENTER);
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("native action completes", |grid| {
        !grid.contains("Do · Project")
    });

    yinz.send_stdin(TOGGLE);
    yinz.send_stdin(b"http old");
    let mut old = receive_http(&listener);
    yinz.send_stdin(&[21]); // Ctrl+u replaces the query.
    yinz.send_stdin(b"http new");
    let mut new = receive_http(&listener);
    new.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\nRemote result",
    )
    .unwrap();
    yinz.wait_until("semantic result survives host matching", |grid| {
        grid.contains("Remote result")
    });
    let _ = old.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nStale result",
    );
    yinz.wait_until("old host reply has been dispatched", |grid| {
        grid.contains("received:2")
            && grid.contains("Remote result")
            && !grid.contains("Stale result")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("nested action list", |grid| {
        grid.contains("Nested") && grid.contains("Form")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("plugin form", |grid| {
        grid.contains("Input") && grid.contains("Name:")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("inline validation error", |grid| {
        grid.contains("Name required")
    });
    yinz.send_stdin(b"\x1b[A");
    yinz.send_stdin(b"retained value");
    yinz.send_stdin(&keys::ENTER);
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("text output", |grid| {
        grid.contains("Output") && grid.contains("retained value")
    });
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("back retains form values", |grid| {
        grid.contains("Input") && grid.contains("retained value")
    });
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("back to nested actions", |grid| {
        grid.contains("Nested") && grid.contains("Close now")
    });
    yinz.send_stdin(b"\x1b[B");
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("callback continues after immediate close", |grid| {
        grid.contains("closed operation finished") && !grid.contains("Do · Project")
    });
    yinz.quit();
}

#[test]
fn steel_palette_shared_clients_reload_and_unload_keep_separate_views() {
    use yinz_integration_tests::keys;
    const TOGGLE: &[u8] = b"\x1b[112;7u";
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("shared-palette.scm");
    let source = r#"
(define version "original") (define state "starting")
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState")) (lambda (_)
    (yinz-palette-register "shared" "Shared" (lambda (query context)
      (yinz-then json-null (lambda (_)
        (unless (= (hash-ref (yinz-command "GetPluginIds") "client_id") (hash-ref context "clientId")) (error "client changed"))
        (if (equal? query "") '()
          (list (hash "id" "item" "label" (string-append version ":" query) "run" (lambda (_ execution)
            (yinz-then json-null (lambda (_)
              (unless (= (hash-ref (yinz-command "GetPluginIds") "client_id") (hash-ref execution "clientId")) (error "execution client changed"))
              (hash "type" "text" "title" (string-append "Private " query) "text" (string-append "client " (number->string (hash-ref execution "clientId"))))))))))))))
    (set! state (string-append version " ready")) (yinz-request-render))))
(define (render rows columns) state)
"#;
    fs::write(&plugin, source).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}\npane_frames false")
        .with_layout(LayoutInfo::Stringified(format!("layout {{ pane; pane focus=true {{ plugin location=\"file:{}\" {{ shared true; }}; }}; }}", plugin.display())))
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(b"terminal ready\r\n");
    yinz.wait_until("shared palette permission", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("shared provider ready", |grid| {
        grid.contains("original ready")
    });
    let other = yinz.attach_client(TERMINAL_SIZE);
    other.wait_until("second client ready", |grid| {
        grid.contains("terminal ready")
    });
    yinz.send_stdin(TOGGLE);
    yinz.send_stdin(b"alpha");
    other.send_stdin(TOGGLE);
    other.send_stdin(b"beta");
    yinz.wait_until("first client results", |grid| {
        grid.contains("original:alpha") && !grid.contains("original:beta")
    });
    other.wait_until("second client results", |grid| {
        grid.contains("original:beta") && !grid.contains("original:alpha")
    });
    // A native pane title/path can also fuzzy-match alpha. Select our provider's action.
    let mut selection = yinz.snapshot();
    while !selection.contains("› original:alpha") {
        yinz.send_stdin(&keys::ctrl('n'));
        selection = yinz.wait_until("palette selection moves to the plugin action", |grid| {
            grid.text != selection.text
        });
    }
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("first private view", |grid| grid.contains("Private alpha"));
    other.wait_until("second remains at root", |grid| {
        grid.contains("original:beta") && !grid.contains("Private alpha")
    });
    fs::write(&plugin, source.replace("\"original\"", "\"replacement\"")).unwrap();
    assert_eq!(
        yinz.run_cli_action(CliAction::StartOrReloadPlugin {
            url: format!("file:{}", plugin.display()),
            configuration: None,
            configuration_json: None,
        }),
        0
    );
    yinz.wait_until("reload returns nested client to root", |grid| {
        grid.contains("replacement:alpha")
            && grid.contains("removed or reloaded")
            && !grid.contains("Private alpha")
    });
    other.wait_until("reload refreshes second client", |grid| {
        grid.contains("replacement:beta") && !grid.contains("original:beta")
    });
    // Closing the focused plugin leaves the terminal and both clients' palettes alive.
    assert_eq!(
        yinz.run_cli_action(CliAction::ClosePane { pane_id: None }),
        0
    );
    yinz.wait_until("unload removes first client's provider entries", |grid| {
        grid.contains("terminal ready") && !grid.contains("replacement:alpha")
    });
    other.wait_until("unload removes second client's provider entries", |grid| {
        !grid.contains("replacement:beta")
    });
    yinz.quit();
}

#[test]
fn steel_unload_awaits_commands_on_reload_close_self_pane_close_and_shutdown() {
    for close in ["pane", "self", "shutdown"] {
        let directory = tempfile::tempdir().unwrap();
        let plugin = directory.path().join("unload.scm");
        let marker = directory.path().join("unloaded");
        fs::write(&plugin, format!(r#"
(define ready #f)
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("RunCommands" "ReadApplicationState" "ChangeApplicationState"))
    (lambda (_) (yinz-command (hash "Subscribe" '("Key"))) (set! ready #t))))
(define (update event)
  (when (and (hash? event) (hash-contains? event "Key"))
    (define bare (hash-ref (hash-ref event "Key") "bare_key"))
    (when (and (hash? bare) (equal? (and (hash-contains? bare "Char") (hash-ref bare "Char")) "q")) (yinz-command "CloseSelf"))) #f)
(define (render rows columns) (if ready "unload fixture ready" "waiting"))
(define (unload)
  (yinz-then (yinz-run-command (hash "command" (list "sh" "-c" "printf 'start\\n' >> \"$1\"" "sh" {marker})))
    (lambda (_) (yinz-then (yinz-sleep 10) (lambda (_)
      (yinz-run-command (hash "command" (list "sh" "-c" "printf 'done\\n' >> \"$1\"" "sh" {marker}))))))))
"#, marker=serde_json::to_string(&marker).unwrap())).unwrap();
        let mut yinz = TestRunner::new(TERMINAL_SIZE)
            .with_config_dir(directory.path().to_owned())
            .with_layout(LayoutInfo::Stringified(format!(
                "layout {{ pane; pane focus=true {{ plugin location=\"file:{}\"; }}; }}",
                plugin.display()
            )))
            .start();
        yinz.wait_until("unload fixture permission prompt", |grid| {
            grid.contains("Plugin permissions")
        });
        yinz.send_stdin(b"\t\r");
        yinz.wait_until("unload fixture ready", |grid| {
            grid.contains("unload fixture ready")
        });
        fs::write(
            &plugin,
            fs::read_to_string(&plugin)
                .unwrap()
                .replace("\"unload fixture ready\"", "\"replacement fixture ready\""),
        )
        .unwrap();
        let reload = || CliAction::StartOrReloadPlugin {
            url: format!("file:{}", plugin.display()),
            configuration: None,
            configuration_json: None,
        };
        let (code, output) = yinz.run_cli_action_reporting(reload());
        assert_eq!(code, 0, "{output}");
        yinz.wait_until(
            "reload cleanup finished before replacement renders",
            |grid| {
                grid.contains("replacement fixture ready")
                    && fs::read_to_string(&marker).unwrap_or_default() == "start\ndone\n"
            },
        );
        match close {
            "self" => {
                yinz.send_stdin(b"q");
                yinz.wait_until("closeSelf removes the pane", |grid| {
                    !grid.contains("replacement fixture ready")
                });
            },
            "pane" => {
                let (code, output) =
                    yinz.run_cli_action_reporting(CliAction::ClosePane { pane_id: None });
                assert_eq!(code, 0, "{output}");
                yinz.wait_until("close removes the pane", |grid| {
                    !grid.contains("replacement fixture ready")
                });
            },
            _ => {},
        }
        // Shutdown joins the plugin executor, including an already queued close job.
        yinz.quit();
        assert_eq!(
            fs::read_to_string(&marker).unwrap(),
            "start\ndone\n".repeat(2),
            "{close}"
        );
    }
}

#[test]
fn steel_palette_static_aliases_nested_reregistration_and_shared_reload() {
    use yinz_integration_tests::keys;
    const TOGGLE: &[u8] = b"\x1b[112;7u";
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("static-palette.scm");
    let source = r#"
(define version "original") (define state "starting") (define unregister #f)
(define (register)
  (set! unregister (yinz-palette-register "pdf" "PDF tools" (list
    (hash "id" "open" "label" (string-append "Open PDF… " version) "aliases" '("reader" "document")
      "run" (lambda (_ context)
        (yinz-then json-null (lambda (_)
          (unless (= (hash-ref (yinz-command "GetPluginIds") "client_id") (hash-ref context "clientId")) (error "wrong client"))
          (hash "type" "actions" "title" "PDF options" "actions" (list
            (hash "id" "reset" "label" "Register again" "aliases" '("reset") "run" (lambda (_ execution)
              ((hash-ref execution "close")) (unregister) (unregister) (register)
              (set! state (string-append version " registered again")) (yinz-request-render)))))))))))))
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState"))
    (lambda (_) (register) (set! state (string-append version " ready")) (yinz-request-render))))
(define (render rows columns) state)
"#;
    fs::write(&plugin, source).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}\npane_frames false")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{ pane; pane focus=true {{ plugin location=\"file:{}\" {{ shared true; }}; }}; }}",
            plugin.display()
        )))
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(b"terminal ready\r\n");
    yinz.wait_until("static provider permission", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("static provider ready", |grid| {
        grid.contains("original ready")
    });
    yinz.send_stdin(TOGGLE);
    for query in ["pdf", "reader", "document"] {
        yinz.send_stdin(&keys::ctrl('u'));
        yinz.send_stdin(query.as_bytes());
        yinz.wait_until("static label and aliases match", |grid| {
            grid.contains(query) && grid.contains("› Open PDF… original")
        });
    }
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("static nested view", |grid| grid.contains("PDF options"));
    yinz.send_stdin(b"reset");
    yinz.wait_until("nested alias matches", |grid| {
        grid.contains("› Register again")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("provider registered again", |grid| {
        grid.contains("original registered again") && !grid.contains("PDF options")
    });
    let other = yinz.attach_client(TERMINAL_SIZE);
    other.wait_until("second client ready", |grid| {
        grid.contains("terminal ready")
    });
    yinz.send_stdin(TOGGLE);
    yinz.send_stdin(b"reader");
    other.send_stdin(TOGGLE);
    other.send_stdin(b"document");
    yinz.wait_until("first client static results", |grid| {
        grid.contains("› Open PDF… original")
    });
    other.wait_until("second client static results", |grid| {
        grid.contains("› Open PDF… original")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("first client nested", |grid| grid.contains("PDF options"));
    fs::write(&plugin, source.replace("\"original\"", "\"replacement\"")).unwrap();
    assert_eq!(
        yinz.run_cli_action(CliAction::StartOrReloadPlugin {
            url: format!("file:{}", plugin.display()),
            configuration: None,
            configuration_json: None,
        }),
        0
    );
    yinz.wait_until("reload restores first client to root", |grid| {
        grid.contains("Open PDF… replacement")
            && grid.contains("removed or reloaded")
            && !grid.contains("PDF options")
    });
    other.wait_until("reload refreshes second client", |grid| {
        grid.contains("Open PDF… replacement") && !grid.contains("Open PDF… original")
    });
    other.send_stdin(&keys::ENTER);
    other.wait_until("replacement executes for second client", |grid| {
        grid.contains("PDF options")
    });
    yinz.quit();
}

#[test]
fn background_startup_rejections_leave_lifecycle_commands_usable() {
    use yinz_types::cli::{PluginOperation as Op, PluginState};
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("valid.scm");
    fs::write(&plugin, "(void)").unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(&format!(
            "load_plugins clear-defaults=true {{\n  \"unknown-alias\"\n  \"yinz:unknown-builtin\"\n  \"file:{}\"\n}}",
            plugin.display()
        ))
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(b"STARTUP SURVIVED\r\n");
    yinz.wait_until("terminal after rejected background entries", |grid| {
        grid.contains("STARTUP SURVIVED")
    });
    let first = wait_for_plugin_state(&yinz, &plugin, PluginState::Running);
    assert!(lifecycle(&yinz, Op::List, &plugin, None)
        .plugins
        .iter()
        .any(|status| status.id == first.id));
    assert_eq!(
        lifecycle(&yinz, Op::Start, &plugin, None).plugins[0].id,
        first.id
    );
    assert_eq!(
        lifecycle(&yinz, Op::Reload, &plugin, None).plugins[0].id,
        first.id
    );
    wait_for_plugin_state(&yinz, &plugin, PluginState::Running);
    assert_eq!(
        lifecycle(&yinz, Op::Stop, &plugin, None).plugins[0].state,
        PluginState::Stopped
    );
    assert!(lifecycle(&yinz, Op::Inspect, &plugin, None)
        .plugins
        .is_empty());
    lifecycle(&yinz, Op::Start, &plugin, None);
    wait_for_plugin_state(&yinz, &plugin, PluginState::Running);
    yinz.quit();
}

#[test]
fn steel_palette_completion_notifies_above_its_result_view() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("completion.scm");
    fs::write(
        &plugin,
        r#"
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState")) (lambda (_)
    (yinz-palette-register "reports" "Reports" (list
      (hash "id" "save" "label" "Save report" "run" (lambda (_ context)
        (yinz-then json-null (lambda (_)
          (hash "type" "complete" "notification" "Report saved successfully" "view"
            (hash "type" "text" "title" "Saved report" "text" "Report contents"))))))))
    (yinz-request-render))))
(define (render rows columns) "Completion provider ready")
"#,
    )
    .unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{ pane; pane focus=true {{ plugin location=\"file:{}\"; }}; }}",
            plugin.display()
        )))
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(b"terminal ready\r\n");
    yinz.wait_until("completion provider permission", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("completion provider ready", |grid| {
        grid.contains("Completion provider ready")
    });
    yinz.send_stdin(b"\x1b[112;7u");
    yinz.send_stdin(b"Save report");
    yinz.wait_until("completion action", |grid| grid.contains("› Save report"));
    yinz.send_stdin(b"\r");
    yinz.wait_until("completion popup and result view", |grid| {
        grid.contains("Report saved successfully") && grid.contains("Report contents")
    });
    yinz.quit();
}

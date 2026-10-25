#![cfg(unix)]

//! Scheme and builtin plugins share a session: the real permission prompt, event delivery, reload and shutdown.

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
    let deadline = std::time::Instant::now() + yinz_integration_tests::default_timeout();
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

fn background_source(version: &str) -> String {
    format!(
        r#"
(define (load configuration)
  (yinz-catch
    (yinz-then (yinz-wait-for-permissions '("ReadApplicationState" "RunActionsAsUser" "ReadCliPipes"))
      (lambda (_)
        (yinz-palette-register "background" "Background"
          (lambda (query context)
            (list (hash "id" "unique" "label" "zzbgpermissionaction {version}"
              "run" (lambda (values execution) (hash "type" "text" "title" "Done" "text" "Approved"))))))))
    (lambda (_) (yinz-log "BACKGROUND DENIED"))))
(define (pipe message)
  (define source (hash-ref message "source"))
  (when (and (hash? source) (hash-contains? source "Cli"))
    (yinz-command (hash "CliPipeOutput" (list (hash-ref source "Cli") "{version}")))) #f)
"#
    )
}

#[test]
fn launch_or_focus_retains_the_pane_after_symlink_reload_by_id_and_url() {
    for (by_id, floating, alias) in [(true, false, false), (false, true, true)] {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("v1.scm");
        let second = directory.path().join("v2.scm");
        let current = directory.path().join("current.scm");
        fs::write(
            &first,
            r#"
(yinz-declare-pane)
(define ready #f)
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ChangeApplicationState")) (lambda (_)
    (yinz-command (hash "Subscribe" '("Key"))) (set! ready #t) (yinz-request-render))))
(define (update event)
  (when (and (hash? event) (hash-contains? event "Key"))
    (yinz-command (hash "ReloadPlugin" (hash-ref (yinz-command "GetPluginIds") "plugin_id")))) #f)
(define (render rows columns) (if ready "symlink version one ready" "waiting"))
"#,
        )
        .unwrap();
        fs::write(
            &second,
            r#"
(yinz-declare-pane)
(define state "symlink version two ready")
(define (load configuration) (yinz-command (hash "Subscribe" '("Key"))))
(define (update event)
  (if (and (hash? event) (hash-contains? event "Key"))
      (begin (set! state "stored binding focused replacement") #t) #f))
(define (render rows columns) state)
"#,
        )
        .unwrap();
        std::os::unix::fs::symlink(&first, &current).unwrap();
        let url = format!("file:{}", current.display());
        let binding_url = if alias { "current" } else { &url };
        let mut yinz = TestRunner::new(TERMINAL_SIZE)
            .with_config_dir(directory.path().to_owned())
            .with_config(&format!(
                r#"
load_plugins clear-defaults=true {{}}
plugins {{ current location="{url}"; }}
keybinds {{ normal {{ bind "Ctrl o" {{
    LaunchOrFocusPlugin "{binding_url}" {{ floating {floating}; }}
}}; }}; }}
"#
            ))
            .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
            .start();
        let terminal = yinz.expect_pty_spawn();
        terminal.output(b"TERMINAL READY\r\n");
        yinz.wait_until("terminal ready", |grid| grid.contains("TERMINAL READY"));
        let launch = |url: String| CliAction::LaunchOrFocusPlugin {
            floating,
            in_place: false,
            close_replaced_pane: false,
            move_to_focused_tab: false,
            url,
            configuration: None,
            configuration_json: None,
            skip_plugin_cache: false,
            tab_id: None,
        };
        let (code, original_pane) = yinz.run_cli_action_reporting(launch(url.clone()));
        assert_eq!(code, 0, "{original_pane}");
        assert!(
            original_pane.trim().starts_with("plugin_"),
            "{original_pane}"
        );
        yinz.wait_until("reload permission prompt", |grid| {
            grid.contains("Plugin permissions")
        });
        yinz.send_stdin(b"\t\r");
        yinz.wait_until("original plugin ready", |grid| {
            grid.contains("symlink version one ready")
        });
        fs::remove_file(&current).unwrap();
        std::os::unix::fs::symlink(&second, &current).unwrap();
        if by_id {
            yinz.send_stdin(b"r");
        } else {
            let (code, output) = yinz.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
                url: url.clone(),
                configuration: None,
                configuration_json: None,
            });
            assert_eq!(code, 0, "{output}");
        }
        yinz.wait_until("replacement plugin ready", |grid| {
            grid.contains("symlink version two ready")
        });
        let (code, output) = yinz.run_cli_action_reporting(CliAction::FocusPaneId {
            pane_id: format!("terminal_{}", terminal.terminal_id()),
        });
        assert_eq!(code, 0, "{output}");
        // This action was parsed while current.scm still pointed at version one.
        yinz.send_stdin(b"\x0fv");
        yinz.wait_until("stored binding focuses the replacement", |grid| {
            grid.contains("stored binding focused replacement")
        });
        let (code, focused_pane) = yinz.run_cli_action_reporting(launch(url.clone()));
        assert_eq!(code, 0, "{focused_pane}");
        assert_eq!(focused_pane.trim(), original_pane.trim());
        let second_url = format!("file:{}", second.display());
        let (code, focused_pane) = yinz.run_cli_action_reporting(launch(second_url.clone()));
        assert_eq!(code, 0, "{focused_pane}");
        assert_eq!(focused_pane.trim(), original_pane.trim());
        // The host's URL reload lookup must agree with the screen's focus lookup.
        let (code, output) = yinz.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
            url: second_url,
            configuration: None,
            configuration_json: None,
        });
        assert_eq!(code, 0, "{output}");
        let (code, output) = yinz.run_cli_action_reporting(CliAction::ListPanes {
            tab: false,
            command: false,
            state: true,
            geometry: false,
            all: true,
            json: true,
        });
        assert_eq!(code, 0, "{output}");
        let panes: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
        let plugins: Vec<_> = panes
            .iter()
            .filter(|pane| {
                pane["plugin_url"]
                    .as_str()
                    .is_some_and(|url| url.starts_with("file:"))
            })
            .collect();
        assert_eq!(plugins.len(), 1, "{output}");
        assert_eq!(plugins[0]["is_focused"], true, "{output}");
        let (code, old_target_pane) =
            yinz.run_cli_action_reporting(launch(format!("file:{}", first.display())));
        assert_eq!(code, 0, "{old_target_pane}");
        assert_ne!(old_target_pane.trim(), original_pane.trim());
        yinz.quit();
    }
}

#[test]
fn launch_or_focus_retains_plugin_directory_fallback_after_reload() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("probe.scm");
    fs::write(
        &plugin,
        r#"(yinz-declare-pane) (define (render rows columns) "fallback version one")"#,
    )
    .unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(&format!(
            "load_plugins clear-defaults=true {{}}\nuser_plugin_dir \"{}\"",
            directory.path().display()
        ))
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    assert!(!std::env::current_dir().unwrap().join("probe.scm").exists());
    let terminal = yinz.expect_pty_spawn();
    terminal.output(b"TERMINAL READY\r\n");
    yinz.wait_until("terminal ready", |grid| grid.contains("TERMINAL READY"));
    let launch = || CliAction::LaunchOrFocusPlugin {
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        move_to_focused_tab: false,
        url: "file:probe.scm".into(),
        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        tab_id: None,
    };
    let (code, original_pane) = yinz.run_cli_action_reporting(launch());
    assert_eq!(code, 0, "{original_pane}");
    assert!(
        original_pane.trim().starts_with("plugin_"),
        "{original_pane}"
    );
    yinz.wait_until("fallback plugin loaded", |grid| {
        grid.contains("fallback version one")
    });
    fs::write(
        &plugin,
        r#"(yinz-declare-pane) (define (render rows columns) "fallback version two")"#,
    )
    .unwrap();
    let (code, output) = yinz.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: "file:probe.scm".into(),
        configuration: None,
        configuration_json: None,
    });
    assert_eq!(code, 0, "{output}");
    yinz.wait_until("fallback plugin reloaded", |grid| {
        grid.contains("fallback version two")
    });
    let (code, focused_pane) = yinz.run_cli_action_reporting(launch());
    assert_eq!(code, 0, "{focused_pane}");
    assert_eq!(focused_pane.trim(), original_pane.trim());
    let (code, output) = yinz.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: false,
        all: true,
        json: true,
    });
    assert_eq!(code, 0, "{output}");
    let panes: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
    assert_eq!(
        panes
            .iter()
            .filter(|pane| {
                pane["plugin_url"]
                    .as_str()
                    .is_some_and(|url| url.starts_with("file:"))
            })
            .count(),
        1,
        "{output}"
    );
    yinz.quit();
}

#[test]
fn background_palette_permission_lifecycle() {
    use yinz_types::cli::{PluginOperation as Op, PluginState};
    const TOGGLE: &[u8] = b"\x1b[112;7u";
    for shared in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let plugin = directory.path().join("background.scm");
        fs::write(&plugin, background_source("first")).unwrap();
        let configuration =
            std::collections::BTreeMap::from([("shared".into(), shared.to_string())]);
        let mut yinz = TestRunner::new(TERMINAL_SIZE)
            .with_config_dir(directory.path().to_owned())
            .with_config("load_plugins clear-defaults=true {}")
            .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
            .start();
        let terminal = yinz.expect_pty_spawn();
        terminal.output(b"TERMINAL AVAILABLE\r\n");
        yinz.wait_until("initial terminal", |grid| {
            grid.contains("TERMINAL AVAILABLE")
        });
        let other = yinz.attach_client(TERMINAL_SIZE);
        other.wait_until("peer terminal", |grid| grid.contains("TERMINAL AVAILABLE"));
        let started = lifecycle(&yinz, Op::Start, &plugin, Some(configuration.clone()));
        let id = started.plugins[0].id;
        assert_eq!(lifecycle(&yinz, Op::Start, &plugin, None).plugins[0].id, id);
        yinz.wait_until("on-demand permission modal", |grid| {
            grid.contains("Plugin permissions")
        });
        assert!(!other.snapshot().contains("Plugin permissions"));
        assert_eq!(
            wait_for_plugin_state(&yinz, &plugin, PluginState::AwaitingPermission).id,
            id
        );
        // Paste cannot activate the selected control. Enter defaults to Deny.
        yinz.send_stdin(b"\x1b[200~\t\r\x1b[201~");
        yinz.send_stdin(b"\r");
        yinz.wait_until("denial restores terminal", |grid| {
            grid.contains("TERMINAL AVAILABLE") && !grid.contains("Plugin permissions")
        });
        wait_for_plugin_state(&yinz, &plugin, PluginState::Running);
        yinz.send_stdin(TOGGLE);
        yinz.send_stdin(b"zzbgpermissionaction");
        yinz.wait_until("denied provider is absent", |grid| {
            grid.contains("No matches")
        });
        yinz.send_stdin(TOGGLE);
        assert_eq!(
            lifecycle(&yinz, Op::Reload, &plugin, None).plugins[0].id,
            id
        );
        yinz.wait_until("reload allows retry", |grid| {
            grid.contains("Plugin permissions")
        });
        yinz.send_stdin(b"\t\r");
        yinz.wait_until("grant restores terminal", |grid| {
            grid.contains("TERMINAL AVAILABLE") && !grid.contains("Plugin permissions")
        });
        let status = wait_for_plugin_state(&yinz, &plugin, PluginState::Running);
        assert_eq!(status.shared, shared);
        assert_eq!(status.clients.len(), if shared { 1 } else { 2 });

        for version in ["first", "second"] {
            if version == "second" {
                fs::write(&plugin, background_source(version)).unwrap();
                assert_eq!(
                    lifecycle(&yinz, Op::Reload, &plugin, None).plugins[0].id,
                    id
                );
            }
            yinz.send_stdin(TOGGLE);
            yinz.send_stdin(b"zzbgpermissionaction");
            let label = format!("zzbgpermissionaction {version}");
            yinz.wait_until("exactly one current provider", |grid| {
                grid.to_string().matches(&label).count() == 1
                    && (version == "first" || !grid.contains("zzbgpermissionaction first"))
            });
            yinz.send_stdin(TOGGLE);
            let mut pipe =
                yinz_cli::projects::pipe_action("background-version", serde_json::Value::Null);
            if let yinz_types::input::actions::Action::CliPipe {
                plugin: target,
                configuration: config,
                ..
            } = &mut pipe
            {
                *target = Some(format!("file:{}", plugin.display()));
                *config = Some(configuration.clone());
            }
            let (code, output) = yinz.run_actions_reporting(vec![pipe]);
            assert_eq!(code, 0, "{output}");
            assert!(output.contains(version), "{output}");
        }
        // A broken replacement stays inspectable and can recover with the same ID.
        fs::write(&plugin, "(define invalid").unwrap();
        lifecycle(&yinz, Op::Reload, &plugin, None);
        assert!(wait_for_plugin_state(&yinz, &plugin, PluginState::Failed)
            .latest_error
            .is_some());
        fs::write(&plugin, background_source("recovered")).unwrap();
        assert_eq!(
            lifecycle(&yinz, Op::Reload, &plugin, None).plugins[0].id,
            id
        );
        wait_for_plugin_state(&yinz, &plugin, PluginState::Running);
        let (code, output) = yinz.run_cli_action_reporting(CliAction::ListPanes {
            tab: false,
            command: false,
            state: true,
            geometry: true,
            all: true,
            json: true,
        });
        assert_eq!(code, 0, "{output}");
        let panes: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
        assert_eq!(
            panes
                .iter()
                .filter(|pane| pane["is_plugin"] == false)
                .count(),
            1,
            "{output}"
        );
        // The plugin declares no pane, so it runs headless and has no pane record at all.
        assert!(
            !panes.iter().any(|pane| pane["is_plugin"] == true
                && (pane["id"] == id.get()
                    || pane["plugin_url"]
                        .as_str()
                        .is_some_and(|url| url.ends_with("background.scm")))),
            "{output}"
        );
        // Source removal must not make the loaded identity unaddressable.
        fs::remove_file(&plugin).unwrap();
        assert_eq!(
            lifecycle(&yinz, Op::Stop, &plugin, None).plugins[0].state,
            PluginState::Stopped
        );
        assert!(lifecycle(&yinz, Op::Inspect, &plugin, None)
            .plugins
            .is_empty());
        yinz.send_stdin(TOGGLE);
        yinz.send_stdin(b"zzbgpermissionaction");
        yinz.wait_until("stop unregisters provider", |grid| {
            grid.contains("No matches")
        });
        yinz.send_stdin(TOGGLE);
        yinz.send_stdin(b"terminal input");
        terminal.wait_for_stdin("focus remains on terminal", |bytes| {
            bytes.ends_with(b"terminal input")
        });
        other.quit();
        yinz.quit();
    }
}

#[test]
fn background_live_configuration_reconciles_and_preserves_explicit_launches() {
    use yinz_types::cli::PluginOperation as Op;
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("configured.scm");
    fs::write(&plugin, "(void)").unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let _terminal = yinz.expect_pty_spawn();
    let configure = |kdl: String| {
        let (code, output) = yinz.run_cli_action_reporting(CliAction::Reconfigure {
            kdl: Some(kdl),
            file: None,
            write: false,
        });
        assert_eq!(code, 0, "{output}");
    };
    let fragment = format!(
        "load_plugins {{ \"file:{}\" {{ shared true; }}; }}",
        plugin.display()
    );
    configure(fragment.clone());
    let id = lifecycle(&yinz, Op::Inspect, &plugin, None).plugins[0].id;
    configure(fragment.clone());
    assert_eq!(
        lifecycle(&yinz, Op::Inspect, &plugin, None).plugins[0].id,
        id
    );
    lifecycle(&yinz, Op::Stop, &plugin, None);
    configure(fragment.clone());
    assert!(lifecycle(&yinz, Op::Inspect, &plugin, None)
        .plugins
        .is_empty());
    configure("load_plugins clear-defaults=true {}".into());
    configure(fragment.clone());
    assert_ne!(
        lifecycle(&yinz, Op::Inspect, &plugin, None).plugins[0].id,
        id
    );
    configure("load_plugins clear-defaults=true {}".into());
    assert!(lifecycle(&yinz, Op::Inspect, &plugin, None)
        .plugins
        .is_empty());
    configure(fragment);
    let id = lifecycle(&yinz, Op::Start, &plugin, None).plugins[0].id;
    configure("load_plugins clear-defaults=true {}".into());
    assert_eq!(
        lifecycle(&yinz, Op::Inspect, &plugin, None).plugins[0].id,
        id
    );
    assert!(
        lifecycle(&yinz, Op::List, &plugin, None)
            .plugins
            .iter()
            .any(|plugin| plugin.location == "yinz:agent-opencode"),
        "configuration removal preserves the host-required provider"
    );
    std::fs::write(directory.path().join("config.d"), "not a directory").unwrap();
    let (code, error) = yinz.run_cli_action_reporting(CliAction::Reconfigure {
        kdl: Some("mouse_mode false".into()),
        file: None,
        write: true,
    });
    assert_ne!(code, 0, "persistence failure must reach the CLI: {error}");
    assert!(!error.trim().is_empty());
    yinz.quit();
}

#[test]
fn optional_providers_do_not_block_startup_and_can_register_later() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.scm");
    let failed = directory.path().join("failed.scm");
    let unregistered = directory.path().join("unregistered.scm");
    let late = directory.path().join("late.scm");
    fs::write(&failed, r#"(error "optional provider failed")"#).unwrap();
    fs::write(&unregistered, "(void)").unwrap();
    fs::write(&late, r#"
(yinz-declare-pane)
(define state "waiting for permission")
(define (has-provider? providers)
  (not (null? (filter (lambda (provider) (equal? (hash-ref provider "kind") "late-fixture")) providers))))
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("RunCommands")) (lambda (_)
    (yinz-command (hash "Subscribe" '("Key" "AgentProvidersChanged")))
    (when (has-provider? (yinz-agent-providers)) (error "unexpected provider at startup"))
    (set! state "session ready before optional provider") (yinz-request-render))))
(define (update event)
  (cond [(and (hash? event) (hash-contains? event "Key"))
         (when (equal? (%async-get (hash-ref (hash-ref event "Key") "bare_key") "Char" #f) "r")
           (yinz-agent-register-provider (hash "kind" "late-fixture" "executable" "late-fixture"))) #f]
        [(and (hash? event) (hash-contains? event "AgentProvidersChanged")
              (has-provider? (hash-ref event "AgentProvidersChanged")))
         (unless (has-provider? (yinz-agent-providers)) (error "missing snapshot"))
         (set! state "late provider available") #t]
        [else #f]))
(define (render rows columns) state)
"#).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(&format!(
            "load_plugins clear-defaults=true {{ \"file:{}\"; \"file:{}\"; \"file:{}\"; }}",
            missing.display(),
            failed.display(),
            unregistered.display(),
        ))
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{ pane {{ plugin location=\"file:{}\" {{ shared true; }}; }}; }}",
            late.display()
        )))
        .start();
    yinz.wait_until("late provider permission prompt", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("session starts without provider registration", |grid| {
        grid.contains("session ready before optional provider")
    });
    yinz.send_stdin(b"r");
    yinz.wait_until("provider registration refreshes the consumer", |grid| {
        grid.contains("late provider available")
    });
    yinz.quit();
}

#[test]
fn steel_background_workers_await_real_commands_and_preserve_state() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("workers.scm");
    fs::write(&plugin, r#"
(yinz-declare-pane)
(define count 0) (define state "starting") (define results '())
(yinz-register-workers (hash "command" (lambda (name payload)
  (yinz-then (yinz-run-command
      (hash "command" '("/bin/sh" "-c" "printf \"%s\" \"$VALUE\"") "env" (hash "VALUE" payload)))
    (lambda (result)
      (unless (= (hash-ref result "exit-code") 0) (error "command failed"))
      (set! count (+ count 1))
      (yinz-post-plugin name (string-append (number->string count) ":" (yinz-decode-utf8 (hash-ref result "stdout")))))))))
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("RunCommands")) (lambda (_)
    (yinz-on-event "CustomMessage" (lambda (event)
      (if (equal? (car event) "result")
          (begin
            (set! results (append results (list (cadr event))))
            (if (= (length results) 2)
                (begin
                  (unless (and (= count 0) (equal? results '("1:hé🙂" "2:second"))) (error "worker state"))
                  (set! state "background commands verified") #t) #f)) #f)))
    (yinz-command (hash "Subscribe" '("CustomMessage")))
    (yinz-post-worker "command" "result" "hé🙂")
    (yinz-post-worker "command" "result" "second"))))
(define (render rows columns) state)
"#).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n tab {{\n pane {{ plugin location=\"file:{}\"; }}\n }}\n}}",
            plugin.display()
        )))
        .start();
    yinz.wait_until("worker command permission prompt", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("workers complete their real commands", |grid| {
        grid.contains("background commands verified")
    });
    yinz.quit();
}

#[test]
fn steel_worker_pane_sequences_and_reruns_complete_before_the_next_job() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("worker-panes.scm");
    fs::write(&plugin, r#"
(yinz-declare-pane)
(define state "starting") (define replies 0)
(define events '("CommandPaneOpened" "CommandPaneReRun" "CommandPaneExited"))
(define (observe)
  (for-each (lambda (event) (yinz-on-event event (lambda (_) (set! replies (+ replies 1)) #f))) events))
(yinz-register-workers (hash
  "runner" (lambda (name payload)
    (if (equal? name "next") (yinz-post-worker "sibling" "check" "")
        (let ([sequence (yinz-pane-sequence (hash "steps"
                (list (hash "path" "first" "stdin" "first-ready") (hash "path" "second" "stdin" "second-ready"))))])
          (yinz-then (hash-ref sequence "done") (lambda (result)
            (unless (and (equal? (hash-ref result "status") "success") (equal? (hash-ref result "exitCodes") '(0 0)))
              (error "sequence result"))
            (yinz-then (yinz-run-in-pane (hash-ref result "paneId") (hash "path" "third" "stdin" "third-ready"))
              (lambda (rerun) (unless (= (hash-ref rerun "exitCode") 0) (error "rerun result"))
                (yinz-post-plugin "done" ""))))))))
  "sibling" (lambda (name payload)
    (if (equal? name "check") (yinz-post-plugin "sibling" (number->string replies))
        (begin (observe) (yinz-command (hash "Subscribe" events)) (yinz-post-plugin "ready" ""))))))
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("RunCommands" "ReadApplicationState" "WriteToStdin")) (lambda (_)
    (observe) (yinz-command (hash "Subscribe" (append events '("CustomMessage"))))
    (yinz-on-event "CustomMessage" (lambda (event)
      (define name (car event)) (define payload (cadr event))
      (cond [(equal? name "ready") (yinz-post-worker "runner" "run" "") (yinz-post-worker "runner" "next" "")]
            [(equal? name "done") (set! state "worker done")]
            [(equal? name "sibling")
             (unless (and (equal? state "worker done") (= replies 0) (equal? payload "0")) (error "misrouted pane completion"))
             (set! state "worker panes verified")]) #t))
    (yinz-post-worker "sibling" "observe" ""))))
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
    yinz.wait_until("worker pane permissions", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    let pane = yinz.expect_pty_spawn();
    for ready in ["first-ready", "second-ready", "third-ready"] {
        pane.wait_for_stdin(ready, |bytes| bytes.ends_with(ready.as_bytes()));
        pane.exit(Some(0));
    }
    yinz.wait_until(
        "pane completions reach only their worker and release its next job",
        |grid| grid.contains("worker panes verified"),
    );
    yinz.quit();
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
fn steel_observes_real_buffered_http_and_ordered_stream_bytes() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let mut socket = receive_http(&listener);
            socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\na\x00b\r\n2\r\n\x80\xff\r\n0\r\n\r\n").unwrap();
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("http.scm");
    fs::write(&plugin, format!(r#"
(yinz-declare-pane)
(define state "starting")
(define (run-http)
  (yinz-then (yinz-wait-for-permissions '("WebAccess")) (lambda (_)
    (define url "http://{address}/bytes")
    (yinz-then (yinz-web-request (hash "url" url)) (lambda (buffered)
      (unless (and (= (hash-ref buffered "status") 200) (bytevector? (hash-ref buffered "body")) (equal? (bytes->list (hash-ref buffered "body")) '(97 0 98 128 255)))
        (error "buffered bytes"))
      (define chunks '()) (define leading-seen #f) (define terminators 0)
      (yinz-on-event "WebRequestResult" (lambda (_) (set! leading-seen #t) #f))
      (yinz-on-event "WebRequestStreamChunk" (lambda (event)
        (when (equal? (hash-ref event "stream_id") "bytes")
          (unless leading-seen (error "stream before response"))
          (set! chunks (append chunks (bytes->list (hash-ref event "chunk"))))
          (when (hash-ref event "done") (set! terminators (+ terminators 1)))) #f))
      (define done (yinz-wait-for-event "WebRequestStreamChunk"
        (lambda (event) (and (equal? (hash-ref event "stream_id") "bytes") (hash-ref event "done")))))
      (yinz-then (yinz-web-request (hash "url" url "stream-id" "bytes")) (lambda (leading)
        (yinz-then done (lambda (event)
          (unless (and (= (hash-ref leading "status") 200) (bytevector? (hash-ref leading "body")) (= (bytes-length (hash-ref leading "body")) 0) (json-null? (hash-ref event "error")))
            (error "stream response"))
          (unless (and (equal? chunks '(97 0 98 128 255)) (= terminators 1)) (error "stream bytes"))
          (set! state "buffered and streamed HTTP verified"))))))))))
(define (load configuration)
  (yinz-catch (run-http) (lambda (message)
    (set! state (string-append "HTTP failed: " message)) (yinz-request-render))))
(define (render rows columns) state)
"#)).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n  tab {{\n    pane {{ plugin location=\"file:{}\"; }}\n  }}\n}}",
            plugin.display()
        )))
        .start();
    yinz.wait_until("HTTP permission prompt", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    let grid = yinz.wait_until(
        "the guest observes real buffered and streamed bytes",
        |grid| {
            grid.contains("buffered and streamed HTTP verified") || grid.contains("HTTP failed:")
        },
    );
    assert!(
        grid.contains("buffered and streamed HTTP verified"),
        "{grid}"
    );
    server.join().unwrap();
    yinz.quit();
}

const SCHEME: &str = r#"
(require "./dependency.scm")
(yinz-declare-pane)
(define state "starting")
(define (load configuration)
  (yinz-command (hash "Subscribe" '("PermissionRequestResult" "Key")))
  (yinz-then (yinz-wait-for-permissions '("ChangeApplicationState" "RunCommands")) (lambda (_)
    (define ids (yinz-command "GetPluginIds"))
    (unless (and (hash? ids) (hash-contains? ids "plugin_id")) (error "missing decoded plugin identity"))
    (define key (yinz-wait-for-event "Key"))
    (set! state (string-append label " granted; awaiting key")) (yinz-request-render)
    (yinz-then key (lambda (_)
      (yinz-then (yinz-run-command
          (hash "command" '("/bin/sh" "-c" "printf \"%s\" \"$MESSAGE\"; printf \"err\" >&2; exit 7")
                "env" (hash "MESSAGE" (string-append label " command hé🙂")) "context" (hash "label" label)))
        (lambda (result)
          (unless (and (= (hash-ref result "exit-code") 7)
                       (equal? (yinz-decode-utf8 (hash-ref result "stdout")) (string-append label " command hé🙂"))
                       (equal? (yinz-decode-utf8 (hash-ref result "stderr")) "err")
                       (equal? (hash-ref (hash-ref result "context") "label") label)
                       (not (hash-contains? (hash-ref result "context") "__yinz_async"))) (error "incorrect command result"))
          (yinz-command (hash "RenameTabWithId" (list 0 (string-append label " host command"))))
          (set! state (string-append label " awaited key rendered")) (yinz-request-render)
          (yinz-wait-for-pipe "acceptance-finish"))))))))
(define (render rows columns) (yinz-then (yinz-sleep 0) (lambda (_) state)))
"#;

#[test]
fn steel_and_builtins_share_a_session_through_permissions_events_reload_and_close() {
    let directory = tempfile::tempdir().unwrap();
    let plugins = directory.path().join("plugins");
    fs::create_dir(&plugins).unwrap();
    fs::write(plugins.join("acceptance.scm"), SCHEME).unwrap();
    let dependency = plugins.join("dependency.scm");
    fs::write(&dependency, r#"(provide label) (define label "first")"#).unwrap();
    fs::write(
        plugins.join("companion.scm"),
        r#"
(yinz-declare-pane)
(define (render rows columns) "Scheme companion is alive")
"#,
    )
    .unwrap();

    eprintln!("lifecycle: starting session");
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(&format!("user_plugin_dir \"{}\"", plugins.display()))
        .with_layout(LayoutInfo::Stringified(
            r#"
layout {
  tab name="builtin companion" {
    pane size=1 borderless=true { plugin location="yinz:tab-bar"; }
    pane size=4 { plugin location="file:companion.scm"; }
    pane focus=true { plugin location="file:acceptance.scm"; }
  }
}
"#
            .to_owned(),
        ))
        .start();

    eprintln!("lifecycle: waiting for permission prompt");
    yinz.wait_until(
        "all plugins loaded and Scheme requested permission",
        |grid| {
            grid.contains("builtin companion")
                && grid.contains("Scheme companion is alive")
                && grid.contains("Plugin permissions")
        },
    );
    eprintln!("lifecycle: granting permission");
    yinz.send_stdin(b"\t\r");
    yinz.wait_until(
        "the async load resumed after the permission grant",
        |grid| grid.contains("first granted; awaiting key") && !grid.contains("Plugin permissions"),
    );
    eprintln!("lifecycle: sending first key");
    yinz.send_stdin(b"a");
    yinz.wait_until(
        "the awaited key produced a Scheme frame and renamed the Builtin tab bar",
        |grid| {
            grid.contains("first awaited key rendered")
                && grid.contains("first host command")
                && grid.contains("Scheme companion is alive")
        },
    );

    // Only the dependency changes. The entrypoint and its pane stay the same.
    fs::write(&dependency, r#"(provide label) (define label "second")"#).unwrap();
    eprintln!("lifecycle: reloading plugin");
    let (code, output) = yinz.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: format!("file:{}", plugins.join("acceptance.scm").display()),
        configuration: None,
        configuration_json: None,
    });
    assert_eq!(code, 0, "{output}");
    yinz.wait_until(
        "reload imports the changed dependency and retains the permission grant",
        |grid| {
            grid.contains("second granted; awaiting key") && !grid.contains("Plugin permissions")
        },
    );
    eprintln!("lifecycle: sending replacement key");
    yinz.send_stdin(b"b");
    let grid = yinz.wait_until(
        "the replacement instance handles input alongside Scheme and Builtin",
        |grid| {
            grid.contains("second awaited key rendered")
                && grid.contains("second host command")
                && grid.contains("Scheme companion is alive")
        },
    );
    assert!(!grid.contains("first awaited key rendered"), "{grid}");
    // quit joins the real client and server with the harness's bounded shutdown checks.
    eprintln!("lifecycle: shutting down");
    yinz.quit();
    eprintln!("lifecycle: complete");
}

#[test]
fn steel_host_queries_configuration_layouts_and_cross_session_warp() {
    // As in the existing warp suite, start the receiver first: both servers share
    // this test process's session-name environment, which must name the sender.
    let receiver = TestRunner::new(TERMINAL_SIZE)
        .skip_concurrency_slot()
        .with_config("load_plugins clear-defaults=true {}")
        .start();
    let receiver_shell = receiver.expect_pty_spawn();
    receiver_shell.output(yinz_integration_tests::PROMPT);
    receiver.wait_for_app_load();
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("surfaces.scm");
    fs::write(&plugin, format!(r#"
(yinz-declare-pane)
(define state "starting")
(define (assert value message) (unless value (error message)))
(define (load configuration)
  (yinz-catch (yinz-then (yinz-wait-for-permissions '("ReadApplicationState" "ChangeApplicationState" "ReadPaneContents" "Reconfigure"))
    (lambda (_)
      (define ids (yinz-command "GetPluginIds"))
      (assert (hash-ref (yinz-command (hash "GetPaneInfo" (hash "Plugin" (hash-ref ids "plugin_id")))) "is_plugin") "plugin identity")
      (assert (json-null? (yinz-command (hash "GetPaneInfo" (hash "Terminal" 4294967295)))) "missing pane")
      (define pane (yinz-command (hash "GetPaneInfo" (hash "Terminal" 0))))
      (assert (and (hash? pane) (not (hash-ref pane "is_plugin")) (equal? (hash-ref pane "target") "Local")) "terminal pane")
      (assert (equal? (hash-ref (yinz-command (hash "GetTabInfo" 0)) "name") "surface test") "tab reply")
      (define parsed (yinz-command (hash "ParseLayout" "layout {{\n tab name=\"parsed\" {{\n pane;\n }}\n}}")))
      (define tab (car (hash-ref parsed "tabs")))
      (assert (and (equal? (hash-ref tab "name") "parsed") (= (length (hash-ref tab "panes")) 1)) "layout metadata")
      (assert (with-handler (lambda (_) #t) (yinz-command (hash "ParseLayout" "layout {{ !!!")) #f) "invalid layout")
      (define session (yinz-command "GetSessionList"))
      (assert (not (null? (filter (lambda (session) (hash-ref session "is_current_session")) (hash-ref session "live_sessions")))) "session snapshot")
      (assert (list? (hash-ref session "resurrectable_sessions")) "absent sessions")
      (define initial-keys (yinz-wait-for-event "InitialKeybinds"))
      (assert (string-contains? (hash-ref (yinz-command (hash "DumpSessionLayout" (hash "tab_index" 0))) "layout") "pane") "serialized layout")
      (yinz-then initial-keys (lambda (keys)
        (assert (list? keys) "initial keybindings")
        (define updated (yinz-wait-for-event "ModeUpdate" (lambda (info) (hash-ref (hash-ref info "style") "rounded_corners"))))
        (yinz-command (hash "Reconfigure" (list "ui {{\n pane_frames {{\n rounded_corners true;\n }}\n}}" #f)))
        (yinz-then updated (lambda (_)
          (yinz-command (hash "RenameTerminalPane" '(0 "Scheme warp payload")))
          (define moved (yinz-command (hash "WarpPaneToSession" (hash "pane_id" (hash "Terminal" 0) "to_session" {receiver:?} "placement" "FocusedTab"))))
          (assert (string? moved) "warp reply")
          (assert (json-null? (yinz-command (hash "GetPaneInfo" (hash "Terminal" 0)))) "pane left sender")
          (set! state "host surfaces verified")))))))
    (lambda (failure) (set! state (string-append "host surfaces failed: " failure)))))
(define (render rows columns) state)
"#, receiver=receiver.session_name())).unwrap();
    let mut sender = TestRunner::new(TERMINAL_SIZE).skip_concurrency_slot()
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!("layout {{\n tab name=\"surface test\" {{\n pane;\n pane focus=true {{plugin location=\"file:{}\";}}\n }}\n}}", plugin.display())))
        .start();
    let terminal = sender.expect_pty_spawn();
    terminal.output(b"Scheme cross-session output\r\n");
    sender.wait_until("Scheme requests host surface permissions", |screen| {
        screen.contains("Plugin permissions")
    });
    sender.send_stdin(b"\t\r");
    let result = sender.wait_until("Scheme completes the host surface round trips", |screen| {
        screen.contains("host surfaces")
    });
    assert!(
        result.contains("host surfaces verified"),
        "{result}\n{}",
        yinz_integration_tests::test_env::log_tail(150)
    );
    receiver.wait_until(
        "Scheme moved the real terminal to the other session",
        |screen| screen.contains("Scheme cross-session output"),
    );
    sender.quit();
}

#[test]
fn steel_layout_query_reports_a_held_pty_and_recovers_during_plugin_loading() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("layout-query.scm");
    fs::write(&plugin, r#"
(yinz-declare-pane)
(define state "starting") (define query 0) (define compatible 0)
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState")) (lambda (_)
    (yinz-command (hash "Subscribe" '("Key" "CustomMessage"))) (set! state "layout query ready"))))
(define (update event)
  (cond [(and (hash? event) (hash-contains? event "CustomMessage")
              (equal? (car (hash-ref event "CustomMessage")) "session_layout"))
         (set! compatible (+ compatible 1)) #t]
        [(and (hash? event) (hash-contains? event "Key"))
         (set! query (+ query 1))
         (yinz-command (hash "Subscribe" '("InitialKeybinds" "HostTerminalThemeChanged")))
         (define outcome (with-handler (lambda (_) "timeout")
           (define result (yinz-command (hash "DumpSessionLayout" (hash))))
           (define plugins (apply append (map (lambda (tab) (filter (lambda (pane) (hash-ref pane "is_plugin")) (hash-ref tab "panes")))
                            (hash-ref (hash-ref result "metadata") "tabs"))))
           (if (and (null? plugins) (string-contains? (hash-ref result "layout") "pane")) "success" "invalid layout")))
         (set! state (string-append "query " (number->string query) " " outcome)) #t]
        [else #f]))
(define (render rows columns) (string-append state " compatible=" (number->string compatible)))
"#).unwrap();
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{ pane; pane focus=true {{ plugin location=\"file:{}\"; }}; }}",
            plugin.display()
        )))
        .start();
    session.wait_until("query permissions", |grid| {
        grid.contains("Plugin permissions")
    });
    session.send_stdin(b"\t\r");
    session.wait_until("query guest ready", |grid| {
        grid.contains("layout query ready")
    });
    for query in 1..=2 {
        let (entered, release) = session.hold_next_metadata_collection();
        session.send_stdin(b"q");
        entered
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        if query == 1 {
            session.wait_until("the unchanged query deadline expires", |grid| {
                grid.contains("query 1 timeout")
            });
            let log = yinz_integration_tests::test_env::log_tail(150);
            assert!(
                log.contains("layout query timeout")
                    && (log.contains("pty queued") || log.contains("pty metadata")),
                "{log}"
            );
            release.send(()).unwrap();
        } else {
            let companion = directory.path().join("subscription-companion.scm");
            fs::write(
                &companion,
                r#"(define (load configuration)
  (yinz-command (hash "Subscribe" '("InitialKeybinds" "HostTerminalThemeChanged"))))"#,
            )
            .unwrap();
            std::thread::scope(|scope| {
                let (finished, completion) = std::sync::mpsc::channel();
                let session_ref = &session;
                let companion_ref = &companion;
                scope.spawn(move || {
                    finished
                        .send(lifecycle(
                            session_ref,
                            yinz_types::cli::PluginOperation::Start,
                            companion_ref,
                            None,
                        ))
                        .unwrap();
                });
                let early = completion.recv_timeout(std::time::Duration::from_millis(100));
                release.send(()).unwrap();
                let result =
                    session.wait_until("query after contention", |grid| grid.contains("query 2"));
                assert!(
                    result.contains("query 2 success"),
                    "{result}\n{}",
                    yinz_integration_tests::test_env::log_tail(150)
                );
                if early.is_err() {
                    completion
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                }
            });
            wait_for_plugin_state(&session, &companion, yinz_types::cli::PluginState::Running);
        }
        if query == 1 {
            session.wait_until(
                "compatibility response survives the expired synchronous receiver",
                |grid| grid.contains("compatible=1"),
            );
        } else {
            let result =
                session.wait_until("query after contention", |grid| grid.contains("query 2"));
            assert!(
                result.contains("query 2 success"),
                "{result}\n{}",
                yinz_integration_tests::test_env::log_tail(150)
            );
            session.wait_until("compatibility response after success", |grid| {
                grid.contains("compatible=2")
            });
        }
    }
    session.quit();
}

#[test]
fn steel_reload_before_close_can_query_layout_with_another_plugin() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("cleanup-query.scm");
    let companion = directory.path().join("companion.scm");
    fs::write(
        &companion,
        r#"(yinz-declare-pane) (define (render rows columns) "companion ready")"#,
    )
    .unwrap();
    fs::write(&plugin, r#"
(yinz-declare-pane)
(define state "starting")
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState" "ReadFiles" "WriteFiles")) (lambda (_)
    (yinz-command (hash "Subscribe" '("BeforeClose"))) (set! state "cleanup query ready"))))
(define (update event)
  (when (equal? event "BeforeClose")
    (define result (with-handler (lambda (failure) (%async-error failure))
      (define layout (yinz-command (hash "DumpSessionLayout" (hash))))
      (define plugins (apply append (map (lambda (tab) (filter (lambda (pane) (hash-ref pane "is_plugin")) (hash-ref tab "panes")))
                       (hash-ref (hash-ref layout "metadata") "tabs"))))
      (if (null? plugins) "missing companion" "success")))
    (yinz-command (hash "FsWrite" (hash "path" "/data/cleanup.txt" "contents" (bytes->list (string->bytes result))
                                       "append" #f "create_dirs" #f)))) #f)
(define (render rows columns) state)
"#).unwrap();
    let mut session = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{ pane; pane {{ plugin location=\"file:{}\"; }}; pane focus=true {{ plugin location=\"file:{}\"; }}; }}",
            companion.display(), plugin.display()
        )))
        .start();
    session.wait_until("cleanup query permissions", |grid| {
        grid.contains("Plugin permissions")
    });
    session.send_stdin(b"\t\r");
    session.wait_until("cleanup query guest ready", |grid| {
        grid.contains("cleanup query ready")
    });
    fs::write(
        &plugin,
        r#"
(yinz-declare-pane)
(define state "starting")
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadFiles")) (lambda (_)
    (set! state (string-append "cleanup result: " (yinz-decode-utf8
      (yinz-command (hash "FsRead" (hash "path" "/data/cleanup.txt" "max_bytes" json-null "offset" json-null)))))))))
(define (render rows columns) state)
"#,
    )
    .unwrap();
    lifecycle(
        &session,
        yinz_types::cli::PluginOperation::Reload,
        &plugin,
        None,
    );
    let result = session.wait_until("replacement reads cleanup result", |grid| {
        grid.contains("cleanup result:")
    });
    assert!(
        result.contains("cleanup result: success"),
        "{result}\n{}",
        yinz_integration_tests::test_env::log_tail(150)
    );
    session.quit();
}

#[test]
fn steel_reload_reuses_stream_id_without_delivering_or_cancelling_the_old_generation() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut old = receive_http(&listener);
        old.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n9\r\nold-start\r\n")
            .unwrap();
        let mut replacement = receive_http(&listener);
        // Reload may close this socket. Either outcome is fine; no old bytes may
        // be observed by the new guest, and old cleanup must not cancel its request.
        let _ = old.write_all(b"8\r\nold-late\r\n0\r\n\r\n");
        replacement.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\na\r\nnew-result\r\n0\r\n\r\n").unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory.path().join("replace-stream.scm");
    let source = format!(
        r#"
(yinz-declare-pane)
(define label "old") (define state "starting") (define contents "")
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("WebAccess")) (lambda (_)
    (yinz-command (hash "Subscribe" '("WebRequestStreamChunk")))
    (yinz-on-event "WebRequestStreamChunk" (lambda (event)
      (set! contents (string-append contents (yinz-decode-utf8 (hash-ref event "chunk"))))
      (set! state (if (hash-ref event "done") (string-append label " stream complete " contents) (string-append label " stream pending")))
      (unless (json-null? (hash-ref event "error")) (set! state (string-append "stream failed " (hash-ref event "error")))) #t))
    (yinz-command (hash "WebRequest" (list "http://{address}/stream" "Get" (hash) '() (hash) "same-id" json-null))))))
(define (render rows columns) state)
"#
    );
    fs::write(&plugin, &source).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{\n tab {{\n pane {{plugin location=\"file:{}\";}}\n }}\n}}",
            plugin.display()
        )))
        .start();
    yinz.wait_until("stream permission prompt", |screen| {
        screen.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("old stream remains in flight", |screen| {
        screen.contains("old stream pending")
    });
    fs::write(
        &plugin,
        source.replace(r#"(define label "old")"#, r#"(define label "replacement")"#),
    )
    .unwrap();
    let (code, output) = yinz.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: format!("file:{}", plugin.display()),
        configuration: None,
        configuration_json: None,
    });
    assert_eq!(code, 0, "{output}");
    let result = yinz.wait_until("replacement stream reaches completion", |screen| {
        screen.contains("stream complete") || screen.contains("stream failed")
    });
    assert!(
        result.contains("replacement stream complete new-result"),
        "{result}"
    );
    server.join().unwrap();
    yinz.quit();
}

#[test]
fn steel_forms_and_markdown_render_beside_builtins() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("forms.scm"),
        include_str!("../../example-plugins/forms.scm"),
    )
    .unwrap();
    fs::write(
        directory.path().join("companion.scm"),
        r#"(yinz-declare-pane) (define (render rows columns) "Scheme companion")"#,
    )
    .unwrap();
    for size in [
        TERMINAL_SIZE,
        yinz_integration_tests::Size {
            cols: 320,
            rows: 120,
        },
    ] {
        let mut yinz = TestRunner::new(size)
            .with_config(&format!(
                "user_plugin_dir \"{}\"",
                directory.path().display()
            ))
            .with_layout(LayoutInfo::Stringified(
                r#"layout {
              tab name="Builtin companion" {
                pane size=1 borderless=true {plugin location="yinz:tab-bar";}
                pane size=3 {plugin location="file:companion.scm";}
                pane focus=true {plugin location="file:forms.scm";}
              }
            }"#
                .into(),
            ))
            .start();
        yinz.wait_until(
            "shared controls and Markdown render in the shared session",
            |screen| {
                screen.contains("Builtin companion")
                    && screen.contains("Scheme companion")
                    && screen.contains("Pane frames")
                    && screen.contains("Shared widgets")
            },
        );
        yinz.send_stdin(b"x");
        yinz.wait_until("Scheme form edits Unicode text", |screen| {
            screen.contains("日本語x")
        });
        yinz.quit();
    }
}

#[test]
fn configured_overlay_tabs_launch_from_plugins_and_keep_permission_failures_in_their_slot() {
    let directory = tempfile::tempdir().unwrap();
    let launcher = directory.path().join("launcher.scm");
    let content = directory.path().join("content.scm");
    let denied = directory.path().join("denied.scm");
    fs::write(
        &launcher,
        r#"
(yinz-declare-pane)
(define ready #f)
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState" "OpenTerminalsOrPlugins"))
    (lambda (_) (yinz-command (hash "Subscribe" '("Key"))) (set! ready #t) (yinz-request-render))))
(define (update event)
  (when (and (hash? event) (hash-contains? event "Key"))
    (yinz-command (hash "OpenPluginPaneFloating" (hash "plugin_url" "content" "configuration" (hash)
      "floating_pane_coordinates" 'json-null "context" (hash))))) #f)
(define (render rows columns) (if ready "OVERLAY LAUNCHER READY" "waiting"))
"#,
    )
    .unwrap();
    fs::write(
        &content,
        r#"(yinz-declare-pane) (define (render rows columns) "OVERLAY CONTENT READY")"#,
    )
    .unwrap();
    fs::write(
        &denied,
        r#"
(yinz-declare-pane)
(define state "waiting")
(define (load configuration)
  (yinz-catch (yinz-then (yinz-wait-for-permissions '("ReadApplicationState"))
      (lambda (_) (set! state "GRANTED") (yinz-request-render)))
    (lambda (_) (set! state "OVERLAY DENIED") (yinz-request-render))))
(define (render rows columns) state)
"#,
    )
    .unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(&format!(
            r#"
load_plugins clear-defaults=true {{}}
plugins {{
    launcher location="file:{}"
    content location="file:{}"
    denied location="file:{}"
}}
overlay_tab_plugins {{ launcher label="Launch"; content label="Content"; denied label="Denied"; }}
"#,
            launcher.display(),
            content.display(),
            denied.display()
        ))
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(b"TERMINAL READY\r\n");
    yinz.wait_until("terminal ready", |grid| grid.contains("TERMINAL READY"));
    let launch = |alias: &str| CliAction::LaunchOrFocusPlugin {
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        move_to_focused_tab: false,
        url: alias.into(),
        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        tab_id: None,
    };
    let (code, original) = yinz.run_cli_action_reporting(launch("content"));
    assert_eq!(code, 0, "{original}");
    yinz.wait_until("cold launch selects the second custom tab", |grid| {
        grid.contains("OVERLAY CONTENT READY")
            && grid.contains("Launch")
            && grid.contains("Content")
    });
    let (code, lazy) = yinz.run_cli_action_reporting(launch("launcher"));
    assert_eq!(code, 0, "{lazy}");
    assert_ne!(
        original.trim(),
        lazy.trim(),
        "lazy launch returns its own plugin ID"
    );
    yinz.wait_until("each tab requests its own permissions", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("launcher is ready", |grid| {
        grid.contains("OVERLAY LAUNCHER READY")
    });
    let (code, focused_launcher) = yinz.run_cli_action_reporting(launch("launcher"));
    assert_eq!(code, 0, "{focused_launcher}");
    assert_eq!(lazy.trim(), focused_launcher.trim());
    yinz.send_stdin(b"x");
    yinz.wait_until("a plugin launch selects its sibling", |grid| {
        grid.contains("OVERLAY CONTENT READY")
    });
    let (code, focused) = yinz.run_cli_action_reporting(launch("content"));
    assert_eq!(code, 0, "{focused}");
    assert_eq!(original.trim(), focused.trim());
    yinz.run_cli_action(launch("denied"));
    yinz.wait_until("third tab requests permissions", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\r");
    yinz.wait_until("permission denial stays inside the group", |grid| {
        grid.contains("OVERLAY DENIED") && grid.contains("Content")
    });
    yinz.run_cli_action(launch("content"));
    yinz.wait_until("denial leaves other tabs usable", |grid| {
        grid.contains("OVERLAY CONTENT READY")
    });
    yinz.quit();
}

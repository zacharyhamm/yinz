#![cfg(unix)]
//! A Scheme plugin spawns an agent, waits for its state, sends a prompt, and observes exit.
//! Fake ptys stand in for agent processes; CLI reports stand in for their hooks.
//! Reload also verifies that granted permissions are available during load.

use std::fs;
use yinz_cli::args::CliAction;
use yinz_integration_tests::{default_timeout, LayoutInfo, TestRunner, TERMINAL_SIZE};

const HARNESS: &str = r#"
(define permissions '("RunCommands" "ReadApplicationState" "WriteToStdin"))
(define (load configuration)
  (yinz-command (hash "RequestPluginPermissions" permissions))
  (define in-force (with-handler (lambda (_) #f) (yinz-command "ListClients") #t))
  (yinz-log (string-append "in force at load: " (if in-force "true" "false")))
  (yinz-then (yinz-wait-for-permissions permissions) (lambda (_)
    (yinz-log "permissions Granted")
    (define pane (yinz-agent-spawn (hash "kind" "claude" "hooks" #f "cwd" "/host" "allow_outside_worktree" #t)))
    (unless (hash-contains? pane "Terminal") (error "Expected a terminal agent"))
    (define id (number->string (hash-ref pane "Terminal")))
    (yinz-log (string-append "spawned pane=" id))
    (yinz-then (yinz-agent-wait-for-state pane "waiting") (lambda (_)
      (yinz-log (string-append "waiting pane=" id))
      (yinz-agent-send pane "go")
      (yinz-then (yinz-wait-for-pane-exit pane) (lambda (result)
        (yinz-log (string-append "exited pane=" id " " (number->string (hash-ref result "exitCode")))))))))))
(define (render rows columns) "harness is up")
"#;

const HARNESS_LAYOUT: &str = r#"layout {
    pane {
        plugin location="file:harness.scm"
    }
}
"#;

/// The log's lines carrying the harness's own tag, in order.
fn harness_log() -> Vec<String> {
    yinz_integration_tests::test_env::log_tail(400)
        .lines()
        .filter(|line| line.contains("[steel:") && line.contains("harness.scm]"))
        .map(|line| {
            line.split("harness.scm] ")
                .nth(1)
                .unwrap_or(line)
                .trim()
                .to_owned()
        })
        .collect()
}

fn wait_for_log_line(what: &str) -> Vec<String> {
    let deadline = std::time::Instant::now() + default_timeout();
    loop {
        let lines = harness_log();
        if lines.iter().any(|line| line.starts_with(what)) {
            return lines;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the harness never logged {what:?}; its lines so far: {lines:?}\nthe log's tail:\n{}",
            yinz_integration_tests::test_env::log_tail(60)
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// The agent's source and state as one `pane_update` event reports them for `terminal_id`.
fn agent_in(
    event_json: &str,
    terminal_id: yinz_types::internal_ids::TerminalId,
) -> Option<(String, String)> {
    let event: serde_json::Value = serde_json::from_str(event_json).ok()?;
    let agent = event
        .get("PaneUpdate")?
        .get("panes")?
        .as_object()?
        .values()
        .filter_map(|panes| panes.as_array())
        .flatten()
        .find(|pane| pane["id"] == terminal_id.get() && pane["is_plugin"] == false)?
        .get("agent")?;
    Some((
        agent.get("source")?.as_str()?.to_owned(),
        agent.get("state")?.as_str()?.to_owned(),
    ))
}

#[test]
fn a_scheme_harness_spawns_an_agent_waits_on_it_prompts_it_and_sees_it_exit() {
    let config_dir = yinz_integration_tests::test_env::init()
        .join("config")
        .join("scheme-agent-harness");
    fs::create_dir_all(&config_dir).unwrap();
    let plugin_dir = config_dir.join("plugins");
    yinz_server::plugin_authoring::write(&plugin_dir, "harness", HARNESS, false, true)
        .expect("the harness is written");

    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(config_dir.clone())
        .with_config(&format!("user_plugin_dir \"{}\"", plugin_dir.display()))
        .with_layout(LayoutInfo::Stringified(HARNESS_LAYOUT.to_owned()))
        .start();

    // The first load: the permissions are asked for and nothing gated works until the user
    // answers - which the task waits for.
    yinz.wait_until("the harness asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Plugin permissions")
    });
    let lines = wait_for_log_line("in force at load:");
    assert!(
        lines.iter().any(|line| line == "in force at load: false"),
        "{lines:?}"
    );
    let events = yinz.stream_events(&["pane_update"]);
    events.wait_for_event("PaneUpdate");
    yinz.send_stdin(b"\t\r");

    // Granted: the task spawns the agent. The pane carries the record the spawn path wrote.
    let agent_pty = yinz.expect_pty_spawn();
    let terminal_id = agent_pty.terminal_id();
    let lines = wait_for_log_line("spawned pane=");
    assert!(
        lines.contains(&"permissions Granted".to_owned())
            && lines.contains(&format!("spawned pane={terminal_id}")),
        "{lines:?}"
    );
    yinz.wait_until("the spawn path's record is on the stream", |_| {
        events
            .streamed_events()
            .iter()
            .any(|(_, json)| agent_in(json, terminal_id) == Some(("Spawned".into(), "Busy".into())))
    });

    // The agent reports for itself, as its hooks would: the record moves to waiting and the
    // task, which was awaiting exactly that, prompts it.
    let (exit_code, reported) = yinz.run_cli_action_reporting(CliAction::AgentReport {
        pane_id: Some(format!("terminal_{terminal_id}")),
        kind: Some("claude".to_owned()),
        state: Some("waiting".to_owned()),
        detail: Some("y/n".to_owned()),
        cwd: None,
        hook_json: false,
        clear: false,
        run_id: None,
    });
    assert_eq!(exit_code, 0, "{reported}");
    wait_for_log_line("waiting pane=");
    let typed = agent_pty.wait_for_stdin("the prompt reaches the agent's pty", |bytes| {
        // Paste and Enter are separate writes; observe both before asserting.
        bytes
            .windows(b"\x1b[200~go\x1b[201~\r".len())
            .any(|part| part == b"\x1b[200~go\x1b[201~\r")
    });
    let typed = String::from_utf8_lossy(&typed);
    assert!(
        typed.contains("\u{1b}[200~go\u{1b}[201~") && typed.ends_with('\r'),
        "a bracketed paste and Enter: {typed:?}"
    );

    // The agent exits; the task awaited that too.
    agent_pty.exit(Some(0));
    let lines = wait_for_log_line("exited pane=");
    let order: Vec<usize> = [
        "permissions Granted",
        &format!("spawned pane={terminal_id}"),
        &format!("waiting pane={terminal_id}"),
        &format!("exited pane={terminal_id} 0"),
    ]
    .iter()
    .map(|wanted| {
        lines
            .iter()
            .position(|line| line == wanted)
            .unwrap_or_else(|| panic!("no {wanted:?} in {lines:?}"))
    })
    .collect();
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "the task ran its steps in order: {lines:?}"
    );
    let reported = events
        .streamed_events()
        .iter()
        .filter_map(|(_, json)| agent_in(json, terminal_id))
        .any(|(source, state)| source == "Reported" && state == "Waiting");
    assert!(
        reported,
        "the report replaced the spawn path's record on the stream"
    );

    // Reloaded, the plugin's `load` runs again with its permissions already granted - and a
    // gated call made right after asking is not refused.
    let (exit_code, reported) = yinz.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: format!("file:{}", plugin_dir.join("harness.scm").display()),
        configuration: None,
        configuration_json: None,
    });
    assert_eq!(exit_code, 0, "{reported}");
    let deadline = std::time::Instant::now() + default_timeout();
    loop {
        let lines = harness_log();
        if lines.iter().any(|line| line == "in force at load: true") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the reload never found its permissions in force: {lines:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    yinz.quit();
}

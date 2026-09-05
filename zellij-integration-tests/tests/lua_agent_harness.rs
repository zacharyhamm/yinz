#![cfg(unix)]
//! A Lua plugin that harnesses an agent, end to end: it spawns the agent through the launch
//! planner (`zellij.agent.spawn`), waits for the agent's record to move (`zellij.async.agent_state`),
//! prompts it (`zellij.agent.send`) and awaits its exit (`zellij.async.pane_exit`) - as one task,
//! in straight-line code, with the fake pty standing in for `claude` and `zellij action
//! agent-report` standing in for its hooks.
//!
//! The same plugin, reloaded, proves the load-time grant: a permission the user answered once is
//! in force from `request_permission` on, so a gated call made from `load` is not refused.

use std::fs;
use zellij_integration_tests::{default_timeout, LayoutInfo, TestRunner, TERMINAL_SIZE};
use zellij_utils::cli::CliAction;

const HARNESS: &str = r#"
local zellij = require("zellij")
local M = {}

function M.load(config)
  zellij.request_permission { "run_commands", "read_application_state", "write_to_stdin" }
  -- Already answered: in force at once. The first time: not until the user says so.
  local in_force = pcall(zellij.list_clients)
  zellij.log("in force at load: " .. tostring(in_force))
  zellij.task(function()
    local answer = zellij.await(zellij.async.permissions { "run_commands", "read_application_state", "write_to_stdin" })
    zellij.log("permissions " .. answer)
    local pane = zellij.agent.spawn { kind = "claude", hooks = false, cwd = "/host" }
    zellij.log("spawned pane=" .. pane.Terminal)
    zellij.await(zellij.async.agent_state(pane, "waiting"))
    zellij.log("waiting pane=" .. pane.Terminal)
    zellij.agent.send(pane, "go")
    local exit = zellij.await(zellij.async.pane_exit(pane))
    zellij.log("exited pane=" .. pane.Terminal .. " " .. tostring(exit.exit_code))
  end)
end

function M.render() return "harness is up" end

return M
"#;

const HARNESS_LAYOUT: &str = r#"layout {
    pane {
        plugin location="file:harness.lua"
    }
}
"#;

/// The log's lines carrying the harness's own tag, in order.
fn harness_log() -> Vec<String> {
    zellij_integration_tests::test_env::log_tail(400)
        .lines()
        .filter(|line| line.contains("[lua:harness.lua]"))
        .map(|line| {
            line.split("[lua:harness.lua] ")
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
            zellij_integration_tests::test_env::log_tail(60)
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// The agent's source and state as one `pane_update` event reports them for `terminal_id`.
fn agent_in(event_json: &str, terminal_id: u32) -> Option<(String, String)> {
    let event: serde_json::Value = serde_json::from_str(event_json).ok()?;
    let agent = event
        .get("PaneUpdate")?
        .get("panes")?
        .as_object()?
        .values()
        .filter_map(|panes| panes.as_array())
        .flatten()
        .find(|pane| pane["id"] == terminal_id && pane["is_plugin"] == false)?
        .get("agent")?;
    Some((
        agent.get("source")?.as_str()?.to_owned(),
        agent.get("state")?.as_str()?.to_owned(),
    ))
}

#[test]
fn a_lua_harness_spawns_an_agent_waits_on_it_prompts_it_and_sees_it_exit() {
    let config_dir = zellij_integration_tests::test_env::init()
        .join("config")
        .join("lua-agent-harness");
    fs::create_dir_all(&config_dir).unwrap();
    let plugin_dir = config_dir.join("plugins");
    zellij_server::plugin_authoring::write(&plugin_dir, "harness", HARNESS, false, true)
        .expect("the harness is written");

    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(config_dir.clone())
        .with_config(&format!("user_plugin_dir \"{}\"", plugin_dir.display()))
        .with_layout(LayoutInfo::Stringified(HARNESS_LAYOUT.to_owned()))
        .start();

    // The first load: the permissions are asked for and nothing gated works until the user
    // answers - which the task waits for.
    zellij.wait_until("the harness asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    let lines = wait_for_log_line("in force at load:");
    assert!(
        lines.iter().any(|line| line == "in force at load: false"),
        "{lines:?}"
    );
    let events = zellij.stream_events(&["pane_update"]);
    events.wait_for_event("PaneUpdate");
    zellij.send_stdin(b"y");

    // Granted: the task spawns the agent. The pane carries the record the spawn path wrote.
    let agent_pty = zellij.expect_pty_spawn();
    let terminal_id = agent_pty.terminal_id();
    let lines = wait_for_log_line("spawned pane=");
    assert!(
        lines.contains(&"permissions Granted".to_owned())
            && lines.contains(&format!("spawned pane={terminal_id}")),
        "{lines:?}"
    );
    zellij.wait_until("the spawn path's record is on the stream", |_| {
        events
            .streamed_events()
            .iter()
            .any(|(_, json)| agent_in(json, terminal_id) == Some(("Spawned".into(), "Busy".into())))
    });

    // The agent reports for itself, as its hooks would: the record moves to waiting and the
    // task, which was awaiting exactly that, prompts it.
    let (exit_code, reported) = zellij.run_cli_action_reporting(CliAction::AgentReport {
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
        bytes.windows(2).any(|pair| pair == b"go")
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
    let (exit_code, reported) = zellij.run_cli_action_reporting(CliAction::StartOrReloadPlugin {
        url: format!("file:{}", plugin_dir.join("harness.lua").display()),
        configuration: None,
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
    zellij.quit();
}

#![cfg(unix)]

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use std::fs;
use zellij_cli::args::CliAction;
use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};
use zellij_types::pane_size::Size;

// Run the shipped bridge, exit hook and view together. Swamp history and supervisor
// queries are fixtures: terminal processes use this suite's fake PTYs. Pane creation,
// replacement, input, hook delivery, rendering and the data store use the real host.
fn fixture() -> String {
    let bridge = zellij_config::consts::builtin_lua_plugin("swamp-bridge").unwrap();
    let exit = zellij_config::consts::builtin_lua_plugin("agent-exit").unwrap();
    format!(
        "{}\nlocal bridge=(function()\n{bridge}\nend)()\nlocal hook=(function()\n{exit}\nend)()\n{}",
        include_str!("swamp_exit_fixture.lua"),
        r#"
local M={}
function M.load(config)
  z.task(function()
    z.await(z.async.permissions{'read_application_state','change_application_state','run_commands','write_to_stdin','read_cli_pipes','plugin_data','read_pane_contents','message_and_launch_other_plugins'})
    bridge.load()
    config.agent_cwd='/host'
    token=config.agent_exit_token
    local source=tonumber(config.agent_pane_id)
    hosts[#hosts+1]={terminal_id=source,attached=true,session_name=z.server_session,host_id='source-'..source,child_pid=100+source}
    hook.load(config)
  end)
end
function M.update(event) return hook.update(event) end
function M.render(rows,cols) return hook.render(rows,cols) end
function M.pipe(message)
  if message.name~='fixture' then hook.pipe(message);return end
  z.task(function()
    if message.payload=='agent' then
      local agent=call{method='agent_spawn',work=token,key='worker',spec={kind='claude',cwd='/host',allow_outside_worktree=true}}
      worker=agent.pane
      emit('SessionStart')
      call{method='agent_prompt',work=token,key='turn',agent='worker',prompt='work'}
    elseif message.payload=='done' then
      emit('UserPromptSubmit','work')
      emit('Stop')
      status='suspended'
      require('zellij.workflow_view').refresh()
    end
    local pipe=message.source.Cli
    z.cli_pipe_output(pipe,'ok');z.unblock_cli_pipe_input(pipe)
  end)
end
return M
"#
    )
}

fn advance(zellij: &TestSession, stage: &str) {
    assert_eq!(
        zellij.run_cli_action(CliAction::Pipe {
            name: Some("fixture".into()),
            payload: Some(stage.into()),
            args: None,
            plugin: None,
            plugin_configuration: None,
            force_launch_plugin: false,
            skip_plugin_cache: false,
            floating_plugin: None,
            in_place_plugin: None,
            plugin_cwd: None,
            plugin_title: None,
        }),
        0
    );
}

#[test]
fn swamp_exit_keeps_status_agent_question_and_result_in_the_original_location() {
    swamp_exit_with_status_bar(false);
}

#[test]
fn swamp_exit_status_bar_tracks_mode_changes_while_the_workflow_view_is_active() {
    swamp_exit_with_status_bar(true);
}

fn swamp_exit_with_status_bar(status_bar: bool) {
    let root = zellij_integration_tests::test_env::init().join("swamp-exit");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("exit.lua");
    fs::write(&path, fixture()).unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "pane_frame_style \"full\"\nmouse_mode true\ncopy_clipboard \"system\"\nload_plugins clear-defaults=true {{}}\nhooks {{ agent_exit \"file:{}\"; }}",
            path.display()
        ))
        .with_layout(LayoutInfo::Stringified(
            if status_bar {
                "layout { pane command=\"claude\" name=\"Review topic\" focus=true; pane size=2 borderless=true { plugin location=\"zellij:status-bar\"; }; }"
            } else {
                "layout { pane command=\"claude\" name=\"Review topic\"; }"
            }.into(),
        ))
        .start();
    let original = zellij.expect_pty_spawn();
    let original_title = format!("[{}] Review topic", original.terminal_id());
    original.output(b"ORIGINAL SCROLLBACK\r\n");
    zellij.wait_until("original output", |s| s.contains("ORIGINAL SCROLLBACK"));
    let events = zellij.stream_events(&["pane_update", "tab_update"]);
    events.wait_for_event("PaneUpdate");
    assert_eq!(
        zellij.run_cli_action(CliAction::AgentReport {
            pane_id: Some(format!("terminal_{}", original.terminal_id())),
            kind: Some("claude".into()),
            state: Some("busy".into()),
            detail: None,
            cwd: None,
            hook_json: false,
            clear: false,
            run_id: None,
        }),
        0
    );
    original.exit(Some(0));
    zellij.wait_until("hook permissions", |s| s.contains("Allow? (y/n)"));
    zellij.send_stdin(b"y");
    zellij.wait_until("workflow status in exit pane", |s| {
        s.contains("Run run: running") && s.contains(&original_title)
    });
    let runner = zellij.expect_pty_spawn();
    runner.output(b"BACKGROUND RUNNER\r\n");
    advance(&zellij, "agent");
    let worker = zellij.expect_pty_spawn();
    worker.output(b"INTERACTIVE AGENT\r\n");
    zellij.wait_until("agent replaces status", |s| s.contains("INTERACTIVE AGENT"));
    zellij.send_stdin(b"interactive input\r");
    worker.wait_for_stdin("interactive input reaches agent", |s| {
        String::from_utf8_lossy(s).contains("interactive input")
    });
    advance(&zellij, "done");
    zellij.wait_until("verified completion restores question", |s| {
        s.contains("Agent exited. Choose")
    });
    if status_bar {
        for _ in 0..2 {
            zellij.send_stdin(&keys::ctrl('g'));
            zellij.wait_until("status bar shows locked mode beside the workflow", |s| {
                s.contains("Agent exited. Choose") && s.contains("INTERFACE LOCKED")
            });
            zellij.send_stdin(&keys::ctrl('g'));
            zellij.wait_until("status bar shows normal mode beside the workflow", |s| {
                s.contains("Agent exited. Choose")
                    && s.contains("Ctrl")
                    && s.contains("LOCK")
                    && !s.contains("INTERFACE LOCKED")
            });
        }
    }
    let question = zellij.wait_until("all five permitted choices are buttons", |s| {
        s.lines().iter().any(|line| {
            [
                "Restart",
                "Duplicate",
                "Fork",
                "Begin review",
                "Leave",
                "Cancel run",
                "Detach",
            ]
            .iter()
            .all(|label| line.contains(label))
        })
    });
    let prompt_row = question
        .lines()
        .iter()
        .position(|line| line.contains("Agent exited. Choose"))
        .unwrap();
    let button_row = question
        .lines()
        .iter()
        .position(|line| line.contains("Cancel run") && line.contains("Restart"))
        .unwrap();
    assert!(!question.contains("Submit") && !question.contains("Next question"));
    zellij.send_stdin(b"\x1b[6~");
    let scrolled = zellij.wait_until("page down scrolls logs with pinned question", |s| {
        s.contains("LOG 020") && s.lines()[prompt_row].contains("Agent exited. Choose")
    });
    assert_eq!(question.lines()[button_row], scrolled.lines()[button_row]);
    zellij.send_stdin(b"\x1b[<64;5;3M");
    zellij.wait_until("wheel scrolls log backward", |s| s.contains("LOG 008"));
    zellij.send_stdin(b"\x1b[5~");
    let logs = zellij.wait_until("page up restores log beginning", |s| s.contains("LOG 001"));
    let log_row = logs
        .lines()
        .iter()
        .position(|line| line.contains("LOG 001"))
        .unwrap();
    let log_col = logs.lines()[log_row]
        .chars()
        .position(|c| c == 'L')
        .unwrap();
    zellij.send_stdin(format!("\x1b[<0;{};{}M", log_col + 1, log_row + 1).as_bytes());
    zellij.send_stdin(format!("\x1b[<32;{};{}M", log_col + 8, log_row + 1).as_bytes());
    zellij.send_stdin(format!("\x1b[<0;{};{}m", log_col + 8, log_row + 1).as_bytes());
    let copied = BASE64.encode("LOG 001");
    zellij.wait_until_raw_output("log selection is copied", |bytes| {
        String::from_utf8_lossy(bytes).contains(&format!(";{copied}"))
    });
    if status_bar {
        let line = zellij.snapshot().lines()[button_row].clone();
        let col = line.find("Leave").unwrap();
        let col = line[..col].chars().count();
        zellij.send_stdin(format!("\x1b[<0;{};{}M", col + 1, button_row + 1).as_bytes());
        zellij.send_stdin(format!("\x1b[<0;{};{}m", col + 1, button_row + 1).as_bytes());
    } else {
        zellij.resize(Size { rows: 8, cols: 24 });
        for _ in 0..4 {
            zellij.send_stdin(&keys::TAB);
        }
        zellij.wait_until("last choice remains visible in a small pane", |s| {
            s.contains("Leave")
        });
        zellij.send_stdin(&keys::ENTER);
        zellij.resize(TERMINAL_SIZE);
    }
    zellij.wait_until("final result stays visible", |s| {
        s.contains("Run run: completed") && s.contains(&original_title)
    });
    // A managed helper's process exit must not replace status with another hook.
    worker.exit(Some(0));
    runner.exit(Some(0));
    zellij.wait_until("managed exits preserve the result", |s| {
        s.contains("Run run: completed")
    });
    zellij.send_stdin(&keys::ESC);
    zellij.wait_until("detach restores original scrollback", |s| {
        s.contains("ORIGINAL SCROLLBACK") && s.contains(&original_title)
    });

    let (mut pane_events, mut tab_events) = (0, 0);
    for (kind, json) in events.streamed_events() {
        let event: serde_json::Value = serde_json::from_str(&json).unwrap();
        if kind == "PaneUpdate" {
            pane_events += 1;
            let panes: Vec<_> = event["PaneUpdate"]["panes"]
                .as_object()
                .unwrap()
                .values()
                .flat_map(|p| p.as_array().unwrap())
                .filter(|p| p["is_suppressed"] == false)
                .collect();
            assert!(
                panes.len() <= 1 + usize::from(status_bar),
                "extra visible pane: {json}"
            );
            assert!(panes.iter().all(|p| p["is_floating"] == false), "{json}");
        } else if kind == "TabUpdate" {
            tab_events += 1;
            assert_eq!(event["TabUpdate"].as_array().unwrap().len(), 1, "{json}");
        }
    }
    assert!(pane_events > 0 && tab_events > 0);
    assert_eq!(
        zellij.run_cli_action(CliAction::ClosePane {
            pane_id: Some(format!("terminal_{}", original.terminal_id())),
        }),
        0
    );
    zellij.wait_for_exit();
}

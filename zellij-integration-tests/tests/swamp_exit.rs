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
local M,source={}
function M.load(config)
  z.task(function()
    z.await(z.async.permissions{'read_application_state','change_application_state','run_commands','write_to_stdin','read_cli_pipes','plugin_data','read_pane_contents','message_and_launch_other_plugins'})
    bridge.load()
    config.agent_cwd='/host'
    token=config.agent_exit_token
    source=tonumber(config.agent_pane_id)
    hosts[#hosts+1]={terminal_id=source,attached=true,session_name=z.server_session,host_id='source-'..source,child_pid=100+source}
    hook.load(config)
  end)
end
function M.update(event) return hook.update(event) end
function M.render(rows,cols) return hook.render(rows,cols) end
function M.pipe(message)
  if message.name=='agent_subscribe' then
    z.message_to_plugin{destination_plugin_id=message.source.Plugin,message_name='agent_snapshot',message_payload='{"panes":[],"activity":{}}'}
    return
  elseif message.name=='swamp-pane-result' then
    bridge.pipe(message)
    local reply=z.json.decode(message.payload)
    if reply.choice=='cancelled' then
      local result=call{method='lookup',work=reply.work,key=reply.key}
      if result.outcome=='completed' then status='suspended';require('zellij.workflow_view').refresh() end
    end
    return
  end
  if message.name~='fixture' then hook.pipe(message);return end
  z.task(function()
    if message.payload=='fork' then
      call{method='pane_action',work=token,key='fork',action='fork'}
    elseif message.payload=='restart' then
      z.rerun_command_pane(source)
    elseif message.payload=='close-source' then
      z.close_terminal_pane(source)
    elseif message.payload=='manual-close-source' then
      -- Manual picker operations have no persisted lifecycle view.
      local uid=z.json.encode{z.get_plugin_ids().host_id,z.server_session,token,'view'}
      z.data.apply(z.data.delete('delivery',z.data.eq('uid',uid)))
      z.hide_pane_with_id(z.plugin_pane(z.get_plugin_ids().plugin_id))
      z.close_terminal_pane(source)
    elseif message.payload=='state' then
      local result=call{method='lookup',work=token,key='fork'}
      local pipe=message.source.Cli
      z.cli_pipe_output(pipe,z.json.encode(result));z.unblock_cli_pipe_input(pipe)
      return
    elseif message.payload=='agent' then
      local agent=call{method='agent_spawn',work=token,key='worker',spec={kind='claude',cwd='/host',allow_outside_worktree=true}}
      worker=agent.pane
      emit('SessionStart')
      call{method='agent_prompt',work=token,key='turn',agent='worker',prompt='work'}
      step='agent'
      require('zellij.workflow_view').refresh()
    elseif message.payload=='test' then
      step='test'
      require('zellij.workflow_view').refresh()
    elseif message.payload=='refresh-failed' then
      refresh_failed=true
      require('zellij.workflow_view').refresh()
    elseif message.payload=='done' then
      refresh_failed=false
      step='choose'
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
    // The terminal frame labels itself with its own id; the exit panel that replaces it is a
    // plugin pane, which does not - so the hook writes an id into its name, and that id is the
    // panel's own. Its value is not known here, so the screen is matched on the rest of the
    // title and the id itself is pinned off the pane events below.
    let panel_title = "] Review topic";
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
    zellij.wait_until("workflow log tail in exit pane", |s| {
        s.contains("LOG 100") && s.contains("Cancel run") && s.contains(panel_title)
    });
    advance(&zellij, "agent");
    let worker = zellij.expect_pty_spawn();
    worker.output(b"INTERACTIVE AGENT\r\n");
    zellij.wait_until("agent shows the workflow subtitle above its output", |s| {
        s.contains("INTERACTIVE AGENT")
            && s.lines()[1].contains("fixture · workflow/agent · running")
    });
    advance(&zellij, "test");
    zellij.wait_until(
        "subtitle follows history while the terminal stays foregrounded",
        |s| {
            s.contains("INTERACTIVE AGENT")
                && s.lines()[1].contains("fixture · workflow/test · running")
        },
    );
    advance(&zellij, "refresh-failed");
    zellij.wait_until(
        "failed refresh marks foreground progress unavailable",
        |s| {
            s.contains("INTERACTIVE AGENT") && s.lines()[1].contains("fixture · status unavailable")
        },
    );
    zellij.send_stdin(b"interactive input\r");
    worker.wait_for_stdin("interactive input reaches agent", |s| {
        String::from_utf8_lossy(s).contains("interactive input")
    });
    advance(&zellij, "done");
    zellij.wait_until("verified completion restores question", |s| {
        s.contains("Agent exited. Choose")
            && s.lines()[1].contains("fixture · workflow/choose · waiting_approval")
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
    let question = zellij.wait_until("all four permitted choices are buttons", |s| {
        s.lines().iter().any(|line| {
            ["Restart", "Fork", "Begin review", "Leave"]
                .iter()
                .all(|label| line.contains(label))
        })
    });
    assert!(
        question.contains("LOG 100"),
        "workflow logs follow by default"
    );
    let prompt_row = question
        .lines()
        .iter()
        .position(|line| line.contains("Agent exited. Choose"))
        .unwrap();
    let button_row = question
        .lines()
        .iter()
        .position(|line| line.contains("Leave") && line.contains("Restart"))
        .unwrap();
    assert!(!question.contains("Duplicate"));
    assert!(
        !question.contains("Submit")
            && !question.contains("Next question")
            && !question.contains("Detach")
    );
    zellij.send_stdin(&keys::ESC);
    zellij.send_stdin(b"\x1b[5~");
    let beginning = zellij.wait_until("page up scrolls logs with pinned question", |s| {
        s.contains("LOG 001") && s.lines()[prompt_row].contains("Agent exited. Choose")
    });
    assert_eq!(question.lines()[button_row], beginning.lines()[button_row]);
    zellij.send_stdin(b"\x1b[6~");
    let scrolled = zellij.wait_until("page down returns to the tail with pinned question", |s| {
        s.contains("LOG 100") && s.lines()[prompt_row].contains("Agent exited. Choose")
    });
    assert_eq!(question.lines()[button_row], scrolled.lines()[button_row]);
    zellij.send_stdin(b"\x1b[<64;5;3M");
    zellij.wait_until("wheel scrolls log backward", |s| {
        !s.contains("LOG 100") && s.contains("↓") && s.contains("Agent exited. Choose")
    });
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
    // A managed helper's process exit must not replace the workflow with another hook.
    worker.exit(Some(0));
    zellij.wait_until("managed exits preserve the workflow question", |s| {
        s.contains("Agent exited. Choose") && s.contains(panel_title)
    });
    if status_bar {
        let line = zellij.snapshot().lines()[button_row].clone();
        let col = line.find("Leave").unwrap();
        let col = line[..col].chars().count();
        zellij.send_stdin(format!("\x1b[<0;{};{}M", col + 1, button_row + 1).as_bytes());
        zellij.send_stdin(format!("\x1b[<0;{};{}m", col + 1, button_row + 1).as_bytes());
    } else {
        zellij.resize(Size { rows: 8, cols: 24 });
        for _ in 0..3 {
            zellij.send_stdin(&keys::TAB);
        }
        zellij.wait_until("last choice remains visible in a small pane", |s| {
            s.contains("Leave")
        });
        zellij.send_stdin(&keys::ENTER);
        zellij.resize(TERMINAL_SIZE);
    }
    zellij.wait_for_exit();

    let (mut pane_events, mut tab_events, mut panel_titles) = (0, 0, 0);
    let mut original_hidden = false;
    for (kind, json) in events.streamed_events() {
        let event: serde_json::Value = serde_json::from_str(&json).unwrap();
        if kind == "PaneUpdate" {
            pane_events += 1;
            let all_panes: Vec<_> = event["PaneUpdate"]["panes"]
                .as_object()
                .unwrap()
                .values()
                .flat_map(|p| p.as_array().unwrap())
                .collect();
            if let Some(original_pane) = all_panes
                .iter()
                .find(|p| p["is_plugin"] == false && p["id"] == original.terminal_id().get())
            {
                if original_hidden {
                    assert_eq!(original_pane["is_suppressed"], true, "{json}");
                }
                original_hidden |= original_pane["is_suppressed"] == true;
            }
            let panes: Vec<_> = all_panes
                .into_iter()
                .filter(|p| p["is_suppressed"] == false)
                .collect();
            assert!(
                panes.len() <= 1 + usize::from(status_bar),
                "extra visible pane: {json}"
            );
            assert!(panes.iter().all(|p| p["is_floating"] == false), "{json}");
            for pane in panes.iter().filter(|p| {
                p["is_plugin"] == true
                    && p["title"]
                        .as_str()
                        .is_some_and(|t| t.ends_with(panel_title))
            }) {
                panel_titles += 1;
                assert_eq!(
                    pane["title"].as_str().unwrap(),
                    format!("[{}] Review topic", pane["id"].as_u64().unwrap()),
                    "the exit panel names the pane it is, not the one suppressed behind it: {json}"
                );
            }
        } else if kind == "TabUpdate" {
            tab_events += 1;
            assert!(event["TabUpdate"].as_array().unwrap().len() <= 1, "{json}");
        }
    }
    assert!(pane_events > 0 && tab_events > 0 && original_hidden);
    assert!(
        panel_titles > 0,
        "no pane event carried the exit panel's title"
    );
}

fn panes(zellij: &TestSession) -> Vec<zellij_types::data::PaneListEntry> {
    let (code, json) = zellij.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: true,
        all: true,
        json: true,
    });
    assert_eq!(code, 0);
    serde_json::from_str(&json).unwrap()
}

#[test]
fn closing_a_workflow_helper_removes_its_source_and_restores_input_and_chrome() {
    close_workflow("agent");
}

#[test]
fn closing_the_exit_question_ends_the_suspended_lifecycle() {
    close_workflow("question");
}

#[test]
fn closing_during_workflow_launch_cancels_the_late_result() {
    close_workflow("delay");
}

fn close_workflow(stage: &str) {
    let root = zellij_integration_tests::test_env::init().join("swamp-close");
    fs::create_dir_all(&root).unwrap();
    let bridge_path = root.join("bridge.lua");
    let exit_path = root.join("exit.lua");
    fs::write(
        &bridge_path,
        format!(
            "{}\nreturn plugin((function()\n{}\nend)())",
            include_str!("swamp_close_fixture.lua"),
            zellij_config::consts::builtin_lua_plugin("swamp-bridge").unwrap()
        ),
    )
    .unwrap();
    fs::write(
        &exit_path,
        format!(
            r#"
local z=require('zellij')
z.project_config.read=function() return {{root='/control',config={{workflow='fixture'}}}} end
local rpc=z.rpc.call
z.rpc.call=function(args) args.plugin_url='file:{}';return rpc(args) end
local hook=(function()
{}
end)()
local load=hook.load
hook.load=function(config)
  z.task(function()
    z.await(z.async.permissions{{'read_application_state','change_application_state','message_and_launch_other_plugins','read_pane_contents'}})
    load(config)
  end)
end
return hook
"#,
            bridge_path.display(),
            zellij_config::consts::builtin_lua_plugin("agent-exit").unwrap()
        ),
    )
    .unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!("load_plugins clear-defaults=true {{ \"file:{}\"; }}\nhooks {{ agent_exit \"file:{}\"; }}", bridge_path.display(), exit_path.display()))
        .with_layout(LayoutInfo::Stringified(r#"layout {
            default_tab_template {
                pane size=1 borderless=true { plugin location="zellij:tab-bar"; }
                children
                pane size=2 borderless=true { plugin location="zellij:status-bar"; }
            }
            tab name="agent" focus=true { pane command="claude" cwd="/control"; }
            tab name="survivor" { pane; }
        }"#.into()))
        .start();
    let original = zellij.expect_pty_spawn();
    let survivor = zellij.expect_pty_spawn();
    survivor.output(b"SURVIVING TERMINAL\r\n");
    zellij.wait_until("bridge permissions", |s| s.contains("Allow? (y/n)"));
    zellij.send_stdin(b"y");
    zellij.wait_until("bridge ready", |s| !s.contains("Allow? (y/n)"));
    if stage == "delay" {
        advance(&zellij, "delay");
    }
    assert_eq!(
        zellij.run_cli_action(CliAction::AgentReport {
            pane_id: Some(format!("terminal_{}", original.terminal_id())),
            kind: Some("claude".into()),
            state: Some("busy".into()),
            cwd: Some("/control".into()),
            detail: None,
            hook_json: false,
            clear: false,
            run_id: None,
        }),
        0
    );
    original.exit(Some(0));
    zellij.wait_until("exit view permissions", |s| s.contains("Allow? (y/n)"));
    zellij.send_stdin(b"y");
    if stage == "delay" {
        zellij.wait_until("launch is in flight", |_| {
            fixture_state(&zellij)["waiting"] == true
        });
    } else {
        zellij.wait_until("workflow launched", |s| s.contains("Run run: running"));
        advance(&zellij, stage);
        if stage == "agent" {
            let worker = zellij.expect_pty_spawn();
            worker.output(b"PRESENTED HELPER\r\n");
            zellij.wait_until("helper presented", |s| s.contains("PRESENTED HELPER"));
        } else {
            zellij.wait_until("exit question", |s| s.contains("Agent exited. Choose"));
        }
    }
    zellij.send_stdin(&keys::ctrl('p'));
    zellij.send_stdin(b"x");
    zellij.wait_until("surviving tab has input and chrome", |s| {
        s.contains("SURVIVING TERMINAL") && s.tab_bar_appears() && s.status_bar_appears()
    });
    zellij.send_stdin(b"AFTER CLOSE");
    survivor.wait_for_stdin("typing reaches the surviving terminal", |bytes| {
        bytes.ends_with(b"AFTER CLOSE")
    });
    if stage == "delay" {
        advance(&zellij, "release");
    }
    zellij.wait_until("all owned panes are gone and workflow is closed", |_| {
        let state = fixture_state(&zellij);
        let terminals: Vec<_> = panes(&zellij)
            .into_iter()
            .filter(|p| !p.pane_info.is_plugin())
            .collect();
        state["closed"] == true
            && (stage == "question" || state["cancelled"] == true)
            && terminals.len() == 1
            && terminals[0].pane_info.is_focused
            && terminals[0].pane_info.is_selectable
            && !terminals[0].pane_info.is_suppressed
            && terminals[0].pane_info.id
                == zellij_types::pane_id::PaneId::Terminal(survivor.terminal_id())
    });
    let (code, tabs) = zellij.run_cli_action_reporting(CliAction::QueryTabNames);
    assert_eq!(code, 0);
    assert_eq!(tabs.trim(), "survivor");
    zellij.quit();
}

fn fixture_state(zellij: &TestSession) -> serde_json::Value {
    let (code, state) = zellij.run_cli_action_reporting(CliAction::Pipe {
        name: Some("fixture".into()),
        payload: Some("state".into()),
        args: None,
        plugin: None,
        plugin_configuration: None,
        force_launch_plugin: false,
        skip_plugin_cache: false,
        floating_plugin: None,
        in_place_plugin: None,
        plugin_cwd: None,
        plugin_title: None,
    });
    assert_eq!(code, 0);
    serde_json::from_str(&state).unwrap()
}

fn fork_picker() -> (
    TestSession,
    zellij_integration_tests::FakePtyHandle,
    Vec<zellij_types::data::PaneListEntry>,
) {
    let root = zellij_integration_tests::test_env::init().join("swamp-fork");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("exit.lua");
    fs::write(&path, fixture()).unwrap();
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "load_plugins clear-defaults=true {{}}\nhooks {{ agent_exit \"file:{}\"; }}",
            path.display()
        ))
        .with_layout(LayoutInfo::Stringified(
            "layout { pane command=\"claude\" cwd=\"/tmp/fork-topic\" focus=true; pane; }".into(),
        ))
        .start();
    let original = zellij.expect_pty_spawn();
    zellij.expect_pty_spawn().output(b"NEIGHBOR\r\n");
    original.output(b"ORIGINAL SCROLLBACK\r\n");
    zellij.wait_until("original output", |s| s.contains("ORIGINAL SCROLLBACK"));
    assert_eq!(
        zellij.run_cli_action(CliAction::AgentReport {
            pane_id: Some(format!("terminal_{}", original.terminal_id())),
            kind: Some("claude".into()),
            state: Some("busy".into()),
            detail: None,
            cwd: Some("/tmp/fork-topic".into()),
            hook_json: false,
            clear: false,
            run_id: None,
        }),
        0
    );
    let before = panes(&zellij);
    original.exit(Some(0));
    zellij.wait_until("hook permissions", |s| s.contains("Allow? (y/n)"));
    zellij.send_stdin(b"y");
    zellij.wait_until("exit view follows the log tail", |s| {
        s.contains("LOG 100") && s.contains("Cancel run")
    });
    advance(&zellij, "fork");
    zellij.wait_until("fork resolves the suppressed source", |s| {
        s.contains("forking claude in fork-topic") && s.contains("Codex")
    });
    (zellij, original, before)
}

#[test]
fn workflow_fork_reuses_the_source_layout_scrollback_directory_and_focus() {
    let (mut zellij, original, before) = fork_picker();
    zellij.send_stdin(&keys::TAB);
    zellij.send_stdin(&keys::ENTER);
    zellij.wait_until("fork closes both views and runs in the source", |_| {
        let now = panes(&zellij);
        now.len() == before.len()
            && now.iter().any(|p| {
                p.pane_info.id == before[0].pane_info.id
                    && !p.pane_info.exited
                    && !p.pane_info.is_suppressed
                    && p.pane_info.terminal_command.as_deref() == Some("codex")
            })
    });
    let after = panes(&zellij);
    for old in &before {
        let new = after
            .iter()
            .find(|p| p.pane_info.id == old.pane_info.id)
            .unwrap();
        assert_eq!(new.tab_id, old.tab_id);
        let a = &old.pane_info;
        let b = &new.pane_info;
        assert_eq!(
            (
                b.pane_x,
                b.pane_y,
                b.pane_rows,
                b.pane_columns,
                b.is_floating
            ),
            (
                a.pane_x,
                a.pane_y,
                a.pane_rows,
                a.pane_columns,
                a.is_floating
            )
        );
        assert_eq!(b.target, a.target);
    }
    let source = after
        .iter()
        .find(|p| p.pane_info.id == before[0].pane_info.id)
        .unwrap();
    assert!(source.pane_info.is_focused);
    let agent = source
        .pane_info
        .agent
        .as_ref()
        .expect("the replacement is an agent");
    assert_eq!(agent.kind, "codex");
    assert_eq!(agent.cwd.as_deref(), Some("/tmp/fork-topic"));
    assert!(
        agent.session_id.is_none(),
        "Fork starts a fresh conversation"
    );
    let (code, scrollback) = zellij.run_cli_action_reporting(CliAction::DumpScreen {
        path: None,
        full: true,
        ansi: false,
        pane_id: Some(format!("terminal_{}", original.terminal_id())),
    });
    assert_eq!(code, 0);
    assert!(scrollback.contains("ORIGINAL SCROLLBACK"), "{scrollback}");
    zellij.send_stdin(b"FORK INPUT");
    original.wait_for_stdin("the original pane receives keyboard focus", |s| {
        s.ends_with(b"FORK INPUT")
    });
    zellij.quit();
}

#[test]
fn workflow_fork_cancellation_returns_to_the_exit_menu_without_starting_an_agent() {
    let (mut zellij, original, before) = fork_picker();
    zellij.send_stdin(&keys::ESC);
    zellij.wait_until("cancellation restores the exit menu", |s| {
        s.contains("Agent exited. Choose Restart, Fork") && !s.contains("Fork agent")
    });
    assert!(original.is_exited());
    let after = panes(&zellij);
    assert_eq!(
        after.iter().filter(|p| !p.pane_info.is_plugin()).count(),
        before.len()
    );
    assert!(!after
        .iter()
        .any(|p| p.pane_info.plugin_url.as_deref() == Some("zellij:hocus-new-agent")));
    zellij.quit();
}

#[test]
fn manual_picker_source_closure_completes_without_a_persisted_view() {
    let (mut zellij, _original, before) = fork_picker();
    advance(&zellij, "manual-close-source");
    zellij.wait_until("source closure records picker cancellation", |_| {
        let state = fixture_state(&zellij);
        state["outcome"] == "completed"
            && state["choice"] == "cancelled"
            && !panes(&zellij)
                .iter()
                .any(|p| p.pane_info.plugin_url.as_deref() == Some("zellij:hocus-new-agent"))
    });
    let after = panes(&zellij);
    assert_eq!(
        after.iter().filter(|p| !p.pane_info.is_plugin()).count(),
        before.len() - 1
    );
    assert!(!after
        .iter()
        .any(|p| p.pane_info.plugin_url.as_deref() == Some("zellij:hocus-new-agent")));
    zellij.quit();
}

#[test]
fn workflow_fork_rechecks_a_restarted_source_before_confirmation() {
    refused_workflow_fork("restart", "source pane is no longer exited");
}

#[test]
fn workflow_fork_rechecks_a_closed_source_before_confirmation() {
    let (mut zellij, _original, before) = fork_picker();
    advance(&zellij, "close-source");
    zellij.wait_until("closing the source also closes its pending picker", |_| {
        let after = panes(&zellij);
        after.iter().filter(|p| !p.pane_info.is_plugin()).count() == before.len() - 1
            && !after
                .iter()
                .any(|p| p.pane_info.plugin_url.as_deref() == Some("zellij:hocus-new-agent"))
    });
    zellij.quit();
}

fn refused_workflow_fork(change: &str, error: &str) {
    let (mut zellij, _original, before) = fork_picker();
    advance(&zellij, change);
    zellij.send_stdin(&keys::TAB);
    zellij.send_stdin(&keys::ENTER);
    zellij.wait_until("changed source is refused", |s| s.contains(error));
    let after = panes(&zellij);
    assert_eq!(
        after.iter().filter(|p| !p.pane_info.is_plugin()).count(),
        before.len() - usize::from(change == "close-source")
    );
    assert!(!after
        .iter()
        .any(|p| p.pane_info.terminal_command.as_deref() == Some("codex")));
    zellij.quit();
}

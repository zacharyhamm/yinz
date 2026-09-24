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
fn body(source: &str) -> String {
    let mut source = source.to_owned();
    while let Some(start) = source.find("import ") {
        let end = start + source[start..].find(';').unwrap() + 1;
        source.replace_range(start..end, "");
    }
    let source = source
        .replace(
            "export default definePlugin",
            "const shipped = definePlugin",
        )
        .replace("export const view", "const view");
    if source.contains("const shipped") {
        format!("{source}\nreturn shipped;")
    } else {
        source
    }
}
fn fixture() -> String {
    let bridge = body(zellij_config::consts::builtin_deno_plugin("swamp-bridge").unwrap());
    let exit = body(zellij_config::consts::builtin_deno_plugin("agent-exit").unwrap());
    let view = body(include_str!(
        "../../zellij-config/assets/plugins/workflow-view.ts"
    ));
    format!(
        "{}\nconst view=(()=>{{{view}\nreturn view;}})();\nconst bridge=(()=>{{{bridge}}})();\nconst hook=(()=>{{{exit}}})();\n{}",
        include_str!("swamp_exit_fixture.ts"),
        r#"
let source: number;
export default definePlugin({
  async load(config) {
    await real.waitForPermissions(['ReadApplicationState','ChangeApplicationState','RunCommands','WriteToStdin','ReadCliPipes','PluginData','ReadPaneContents','MessageAndLaunchOtherPlugins']);
    bridge.load(); config.agent_cwd='/host'; token=config.agent_exit_token;
    source=Number(config.agent_pane_id);
    hosts.push({terminal_id:source,attached:true,session_name:real.serverSession,host_id:'source-'+source,child_pid:100+source});
    hook.load(config);
  },
  update: event => hook.update(event),
  render: (rows,cols) => hook.render(rows,cols),
  async pipe(message) {
    if(message.name==='agent_subscribe') {
      real.command('MessageToPlugin',{plugin_url:null,destination_plugin_id:message.source.Plugin,plugin_config:{},message_name:'agent_snapshot',message_payload:'{"panes":[],"activity":{}}',message_args:{},new_plugin_args:null,floating_pane_coordinates:null}); return;
    }
    if(message.name==='swamp-pane-result') {
      const reply=JSON.parse(message.payload);
      if(reply.choice==='started') {
        if(reply.pane_id?.Terminal!==source) throw Error('the fork picker must report the terminal it reused');
      } else if(reply.pane_id!==null) throw Error('cancelled picker results must carry a null pane ID');
      await bridge.pipe(message);
      if(reply.choice==='cancelled' && reply.key==='fork' && (await call({method:'lookup',work:reply.work,key:reply.key})).outcome==='completed') {status='suspended';await view.refresh();}
      return;
    }
    if(message.name!=='fixture') {hook.pipe?.(message);return;}
    if(message.payload==='fork') await call({method:'pane_action',work:token,key:'fork',action:'fork'});
    else if(message.payload==='restart') real.command('RerunCommandPane',source);
    else if(message.payload==='close-source') real.command('CloseTerminalPane',source);
    else if(message.payload==='manual-close-source') {
      const uid=JSON.stringify([real.getPluginIds().host_id,real.serverSession,token,'view']);
      real.data.apply([real.data.delete('delivery',real.data.eq('uid',uid))]);
      real.command('HidePaneWithId',{Plugin:real.getPluginIds().plugin_id}); real.command('CloseTerminalPane',source);
    } else if(message.payload==='state') {
      const result=await call({method:'lookup',work:token,key:'fork'});
      real.command('CliPipeOutput',[message.source.Cli,JSON.stringify(result)]);real.command('UnblockCliPipeInput',message.source.Cli);return;
    } else if(message.payload==='status') {
      real.command('CliPipeOutput',[message.source.Cli,status]);real.command('UnblockCliPipeInput',message.source.Cli);return;
    } else if(message.payload==='agent') {
      const agent=await call({method:'agent_spawn',work:token,key:'worker',spec:{kind:'claude',cwd:'/host',allow_outside_worktree:true}});
      worker=agent.pane; emit('SessionStart');
      await call({method:'agent_prompt',work:token,key:'turn',agent:'worker',prompt:'work'});
      step='agent'; await view.refresh();
    } else if(message.payload==='test') {step='test';await view.refresh();}
    else if(message.payload==='refresh-failed') {refreshFailed=true;await view.refresh();}
    else if(message.payload==='done') {refreshFailed=false;step='choose';emit('UserPromptSubmit','work');emit('Stop');status='suspended';await view.refresh();}
    if(typeof message.source==='object' && 'Cli' in message.source) {
      real.command('CliPipeOutput',[message.source.Cli,'ok']);real.command('UnblockCliPipeInput',message.source.Cli);
    }
  },
});

"#
    )
}

fn advance(zellij: &TestSession, stage: &str) {
    assert_eq!(
        zellij.run_cli_action(CliAction::Pipe {
            name: Some("fixture".into()),
            payload: Some(stage.into()),
            args: None,
            plugin: fixture_plugin(zellij),
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
    let path = root.join("exit.ts");
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
    let overview = zellij.wait_until("workflow summary in exit pane", |s| {
        s.contains("Running: prepare") && s.contains("Cancel run") && s.contains(panel_title)
    });
    assert!(!overview.contains("LOG 100"));
    click(&zellij, "Details");
    zellij.send_stdin(b"\x1b[F");
    zellij.wait_until("workflow details expose the log tail", |s| {
        s.contains("LOG 100") && s.contains("Hide details")
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
            line.contains('')
                && ["Restart", "Fork", "Begin review", "Leave"]
                    .iter()
                    .all(|label| line.contains(label))
        })
    });
    assert!(
        question.contains("LOG 100"),
        "opened workflow details retain the log position"
    );
    let prompt_row = question
        .lines()
        .iter()
        .position(|line| line.contains("Agent exited. Choose"))
        .unwrap();
    let button_row = question
        .lines()
        .iter()
        .position(|line| line.contains('') && line.contains("Leave") && line.contains("Restart"))
        .unwrap();
    assert!(!question.contains("Duplicate"));
    assert!(
        !question.contains("Submit")
            && !question.contains("Next question")
            && !question.contains("Detach")
    );
    zellij.send_stdin(b"\x1b[H");
    let beginning = zellij.wait_until("Home scrolls logs with pinned question", |s| {
        s.contains("LOG 001") && s.lines()[prompt_row].contains("Agent exited. Choose")
    });
    assert_eq!(question.lines()[button_row], beginning.lines()[button_row]);
    zellij.send_stdin(b"\x1b[6~");
    let scrolled = zellij.wait_until("page down scrolls output with pinned question", |s| {
        !s.contains("LOG 001")
            && s.contains("↑")
            && s.lines()[prompt_row].contains("Agent exited. Choose")
    });
    assert_eq!(question.lines()[button_row], scrolled.lines()[button_row]);
    zellij.send_stdin(b"\x1b[F");
    zellij.wait_until("End reaches the log tail", |s| s.contains("LOG 100"));
    zellij.send_stdin(b"\x1b[<64;5;3M");
    zellij.wait_until("wheel scrolls log backward", |s| {
        !s.contains("LOG 100") && s.contains("↓") && s.contains("Agent exited. Choose")
    });
    zellij.send_stdin(b"\x1b[H");
    let logs = zellij.wait_until("Home restores log beginning", |s| s.contains("LOG 001"));
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
        click(&zellij, "Leave");
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
                    format!("[plugin_{}] Review topic", pane["id"].as_u64().unwrap()),
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

fn separate_workflow_fixture(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let bridge_path = root.join("bridge.ts");
    let exit_path = root.join("exit.ts");
    fs::write(
        &bridge_path,
        format!(
            "{}\nexport default plugin((()=>{{\n{}\n}})());",
            include_str!("swamp_close_fixture.ts"),
            body(zellij_config::consts::builtin_deno_plugin("swamp-bridge").unwrap())
        ),
    )
    .unwrap();
    fs::write(
        &exit_path,
        format!(
            r#"
import real, {{definePlugin}} from 'zellij:plugin';
const z: any = {{...real, projectConfig: {{read: () => ({{root: '/control', config: {{workflow: 'fixture'}}}})}},
  rpc: {{...real.rpc, call: (args: any) => real.rpc.call({{...args, pluginUrl: 'file:{}'}})}}}};
const view = (() => {{ {}
return view; }})();
const hook = (() => {{ {} }})();
const load = hook.load!;
hook.load = async config => {{
  await real.waitForPermissions(['ReadApplicationState','ChangeApplicationState','MessageAndLaunchOtherPlugins','ReadPaneContents']);
  load(config);
}};
export default hook;
"#,
            bridge_path.display(),
            body(include_str!("../../zellij-config/assets/plugins/workflow-view.ts")),
            body(zellij_config::consts::builtin_deno_plugin("agent-exit").unwrap())
        ),
    )
    .unwrap();
    (bridge_path, exit_path)
}

fn close_workflow(stage: &str) {
    let root = zellij_integration_tests::test_env::init().join("swamp-close");
    fs::create_dir_all(&root).unwrap();
    let (bridge_path, exit_path) = separate_workflow_fixture(&root);
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
        zellij.wait_until("workflow launched", |s| {
            s.contains("Running: fixture") && s.contains("Cancel run")
        });
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
    serde_json::from_str(&fixture_reply(zellij, "state")).unwrap()
}

fn fixture_plugin(zellij: &TestSession) -> Option<String> {
    panes(zellij)
        .into_iter()
        .filter_map(|pane| pane.pane_info.plugin_url)
        .find(|url| url.ends_with("/bridge.ts"))
}

fn fixture_reply(zellij: &TestSession, payload: &str) -> String {
    let (code, reply) = zellij.run_cli_action_reporting(CliAction::Pipe {
        name: Some("fixture".into()),
        payload: Some(payload.into()),
        args: None,
        plugin: fixture_plugin(zellij),
        plugin_configuration: None,
        force_launch_plugin: false,
        skip_plugin_cache: false,
        floating_plugin: None,
        in_place_plugin: None,
        plugin_cwd: None,
        plugin_title: None,
    });
    assert_eq!(code, 0);
    reply
}

/// Clicks the native form button, whose label can also appear in the focus hint.
fn click(zellij: &TestSession, label: &str) {
    let screen = zellij.wait_until("workflow action button", |s| {
        // Installing the workflow subtitle changes the content origin and height.
        // Wait for the subtitle and bottom help to render together before hit-testing.
        s.contains("fixture ·")
            && s.contains("Esc: close pane")
            && s.lines()
                .iter()
                .any(|line| line.contains('') && line.contains(label))
    });
    let (row, line) = screen
        .lines()
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains('') && line.contains(label))
        .map(|(row, line)| (row, line.clone()))
        .unwrap_or_else(|| panic!("{label} is not on screen"));
    let col = line[..line.find(label).unwrap()].chars().count();
    zellij.send_stdin(format!("\x1b[<0;{};{}M", col + 1, row + 1).as_bytes());
    zellij.send_stdin(format!("\x1b[<0;{};{}m", col + 1, row + 1).as_bytes());
}

fn fork_picker() -> (
    TestSession,
    zellij_integration_tests::FakePtyHandle,
    Vec<zellij_types::data::PaneListEntry>,
) {
    let (zellij, original, before) = running_exit_view();
    advance(&zellij, "fork");
    zellij.wait_until("fork resolves the suppressed source", |s| {
        s.contains("forking claude in fork-topic") && s.contains("Codex")
    });
    (zellij, original, before)
}

#[test]
fn restart_and_fork_are_offered_while_the_workflow_runs_and_a_started_fork_cancels_it() {
    let (mut zellij, _original, before) = running_exit_view();
    zellij.wait_until("a running step offers restart and fork", |s| {
        s.contains("Cancel run") && s.contains("Restart agent") && s.contains("Fork agent")
    });
    click(&zellij, "Fork agent");
    zellij.wait_until("the view's fork opens the picker", |s| {
        s.contains("forking claude") && s.contains("Codex")
    });
    zellij.send_stdin(&keys::ESC);
    zellij.wait_until("a cancelled picker returns to the running view", |_| {
        !panes(&zellij)
            .iter()
            .any(|p| p.pane_info.plugin_url.as_deref() == Some("zellij:hocus-new-agent"))
    });
    assert_eq!(fixture_reply(&zellij, "status"), "running");
    zellij.wait_until("the running view offers fork again", |s| {
        s.contains("Cancel run") && s.contains("Fork agent")
    });
    click(&zellij, "Fork agent");
    zellij.wait_until("a second fork opens the picker", |s| {
        s.contains("forking claude") && s.contains("Codex")
    });
    zellij.send_stdin(&keys::TAB);
    zellij.send_stdin(&keys::ENTER);
    zellij.wait_until(
        "the started fork runs in the source and cancels the run",
        |_| {
            panes(&zellij).iter().any(|p| {
                p.pane_info.id == before[0].pane_info.id
                    && !p.pane_info.exited
                    && p.pane_info.terminal_command.as_deref() == Some("codex")
            }) && fixture_reply(&zellij, "status") == "cancelled"
        },
    );
    let source = panes(&zellij)
        .into_iter()
        .find(|p| p.pane_info.id == before[0].pane_info.id)
        .unwrap();
    assert_eq!(
        source
            .pane_info
            .agent
            .and_then(|agent| agent.cwd)
            .as_deref(),
        Some("/tmp/fork-topic")
    );
    zellij.quit();
}

fn running_exit_view() -> (
    TestSession,
    zellij_integration_tests::FakePtyHandle,
    Vec<zellij_types::data::PaneListEntry>,
) {
    running_exit_view_with_args(
        "\"--model\" \"chosen\" \"--permission-mode\" \"plan\" \"original prompt\"",
    )
}

fn running_exit_view_with_args(
    args: &str,
) -> (
    TestSession,
    zellij_integration_tests::FakePtyHandle,
    Vec<zellij_types::data::PaneListEntry>,
) {
    let root = zellij_integration_tests::test_env::init().join("swamp-fork");
    fs::create_dir_all(&root).unwrap();
    let (bridge_path, exit_path) = separate_workflow_fixture(&root);
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "load_plugins clear-defaults=true {{ \"file:{}\"; }}\nhooks {{ agent_exit \"file:{}\"; }}",
            bridge_path.display(), exit_path.display()
        ))
        .with_layout(LayoutInfo::Stringified(
            format!("layout {{ pane command=\"claude\" cwd=\"/tmp/fork-topic\" focus=true {{ args {args}; }}; pane; }}"),
        ))
        .start();
    let original = zellij.expect_pty_spawn();
    zellij.expect_pty_spawn().output(b"NEIGHBOR\r\n");
    zellij.wait_until("bridge permissions", |s| s.contains("Allow? (y/n)"));
    zellij.send_stdin(b"y");
    zellij.wait_until("bridge ready", |s| !s.contains("Allow? (y/n)"));
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
    let before = panes(&zellij)
        .into_iter()
        .filter(|p| !p.pane_info.is_plugin())
        .collect();
    original.exit(Some(0));
    zellij.wait_until("hook permissions", |s| s.contains("Allow? (y/n)"));
    zellij.send_stdin(b"y");
    zellij.wait_until("exit view shows the active step", |s| {
        s.contains("Running: fixture") && s.contains("Cancel run")
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
        now.iter().filter(|p| !p.pane_info.is_plugin()).count() == before.len()
            && !now.iter().any(|p| {
                p.pane_info
                    .plugin_url
                    .as_deref()
                    .is_some_and(|url| url == "zellij:hocus-new-agent" || url.ends_with("/exit.ts"))
            })
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
fn workflow_fork_cancels_when_the_source_restarts_before_confirmation() {
    let (mut zellij, original, before) = fork_picker();
    let expected = original.run_command();
    advance(&zellij, "restart");
    zellij.wait_until("source restart records picker cancellation", |_| {
        let state = fixture_state(&zellij);
        state["outcome"] == "completed"
            && state["choice"] == "cancelled"
            && !original.is_exited()
            && !panes(&zellij)
                .iter()
                .any(|p| p.pane_info.plugin_url.as_deref() == Some("zellij:hocus-new-agent"))
    });
    let after = panes(&zellij);
    assert_eq!(
        after.iter().filter(|p| !p.pane_info.is_plugin()).count(),
        before.len()
    );
    assert!(!after
        .iter()
        .any(|p| p.pane_info.terminal_command.as_deref() == Some("codex")));
    let mut restarted = expected;
    assert_eq!(restarted.args.pop().as_deref(), Some("original prompt"));
    assert_eq!(original.run_command(), restarted);
    zellij.quit();
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

#[test]
fn workflow_restart_delivers_no_prompt_on_repeated_restarts() {
    let (mut zellij, original, _) = running_exit_view();
    let mut expected = original.run_command();
    assert_eq!(expected.args.pop().as_deref(), Some("original prompt"));
    for cycle in 0..2 {
        if cycle > 0 {
            original.exit(Some(0));
            zellij.wait_until("next workflow exit view", |s| {
                s.contains("Allow? (y/n)") || s.contains("Restart agent")
            });
            if zellij.snapshot().contains("Allow? (y/n)") {
                zellij.send_stdin(b"y");
            }
        }
        // On a repeated exit the view first attaches to the previous, cancelled run.
        // As on initial launch, wait for this run before requesting its restart.
        zellij.wait_until("new workflow is ready to restart", |s| {
            s.contains("Running: fixture")
                && s.contains("Restart agent")
                && s.contains("Cancel run")
        });
        click(&zellij, "Restart agent");
        zellij.wait_until("source process restarted", |_| !original.is_exited());
        assert_eq!(original.run_command(), expected);
        zellij.wait_until("restarted source is visible", |_| {
            panes(&zellij).iter().any(|p| {
                p.pane_info.id == zellij_types::pane_id::PaneId::Terminal(original.terminal_id())
                    && !p.pane_info.exited
                    && !p.pane_info.is_suppressed
            })
        });
        let restarted_output = format!("RESTARTED AGENT {cycle}");
        original.output(format!("{restarted_output}\r\n").as_bytes());
        zellij.wait_until("restarted agent has rendered", |s| {
            s.contains(&restarted_output)
        });
    }
    zellij.quit();
}

#[test]
fn exit_panel_restart_delivers_no_prompt_on_repeated_restarts() {
    let root = zellij_integration_tests::test_env::init().join("plain-agent-restart");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("exit.ts");
    fs::write(
        &path,
        format!(
            r#"
import real, {{definePlugin}} from 'zellij:plugin';
const z = {{...real, projectConfig: {{read: () => ({{config: {{}}}})}}}};
const view = (() => {{ {}
return view; }})();
const hook = (() => {{ {} }})();
const load = hook.load;
hook.load = async config => {{
  await real.waitForPermissions(['ReadApplicationState','ChangeApplicationState','RunCommands']);
  load(config);
}};
export default hook;
"#,
            body(include_str!(
                "../../zellij-config/assets/plugins/workflow-view.ts"
            )),
            body(zellij_config::consts::builtin_deno_plugin("agent-exit").unwrap())
        ),
    )
    .unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!("load_plugins clear-defaults=true {{}}\nhooks {{ agent_exit \"file:{}\"; }}", path.display()))
        .with_layout(LayoutInfo::Stringified(r#"layout {
            pane command="claude" cwd="/work" { args "--resume" "old-session" "--model" "chosen" "--permission-mode" "plan" "original prompt"; }
        }"#.into()))
        .start();
    let original = zellij.expect_pty_spawn();
    let mut expected = original.run_command();
    assert_eq!(expected.args.pop().as_deref(), Some("original prompt"));
    expected.args.drain(..2);
    original.output(b"AGENT READY\r\n");
    zellij.wait_until("initial agent pane is ready", |s| s.contains("AGENT READY"));
    // The hook reports a real conversation identity before exit, as an agent would.
    let payload = r#"{"hook_event_name":"SessionStart","source":"resume","session_id":"old-session","cwd":"/work"}"#;
    let actions = zellij_types::input::actions::hook_report_actions(
        "claude",
        zellij_types::pane_id::PaneId::Terminal(original.terminal_id()),
        &serde_json::from_str(payload).unwrap(),
        payload.into(),
    );
    assert_eq!(zellij.run_actions_reporting(actions).0, 0);
    assert!(panes(&zellij).iter().any(|p| p
        .pane_info
        .agent
        .as_ref()
        .is_some_and(|a| { a.session_id.as_deref() == Some("old-session") })));
    for _ in 0..2 {
        original.exit(Some(0));
        zellij.wait_until("exit panel or its permissions", |s| {
            s.contains("Allow? (y/n)") || s.contains("Enter: restart")
        });
        if zellij.snapshot().contains("Allow? (y/n)") {
            zellij.send_stdin(b"y");
        }
        zellij.wait_until("exit panel ready", |s| s.contains("Enter: restart"));
        zellij.send_stdin(&keys::ENTER);
        zellij.wait_until("source process restarted", |_| !original.is_exited());
        assert_eq!(original.run_command(), expected);
        zellij.wait_until("fresh conversation is reported", |_| {
            panes(&zellij).iter().any(|p| {
                p.pane_info.agent.as_ref().is_some_and(|a| {
                    a.state == zellij_types::agent::AgentState::Busy
                        && a.session_id.is_none()
                        && a.run_id.is_none()
                }) && !p.pane_info.exited
            })
        });
    }
    zellij.quit();
}

#[test]
fn workflow_restart_refusal_keeps_the_run_and_exit_controls_usable() {
    let (mut zellij, original, before) =
        running_exit_view_with_args("\"--print\" \"original prompt\"");
    let command = original.run_command();
    zellij.wait_until("restart control is ready", |s| s.contains("Restart agent"));
    click(&zellij, "Restart agent");
    zellij.wait_until("restart refusal is visible", |s| {
        s.contains("Cannot restart agent")
    });
    assert!(original.is_exited());
    assert_eq!(original.run_command(), command);
    assert_eq!(fixture_reply(&zellij, "status"), "running");
    let source = panes(&zellij)
        .into_iter()
        .find(|p| p.pane_info.id == before[0].pane_info.id)
        .unwrap();
    assert!(source.pane_info.is_suppressed && source.pane_info.is_held);
    assert!(source.pane_info.restart_error.unwrap().contains("--print"));
    click(&zellij, "Fork agent");
    zellij.wait_until("fork remains usable after refusal", |s| {
        s.contains("forking claude") && s.contains("Codex")
    });
    zellij.send_stdin(&keys::ESC);
    zellij.wait_until("cancelled fork restores exit controls", |s| {
        s.contains("Restart agent") && s.contains("Cancel run")
    });
    assert_eq!(fixture_reply(&zellij, "status"), "running");
    zellij.quit();
}

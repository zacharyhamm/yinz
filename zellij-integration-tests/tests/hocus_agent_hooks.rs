#![cfg(unix)]

//! The one thing the plugin's own tests cannot prove: that a hook script's
//! `zellij pipe` actually arrives at `hocus-agentd` carrying the pane it fired in.
//!
//! Everything on either side of that is unit tested — `agents::hook_phase` and the
//! status cell in `hocus-proto`, the daemon's handling in `hocus-agentd`, the settings
//! merge in `zellij hocus hooks`. What none of them can see is the seam in the middle:
//! a CLI pipe carries a `PipeSource::Cli` with nothing but a pipe id in it, so the pane
//! id has to travel as a `--args` pair, through the same `PluginUserConfiguration`
//! parser a plugin's KDL block uses. If that pair does not survive the trip, every
//! event is dropped in silence and every test above still passes.
//!
//! So the pipe comes from outside every plugin, exactly as the generated script sends
//! it, and the assertion is read off hocus's own `agent_list` on the grid.

use zellij_cli::args::CliAction;
use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};
use zellij_types::data::PaneId;
use zellij_types::input::actions::hook_report_actions;

/// hocus and the daemon, both shared, with the daemon loaded in the background as the
/// real config has it. `zellij:` rather than `file:` for the reason `hocus_store.rs`
/// gives: neither plugin asks for permissions it expects a builtin to be granted.
const HOCUS_CONFIG: &str = r#"notification_timeout 30
plugins {
  hocus location="zellij:hocus" {
    shared true
  }
  hocus-agentd location="zellij:hocus-agentd" {
    shared true
  }
}
load_plugins clear-defaults=true {
  "hocus-agentd"
}
"#;

/// A hocus pane to read the answer off, and one terminal pane to be the agent.
///
/// `focus=true` on the hocus pane because there are two of them: without it the
/// keystrokes this test sends land in the sleeping terminal instead, starting with the
/// `y` that answers the permission prompt.
///
/// The terminal pane is named rather than running anything: `agents::kind_from_title`
/// is enough to make it an agent for the daemon, and a pane that only sleeps cannot
/// print anything that would muddle the transcript below.
const HOCUS_LAYOUT: &str = r#"layout {
    pane focus=true size="70%" {
        plugin location="hocus"
    }
    pane name="claude" command="sleep" {
        args "600"
    }
}
"#;

/// Run a hocus slash command and wait for the line to be in the box before sending it.
///
/// Pasted rather than typed, for the reason `hocus_store.rs` gives: typed, the burst
/// outruns the render and the line arrives half finished.
fn submit(zellij: &TestSession, line: &str) {
    zellij.send_stdin(&keys::BRACKETED_PASTE_START);
    zellij.send_stdin(line.as_bytes());
    zellij.send_stdin(&keys::BRACKETED_PASTE_END);
    zellij.wait_until("the line is in the input box", |grid_snapshot| {
        grid_snapshot.contains(&line[line.len().saturating_sub(12)..])
    });
    zellij.send_stdin(&keys::ENTER);
}

/// One Claude Code hook event, sent the way the generated script sends it: `zellij action
/// agent-report --hook-json`, which sets the pane's record and sends the event, typed, to
/// every plugin subscribed to `agent_event` - the daemon among them.
///
/// The actions are built here rather than through `CliAction::AgentReport { hook_json: true }`
/// because that reads the event off stdin, which an in-process client cannot be handed.
fn send_hook(zellij: &mut TestSession, pane_id: u32, payload: &str) {
    let event = serde_json::from_str(payload).expect("the test's own hook event parses");
    let actions = hook_report_actions(
        "claude",
        PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(pane_id)),
        &event,
        payload.to_owned(),
    );
    let (exit_code, reported) = zellij.run_actions_reporting(actions);
    assert_eq!(exit_code, 0, "{reported}");
}

/// Clear hocus's transcript, so the next answer is the only thing that can put a word
/// back on the screen.
fn clear(zellij: &TestSession, gone: &str) {
    zellij.send_stdin(&[0x0c]); // Ctrl-l
    zellij.wait_until("the transcript is cleared", |grid_snapshot| {
        !grid_snapshot.contains(gone)
    });
}

#[test]
fn hooks_keep_the_pane_record_and_hocus_waiting_status_in_agreement() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();

    zellij.wait_until("hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    zellij.wait_until("the prompt is dismissed", |grid_snapshot| {
        !grid_snapshot.contains("Allow? (y/n)")
    });

    // The daemon has to have seen the pane before a hook about it can mean anything,
    // and this is also where the pane's id is learned: nothing else in the test knows
    // what the layout's one terminal was numbered.
    submit(&zellij, r#"/agent_list {}"#);
    zellij.wait_until("the daemon has found the agent pane", |grid_snapshot| {
        grid_snapshot.contains("agents")
    });
    let listed = zellij.snapshot().to_string();
    let pane_id = agent_pane_id(&listed);
    assert!(
        !listed.contains("waiting"),
        "nothing has reported anything yet:\n{listed}"
    );

    clear(&zellij, "agents");

    let transcripts = tempfile::tempdir().unwrap();
    let transcript = transcripts.path().join("codex.jsonl");
    std::fs::write(&transcript, concat!(
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-session\"}}\n",
        "{\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"codex-turn\",\"approvals_reviewer\":\"auto_review\"}}\n",
    )).unwrap();
    let codex_permission = serde_json::json!({
        "hook_event_name": "PermissionRequest", "permission_mode": "default",
        "session_id": "codex-session", "turn_id": "codex-turn", "transcript_path": transcript,
        "tool_name": "Bash", "tool_input": {"command": "git status"}
    })
    .to_string();
    for kind in ["claude", "codex"] {
        for payload in [
            if kind == "codex" {
                codex_permission.as_str()
            } else {
                r#"{"hook_event_name":"PermissionRequest","permission_mode":"auto","tool_name":"Bash","tool_input":{"command":"git status"}}"#
            },
            r#"{"hook_event_name":"Notification","notification_type":"permission_prompt"}"#,
            r#"{"hook_event_name":"Notification","notification_type":"worker_permission_prompt"}"#,
        ] {
            // The same host-side parser as local CLI ingestion; the in-process client cannot
            // supply stdin to `--hook-json`.
            let (event, payload) =
                zellij_runtime::agent::hooks::parse_hook_event(kind, payload.to_owned()).unwrap();
            let actions = hook_report_actions(
                kind,
                PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(pane_id)),
                &event,
                payload,
            );
            let (exit_code, output) = zellij.run_actions_reporting(actions);
            assert_eq!(exit_code, 0, "{output}");
            let (exit_code, listed) = zellij.run_cli_action_reporting(CliAction::ListPanes {
                tab: false,
                command: false,
                state: true,
                geometry: false,
                all: true,
                json: true,
            });
            assert_eq!(exit_code, 0, "{listed}");
            let panes: serde_json::Value = serde_json::from_str(&listed).unwrap();
            let pane = panes
                .as_array()
                .unwrap()
                .iter()
                .find(|pane| pane["id"] == pane_id && pane["is_plugin"] == false)
                .unwrap();
            assert_eq!(pane["agent"]["state"], "Busy");
            assert_eq!(pane["agent"]["detail"], "Bash git");
            assert!(pane["agent"]["waiting_on"].is_null());
            assert_eq!(pane["agent"]["wait_needs_a_person"], false);
            submit(&zellij, r#"/agent_list {}"#);
            zellij.wait_until("hocus agrees that automatic approval is busy", |grid| {
                grid.contains("busy") && grid.contains("Bash git")
            });
            // The daemon publishes its list after processing the event and deciding whether
            // to notify. A persistent toast would therefore be visible with this busy result.
            assert!(!zellij.snapshot().to_string().contains("waiting"));
            clear(&zellij, "agents");
        }
    }

    // The message the generated hook script sends, and the only thing in this session
    // that can put the word "waiting" on the screen.
    send_hook(
        &mut zellij,
        pane_id,
        r#"{"hook_event_name":"PermissionRequest","session_id":"cc-1"}"#,
    );
    zellij.wait_until("a genuine wait still raises a notification", |grid| {
        grid.contains("agent waiting")
    });

    submit(&zellij, r#"/agent_list {}"#);
    zellij.wait_until(
        "hocus reports the agent as blocked on the user",
        |grid_snapshot| grid_snapshot.contains("waiting"),
    );
}

/// The pane id `agent_list` drew, off the grid.
///
/// The list is presented rather than printed as JSON: hocus renders the markdown, so
/// one agent is a line reading `• 0 running — claude`. This takes the number off the
/// first such line.
fn agent_pane_id(screen: &str) -> u32 {
    screen
        .lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix('\u{2022}')?;
            rest.split_whitespace().next()?.parse().ok()
        })
        .unwrap_or_else(|| panic!("agent_list listed no pane:\n{screen}"))
}

#[test]
fn assistant_messages_reach_the_timeline_and_history_search() {
    let mut size = TERMINAL_SIZE;
    size.rows = 70;
    let mut zellij = TestRunner::new(size)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();
    zellij.wait_until("hocus asks for permissions", |grid| {
        grid.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    zellij.wait_until("the permission prompt is dismissed", |grid| {
        !grid.contains("Allow? (y/n)")
    });
    submit(&zellij, "/agent_list {}");
    zellij.wait_until("the daemon found the pane", |grid| grid.contains("agents"));
    let pane_id = agent_pane_id(&zellij.snapshot().to_string());
    clear(&zellij, "agents");
    for payload in [
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"conversation","prompt":"Inspect the parser"}"#,
        r#"{"hook_event_name":"MessageDisplay","session_id":"conversation","turn_id":"turn","message_id":"commentary","index":0,"final":true,"delta":"Checking the lexer"}"#,
        r#"{"hook_event_name":"PreToolUse","session_id":"conversation","tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Keep Unicode?"}]}}"#,
        r#"{"hook_event_name":"MessageDisplay","session_id":"conversation","turn_id":"turn","message_id":"final","index":0,"final":true,"delta":"Parser verified with Unicode"}"#,
        r#"{"hook_event_name":"Stop","session_id":"conversation","turn_id":"turn","last_assistant_message":"Parser verified with Unicode"}"#,
    ] {
        send_hook(&mut zellij, pane_id, payload);
    }
    submit(&zellij, &format!("/agent_run {{\"pane_id\":{pane_id}}}"));
    zellij.wait_until("the conversation is visible", |grid| {
        grid.contains("Inspect the parser")
            && grid.contains("Checking the lexer")
            && grid.contains("Keep Unicode?")
            && grid.contains("Parser verified with Unicode")
            && grid.contains("4 of 4 entries")
    });
    clear(&zellij, "4 of 4 entries");
    send_hook(
        &mut zellij,
        pane_id,
        r#"{"hook_event_name":"SessionEnd","session_id":"conversation"}"#,
    );
    submit(&zellij, r#"/agent_history_search {"query":"lexer"}"#);
    zellij.wait_until("archived assistant commentary is searchable", |grid| {
        grid.contains("Checking the lexer") && grid.contains("message")
    });
    clear(&zellij, "Checking the lexer");
    // Replay after persistence, as duplicate installed hooks can do. The completed
    // message has the same source identity as MessageDisplay above.
    send_hook(
        &mut zellij,
        pane_id,
        r#"{"hook_event_name":"SessionEnd","session_id":"conversation","assistant_messages":[{"source_id":"conversation:final","turn_id":"turn","text":"Parser verified with Unicode"}]}"#,
    );
    submit(&zellij, r#"/agent_run {"run_id":"claude:conversation"}"#);
    zellij.wait_until(
        "a duplicate close preserves the archived conversation",
        |grid| {
            grid.contains("prompt")
                && grid.contains("Inspect the parser")
                && grid.contains("Checking the lexer")
                && grid.contains("Keep Unicode?")
                && grid.contains("Parser verified with Unicode")
        },
    );
}

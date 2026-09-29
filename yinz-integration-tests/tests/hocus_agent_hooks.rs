#![cfg(unix)]

//! The one thing the plugin's own tests cannot prove: that a hook script's
//! `yinz pipe` actually arrives at `hocus-agentd` carrying the pane it fired in.
//!
//! Everything on either side of that is unit tested — `agents::hook_phase` and the
//! status cell in `hocus-proto`, the daemon's handling in `hocus-agentd`, the settings
//! merge in `yinz hocus hooks`. What none of them can see is the seam in the middle:
//! a CLI pipe carries a `PipeSource::Cli` with nothing but a pipe id in it, so the pane
//! id has to travel as a `--args` pair, through the same `PluginUserConfiguration`
//! parser a plugin's KDL block uses. If that pair does not survive the trip, every
//! event is dropped in silence and every test above still passes.
//!
//! So the pipe comes from outside every plugin, exactly as the generated script sends
//! it, and the assertion is read off hocus's own `agent_list` on the grid.

use yinz_cli::args::CliAction;
use yinz_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};
use yinz_types::data::PaneId;
use yinz_types::input::actions::hook_report_actions;

/// hocus and the daemon, both shared, with the daemon loaded in the background as the
/// real config has it. `yinz:` rather than `file:` for the reason `hocus_store.rs`
/// gives: neither plugin asks for permissions it expects a builtin to be granted.
const HOCUS_CONFIG: &str = r#"notification_timeout 30
plugins {
  hocus location="yinz:hocus" {
    shared true
  }
  hocus-store location="yinz:hocus-store"
  hocus-agentd location="yinz:hocus-agentd" {
    shared true
  }
}
load_plugins clear-defaults=true {
  "hocus-agentd"
  "hocus-store"
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
fn submit(yinz: &TestSession, line: &str) {
    yinz.send_stdin(&keys::BRACKETED_PASTE_START);
    yinz.send_stdin(line.as_bytes());
    yinz.send_stdin(&keys::BRACKETED_PASTE_END);
    yinz.wait_until("the line is in the input box", |grid_snapshot| {
        grid_snapshot.contains(&line[line.len().saturating_sub(12)..])
    });
    yinz.send_stdin(&keys::ENTER);
}

/// One Claude Code hook event, sent the way the generated script sends it: `yinz action
/// agent-report --hook-json`, which sets the pane's record and sends the event, typed, to
/// every plugin subscribed to `agent_event` - the daemon among them.
///
/// The actions are built here rather than through `CliAction::AgentReport { hook_json: true }`
/// because that reads the event off stdin, which an in-process client cannot be handed.
fn send_hook(yinz: &mut TestSession, pane_id: u32, payload: &str) {
    let event = serde_json::from_str(payload).expect("the test's own hook event parses");
    let actions = hook_report_actions(
        "claude",
        PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(pane_id)),
        &event,
        payload.to_owned(),
    );
    let (exit_code, reported) = yinz.run_actions_reporting(actions);
    assert_eq!(exit_code, 0, "{reported}");
}

/// The pane's agent record off `list-panes --json`.
fn pane_agent(yinz: &mut TestSession, pane_id: u32) -> serde_json::Value {
    let (exit_code, listed) = yinz.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: false,
        all: true,
        json: true,
    });
    assert_eq!(exit_code, 0, "{listed}");
    let panes: serde_json::Value = serde_json::from_str(&listed).unwrap();
    panes
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["id"] == pane_id && pane["is_plugin"] == false)
        .expect("the agent pane is in the manifest")["agent"]
        .clone()
}

/// One opencode event, translated on the way in exactly as the installed plugin's payload is,
/// then reported the way the plugin's `yinz action agent-report` reports it.
fn send_opencode_hook(yinz: &mut TestSession, pane_id: u32, native: &str) {
    let (event, payload) =
        yinz_runtime::agent::hooks::parse_hook_event("opencode", native.to_owned())
            .expect("the test's own opencode event translates");
    let actions = hook_report_actions(
        "opencode",
        PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(pane_id)),
        &event,
        payload,
    );
    let (exit_code, reported) = yinz.run_actions_reporting(actions);
    assert_eq!(exit_code, 0, "{reported}");
}

/// Clear hocus's transcript, so the next answer is the only thing that can put a word
/// back on the screen.
fn clear(yinz: &TestSession, gone: &str) {
    yinz.send_stdin(&[0x0c]); // Ctrl-l
    yinz.wait_until("the transcript is cleared", |grid_snapshot| {
        !grid_snapshot.contains(gone)
    });
}

#[test]
fn hooks_keep_the_pane_record_and_hocus_waiting_status_in_agreement() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();

    yinz.wait_until("hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    yinz.send_stdin(b"y");
    yinz.wait_until("the prompt is dismissed", |grid_snapshot| {
        !grid_snapshot.contains("Allow? (y/n)")
    });

    // The daemon has to have seen the pane before a hook about it can mean anything,
    // and this is also where the pane's id is learned: nothing else in the test knows
    // what the layout's one terminal was numbered.
    submit(&yinz, r#"/agent_list {}"#);
    yinz.wait_until("the daemon has found the agent pane", |grid_snapshot| {
        grid_snapshot.contains("agents")
    });
    let listed = yinz.snapshot().to_string();
    let pane_id = agent_pane_id(&mut yinz);
    assert!(
        !listed.contains("waiting"),
        "nothing has reported anything yet:\n{listed}"
    );

    clear(&yinz, "agents");

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
                yinz_runtime::agent::hooks::parse_hook_event(kind, payload.to_owned()).unwrap();
            let actions = hook_report_actions(
                kind,
                PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(pane_id)),
                &event,
                payload,
            );
            let (exit_code, output) = yinz.run_actions_reporting(actions);
            assert_eq!(exit_code, 0, "{output}");
            let (exit_code, listed) = yinz.run_cli_action_reporting(CliAction::ListPanes {
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
            submit(&yinz, r#"/agent_list {}"#);
            yinz.wait_until("hocus agrees that automatic approval is busy", |grid| {
                grid.contains("busy") && grid.contains("Bash git")
            });
            // The daemon publishes its list after processing the event and deciding whether
            // to notify. A persistent toast would therefore be visible with this busy result.
            assert!(!yinz.snapshot().to_string().contains("waiting"));
            clear(&yinz, "agents");
        }
    }

    // The message the generated hook script sends, and the only thing in this session
    // that can put the word "waiting" on the screen.
    send_hook(
        &mut yinz,
        pane_id,
        r#"{"hook_event_name":"PermissionRequest","session_id":"cc-1"}"#,
    );
    yinz.wait_until("a genuine wait still raises a notification", |grid| {
        grid.contains("agent waiting")
    });

    submit(&yinz, r#"/agent_list {}"#);
    yinz.wait_until(
        "hocus reports the agent as blocked on the user",
        |grid_snapshot| grid_snapshot.contains("waiting"),
    );
}

// These fixtures run sleep with an agent title. Discover the general pane before
// sending its first provider hook; inferred titles are not authoritative agent records.
fn agent_pane_id(yinz: &mut TestSession) -> u32 {
    let (code, output) = yinz.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: false,
        all: true,
        json: true,
    });
    assert_eq!(code, 0, "{output}");
    let panes: serde_json::Value = serde_json::from_str(&output).unwrap();
    panes
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["is_plugin"] == false)
        .unwrap()["id"]
        .as_u64()
        .unwrap() as u32
}

#[test]
fn assistant_messages_reach_the_timeline_and_history_search() {
    let mut size = TERMINAL_SIZE;
    size.rows = 70;
    let mut yinz = TestRunner::new(size)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();
    yinz.wait_until("hocus asks for permissions", |grid| {
        grid.contains("Allow? (y/n)")
    });
    yinz.send_stdin(b"y");
    yinz.wait_until("the permission prompt is dismissed", |grid| {
        !grid.contains("Allow? (y/n)")
    });
    submit(&yinz, "/agent_list {}");
    yinz.wait_until("the daemon found the pane", |grid| grid.contains("agents"));
    let pane_id = agent_pane_id(&mut yinz);
    clear(&yinz, "agents");
    for payload in [
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"conversation","prompt":"Inspect the parser"}"#,
        r#"{"hook_event_name":"MessageDisplay","session_id":"conversation","turn_id":"turn","message_id":"commentary","index":0,"final":true,"delta":"Checking the lexer"}"#,
        r#"{"hook_event_name":"PreToolUse","session_id":"conversation","tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Keep Unicode?"}]}}"#,
        r#"{"hook_event_name":"MessageDisplay","session_id":"conversation","turn_id":"turn","message_id":"final","index":0,"final":true,"delta":"Parser verified with Unicode"}"#,
        r#"{"hook_event_name":"Stop","session_id":"conversation","turn_id":"turn","last_assistant_message":"Parser verified with Unicode"}"#,
    ] {
        send_hook(&mut yinz, pane_id, payload);
    }
    submit(&yinz, &format!("/agent_run {{\"pane_id\":{pane_id}}}"));
    yinz.wait_until("the conversation is visible", |grid| {
        grid.contains("Inspect the parser")
            && grid.contains("Checking the lexer")
            && grid.contains("Keep Unicode?")
            && grid.contains("Parser verified with Unicode")
            && grid.contains("4 of 4 entries")
    });
    clear(&yinz, "4 of 4 entries");
    send_hook(
        &mut yinz,
        pane_id,
        r#"{"hook_event_name":"SessionEnd","session_id":"conversation"}"#,
    );
    submit(&yinz, r#"/agent_history_search {"query":"lexer"}"#);
    yinz.wait_until("archived assistant commentary is searchable", |grid| {
        grid.contains("Checking the lexer") && grid.contains("message")
    });
    clear(&yinz, "Checking the lexer");
    // Replay after persistence, as duplicate installed hooks can do. The completed
    // message has the same source identity as MessageDisplay above.
    send_hook(
        &mut yinz,
        pane_id,
        r#"{"hook_event_name":"SessionEnd","session_id":"conversation","assistant_messages":[{"source_id":"conversation:final","turn_id":"turn","text":"Parser verified with Unicode"}]}"#,
    );
    submit(&yinz, r#"/agent_run {"run_id":"claude:conversation"}"#);
    yinz.wait_until(
        "a duplicate close preserves the archived conversation",
        |grid| {
            grid.contains("prompt")
                && grid.contains("Inspect the parser")
                && grid.contains("Checking the lexer")
                && grid.contains("Keep Unicode?")
                && grid.contains("Parser verified with Unicode")
        },
    );
    clear(&yinz, "Parser verified with Unicode");
    send_hook(
        &mut yinz,
        pane_id,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"replacement","prompt":"Review the replacement session"}"#,
    );
    submit(&yinz, r#"/agent_run {"run_id":"claude:replacement"}"#);
    yinz.wait_until("the replacement run is active", |grid| {
        grid.contains("Review the replacement session")
            && grid.contains("active")
            && grid.contains("1 of 1 entries")
    });
    clear(&yinz, "1 of 1 entries");
    // The original run is now only in storage. A delayed close must recover its
    // entries and preserve new closing words without closing the replacement.
    let delayed_end = r#"{"hook_event_name":"SessionEnd","session_id":"conversation","assistant_messages":[{"source_id":"conversation:final","turn_id":"turn","text":"Parser verified with Unicode"},{"source_id":"conversation:closing","turn_id":"turn","text":"Final parser handoff recorded"}]}"#;
    for _ in 0..2 {
        send_hook(&mut yinz, pane_id, delayed_end);
        submit(&yinz, r#"/agent_run {"run_id":"claude:conversation"}"#);
        yinz.wait_until(
            "the delayed close adds exactly one archived message",
            |grid| {
                grid.contains("Final parser handoff recorded")
                    && grid.contains("Checking the lexer")
                    // An asynchronous summary can add a sixth entry. Only the
                    // three assistant messages are controlled by these hook events.
                    && grid.to_string().matches("message ·").count() == 3
            },
        );
        clear(&yinz, "Final parser handoff recorded");
    }
    submit(&yinz, r#"/agent_run {"run_id":"claude:replacement"}"#);
    yinz.wait_until("the delayed close leaves the replacement active", |grid| {
        grid.contains("Review the replacement session")
            && grid.contains("active")
            && grid.contains("1 of 1 entries")
    });
}

#[test]
fn delayed_daemon_events_do_not_overwrite_newer_pane_state() {
    let mut size = TERMINAL_SIZE;
    size.cols = 180;
    let layout = r#"layout {
        pane focus=true size="50%" { plugin location="hocus"; }
        pane name="claude" command="sleep" { args "600"; }
        pane size=1 borderless=true { plugin location="yinz:status-bar"; }
    }"#
    .to_owned();
    let mut yinz = TestRunner::new(size)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    yinz.wait_until("hocus asks for permissions", |grid| {
        grid.contains("Allow? (y/n)")
    });
    yinz.send_stdin(b"y");
    yinz.wait_until("permissions granted", |grid| !grid.contains("Allow? (y/n)"));
    submit(&yinz, "/agent_list {}");
    yinz.wait_until("the initial agent list returned", |grid| {
        grid.contains("agents")
    });
    let pane_id = agent_pane_id(&mut yinz);
    clear(&yinz, "•");

    for (hook, expected, mark) in [
        ("UserPromptSubmit", "Busy", "⣾1"),
        ("Stop", "Idle", "·1"),
        ("PermissionRequest", "Waiting", "◆1"),
    ] {
        send_hook(
            &mut yinz,
            pane_id,
            &serde_json::json!({
                "hook_event_name": hook, "session_id": "current", "tool_name": "Bash",
            })
            .to_string(),
        );
        assert_eq!(pane_agent(&mut yinz, pane_id)["state"], expected);

        // Model a daemon lagging behind the screen: its old Stop arrives after the
        // newer record. Only deliver the broadcast; the screen already applied it earlier.
        let payload = r#"{"hook_event_name":"Stop","session_id":"current"}"#;
        let event = serde_json::from_str(payload).unwrap();
        let actions = hook_report_actions(
            "claude",
            PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(pane_id)),
            &event,
            payload.to_owned(),
        )
        .into_iter()
        .filter(|action| matches!(action, yinz_types::input::actions::Action::AgentEvent(_)))
        .collect();
        let (code, output) = yinz.run_actions_reporting(actions);
        assert_eq!(code, 0, "{output}");
        submit(&yinz, "/agent_list {}");
        yinz.wait_until("the authoritative list retains the current state", |grid| {
            grid.contains("•") && grid.contains(&expected.to_lowercase())
        });
        assert_eq!(pane_agent(&mut yinz, pane_id)["state"], expected);
        yinz.wait_until("the status bar retains the current state", |grid| {
            // Busy animates, so accept every frame of its mark.
            if expected == "Busy" {
                ["⣾1", "⣽1", "⣻1", "⢿1", "⡿1", "⣟1", "⣯1", "⣷1"]
                    .iter()
                    .any(|mark| grid.contains(mark))
            } else {
                grid.contains(mark)
            }
        });
        clear(&yinz, "•");
    }
}

#[test]
fn background_workers_preserve_the_parent_record_and_idle_count() {
    let mut size = TERMINAL_SIZE;
    size.rows = 70;
    size.cols = 180;
    let layout = r#"layout {
        pane focus=true size="50%" { plugin location="hocus"; }
        pane name="claude" command="sleep" { args "600"; }
        pane size=1 borderless=true { plugin location="yinz:status-bar"; }
    }"#
    .to_owned();
    let config = HOCUS_CONFIG.replace(
        "plugins {",
        "plugins {\n  hocus-agents location=\"yinz:hocus-agents\" { shared true; }",
    );
    let config = format!(
        r#"{config}
keybinds {{
  normal {{
    bind "Ctrl b" {{
      LaunchOrFocusPlugin "hocus" {{
        floating true; overlay true; awaits_placement true; move_to_focused_tab true;
      }}
    }}
    bind "Ctrl a" {{
      LaunchOrFocusPlugin "hocus-agents" {{
        floating true; overlay true; awaits_placement true; move_to_focused_tab true;
      }}
    }}
  }}
}}
"#
    );
    let mut yinz = TestRunner::new(size)
        .with_config(&config)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    yinz.wait_until("hocus asks for permissions", |grid| {
        grid.contains("Allow? (y/n)")
    });
    yinz.send_stdin(b"y");
    yinz.wait_until("permissions granted", |grid| !grid.contains("Allow? (y/n)"));
    submit(&yinz, "/agent_list {}");
    yinz.wait_until("the initial agent list returned", |grid| {
        grid.contains("agents")
    });
    let pane_id = agent_pane_id(&mut yinz);
    clear(&yinz, "•");

    let record = |yinz: &mut TestSession| {
        let (code, output) = yinz.run_cli_action_reporting(CliAction::ListPanes {
            tab: false,
            command: false,
            state: true,
            geometry: false,
            all: true,
            json: true,
        });
        assert_eq!(code, 0, "{output}");
        let panes: serde_json::Value = serde_json::from_str(&output).unwrap();
        panes
            .as_array()
            .unwrap()
            .iter()
            .find(|pane| pane["id"] == pane_id && pane["is_plugin"] == false)
            .unwrap()["agent"]
            .clone()
    };
    for (parent, expected) in [
        ("UserPromptSubmit", "Busy"),
        ("Stop", "Idle"),
        ("PreToolUse", "Busy"),
        ("PermissionRequest", "Waiting"),
        ("Stop", "Idle"),
    ] {
        send_hook(
            &mut yinz,
            pane_id,
            &serde_json::json!({
                "hook_event_name": parent, "session_id": "recap-parent", "cwd": "/work",
                "tool_name": "Bash", "tool_input": {"command": "git status"},
            })
            .to_string(),
        );
        submit(&yinz, "/agent_list {}");
        yinz.wait_until("the daemon processes the parent event", |grid| {
            grid.contains("•") && grid.contains(&expected.to_lowercase())
        });
        clear(&yinz, "•");
        let before = record(&mut yinz);
        assert_eq!(before["state"], expected);
        for worker in ["SubagentStart", "SubagentStop"] {
            send_hook(
                &mut yinz,
                pane_id,
                &serde_json::json!({
                    "hook_event_name": worker, "session_id": "recap-parent",
                    "agent_id": "recap", "last_assistant_message": "Recap completed",
                })
                .to_string(),
            );
            assert_eq!(record(&mut yinz), before, "{parent} then {worker}");
            if expected == "Idle" {
                yinz.wait_until("the status bar retains one idle agent", |grid| {
                    grid.contains("·1") || grid.contains("1i")
                });
            }
        }
    }
    submit(&yinz, "/agent_list {}");
    yinz.wait_until("the daemon preserves the idle parent", |grid| {
        grid.contains("idle")
    });
    clear(&yinz, "•");
    submit(&yinz, &format!("/agent_run {{\"pane_id\":{pane_id}}}"));
    yinz.wait_until("the recap is recorded once", |grid| {
        grid.contains("Recap completed") && grid.to_string().matches("Recap completed").count() == 1
    });
    // Open the paired overlays through their normal placement controls; an unplaced
    // floating conversation can otherwise cover the agent row this test inspects.
    yinz.send_stdin(&keys::ctrl('b'));
    yinz.wait_until("the conversation overlay is placed", |grid| {
        grid.contains("Ask something.")
    });
    yinz.send_stdin(&keys::ctrl('a'));
    yinz.wait_until("the agents panel retains the idle parent", |grid| {
        grid.to_string()
            .lines()
            .any(|line| line.contains("claude") && line.contains("idle"))
    });
    send_hook(
        &mut yinz,
        pane_id,
        r#"{"hook_event_name":"SessionEnd","session_id":"recap-parent"}"#,
    );
    assert!(record(&mut yinz).is_null());
    yinz.wait_until("the ended agent leaves active counts", |grid| {
        !grid.contains("·1") && !grid.contains("1i")
    });
}

/// opencode has no shell hook to run, so the seam this proves is the one it has instead: the
/// Rust translation (`yinz-types/src/agent/opencode.rs`) reading opencode's own event shape
/// into the same `HookEvent` the other two agents write, through the real session.
#[test]
fn opencode_events_report_the_pane_and_a_question_waits() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();
    yinz.wait_until("hocus asks for permissions", |grid| {
        grid.contains("Allow? (y/n)")
    });
    yinz.send_stdin(b"y");
    yinz.wait_until("the prompt is dismissed", |grid| {
        !grid.contains("Allow? (y/n)")
    });
    submit(&yinz, r#"/agent_list {}"#);
    yinz.wait_until("the daemon has found the agent pane", |grid| {
        grid.contains("agents")
    });
    let pane_id = agent_pane_id(&mut yinz);
    clear(&yinz, "agents");

    // A prompt, then a tool starting: busy, naming the tool.
    send_opencode_hook(
        &mut yinz,
        pane_id,
        r#"{"type":"chat.message","properties":{"sessionID":"ses_1","cwd":"/work","prompt":"fix the parser"}}"#,
    );
    send_opencode_hook(
        &mut yinz,
        pane_id,
        r#"{"type":"tool.execute.before","properties":{"sessionID":"ses_1","cwd":"/work","callID":"call_1","tool":"bash","args":{"command":"cargo build"}}}"#,
    );
    let busy = pane_agent(&mut yinz, pane_id);
    assert_eq!(busy["kind"], "opencode");
    assert_eq!(busy["state"], "Busy");
    assert_eq!(busy["detail"], "Bash cargo");
    assert_eq!(busy["cwd"], "/work");

    // opencode's question tool is the wait, and the toast is the answer a person sees.
    send_opencode_hook(
        &mut yinz,
        pane_id,
        r#"{"type":"tool.execute.before","properties":{"sessionID":"ses_1","cwd":"/work","callID":"call_2","tool":"question","args":{"questions":[{"question":"Which library should we use?"}]}}}"#,
    );
    yinz.wait_until("opencode's question raises the waiting toast", |grid| {
        grid.contains("agent waiting")
    });
    let waiting = pane_agent(&mut yinz, pane_id);
    assert_eq!(waiting["state"], "Waiting");
    assert_eq!(waiting["waiting_on"], "Which library should we use?");

    // The turn ends, and the record goes back to idle.
    send_opencode_hook(
        &mut yinz,
        pane_id,
        r#"{"type":"session.idle","properties":{"sessionID":"ses_1","cwd":"/work","lastAssistantMessage":"Done."}}"#,
    );
    assert_eq!(pane_agent(&mut yinz, pane_id)["state"], "Idle");
}

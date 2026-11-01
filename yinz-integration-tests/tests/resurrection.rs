#![cfg(unix)]

use insta::assert_snapshot;
use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    keys, normalized, LayoutInfo, TestRunner, TestSession, PROMPT, TERMINAL_SIZE,
};

const RESURRECT_LAYOUT: &str = r#"
layout {
    default_tab_template {
        pane size=1 borderless=true {
            plugin location="tab-bar"
        }
        children
        pane size=1 borderless=true {
            plugin location="status-bar"
        }
    }
    tab name="alpha" {
        pane
    }
    tab name="beta" focus=true {
        pane
    }
}
"#;

fn start_serializing_session(extra_config: &str) -> TestSession {
    TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!("session_serialization true\n{}", extra_config))
        .with_layout(LayoutInfo::Stringified(RESURRECT_LAYOUT.to_string()))
        .start()
}

fn wait_for_layout_loaded(yinz: &TestSession) {
    yinz.wait_until("layout loaded with both tabs", |grid_snapshot| {
        grid_snapshot.contains("alpha")
            && grid_snapshot.contains("beta")
            && grid_snapshot.status_bar_appears()
    });
}

#[test]
fn quit_and_resurrect_session() {
    let mut yinz = start_serializing_session("");
    wait_for_layout_loaded(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('r'));
    let new_pane = yinz.expect_pty_spawn();
    new_pane.output(PROMPT);
    yinz.wait_until("new pane opened before serialization", |grid_snapshot| {
        grid_snapshot.contains("│")
    });

    yinz.save_session();
    yinz.wait_for_serialized_session();
    yinz.quit();

    yinz.resurrect(TERMINAL_SIZE);
    let grid_snapshot = yinz.wait_until(
        "resurrected session restored the runtime pane",
        |grid_snapshot| {
            grid_snapshot.contains("Yinz (test")
                && grid_snapshot.contains("alpha")
                && grid_snapshot.contains("beta")
                && grid_snapshot.contains("│")
                && grid_snapshot.status_bar_appears()
        },
    );
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn quit_and_resurrect_session_with_viewport_serialization() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("session_serialization true\nserialize_pane_viewport true")
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(b"VIEWPORT_CONTENT_TO_RESTORE\n");
    yinz.wait_until(
        "pane content rendered before serialization",
        |grid_snapshot| {
            grid_snapshot.contains("VIEWPORT_CONTENT_TO_RESTORE")
                && grid_snapshot.status_bar_appears()
        },
    );

    yinz.save_session();
    yinz.wait_for_serialized_session();
    yinz.quit();

    yinz.resurrect(TERMINAL_SIZE);
    let grid_snapshot =
        yinz.wait_until("resurrected viewport and chrome settled", |grid_snapshot| {
            grid_snapshot.contains("VIEWPORT_CONTENT_TO_RESTORE")
                && grid_snapshot.contains("Yinz (test")
                && grid_snapshot.tab_bar_appears()
                && grid_snapshot.status_bar_appears()
        });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn saved_agent_identity_survives_repeated_resurrection() {
    let layout = r#"
layout {
    tab name="agents" {
        pane command="claude" {
            agent "{\"kind\":\"claude\",\"state\":\"Idle\",\"session_id\":\"saved-conversation\",\"cwd\":\"/tmp\"}"
        }
    }
}
"#;
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("session_serialization true")
        .with_layout(LayoutInfo::Stringified(layout.to_owned()))
        .start();
    for generation in 0..3 {
        let pane = yinz.expect_pty_spawn();
        let command = pane.run_command();
        assert_eq!(
            command.resume_agent.as_ref().unwrap().session_id.as_deref(),
            Some("saved-conversation")
        );
        assert!(!command.hold_on_start);
        let marker = format!("RESTORED_AGENT_{generation}");
        pane.output(marker.as_bytes());
        yinz.wait_until("restored agent rendered", |screen| screen.contains(&marker));
        yinz.save_session();
        yinz.wait_for_serialized_session();
        yinz.quit();
        if generation < 2 {
            yinz.resurrect(TERMINAL_SIZE);
        }
    }
}

#[test]
fn restored_agent_that_exited_is_not_resumed_again() {
    let layout = r#"
layout {
    tab name="agents" {
        pane command="claude" {
            agent "{\"kind\":\"claude\",\"state\":\"Idle\",\"session_id\":\"saved-conversation\",\"cwd\":\"/tmp\"}"
        }
    }
}
"#;
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        // Leave the held pane in view rather than covered by the agent-exit prompt.
        .with_config("session_serialization true\nhooks { agent_exit \"\"; }")
        .with_layout(LayoutInfo::Stringified(layout.to_owned()))
        .start();
    let pane = yinz.expect_pty_spawn();
    assert!(pane.run_command().resume_agent.is_some());
    pane.exit(Some(0));
    yinz.wait_until("agent exit is held", |screen| {
        screen.contains("EXIT CODE: 0")
    });

    yinz.save_session();
    let saved = yinz.wait_for_serialized_layout("exited agent saved", |_| true);
    assert!(!saved.contains("session_id"), "{saved}");
    assert!(saved.contains("start_suspended"), "{saved}");
    yinz.quit();

    // Suspended: a resumed agent would spawn and never show this.
    yinz.resurrect(TERMINAL_SIZE);
    yinz.wait_until("restored pane waits to be run", |screen| {
        screen.contains("<ENTER>") && !screen.contains("EXIT CODE")
    });
    yinz.quit();
}

#[test]
fn agent_gone_from_a_clean_session_leaves_the_saved_layout() {
    // The default layout: nothing but the agent separates this session from what it started as.
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            "session_serialization true\nserialization_interval 1\ndisable_session_metadata false",
        )
        .start();
    let pane = yinz.expect_pty_spawn();
    let json = r#"{"hook_event_name":"Stop","session_id":"cc-1"}"#;
    let hook: yinz_types::agent::HookEvent = serde_json::from_str(json).unwrap();
    let (exit_code, reported) =
        yinz.run_actions_reporting(yinz_types::input::actions::hook_report_actions(
            "claude",
            yinz_types::data::PaneId::Terminal(pane.terminal_id()),
            &hook,
            json.to_owned(),
        ));
    assert_eq!(exit_code, 0, "{reported}");
    yinz.wait_for_serialized_layout("running agent saved", |layout| layout.contains("cc-1"));

    // Back to a layout with nothing to tell it from the one it started as.
    yinz.run_cli_action(CliAction::AgentReport {
        pane_id: Some(format!("terminal_{}", pane.terminal_id())),
        kind: None,
        state: None,
        detail: None,
        cwd: None,
        hook_json: false,
        clear: true,
        run_id: None,
    });
    yinz.wait_for_serialized_layout("agent gone from saved layout", |layout| {
        !layout.contains("cc-1")
    });
    yinz.quit();
}

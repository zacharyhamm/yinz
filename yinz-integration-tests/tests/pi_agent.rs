#![cfg(unix)]
//! Pi's launch and prompt-control path through the host, PTY and screen threads.
use std::time::Duration;
use yinz_integration_tests::{TestRunner, TERMINAL_SIZE};
use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
use yinz_types::{
    agent::{
        control::{AgentOutcome, AgentPlacement, AgentRequest},
        hook_outcome,
        report::{AgentReport, AgentReportState},
        AgentEvent, AgentSpawnSpec, AgentState, HookEvent, HookOutcome,
    },
    pane_id::PaneId,
};

#[test]
fn pi_launch_resolves_extension_and_correlates_a_prompt_end_to_end() {
    exercise_pi_launch(false);
}

#[test]
fn suppressed_agent_launch_prompt_read_show_and_hide_end_to_end() {
    exercise_pi_launch(true);
}

fn exercise_pi_launch(suppressed: bool) {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_config(if suppressed {
            "agent_pane_min_columns 80"
        } else {
            "agent_pane_min_columns 0"
        })
        .start();
    let original = yinz.expect_pty_spawn();
    original.output(b"shell$ ");
    yinz.wait_until("original shell is rendered", |snapshot| {
        snapshot.contains("shell$")
    });
    let socket = yinz_ipc::consts::ipc_connect(
        &yinz_runtime::consts::YINZ_SOCK_DIR.join(yinz.session_name()),
    )
    .unwrap();
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
    let mut receiver = sender.get_receiver::<ServerToClientMsg>().unwrap();
    receiver
        .set_recv_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let cwd = tempfile::tempdir().unwrap();
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Spawn {
            placement: if suppressed {
                AgentPlacement::Suppressed(None)
            } else {
                AgentPlacement::Automatic
            },
            spec: AgentSpawnSpec {
                kind: Some("pi".into()),
                allow_outside_worktree: Some(true),
                ..Default::default()
            },
            cwd: Some(cwd.path().into()),
            project: None,
            remote_host: None,
        }))
        .unwrap();
    let response = loop {
        match receiver.recv_server_msg() {
            IpcRecv::Msg(ServerToClientMsg::AgentControl(response), _) => break response,
            IpcRecv::Msg(_, _) => continue,
            _ => panic!("agent reply disconnected, malformed, or timed out"),
        }
    };
    assert_eq!(response.outcome, AgentOutcome::Ok, "{response:?}");
    let agent = yinz.expect_pty_spawn();
    let spawned = response.spawned.unwrap();
    if suppressed {
        assert_eq!(
            spawned.tab_id,
            Some(yinz_types::internal_ids::TabId::new(0))
        );
        assert!(spawned.created_tab_id.is_none());
    }
    let pane = spawned.pane_id;
    assert_eq!(pane, PaneId::Terminal(agent.terminal_id()));
    let command = agent.run_command();
    let extension = command
        .args
        .windows(2)
        .find(|args| args[0] == "--extension")
        .unwrap();
    assert_ne!(extension[1], yinz_runtime::agent::pi::EXTENSION_ARGUMENT);
    assert!(std::fs::read_to_string(&extension[1])
        .unwrap()
        .contains("yinz-pi-agent-report"));
    if !suppressed {
        yinz.run_cli_action(yinz_cli::args::CliAction::FocusPaneId {
            pane_id: pane.to_string(),
        });
    }
    agent.output(b"\x1b[?2004hPi ready");
    if !suppressed {
        yinz.wait_until("Pi enables its composer", |snapshot| {
            snapshot.contains("Pi ready")
        });
    }
    let report = |yinz: &mut yinz_integration_tests::TestSession, name: &str, turn: &str| {
        let hook = HookEvent {
            hook_event_name: name.into(),
            session_id: "pi-session".into(),
            source: (name == "SessionStart").then(|| "startup".into()),
            turn_id: Some(turn.into()),
            prompt: (name == "UserPromptSubmit").then(|| "review".into()),
            last_assistant_message: (name == "Stop").then(|| "done".into()),
            ..Default::default()
        };
        let raw = serde_json::to_string(&hook).unwrap();
        let socket = yinz_ipc::consts::ipc_connect(
            &yinz_runtime::consts::YINZ_SOCK_DIR.join(yinz.session_name()),
        )
        .unwrap();
        let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
        let mut receiver = sender.get_receiver::<ServerToClientMsg>().unwrap();
        receiver
            .set_recv_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let trace = yinz_runtime::agent::report::ReportTrace::new(std::time::Instant::now());
        sender
            .send_client_msg(ClientToServerMsg::AgentReport(AgentReport {
                report_id: trace.report_id,
                pane_id: pane,
                state: match hook_outcome("pi", &hook) {
                    HookOutcome::Reported(agent) => AgentReportState::Set(agent),
                    HookOutcome::Ended => AgentReportState::Clear,
                    HookOutcome::Nothing => AgentReportState::Unchanged,
                },
                event: Some(AgentEvent::from_hook("pi", agent.terminal_id(), &hook, raw)),
                trace: false,
                process_id: trace.process_id,
                kind: "pi".into(),
            }))
            .unwrap();
        assert!(matches!(
            receiver.recv_server_msg(),
            IpcRecv::Msg(ServerToClientMsg::AgentReportAccepted { .. }, _)
        ));
    };
    report(&mut yinz, "SessionStart", "");
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Wait {
            pane_id: pane,
            expected_session_id: None,
            until: AgentState::Idle,
            timeout_ms: 30_000,
        }))
        .unwrap();
    let response = loop {
        match receiver.recv_server_msg() {
            IpcRecv::Msg(ServerToClientMsg::AgentControl(response), _) => break response,
            IpcRecv::Msg(_, _) => continue,
            _ => panic!("agent reply disconnected, malformed, or timed out"),
        }
    };
    assert!(response.agents[0].ready, "{response:?}");
    assert_eq!(response.agents[0].is_suppressed, suppressed);
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Prompt {
            preserve_draft: false,
            submission_id: None,
            pane_id: pane,
            expected_session_id: Some("pi-session".into()),
            prompt: "review".into(),
            wait: true,
            timeout_ms: 30_000,
        }))
        .unwrap();
    agent.wait_for_stdin(
        "Pi receives clearing followed by the prompt and Enter",
        |bytes| {
            let prompt = b"\x1b[200~review\x1b[201~\r";
            let Some(at) = bytes
                .windows(prompt.len())
                .position(|bytes| bytes == prompt)
            else {
                return false;
            };
            assert!(bytes[..at]
                .windows(b"\x1b[200~yz".len())
                .any(|bytes| bytes == b"\x1b[200~yz"));
            assert!(bytes[..at].ends_with(b"\x1b[201~\x03"));
            true
        },
    );
    report(&mut yinz, "UserPromptSubmit", "turn-1");
    report(&mut yinz, "Stop", "stale-turn");
    report(&mut yinz, "Stop", "turn-1");
    let response = loop {
        match receiver.recv_server_msg() {
            IpcRecv::Msg(ServerToClientMsg::AgentControl(response), _) => break response,
            IpcRecv::Msg(_, _) => continue,
            _ => panic!("agent reply disconnected, malformed, or timed out"),
        }
    };
    assert_eq!(response.outcome, AgentOutcome::Completed, "{response:?}");
    assert!(response.acknowledged);
    assert_eq!(response.turn_id.as_deref(), Some("turn-1"));
    assert!(response.agents[0].ready);
    if suppressed {
        for request in [
            AgentRequest::Read {
                pane_id: pane,
                expected_session_id: Some("pi-session".into()),
                lines: 80,
            },
            AgentRequest::Show {
                pane_id: pane,
                expected_session_id: Some("pi-session".into()),
            },
            AgentRequest::Hide {
                pane_id: pane,
                expected_session_id: Some("pi-session".into()),
            },
        ] {
            let reading = matches!(request, AgentRequest::Read { .. });
            let showing = matches!(request, AgentRequest::Show { .. });
            sender
                .send_client_msg(ClientToServerMsg::AgentControl(request))
                .unwrap();
            let response = loop {
                match receiver.recv_server_msg() {
                    IpcRecv::Msg(ServerToClientMsg::AgentControl(response), _) => break response,
                    IpcRecv::Msg(_, _) => continue,
                    _ => panic!("agent visibility reply disconnected or timed out"),
                }
            };
            assert_eq!(response.outcome, AgentOutcome::Ok, "{response:?}");
            assert_eq!(response.agents[0].is_suppressed, !showing);
            if reading {
                assert!(response.output.unwrap().contains("Pi ready"));
            }
            if showing {
                yinz.wait_until("shown agent is rendered", |snapshot| {
                    snapshot.contains("Pi ready")
                });
            }
            let keyboard = if showing {
                b"aftershow".as_slice()
            } else {
                b"whilehidden".as_slice()
            };
            yinz.send_stdin(keyboard);
            original.wait_for_stdin("agent operations preserve shell focus", |bytes| {
                bytes.windows(keyboard.len()).any(|s| s == keyboard)
            });
        }
        yinz.wait_until("hidden agent leaves shell visible", |snapshot| {
            snapshot.contains("whilehidden") && !snapshot.contains("Pi ready")
        });
    }
    drop(sender);
    drop(receiver);
    yinz.quit();
}

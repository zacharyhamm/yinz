#![cfg(unix)]
//! Claude steering and background activity across IPC, screen, and PTY delivery.
use std::time::Duration;
use yinz_integration_tests::{LayoutInfo, TestRunner, TERMINAL_SIZE};
use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
use yinz_types::agent::{
    control::{AgentOutcome, AgentRequest, AgentResponse},
    hook_outcome,
    report::{AgentReport, AgentReportState},
    AgentEvent, AgentState, ClaudeTaskExecution, HookEvent, HookOutcome,
};
use yinz_types::pane_id::PaneId;

#[test]
fn claude_steering_delivers_without_interrupting_and_publishes_background_activity() {
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(
            "layout { pane command=\"claude\"; }".into(),
        ))
        .start();
    let agent = yinz.expect_pty_spawn();
    let pane = PaneId::Terminal(agent.terminal_id());
    let (cols, _) =
        agent.wait_for_size("Claude pane has a size", |cols, rows| cols > 20 && rows > 8);
    let border = "─".repeat(usize::from(cols));
    let composer = |label: &str| {
        format!("\x1b[?2004h\x1b[2J\x1b[H{label}\r\n{border}\r\n❯ \r\n{border}\r\n  auto mode on\x1b[3;3H\x1b[?25h")
    };
    agent.output(composer("Claude composer").as_bytes());
    yinz.wait_until("the composer is rendered", |snapshot| {
        snapshot.contains("Claude composer")
    });

    let socket_path = yinz_runtime::consts::YINZ_SOCK_DIR.join(yinz.session_name());
    let connect = || {
        let socket = yinz_ipc::consts::ipc_connect(&socket_path).unwrap();
        let sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
        let receiver = sender.get_receiver::<ServerToClientMsg>().unwrap();
        receiver
            .set_recv_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        (sender, receiver)
    };
    let (mut sender, mut receiver) = connect();
    let send_report = |hook: HookEvent| {
        let (mut reports, mut replies) = connect();
        let trace = yinz_runtime::agent::report::ReportTrace::new(std::time::Instant::now());
        reports
            .send_client_msg(ClientToServerMsg::AgentReport(AgentReport {
                report_id: trace.report_id,
                pane_id: pane,
                state: match hook_outcome("claude", &hook) {
                    HookOutcome::Reported(agent) => AgentReportState::Set(agent),
                    HookOutcome::Ended => AgentReportState::Clear,
                    HookOutcome::Nothing => AgentReportState::Unchanged,
                },
                event: Some(AgentEvent::from_hook(
                    "claude",
                    agent.terminal_id(),
                    &hook,
                    serde_json::to_string(&hook).unwrap(),
                )),
                trace: false,
                process_id: trace.process_id,
                kind: "claude".into(),
            }))
            .unwrap();
        assert!(matches!(
            replies.recv_server_msg(),
            IpcRecv::Msg(ServerToClientMsg::AgentReportAccepted { .. }, _)
        ));
    };
    let report = |name: &str, prompt_id: &str, worker: Option<&str>, tasks: &[&str]| {
        let hook = HookEvent {
            hook_event_name: name.into(), session_id: "claude-session".into(),
            source: (name == "SessionStart").then(|| "startup".into()),
            prompt_id: Some(prompt_id.into()),
            prompt: (name == "UserPromptSubmit").then(|| if prompt_id == "notification" {
                "<task-notification><task-id>bash</task-id><status>completed</status></task-notification>".into()
            } else if prompt_id == "initial" {
                "start".into()
            } else { "steer".into() }),
            agent_id: worker.map(str::to_owned),
            claude_turn_complete: (name == "Stop").then_some(true),
            claude_background_tasks: tasks.iter().map(|s| s.to_string()).collect(),
            last_assistant_message: (name == "Stop").then(|| format!("finished {prompt_id}")),
            ..Default::default()
        };
        send_report(hook);
    };
    // Process fixture tests validate these identities against persisted launch results and
    // successful handback notifications. Replay their observer reports over IPC as well as
    // the raw hooks; a raw SubagentStop alone cannot identify the completed execution.
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../yinz-process/src/unit/fixtures/claude-task-executions.json"
    ))
    .unwrap();
    let runs = fixture["cases"]["same-prompt"]["runs"].as_array().unwrap();
    let worker = runs[0]["record"]["task_id"].as_str().unwrap();
    let observed_worker = |index: usize| {
        let run = &runs[index]["record"];
        let complete = run["subtype"] == "task_notification";
        HookEvent {
            hook_event_name: if complete {
                "SubagentStop"
            } else {
                "SubagentStart"
            }
            .into(),
            session_id: "claude-session".into(),
            agent_id: Some(run["task_id"].as_str().unwrap().into()),
            claude_task_execution: Some(ClaudeTaskExecution {
                tool_use_id: run["tool_use_id"].as_str().unwrap().into(),
                run_id: complete.then(|| run["run_id"].as_str().unwrap().into()),
            }),
            ..Default::default()
        }
    };
    let reply =
        |receiver: &mut yinz_ipc::ipc::IpcReceiverWithContext<ServerToClientMsg>| -> AgentResponse {
            loop {
                match receiver.recv_server_msg() {
                    IpcRecv::Msg(ServerToClientMsg::AgentControl(response), _) => return response,
                    IpcRecv::Msg(_, _) => {},
                    _ => panic!("agent response disconnected or timed out"),
                }
            }
        };
    let status = || {
        let (mut sender, mut receiver) = connect();
        sender
            .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Status {
                pane_id: pane,
                expected_session_id: None,
            }))
            .unwrap();
        reply(&mut receiver)
    };
    report("SessionStart", "", None, &[]);
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Prompt {
            pane_id: pane,
            expected_session_id: Some("claude-session".into()),
            prompt: "start".into(),
            submission_id: Some("initial".into()),
            preserve_draft: false,
            wait: false,
            timeout_ms: 30_000,
        }))
        .unwrap();
    // Include draft preparation in the initial baseline; preserving steering adds
    // only its own paste and Enter below.
    let initial_bytes = agent.wait_for_stdin("initial paste and Enter reach Claude", |bytes| {
        bytes.ends_with(b"\x1b[200~start\x1b[201~\r")
    });
    report("UserPromptSubmit", "initial", None, &[]);
    let submitted = reply(&mut receiver);
    assert_eq!(submitted.outcome, AgentOutcome::Submitted);
    assert_eq!(submitted.submission_id.as_deref(), Some("initial"));
    assert!(submitted.acknowledged);
    agent.output(composer("Initial prompt accepted").as_bytes());
    yinz.wait_until("Claude consumed the initial prompt", |snapshot| {
        snapshot.contains("Initial prompt accepted")
    });
    report("SubagentStart", "initial", Some(worker), &[]);
    send_report(observed_worker(0));
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Status {
            pane_id: pane,
            expected_session_id: None,
        }))
        .unwrap();
    let working = reply(&mut receiver);
    assert_eq!(working.agents[0].agent.state, AgentState::Busy);
    assert!(working.agents[0].ready, "{working:?}");
    assert_eq!(working.agents[0].background_tasks, [worker]);
    let events = yinz.stream_events(&["pane_update"]);
    events.wait_for_event("PaneUpdate");
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Prompt {
            pane_id: pane,
            expected_session_id: Some("claude-session".into()),
            prompt: "steer".into(),
            submission_id: Some("steering".into()),
            preserve_draft: true,
            wait: true,
            timeout_ms: 30_000,
        }))
        .unwrap();
    let bytes = agent.wait_for_stdin("steering paste and Enter reach Claude", |bytes| {
        bytes[initial_bytes.len()..].ends_with(b"\x1b[200~steer\x1b[201~\r")
    });
    assert_eq!(&bytes[initial_bytes.len()..], b"\x1b[200~steer\x1b[201~\r");
    report("UserPromptSubmit", "steering", None, &[]);
    agent.output(composer("Steering accepted").as_bytes());
    yinz.wait_until("Claude consumed the steering prompt", |snapshot| {
        snapshot.contains("Steering accepted")
    });
    report("Stop", "initial", None, &[worker]);
    report("Stop", "steering", None, &["bash"]);
    let completed = reply(&mut receiver);
    assert_eq!(completed.outcome, AgentOutcome::Completed);
    assert_eq!(completed.submission_id.as_deref(), Some("steering"));
    assert_eq!(completed.message.as_deref(), Some("finished steering"));
    assert_eq!(completed.agents[0].agent.state, AgentState::Busy);
    assert!(completed.agents[0].ready);
    report("SubagentStop", "steering", Some(worker), &[]);
    let uncorrelated = status();
    assert_eq!(uncorrelated.agents[0].agent.state, AgentState::Busy);
    assert!(uncorrelated.agents[0]
        .background_tasks
        .iter()
        .any(|id| id == worker));
    send_report(observed_worker(1));
    // A duplicate completion and late parent launch list cannot resurrect finished work.
    send_report(observed_worker(1));
    report("Stop", "initial", None, &[worker]);
    let background = status();
    assert_eq!(background.agents[0].agent.state, AgentState::Busy);
    assert_eq!(background.agents[0].background_tasks, ["bash"]);
    report("UserPromptSubmit", "notification", None, &[]);
    // A status reply is a screen-thread barrier after every preceding report.
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Status {
            pane_id: pane,
            expected_session_id: None,
        }))
        .unwrap();
    assert_eq!(reply(&mut receiver).agents[0].agent.state, AgentState::Busy);
    let pane_state = |json: &str| -> Option<String> {
        let event: serde_json::Value = serde_json::from_str(json).ok()?;
        event["PaneUpdate"]["panes"]
            .as_object()?
            .values()
            .filter_map(|p| p.as_array())
            .flatten()
            .find(|p| p["id"] == agent.terminal_id().get() && p["is_plugin"] == false)?["agent"]
            ["state"]
            .as_str()
            .map(str::to_owned)
    };
    assert!(events
        .streamed_events()
        .iter()
        .all(|(_, json)| pane_state(json).as_deref() != Some("Idle")));
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Wait {
            pane_id: pane,
            expected_session_id: None,
            until: AgentState::Idle,
            timeout_ms: 30_000,
        }))
        .unwrap();
    report("Stop", "notification", None, &[]);
    let idle = reply(&mut receiver);
    assert_eq!(idle.outcome, AgentOutcome::Ok);
    assert_eq!(idle.message.as_deref(), Some("finished notification"));
    assert_eq!(idle.agents[0].agent.state, AgentState::Idle);
    assert!(idle.agents[0].background_tasks.is_empty());
    yinz.wait_until(
        "the idle pane is published after background processing",
        |_| {
            events
                .streamed_events()
                .iter()
                .any(|(_, json)| pane_state(json).as_deref() == Some("Idle"))
        },
    );
}

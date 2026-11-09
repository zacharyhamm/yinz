#![cfg(unix)]
//! Claude steering and background activity across IPC, screen, and PTY delivery.
use std::time::Duration;
use yinz_integration_tests::{LayoutInfo, TestRunner, TERMINAL_SIZE};
use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
use yinz_types::agent::{
    control::{AgentOutcome, AgentRequest, AgentResponse},
    hook_outcome,
    report::{AgentReport, AgentReportState},
    AgentEvent, AgentState, HookEvent, HookOutcome, CLAUDE_BACKGROUND_IDLE,
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
    let report = |name: &str, prompt_id: &str, worker: Option<&str>, background: Option<bool>| {
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
            claude_background: background,
            last_assistant_message: (name == "Stop").then(|| format!("finished {prompt_id}")),
            ..Default::default()
        };
        send_report(hook);
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
    report("SessionStart", "", None, None);
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
    report("UserPromptSubmit", "initial", None, None);
    let submitted = reply(&mut receiver);
    assert_eq!(submitted.outcome, AgentOutcome::Submitted);
    assert_eq!(submitted.submission_id.as_deref(), Some("initial"));
    assert!(submitted.acknowledged);
    agent.output(composer("Initial prompt accepted").as_bytes());
    yinz.wait_until("Claude consumed the initial prompt", |snapshot| {
        snapshot.contains("Initial prompt accepted")
    });
    let worker = "worker";
    report("SubagentStart", "initial", Some(worker), None);
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Status {
            pane_id: pane,
            expected_session_id: None,
        }))
        .unwrap();
    let working = reply(&mut receiver);
    assert_eq!(working.agents[0].agent.state, AgentState::Busy);
    assert!(working.agents[0].ready, "{working:?}");
    assert!(!working.agents[0].background);
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
    report("UserPromptSubmit", "steering", None, None);
    agent.output(composer("Steering accepted").as_bytes());
    yinz.wait_until("Claude consumed the steering prompt", |snapshot| {
        snapshot.contains("Steering accepted")
    });
    // Native Post acquisition protects even work completed before the first registry sample.
    let (mut launch, _) = yinz_runtime::agent::hooks::parse_hook_event(
        "claude",
        serde_json::json!({
            "hook_event_name":"PostToolUse","session_id":"claude-session","prompt_id":"initial",
            "tool_name":"Bash","tool_use_id":"bash-tool","tool_response":{"backgroundTaskId":"bash"}
        })
        .to_string(),
    )
    .unwrap();
    launch.claude_notifications.as_mut().unwrap().source =
        Some(yinz_types::agent::claude_notifications::Source {
            pid: 123,
            started: 456,
        });
    send_report(launch);
    // Claude's session registry still shows the worker and background Bash running.
    report("Stop", "initial", None, Some(true));
    report("Stop", "steering", None, Some(true));
    let completed = reply(&mut receiver);
    assert_eq!(completed.outcome, AgentOutcome::Completed);
    assert_eq!(completed.submission_id.as_deref(), Some("steering"));
    assert_eq!(completed.message.as_deref(), Some("finished steering"));
    assert_eq!(completed.agents[0].agent.state, AgentState::Busy);
    assert!(completed.agents[0].ready);
    report("SubagentStop", "steering", Some(worker), None);
    // The superseded turn's watch cannot end work the newer confirmation reported.
    report(CLAUDE_BACKGROUND_IDLE, "initial", None, Some(false));
    report("Stop", "initial", None, Some(true));
    let background = status();
    assert_eq!(background.agents[0].agent.state, AgentState::Busy);
    assert!(background.agents[0].background);
    // Registry completion is deliberately applied before the notice becomes visible.
    report(CLAUDE_BACKGROUND_IDLE, "steering", None, Some(false));
    let gap = status();
    assert_eq!(gap.agents[0].agent.state, AgentState::Busy);
    assert!(gap.agents[0].background);
    report("UserPromptSubmit", "notification", None, None);
    send_report(HookEvent {
        hook_event_name: "NotificationAccepted".into(),
        session_id: "claude-session".into(),
        prompt_id: Some("notification".into()),
        claude_notifications: Some(yinz_types::agent::claude_notifications::Evidence {
            source: Some(yinz_types::agent::claude_notifications::Source {
                pid: 123,
                started: 456,
            }),
            notice: Some(yinz_types::agent::claude_notifications::Notice {
                task: "bash".into(),
                tool: "bash-tool".into(),
                run: "run".into(),
                prompt: "notification".into(),
                row: "row".into(),
            }),
            ..Default::default()
        }),
        ..Default::default()
    });
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
    let mut confirmation = HookEvent {
        hook_event_name: "Stop".into(),
        session_id: "claude-session".into(),
        prompt_id: Some("notification".into()),
        claude_turn_complete: Some(true),
        claude_background: Some(false),
        last_assistant_message: Some("finished notification".into()),
        ..Default::default()
    };
    confirmation.claude_notifications = Some(yinz_types::agent::claude_notifications::Evidence {
        source: Some(yinz_types::agent::claude_notifications::Source {
            pid: 123,
            started: 456,
        }),
        ..Default::default()
    });
    send_report(confirmation);
    let idle = reply(&mut receiver);
    assert_eq!(idle.outcome, AgentOutcome::Ok);
    assert_eq!(idle.message.as_deref(), Some("finished notification"));
    assert_eq!(idle.agents[0].agent.state, AgentState::Idle);
    assert!(!idle.agents[0].background);
    sender
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Wait {
            pane_id: pane,
            expected_session_id: None,
            until: AgentState::Idle,
            timeout_ms: 30_000,
        }))
        .unwrap();
    assert_eq!(
        reply(&mut receiver).message.as_deref(),
        Some("finished notification")
    );
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

/// Real IPC application with a status barrier after each report, rather than
/// treating AgentReportAccepted (queue receipt) as screen application.
struct NotificationIpc {
    yinz: yinz_integration_tests::TestSession,
    agent: yinz_integration_tests::FakePtyHandle,
    pane: PaneId,
}

impl NotificationIpc {
    fn new() -> Self {
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
        agent.output(format!("\x1b[?2004h\x1b[2J\x1b[HClaude composer\r\n{border}\r\n❯ \r\n{border}\r\n  auto mode on\x1b[3;3H\x1b[?25h").as_bytes());
        yinz.wait_until("the composer is rendered", |s| {
            s.contains("Claude composer")
        });
        let fixture = Self { yinz, agent, pane };
        fixture.report(HookEvent {
            source: Some("startup".into()),
            ..fixture.hook("SessionStart", "")
        });
        fixture
    }

    fn connect(
        &self,
    ) -> (
        IpcSenderWithContext<ClientToServerMsg>,
        yinz_ipc::ipc::IpcReceiverWithContext<ServerToClientMsg>,
    ) {
        let path = yinz_runtime::consts::YINZ_SOCK_DIR.join(self.yinz.session_name());
        let sender = IpcSenderWithContext::<ClientToServerMsg>::new(
            yinz_ipc::consts::ipc_connect(&path).unwrap(),
        );
        let receiver = sender.get_receiver().unwrap();
        receiver
            .set_recv_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        (sender, receiver)
    }

    fn reply(
        receiver: &mut yinz_ipc::ipc::IpcReceiverWithContext<ServerToClientMsg>,
    ) -> AgentResponse {
        loop {
            match receiver.recv_server_msg() {
                IpcRecv::Msg(ServerToClientMsg::AgentControl(response), _) => return response,
                IpcRecv::Msg(_, _) => {},
                _ => panic!("agent response disconnected or timed out"),
            }
        }
    }

    fn request(&self, request: AgentRequest) -> AgentResponse {
        let (mut sender, mut receiver) = self.connect();
        sender
            .send_client_msg(ClientToServerMsg::AgentControl(request))
            .unwrap();
        Self::reply(&mut receiver)
    }

    fn status(&self) -> AgentResponse {
        self.request(AgentRequest::Status {
            pane_id: self.pane,
            expected_session_id: None,
        })
    }

    fn hook(&self, name: &str, prompt: &str) -> HookEvent {
        HookEvent {
            hook_event_name: name.into(),
            session_id: "conversation".into(),
            prompt_id: Some(prompt.into()),
            ..Default::default()
        }
    }

    fn report(&self, hook: HookEvent) -> AgentResponse {
        let (mut sender, mut receiver) = self.connect();
        let trace = yinz_runtime::agent::report::ReportTrace::new(std::time::Instant::now());
        sender
            .send_client_msg(ClientToServerMsg::AgentReport(AgentReport {
                report_id: trace.report_id,
                pane_id: self.pane,
                kind: "claude".into(),
                trace: false,
                process_id: trace.process_id,
                state: match hook_outcome("claude", &hook) {
                    HookOutcome::Reported(agent) => AgentReportState::Set(agent),
                    HookOutcome::Ended => AgentReportState::Clear,
                    HookOutcome::Nothing => AgentReportState::Unchanged,
                },
                event: Some(AgentEvent::from_hook(
                    "claude",
                    self.agent.terminal_id(),
                    &hook,
                    serde_json::to_string(&hook).unwrap(),
                )),
            }))
            .unwrap();
        assert!(matches!(
            receiver.recv_server_msg(),
            IpcRecv::Msg(ServerToClientMsg::AgentReportAccepted { .. }, _)
        ));
        self.status()
    }

    fn source() -> yinz_types::agent::claude_notifications::Source {
        yinz_types::agent::claude_notifications::Source {
            pid: 123,
            started: 456,
        }
    }

    fn native(&self, raw: serde_json::Value) -> HookEvent {
        let (mut hook, _) =
            yinz_runtime::agent::hooks::parse_hook_event("claude", raw.to_string()).unwrap();
        hook.claude_notifications
            .get_or_insert_with(Default::default)
            .source = Some(Self::source());
        hook
    }

    fn submit(&self, prompt: &str, task: Option<&str>) {
        self.report(HookEvent {
            prompt: Some(task.map_or_else(
                || "human".into(),
                |task| format!("<task-notification><task-id>{task}</task-id></task-notification>"),
            )),
            ..self.hook("UserPromptSubmit", prompt)
        });
    }

    fn launch(&self, task: &str, tool: &str) {
        self.report(self.native(serde_json::json!({"hook_event_name":"PostToolUse","session_id":"conversation","prompt_id":"launch",
            "tool_name":"Bash","tool_use_id":tool,"tool_response":{"backgroundTaskId":task}})));
    }

    fn notice(&self, prompt: &str, task: &str, tool: &str) -> HookEvent {
        use yinz_types::agent::claude_notifications::{Evidence, Notice};
        HookEvent {
            claude_notifications: Some(Evidence {
                source: Some(Self::source()),
                notice: Some(Notice {
                    task: task.into(),
                    tool: tool.into(),
                    prompt: prompt.into(),
                    run: format!("run-{prompt}"),
                    row: format!("row-{prompt}"),
                }),
                ..Default::default()
            }),
            ..self.hook("NotificationAccepted", prompt)
        }
    }

    fn stop(&self, prompt: &str, text: &str) -> HookEvent {
        HookEvent {
            claude_turn_complete: Some(true),
            claude_background: Some(false),
            last_assistant_message: Some(text.into()),
            claude_notifications: Some(yinz_types::agent::claude_notifications::Evidence {
                source: Some(Self::source()),
                ..Default::default()
            }),
            ..self.hook("Stop", prompt)
        }
    }
}

fn notification_disposition_over_ipc(disposition: &str) {
    for registry_first in [false, true] {
        for concurrent in [false, true] {
            let fixture = NotificationIpc::new();
            fixture.submit("launch", None);
            fixture.launch("bash", "tool");
            if concurrent {
                fixture.launch("other", "other-tool");
            }
            let mut stop = fixture.stop("launch", "LAUNCH");
            stop.claude_background = Some(true);
            fixture.report(stop);
            let events = fixture
                .yinz
                .stream_events(&["pane_update", "agent_lifecycle"]);
            events.wait_for_event("PaneUpdate");
            events.wait_for_event_matching("agent_lifecycle", |json| {
                serde_json::from_str::<serde_json::Value>(json).unwrap()["event_type"] == "ready"
            });
            let (mut waiter, mut waited) = fixture.connect();
            let wait = || AgentRequest::Wait {
                pane_id: fixture.pane,
                expected_session_id: None,
                until: AgentState::Idle,
                timeout_ms: 30_000,
            };
            waiter
                .send_client_msg(ClientToServerMsg::AgentControl(wait()))
                .unwrap();
            let clear_registry = || {
                fixture.report(HookEvent {
                    claude_background: Some(false),
                    ..fixture.hook(CLAUDE_BACKGROUND_IDLE, "launch")
                })
            };
            if registry_first {
                assert_eq!(clear_registry().agents[0].agent.state, AgentState::Busy);
            }
            if disposition == "cancel" {
                fixture.submit("cancel", None);
                fixture.report(fixture.native(serde_json::json!({"hook_event_name":"PostToolUse","session_id":"conversation","prompt_id":"cancel",
                    "tool_name":"TaskStop","tool_use_id":"cancel-tool","tool_input":{"task_id":"bash"},
                    "tool_response":{"task_id":"bash","task_type":"local_bash","command":"private"}})));
            } else {
                fixture.submit("notice", Some("bash"));
                fixture.report(fixture.notice("notice", "bash", "tool"));
            }
            if !registry_first {
                assert_eq!(clear_registry().agents[0].agent.state, AgentState::Busy);
            }
            assert_eq!(fixture.status().agents[0].agent.state, AgentState::Busy);
            let final_prompt = if disposition == "cancel" {
                "cancel"
            } else {
                "notice"
            };
            if disposition == "unavailable" && registry_first {
                // A duplicate false Stop may add the first scoped loss evidence.
                // Deduplicating its state must not skip the resulting publication.
                let mut unknown = fixture.stop("notice", "");
                unknown.claude_turn_complete = Some(false);
                assert_eq!(
                    fixture.report(unknown).agents[0].agent.state,
                    AgentState::Busy
                );
            }
            let mut answer = fixture.stop(final_prompt, "FINAL");
            if disposition == "unavailable" {
                answer.claude_turn_complete = Some(false);
                answer.claude_notifications = fixture
                    .notice("notice", "bash", "tool")
                    .claude_notifications;
                answer.claude_notifications.as_mut().unwrap().unavailable = true;
            }
            let after = fixture.report(answer);
            assert_eq!(after.agents[0].background, concurrent);
            assert_eq!(
                after.agents[0].agent.state,
                if concurrent {
                    AgentState::Busy
                } else {
                    AgentState::Idle
                }
            );
            let expected = if concurrent {
                fixture.submit("other-notice", Some("other"));
                fixture.report(fixture.notice("other-notice", "other", "other-tool"));
                fixture.report(fixture.stop("other-notice", "FINAL_OTHER"));
                "FINAL_OTHER"
            } else if disposition == "unavailable" {
                "LAUNCH"
            } else {
                "FINAL"
            };
            let done = NotificationIpc::reply(&mut waited);
            assert_eq!(done.outcome, AgentOutcome::Ok);
            assert_eq!(done.message.as_deref(), Some(expected));
            assert_eq!(fixture.request(wait()).message.as_deref(), Some(expected));
            events.wait_for_event_matching("agent_lifecycle", |json| {
                let value: serde_json::Value = serde_json::from_str(json).unwrap();
                value["event_type"] == "state_changed" && value["state"] == "idle"
            });
            events.wait_for_event_matching("PaneUpdate", |json| {
                let value: serde_json::Value = serde_json::from_str(json).unwrap();
                value["PaneUpdate"]["panes"]
                    .as_object()
                    .is_some_and(|panes| {
                        panes
                            .values()
                            .filter_map(|p| p.as_array())
                            .flatten()
                            .any(|p| {
                                p["id"] == fixture.agent.terminal_id().get()
                                    && p["is_plugin"] == false
                                    && p["agent"]["state"] == "Idle"
                            })
                    })
            });
            // Ordered stream delivery reaches final Idle after all prior state edges.
            // No Busy may follow an earlier Idle in these protected phases.
            let mut idle = [false; 2];
            for (kind, json) in events.streamed_events() {
                let value: serde_json::Value = serde_json::from_str(&json).unwrap();
                let state = if kind == "agent_lifecycle" && value["event_type"] == "state_changed" {
                    value["state"].as_str().map(|s| (0, s.to_owned()))
                } else if kind == "PaneUpdate" {
                    value["PaneUpdate"]["panes"].as_object().and_then(|panes| {
                        panes
                            .values()
                            .filter_map(|p| p.as_array())
                            .flatten()
                            .find(|p| {
                                p["id"] == fixture.agent.terminal_id().get()
                                    && p["is_plugin"] == false
                            })
                            .and_then(|p| p["agent"]["state"].as_str())
                            .map(|s| (1, s.to_lowercase()))
                    })
                } else {
                    None
                };
                if let Some((feed, state)) = state {
                    if state == "idle" {
                        idle[feed] = true;
                    }
                    if state == "busy" {
                        assert!(!idle[feed], "premature Idle in {kind}, {disposition}, registry_first={registry_first}: {json}");
                    }
                }
            }
            assert!(
                idle.into_iter().all(|s| s),
                "both lifecycle and PaneUpdate publish final Idle"
            );
            // R69-3 must also allow subsequent ordinary use; a late valid answer
            // cannot overwrite the newer human turn or resurrect its failed observer.
            fixture.submit("later-human", None);
            fixture.report(fixture.stop("later-human", "LATER"));
            if disposition == "unavailable" {
                fixture.report(fixture.stop("notice", "OLDER_REAL_ANSWER"));
            }
            let later = fixture.request(wait());
            assert_eq!(later.message.as_deref(), Some("LATER"));
            assert_eq!(later.agents[0].agent.state, AgentState::Idle);
        }
    }
}

#[test]
fn claude_steering_unavailable_notification_falls_back_without_stranding_other_work() {
    notification_disposition_over_ipc("unavailable");
}

#[test]
fn claude_steering_observer_loss_keeps_native_permission_wait_until_release() {
    for worker in [false, true] {
        let fixture = NotificationIpc::new();
        fixture.submit("launch", None);
        fixture.launch("bash", "tool");
        fixture.report(fixture.stop("launch", "LAUNCH"));
        fixture.submit("notice", Some("bash"));
        fixture.report(fixture.notice("notice", "bash", "tool"));
        let waiting_hook = if worker {
            HookEvent {
                agent_id: Some("worker".into()),
                tool_name: Some("AskUserQuestion".into()),
                tool_use_id: Some("question".into()),
                ..fixture.hook("PreToolUse", "notice")
            }
        } else {
            HookEvent {
                tool_name: Some("Bash".into()),
                tool_use_id: Some("permission-tool".into()),
                notification_type: Some("permission_prompt".into()),
                ..fixture.hook("Notification", "notice")
            }
        };
        let before = fixture.report(waiting_hook.clone());
        assert_eq!(before.agents[0].agent.state, AgentState::Waiting);
        assert!(before.agents[0].agent.wait_needs_a_person);
        // This is the real follow-error shape: no structural notice in the Stop,
        // no confirmed answer, and source-scoped unavailable evidence only.
        let mut loss = fixture.stop("notice", "");
        loss.claude_turn_complete = Some(false);
        loss.claude_notifications.as_mut().unwrap().unavailable = true;
        let after = fixture.report(loss.clone());
        assert_eq!(after.agents[0].agent.state, AgentState::Waiting);
        assert!(after.agents[0].agent.wait_needs_a_person);
        assert!(!after.agents[0].background);
        // Status is a screen-applied barrier; the query must report input needed,
        // never a completed idle wait with the old launching answer.
        let wait = || AgentRequest::Wait {
            pane_id: fixture.pane,
            expected_session_id: None,
            until: AgentState::Idle,
            timeout_ms: 30_000,
        };
        let result = fixture.request(wait());
        assert_eq!(result.outcome, AgentOutcome::NeedsInput);
        assert_eq!(result.agents[0].agent.state, AgentState::Waiting);
        assert_ne!(result.message.as_deref(), Some("LAUNCH"));
        fixture.report(loss);
        assert_eq!(fixture.status().agents[0].agent.state, AgentState::Waiting);
        let mut released = waiting_hook;
        released.hook_event_name = "PostToolUse".into();
        released.notification_type = None;
        if worker {
            let mut unrelated = released.clone();
            unrelated.tool_use_id = Some("unrelated".into());
            assert_eq!(
                fixture.report(unrelated).agents[0].agent.state,
                AgentState::Waiting
            );
        }
        assert_eq!(
            fixture.report(released).agents[0].agent.state,
            AgentState::Idle
        );
        fixture.submit("later", None);
        fixture.report(fixture.stop("later", "LATER"));
        let done = fixture.request(wait());
        assert_eq!(done.outcome, AgentOutcome::Ok);
        assert_eq!(done.message.as_deref(), Some("LATER"));
    }
}

#[test]
fn claude_steering_exact_cancellation_waits_for_its_answer_in_both_registry_orders() {
    notification_disposition_over_ipc("cancel");
}

#[test]
fn claude_steering_supported_answers_preserve_other_work_in_both_registry_orders() {
    notification_disposition_over_ipc("answer");
}

#[test]
fn claude_steering_resume_replaces_provider_without_a_new_async_launch() {
    use yinz_types::agent::claude_notifications::Source;
    let fixture = NotificationIpc::new();
    let replacement = Source {
        pid: 124,
        started: 457,
    };
    let sourced = |mut hook: HookEvent, source: Source| {
        hook.claude_notifications
            .get_or_insert_with(Default::default)
            .source = Some(source);
        hook
    };
    let resume = || {
        fixture.native(serde_json::json!({
            "hook_event_name":"SessionStart", "session_id":"conversation", "source":"resume"
        }))
    };
    fixture.submit("launch", None);
    fixture.launch("bash", "tool");
    assert_eq!(
        fixture.report(fixture.stop("launch", "LAUNCH")).agents[0]
            .agent
            .state,
        AgentState::Busy
    );
    assert_eq!(
        fixture.report(resume()).agents[0].agent.state,
        AgentState::Busy
    );
    let (mut waiter, mut waited) = fixture.connect();
    let wait = || AgentRequest::Wait {
        pane_id: fixture.pane,
        expected_session_id: None,
        until: AgentState::Idle,
        timeout_ms: 30_000,
    };
    waiter
        .send_client_msg(ClientToServerMsg::AgentControl(wait()))
        .unwrap();
    // The Status on the waiter's own channel proves the wait has reached the screen.
    waiter
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Status {
            pane_id: fixture.pane,
            expected_session_id: None,
        }))
        .unwrap();
    assert_eq!(
        NotificationIpc::reply(&mut waited).agents[0].agent.state,
        AgentState::Busy
    );
    let adopted = fixture.report(sourced(resume(), replacement.clone()));
    assert_eq!(adopted.agents[0].agent.state, AgentState::Idle);
    assert!(!adopted.agents[0].background);
    assert_eq!(
        NotificationIpc::reply(&mut waited).outcome,
        AgentOutcome::TargetChanged
    );
    fixture.report(sourced(
        fixture.hook("UserPromptSubmit", "new"),
        replacement.clone(),
    ));
    assert_eq!(
        fixture
            .report(sourced(resume(), replacement.clone()))
            .agents[0]
            .agent
            .state,
        AgentState::Busy
    );
    let events = fixture
        .yinz
        .stream_events(&["pane_update", "agent_lifecycle"]);
    events.wait_for_event("PaneUpdate");
    waiter
        .send_client_msg(ClientToServerMsg::AgentControl(wait()))
        .unwrap();
    waiter
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Status {
            pane_id: fixture.pane,
            expected_session_id: None,
        }))
        .unwrap();
    assert_eq!(
        NotificationIpc::reply(&mut waited).agents[0].agent.state,
        AgentState::Busy
    );
    let done = fixture.report(sourced(
        fixture.stop("new", "NEW_ANSWER"),
        replacement.clone(),
    ));
    assert_eq!(done.agents[0].agent.state, AgentState::Idle);
    assert!(done.agents[0].ready);
    assert!(!done.agents[0].background);
    let answer = NotificationIpc::reply(&mut waited);
    assert_eq!(answer.outcome, AgentOutcome::Ok);
    assert_eq!(answer.message.as_deref(), Some("NEW_ANSWER"));
    assert_eq!(
        fixture.request(wait()).message.as_deref(),
        Some("NEW_ANSWER")
    );
    events.wait_for_event_matching("agent_lifecycle", |json| {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        value["event_type"] == "state_changed" && value["state"] == "idle"
    });
    events.wait_for_event_matching("PaneUpdate", |json| {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        value["PaneUpdate"]["panes"]
            .as_object()
            .is_some_and(|panes| {
                panes.values().any(|panes| {
                    panes.as_array().is_some_and(|panes| {
                        panes.iter().any(|pane| {
                            pane["id"] == fixture.agent.terminal_id().get()
                                && pane["is_plugin"] == false
                                && pane["agent"]["state"] == "Idle"
                        })
                    })
                })
            })
    });
    // Old native hooks and detached callbacks cannot reclaim the conversation,
    // change its text, or clear the replacement's later background work.
    for old in [
        resume(),
        fixture.stop("new", "STALE"),
        fixture.notice("old", "bash", "tool"),
        fixture.hook("SessionEnd", ""),
    ] {
        let after = fixture.report(sourced(old, NotificationIpc::source()));
        assert_eq!(after.agents[0].agent.state, AgentState::Idle);
        assert!(!after.agents[0].background);
        assert_eq!(
            fixture.request(wait()).message.as_deref(),
            Some("NEW_ANSWER")
        );
    }
    let recovered = fixture.request(AgentRequest::Recover {
        pane_id: fixture.pane,
        expected_session_id: "conversation".into(),
        confirm_empty: true,
        interrupted: false,
    });
    assert_eq!(recovered.outcome, AgentOutcome::Ok);
    fixture.report(sourced(
        fixture.hook("UserPromptSubmit", "launch"),
        replacement.clone(),
    ));
    fixture.report(sourced(
        fixture.native(serde_json::json!({"hook_event_name":"PostToolUse",
        "session_id":"conversation", "prompt_id":"launch", "tool_name":"Bash", "tool_use_id":"tool",
        "tool_response":{"backgroundTaskId":"bash"}})),
        replacement.clone(),
    ));
    fixture.report(sourced(
        fixture.stop("launch", "NEW_LAUNCH"),
        replacement.clone(),
    ));
    assert!(fixture.report(fixture.stop("launch", "OLD_LAUNCH")).agents[0].background);
    // Use the native task-notification submit shape; the accepted row is separate.
    fixture.report(sourced(
        HookEvent {
            prompt: Some("<task-notification><task-id>bash</task-id></task-notification>".into()),
            ..fixture.hook("UserPromptSubmit", "notice-actual")
        },
        replacement.clone(),
    ));
    fixture.report(sourced(
        fixture.notice("notice-actual", "bash", "tool"),
        replacement.clone(),
    ));
    waiter
        .send_client_msg(ClientToServerMsg::AgentControl(wait()))
        .unwrap();
    waiter
        .send_client_msg(ClientToServerMsg::AgentControl(AgentRequest::Status {
            pane_id: fixture.pane,
            expected_session_id: None,
        }))
        .unwrap();
    assert!(NotificationIpc::reply(&mut waited).agents[0].background);
    fixture.report(sourced(
        fixture.stop("notice-actual", "FINAL_NEW"),
        replacement,
    ));
    let final_answer = NotificationIpc::reply(&mut waited);
    assert_eq!(final_answer.outcome, AgentOutcome::Ok);
    assert_eq!(final_answer.message.as_deref(), Some("FINAL_NEW"));
    assert!(!fixture.status().agents[0].background);
}

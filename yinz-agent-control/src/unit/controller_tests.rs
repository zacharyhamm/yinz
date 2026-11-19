use super::AgentController;
use super::{close_completed::AgentClose, host::*, request::Completion, Reply};
use std::{cell::RefCell, time::Instant};
use yinz_types::{
    agent::{
        control::{AgentOutcome, AgentRequest, AgentResponse, AgentRestartSession, RestartedAgent},
        AgentInfo, AgentSource, AgentState,
    },
    data::{OriginatingPlugin, PaneContents, PaneInfo},
    input::command::RunCommand,
    internal_ids::{ClientId, TabId, TerminalId},
    pane_id::PaneId,
};

struct RecordingHost {
    pane: PaneId,
    current: AgentInfo,
    live_input: bool,
    enter_revision: u64,
    input_revision: u64,
    reject_preparation: bool,
    reject_application: bool,
    trace: RefCell<Vec<&'static str>>,
    replies: RefCell<Vec<AgentResponse>>,
    events: RefCell<Vec<serde_json::Value>>,
}

impl RecordingHost {
    fn new() -> Self {
        Self {
            pane: PaneId::Terminal(TerminalId::new(7)),
            current: AgentInfo {
                kind: "claude".into(),
                state: AgentState::Idle,
                source: AgentSource::Reported,
                detail: None,
                waiting_on: None,
                wait_needs_a_person: false,
                cwd: None,
                session_id: Some("old".into()),
                run_id: None,
            },
            live_input: false,
            enter_revision: 0,
            input_revision: 4,
            reject_preparation: false,
            reject_application: false,
            trace: Default::default(),
            replies: Default::default(),
            events: Default::default(),
        }
    }
}
impl AgentHost for RecordingHost {
    fn restore_terminal_state(&mut self, _pane: PaneId, _state: &str) {
        unreachable!("unexpected terminal restoration")
    }
    fn restored_observer(&self) -> Option<RestoredObserverRegistration> {
        None
    }
    fn agent_enter_revision(&self, _pane: PaneId) -> u64 {
        self.enter_revision
    }
    fn pane_info(&self, pane: PaneId) -> Option<PaneInfo> {
        (pane == self.pane).then(|| PaneInfo {
            id: pane,
            agent: Some(self.current.clone()),
            ..Default::default()
        })
    }
    fn session_name(&self) -> &str {
        "test"
    }
    fn tab_for_pane(&self, pane: PaneId) -> Option<TabId> {
        (self.live_input && pane == self.pane).then_some(TabId::new(0))
    }
    fn terminal_cwd(&self, _pane: PaneId) -> Option<String> {
        None
    }
    fn foreground_command(&self, _id: TerminalId) -> Option<&[String]> {
        unreachable!("unexpected host operation")
    }
    fn last_output(&self, _pane: PaneId) -> Option<Instant> {
        None
    }
    fn pane_input(&self, pane: PaneId) -> Option<PaneInput> {
        (self.live_input && pane == self.pane).then(|| PaneInput {
            suspended: false,
            sync_active: false,
            excluded_from_sync: false,
            command_present: true,
            command_kind: Some(self.current.kind.clone()),
            bracketed_paste: true,
        })
    }
    fn viewport(&self, _pane: PaneId, _ansi: bool) -> Option<(usize, PaneContents)> {
        None
    }
    fn agent_input_revision(&self, _pane: PaneId) -> u64 {
        self.input_revision
    }
    fn agent_input_at(&self, _pane: PaneId) -> Option<Instant> {
        None
    }
    fn prepare_restart(
        &self,
        _pane_id: PaneId,
        _prompt: Option<&str>,
        _session: AgentRestartSession,
        _originating_plugin: Option<OriginatingPlugin>,
    ) -> Result<PreparedRestart, String> {
        self.trace.borrow_mut().push("prepare");
        if self.reject_preparation {
            return Err("no held command".into());
        }
        let current = AgentInfo {
            state: AgentState::Busy,
            source: AgentSource::Spawned,
            session_id: None,
            ..self.current.clone()
        };
        Ok(PreparedRestart {
            current,
            previous: Some(self.current.clone()),
            fresh: true,
            command: RunCommand::default(),
            result: RestartedAgent {
                pane_id: self.pane,
                command: vec!["claude".into()],
                session_id: None,
            },
        })
    }
    fn apply_restart(
        &mut self,
        _terminal: TerminalId,
        _command: RunCommand,
        _fresh: bool,
        current: &AgentInfo,
    ) -> Result<(), String> {
        self.trace.borrow_mut().push("apply");
        if self.reject_application {
            return Err("pane refused restart".into());
        }
        self.current = current.clone();
        Ok(())
    }
    fn pane_ids(&self) -> Vec<PaneId> {
        self.trace.borrow_mut().push("refresh");
        vec![self.pane]
    }
    fn pane_agent(&self, _pane: PaneId) -> Option<AgentInfo> {
        self.trace.borrow_mut().push("read old agent");
        Some(self.current.clone())
    }
    fn pane_was_destroyed(&self, _pane: PaneId) -> bool {
        false
    }
    fn send_control_reply(
        &self,
        _client: yinz_types::internal_ids::ClientId,
        response: yinz_types::agent::control::AgentResponse,
    ) {
        self.trace
            .borrow_mut()
            .push(if response.submission_id.is_some() {
                "wait reply"
            } else {
                "reply"
            });
        self.replies.borrow_mut().push(response);
    }
    fn remove_event_subscriber(&self, _client: yinz_types::internal_ids::ClientId) {
        unreachable!("unexpected host operation")
    }
    fn resolve_event_projects(
        &self,
        _version: u64,
        _agents: Vec<yinz_types::agent::control::AgentStatus>,
        _projects: Vec<String>,
    ) {
        unreachable!("unexpected host operation")
    }
    fn state_hooks(
        &self,
        _pane: PaneId,
        previous: Option<&AgentInfo>,
        current: Option<&AgentInfo>,
    ) {
        assert_eq!(
            previous.and_then(|agent| agent.session_id.as_deref()),
            Some("old")
        );
        assert_eq!(current, Some(&self.current));
        self.trace.borrow_mut().push("hooks");
    }
    fn explicit_close_is_single_pane(&self, _pane: PaneId) -> bool {
        unreachable!("unexpected host operation")
    }
    fn dump_screen(&mut self, _pane: PaneId) -> Option<String> {
        unreachable!("unexpected host operation")
    }
    fn event_transport_available(&self) -> bool {
        true
    }
    fn send_event(&self, _client: yinz_types::internal_ids::ClientId, event: String) -> bool {
        self.events
            .borrow_mut()
            .push(serde_json::from_str(&event).expect("valid lifecycle event"));
        true
    }
    fn subscription_error(&self, _client: yinz_types::internal_ids::ClientId, _message: String) {
        unreachable!("unexpected host operation")
    }
    fn local_agent_directories(&self) -> std::collections::HashMap<PaneId, std::path::PathBuf> {
        unreachable!("unexpected host operation")
    }
    fn set_worktree_roots(
        &mut self,
        _roots: std::collections::HashMap<PaneId, std::path::PathBuf>,
    ) {
        unreachable!("unexpected host operation")
    }
    fn worktree_sender(
        &self,
    ) -> Option<yinz_runtime::channels::SenderWithContext<yinz_bus::screen::ScreenInstruction>>
    {
        unreachable!("unexpected host operation")
    }
    fn set_agent(&mut self, pane: PaneId, agent: Option<AgentInfo>) -> bool {
        assert_eq!(pane, self.pane);
        let agent = agent.expect("live pane");
        let changed = self.current != agent;
        self.current = agent;
        changed
    }
    fn write_key(
        &mut self,
        _pane: PaneId,
        _key: yinz_types::keys::KeyWithModifier,
        _completion: Option<yinz_bus::route::NotificationEnd>,
    ) -> yinz_runtime::errors::prelude::Result<()> {
        assert!(self.live_input);
        self.trace.borrow_mut().push("write_key");
        self.input_revision += 1;
        Ok(())
    }
    fn write_bytes(
        &mut self,
        _pane: PaneId,
        _bytes: Vec<u8>,
        _completion: Option<yinz_bus::route::NotificationEnd>,
    ) -> yinz_runtime::errors::prelude::Result<()> {
        assert!(self.live_input);
        self.trace.borrow_mut().push("write_bytes");
        self.input_revision += 1;
        Ok(())
    }
    fn paste(
        &mut self,
        _pane: PaneId,
        _bytes: Vec<u8>,
        _completion: Option<yinz_bus::route::NotificationEnd>,
    ) -> yinz_runtime::errors::prelude::Result<()> {
        assert!(self.live_input);
        self.trace.borrow_mut().push("paste");
        self.input_revision += 1;
        Ok(())
    }
    fn write_command(
        &mut self,
        _pane: PaneId,
        _prefix: Vec<u8>,
        _paste: Vec<u8>,
        _completion: yinz_bus::route::NotificationEnd,
    ) -> yinz_runtime::errors::prelude::Result<()> {
        unreachable!("unexpected host operation")
    }
    fn instance_id(&self) -> uuid::Uuid {
        unreachable!("unexpected host operation")
    }
    fn held_agent_conversation(&self, _pane: PaneId) -> Option<Option<&str>> {
        unreachable!("unexpected host operation")
    }
    fn pane_is_agent(&self, _pane: PaneId) -> bool {
        unreachable!("unexpected host operation")
    }
    fn set_suppressed(&mut self, _pane: PaneId, _suppress: bool) -> Result<(), &'static str> {
        unreachable!("unexpected host operation")
    }
}

#[test]
fn restart_prepares_refreshes_and_retires_only_on_acceptance() {
    for (prepare, apply) in [(false, false), (true, false), (true, true)] {
        let mut agents = AgentController::default();
        let mut host = RecordingHost::new();
        let pane = host.pane;
        host.reject_preparation = !prepare;
        host.reject_application = !apply;
        agents
            .agent_terminal_claims
            .insert(pane, (4, "claude".into(), "old".into()));
        let id = "first-turn".to_owned();
        agents
            .agent_submissions
            .entry(pane)
            .or_default()
            .push_back(super::Submission {
                id: id.clone(),
                kind: host.current.kind.clone(),
                conversation: host.current.session_id.clone(),
                acknowledged: true,
                turn_id: Some("turn".into()),
                prompt_delivery: None,
                result: None,
            });
        let now = Instant::now();
        assert!(matches!(
            agents.wait_for_turn(
                &host,
                pane,
                Reply::Client(ClientId::from(2)),
                now,
                id,
                30_000,
                host.current.session_id.clone()
            ),
            Some(Completion::Refresh)
        ));
        agents.subscribe_agent_events(&host, ClientId::from(3), Default::default());
        let generation = host
            .events
            .borrow()
            .iter()
            .find(|event| event["event_type"] == "snapshot")
            .expect("old pane snapshot")["pane_generation"]
            .clone();
        host.events.borrow_mut().clear();
        host.trace.borrow_mut().clear();
        let result = agents.handle(
            &mut host,
            AgentRequest::Restart {
                pane_id: pane,
                session: AgentRestartSession::Fresh,
                prompt: None,
            },
            Reply::Client(ClientId::from(1)),
            Instant::now(),
        );
        let expected = if !prepare {
            vec!["prepare", "reply"]
        } else if !apply {
            vec!["prepare", "refresh", "read old agent", "apply", "reply"]
        } else {
            vec![
                "prepare",
                "refresh",
                "read old agent",
                "apply",
                "wait reply",
                "hooks",
            ]
        };
        assert_eq!(*host.trace.borrow(), expected);
        assert_eq!(agents.agent_terminal_claims.contains_key(&pane), !apply);
        if apply {
            let Some(Completion::Restarted { response, .. }) = result else {
                panic!("missing restart completion")
            };
            assert_eq!(response.restarted.unwrap().pane_id, pane);
            assert!(agents.agent_controls.is_empty());
            let replies = host.replies.borrow();
            assert_eq!(replies.len(), 1);
            assert_eq!(replies[0].outcome, AgentOutcome::TargetChanged);
            assert_eq!(replies[0].submission_id.as_deref(), Some("first-turn"));
            agents.flush_agent_events(&host);
            let events = host.events.borrow();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["event_type"], "pane_replaced");
            assert_eq!(events[0]["pane_generation"], generation);
            assert_eq!(events[0]["conversation_id"], "old");
        } else {
            assert!(result.is_none());
            assert_eq!(agents.agent_controls.len(), 1);
            agents.flush_agent_events(&host);
            assert!(host.events.borrow().is_empty());
            let replies = host.replies.borrow();
            assert_eq!(replies.len(), 1);
            assert_eq!(replies[0].outcome, AgentOutcome::Rejected);
            assert_eq!(
                replies[0].message.as_deref(),
                Some(if prepare {
                    "pane refused restart"
                } else {
                    "no held command"
                })
            );
            assert_eq!(host.current.session_id.as_deref(), Some("old"));
        }
    }
}

#[test]
fn surviving_closeout_target_receives_one_refusal() {
    let agents = AgentController::default();
    let host = RecordingHost::new();
    AgentClose::controller(host.pane, Reply::Client(ClientId::from(1))).finish(
        &agents,
        &host,
        host.pane_info(host.pane).is_some(),
    );
    let replies = host.replies.borrow();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].outcome, AgentOutcome::Rejected);
    assert_eq!(
        replies[0].message.as_deref(),
        Some("Controller pane could not be closed")
    );
}

#[test]
fn closed_submission_retention_keeps_the_newest_1024_and_expires_at_one_day() {
    use std::time::Duration;
    use yinz_types::agent::control::{PromptDelivery, RetainedSubmission};
    let mut agents = AgentController::default();
    let mut host = RecordingHost::new();
    let closed_pane = PaneId::Terminal(TerminalId::new(8));
    let now = Instant::now();
    for index in 0..=1024 {
        let proof = RetainedSubmission {
            server_instance: "test-instance".into(),
            session: host.session_name().into(),
            pane_id: closed_pane,
            kind: host.current.kind.clone(),
            cwd: "/work".into(),
            conversation_id: "old".into(),
            submission_id: format!("submission-{index}"),
            turn_id: None,
            prompt_id: "prompt".into(),
            acknowledged: true,
            prompt_delivery: PromptDelivery::Started,
            outcome: AgentOutcome::Completed,
            message: Some("DONE".into()),
            answered: true,
            quiescent: true,
        };
        agents
            .closed_agent_submissions
            .push_back((now, proof, 4, None));
    }
    for (id, age, outcome) in [
        ("submission-0", 86399, AgentOutcome::TargetClosed),
        ("submission-1", 86399, AgentOutcome::Completed),
        ("submission-1", 86400, AgentOutcome::TargetClosed),
    ] {
        let request = AgentRequest::WaitTurn {
            pane_id: closed_pane,
            expected_session_id: Some("old".into()),
            submission_id: id.into(),
            timeout_ms: 1,
        };
        assert!(agents
            .handle(
                &mut host,
                request,
                Reply::Client(ClientId::from(1)),
                now + Duration::from_secs(age),
            )
            .is_none());
        let replies = host.replies.take();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].outcome, outcome);
        assert_eq!(replies[0].submission_id.as_deref(), Some(id));
        if age < 86400 {
            assert_eq!(agents.closed_agent_submissions.len(), 1024);
            assert_eq!(
                agents.closed_agent_submissions[0].1.submission_id,
                "submission-1"
            );
        }
    }
    assert!(agents.closed_agent_submissions.is_empty());
}

#[test]
fn claude_permission_wait_requires_the_waiting_conversation_and_keeps_its_input_revision() {
    use yinz_types::agent::HookEvent;

    let mut agents = AgentController::default();
    let host = RecordingHost::new();
    let pane = host.pane;
    let waiting = AgentInfo {
        state: AgentState::Waiting,
        ..host.current.clone()
    };
    let mut hook = HookEvent {
        hook_event_name: "Notification".into(),
        notification_type: Some("permission_prompt".into()),
        session_id: "other".into(),
        ..Default::default()
    };
    for (conversation, agent, accepted) in [
        ("other", &waiting, false),
        ("old", &host.current, false),
        ("old", &waiting, true),
    ] {
        hook.session_id = conversation.into();
        let wait = agents.claude.report_permission_wait(
            &host,
            pane,
            "claude",
            Some(&hook),
            Some(agent),
            false,
        );
        assert_eq!(wait.is_some(), accepted);
        agents.claude.restore_permission_wait(pane, wait);
    }
    let expected = agents.claude.permission_waits[&pane].clone();
    assert_eq!(expected.conversation, "old");
    assert_eq!(expected.input_revision, host.agent_input_revision(pane));

    let notice = agents
        .claude
        .report_permission_wait(&host, pane, "claude", None, None, true);
    assert!(agents.claude.permission_waits.is_empty());
    agents.claude.restore_permission_wait(pane, notice);
    assert_eq!(agents.claude.permission_waits.get(&pane), Some(&expected));
}

fn observe(agents: &mut AgentController, host: &RecordingHost, hook: &super::HookEvent) {
    let event = super::AgentEvent::from_hook(
        &host.current.kind,
        TerminalId::new(7),
        hook,
        serde_json::to_string(hook).unwrap(),
    );
    agents.observe_control_hook(host, &event, Some(hook));
}

fn hook(name: &str) -> super::HookEvent {
    super::HookEvent {
        source: (name == "SessionStart").then(|| "startup".into()),
        hook_event_name: name.into(),
        session_id: "old".into(),
        ..Default::default()
    }
}

#[test]
fn codex_cleanup_discards_pending_dialog_input() {
    use yinz_types::keys::{BareKey, KeyWithModifier};

    for cleanup in ["recover", "invalidate", "close", "restart", "unverified"] {
        let mut agents = AgentController::default();
        let mut host = RecordingHost::new();
        let pane = host.pane;
        host.live_input = true;
        host.input_revision = 0;
        host.current.kind = "codex".into();
        observe(&mut agents, &host, &hook("SessionStart"));
        host.current.state = AgentState::Waiting;
        host.current.wait_needs_a_person = true;
        observe(&mut agents, &host, &hook("PermissionRequest"));
        let request = AgentRequest::SendKeys {
            pane_id: pane,
            expected_session_id: Some("old".into()),
            keys: vec![KeyWithModifier::new(BareKey::Enter)],
        };
        assert!(agents
            .handle(
                &mut host,
                request,
                Reply::Client(ClientId::new(1)),
                Instant::now(),
            )
            .is_none());
        assert_eq!(host.replies.take()[0].outcome, AgentOutcome::Ok);
        assert_eq!(agents.agent_dialog_inputs[&pane].input_revision, 1);

        match cleanup {
            "recover" => agents.finish_recovery(pane),
            "invalidate" => agents.invalidate_agent_lifecycle(&host, pane, "test"),
            "close" => agents.invalidate_agent_lifecycle(&host, pane, "pane_closed"),
            "restart" => observe(&mut agents, &host, &hook("SessionStart")),
            "unverified" => observe(
                &mut agents,
                &host,
                &super::HookEvent {
                    codex_hooks_ready: Some(false),
                    ..hook("SessionStart")
                },
            ),
            _ => unreachable!(),
        }
        assert!(!agents.agent_dialog_inputs.contains_key(&pane), "{cleanup}");
    }
}

#[test]
fn expiry_preserves_wait_ownership_and_run_identity_through_handoff() {
    use std::time::Duration;
    use yinz_types::agent::{
        claude_notifications::{Evidence, Launch, Notice, Obligation, Source, TaskKind},
        tracking::{ClaudeNotification, PermissionWait, WorkerWaits},
    };
    for waiting in [false, true] {
        let mut agents = AgentController::default();
        let mut host = RecordingHost::new();
        host.live_input = true;
        let pane = host.pane;
        observe(&mut agents, &host, &hook("SessionStart"));
        let parent = host.current.clone();
        let waiting_agent = AgentInfo {
            state: AgentState::Waiting,
            waiting_on: Some("permission".into()),
            wait_needs_a_person: true,
            ..parent.clone()
        };
        let lifecycle = agents.agent_lifecycle.get_mut(&pane).unwrap();
        lifecycle.last_message = Some("LAUNCH".into());
        lifecycle.claude.activity.parent = Some(parent.clone());
        let now = lifecycle.claude.observed_at;
        let obligations = &mut lifecycle.claude.activity.obligations;
        let source = Source { pid: 1, started: 2 };
        let notice = Notice {
            task: "first".into(),
            tool: "tool-first".into(),
            prompt: "first".into(),
            run: "run".into(),
            row: "row".into(),
            queued: false,
        };
        obligations.source = Some(source.clone());
        obligations.notices.push(notice.clone());
        for (task, remaining_ms) in [("first", 10), ("second", 20)] {
            obligations.launches.push(Obligation {
                launch: Launch {
                    prompt: "prompt".into(),
                    tool: format!("tool-{task}"),
                    task: task.into(),
                    kind: TaskKind::Bash,
                },
                answer_prompt: Some(task.into()),
                remaining_ms,
                unavailable: false,
                settled: false,
                drained: None,
            });
        }
        lifecycle.claude.notifications.insert(
            "first".into(),
            ClaudeNotification {
                observation_unavailable: false,
                waiting: waiting.then(|| waiting_agent.clone()),
                permission_revision: waiting.then(|| host.agent_input_revision(pane)),
            },
        );
        let permission = PermissionWait {
            conversation: "old".into(),
            input_revision: 4,
        };
        agents
            .claude
            .permission_waits
            .insert(pane, permission.clone());
        let workers = WorkerWaits {
            parent: parent.clone(),
            waits: [("worker".into(), Some("question".into()))].into(),
        };
        agents.agent_worker_waits.insert(pane, workers.clone());

        agents.expire_claude_observations(now + Duration::from_millis(9));
        assert!(
            !agents.agent_lifecycle[&pane].claude.notifications["first"].observation_unavailable
        );
        agents.expire_claude_observations(now + Duration::from_millis(10));
        let state = &agents.agent_lifecycle[&pane];
        let launches = &state.claude.activity.obligations.launches;
        assert!(launches[0].unavailable && !launches[0].settled);
        assert_eq!(launches[0].answer_prompt.as_deref(), Some("first"));
        assert!(!launches[1].unavailable && !launches[1].settled);
        assert_eq!(launches[1].remaining_ms, 10);
        let notification = state.claude.notifications.get("first");
        assert_eq!(notification.is_some(), waiting);
        if let Some(notice) = notification {
            assert!(notice.observation_unavailable);
            assert_eq!(notice.waiting.as_ref(), Some(&waiting_agent));
            assert_eq!(notice.permission_revision, Some(4));
        }

        let expected_notifications = state.claude.notifications.clone();
        let transfer = agents.capture_agent_tracking(&host, pane).unwrap();
        let expected_launches = &transfer
            .lifecycle
            .as_ref()
            .unwrap()
            .claude
            .obligations
            .launches;
        let wire = serde_json::to_vec(&transfer).unwrap();
        let mut destination = AgentController::default();
        destination.stage_transfer_identity(
            pane,
            (
                Some(parent.clone()),
                Some(serde_json::from_slice(&wire).unwrap()),
            ),
        );
        assert!(destination.restore_transfer_state(&mut host, pane));
        let restored = &destination.agent_lifecycle[&pane];
        assert_eq!(restored.last_message.as_deref(), Some("LAUNCH"));
        assert_eq!(restored.claude.activity.parent.as_ref(), Some(&parent));
        assert_eq!(restored.claude.notifications, expected_notifications);
        assert_eq!(
            &restored.claude.activity.obligations.launches,
            expected_launches
        );
        let replay = super::HookEvent {
            claude_notifications: Some(Evidence {
                source: Some(source),
                notice: Some(Notice {
                    prompt: "duplicate".into(),
                    ..notice.clone()
                }),
                ..Default::default()
            }),
            ..hook("Stop")
        };
        destination.retain_claude_evidence(pane, &replay);
        let restored = &destination.agent_lifecycle[&pane];
        assert_eq!(restored.claude.activity.obligations.notices, [notice]);
        assert_eq!(restored.claude.notifications, expected_notifications);
        assert_eq!(destination.agent_worker_waits[&pane], workers);
        assert_eq!(destination.claude.permission_waits[&pane], permission);
    }
}

#[test]
fn previous_prompt_blocks_agy_replay_only_after_confirmation_in_that_conversation() {
    let mut agents = AgentController::default();
    let mut host = RecordingHost::new();
    host.current.kind = "agy".into();
    let submit = |id: &str| super::HookEvent {
        prompt_id: Some(id.into()),
        ..hook("UserPromptSubmit")
    };
    observe(&mut agents, &host, &submit("first"));
    observe(&mut agents, &host, &submit("unconfirmed"));
    assert_eq!(agents.agent_lifecycle[&host.pane].previous_prompt_id, None);
    observe(
        &mut agents,
        &host,
        &super::HookEvent {
            prompt_id: Some("unconfirmed".into()),
            agy_turn_complete: Some(true),
            ..hook("Stop")
        },
    );
    observe(&mut agents, &host, &submit("new"));
    assert_eq!(
        agents.agent_lifecycle[&host.pane]
            .previous_prompt_id
            .as_deref(),
        Some("unconfirmed")
    );
    assert!(agents.ignore_agy_confirmation(host.pane, &submit("unconfirmed")));
    assert!(!agents.ignore_agy_confirmation(host.pane, &submit("next")));
    assert!(!agents.ignore_agy_confirmation(
        host.pane,
        &super::HookEvent {
            session_id: "other".into(),
            ..submit("unconfirmed")
        }
    ));
}

#[test]
fn held_completion_freezes_one_proof_and_its_original_expiry() {
    use std::time::Duration;
    use yinz_types::agent::{
        control::PromptDelivery,
        tracking::{Submission, SubmissionResult},
    };
    let mut host = RecordingHost::new();
    host.live_input = true;
    host.current.cwd = Some("/work".into());
    let pane = host.pane;
    let mut agents = AgentController::default();
    observe(&mut agents, &host, &hook("SessionStart"));
    agents
        .agent_lifecycle
        .get_mut(&pane)
        .unwrap()
        .input_revision = Some(4);
    agents
        .agent_terminal_claims
        .insert(pane, (4, "work".into(), "prompt".into()));
    agents
        .agent_submissions
        .entry(pane)
        .or_default()
        .push_back(Submission {
            id: "work".into(),
            kind: "claude".into(),
            conversation: Some("old".into()),
            acknowledged: true,
            prompt_delivery: Some(PromptDelivery::Started),
            turn_id: Some("turn".into()),
            result: Some(SubmissionResult {
                outcome: AgentOutcome::Completed,
                message: Some("DONE".into()),
                background: false,
            }),
        });
    let now = Instant::now();
    agents.retain_closed_agent(&host, pane, now);
    let frozen = agents.closed_agent_submissions.clone();
    assert_eq!(frozen.len(), 1);
    assert_eq!(frozen[0].0, now);
    assert_eq!(frozen[0].1.message.as_deref(), Some("DONE"));
    agents.retain_closed_agent(&host, pane, now + Duration::from_secs(1));
    agents.invalidate_agent_lifecycle(&host, pane, "held");
    host.current.state = AgentState::Exited;
    agents.invalidate_agent_lifecycle(&host, pane, "pane_closed");
    assert_eq!(agents.closed_agent_submissions, frozen);
    assert!(!agents.agent_lifecycle.contains_key(&pane));
    assert!(!agents.agent_submissions.contains_key(&pane));
    agents.prune_closed_agents(now + Duration::from_secs(86400));
    assert!(agents.closed_agent_submissions.is_empty());
}

#[test]
fn recent_history_keeps_old_pending_submissions_and_live_waiters() {
    use super::{Submission, SubmissionResult, SUBMISSION_HISTORY_LIMIT};
    let mut agents = AgentController::default();
    let mut host = RecordingHost::new();
    host.live_input = true;
    let pane = host.pane;
    let now = Instant::now();
    let submission = |id: &str| Submission {
        id: id.into(),
        kind: host.current.kind.clone(),
        conversation: host.current.session_id.clone(),
        acknowledged: true,
        prompt_delivery: None,
        turn_id: Some("turn".into()),
        result: None,
    };
    let history = agents.agent_submissions.entry(pane).or_default();
    history.push_back(submission("waited"));
    history.push_back(submission("unresolved"));
    for i in 0..SUBMISSION_HISTORY_LIMIT {
        history.push_back(submission(&format!("recent-{i}")));
    }
    for client in [1, 2] {
        agents.wait_for_turn(
            &host,
            pane,
            Reply::Client(ClientId::new(client)),
            now,
            "waited".into(),
            100,
            Some("old".into()),
        );
    }
    agents.submission_mut(pane, "waited").unwrap().result = Some(SubmissionResult {
        outcome: AgentOutcome::Completed,
        message: Some("done".into()),
        background: false,
    });
    for client in [1, 2] {
        assert_eq!(agents.agent_controls.len(), 3 - client as usize);
        agents.prune_agent_submissions(&agents.protected_submissions());
        assert_eq!(
            agents.agent_submissions[&pane].len(),
            SUBMISSION_HISTORY_LIMIT + 2
        );
        assert!(agents.submission(pane, "waited").unwrap().result.is_some());
        agents.detach_agent_controls(&host, ClientId::new(client));
    }
    assert!(agents.agent_controls.is_empty());
    assert!(agents.submission(pane, "waited").is_none());
    assert!(agents.submission(pane, "unresolved").is_some());
    agents.submission_mut(pane, "unresolved").unwrap().result = Some(SubmissionResult {
        outcome: AgentOutcome::Interrupted,
        message: None,
        background: false,
    });
    agents.prune_agent_submissions(&agents.protected_submissions());
    assert_eq!(
        agents.agent_submissions[&pane].len(),
        SUBMISSION_HISTORY_LIMIT
    );
    assert_eq!(
        agents.agent_submissions[&pane].front().unwrap().id,
        "recent-0"
    );
}

#[test]
fn passive_permission_samples_preserve_the_report_fence_and_turn() {
    use std::time::Duration;
    use yinz_types::agent::{
        claude_notifications::Source,
        claude_permissions::{Sample, Status, EVENT},
    };
    for status in [Status::Inactive, Status::Unavailable, Status::Active] {
        let mut agents = AgentController::default();
        let host = RecordingHost::new();
        let pane = host.pane;
        observe(&mut agents, &host, &hook("SessionStart"));
        let baseline = Instant::now() - Duration::from_secs(1);
        let state = agents.agent_lifecycle.get_mut(&pane).unwrap();
        state.last_report = baseline;
        state.input_revision = Some(3);
        state.claude.activity.parent = Some(host.current.clone());
        state.claude.confirmation = Some(true);
        observe(
            &mut agents,
            &host,
            &super::HookEvent {
                claude_permission: Some(Sample {
                    source: Source {
                        pid: 123,
                        started: 456,
                    },
                    updated_ms: 500_001,
                    observed_ms: 500_001,
                    status,
                }),
                ..hook(EVENT)
            },
        );
        let state = &agents.agent_lifecycle[&pane];
        assert_eq!(state.last_report > baseline, status == Status::Active);
        assert_eq!(state.input_revision, Some(3));
        assert_eq!(state.claude.activity.parent.as_ref(), Some(&host.current));
        assert_eq!(state.claude.confirmation, Some(true));
    }
}

#[test]
fn permission_enter_restores_the_raw_parent_and_preserves_worker_waits() {
    use super::{PermissionWait, WorkerWaits};
    for confirmed in [None, Some(false), Some(true)] {
        let mut agents = AgentController::default();
        let mut host = RecordingHost::new();
        let pane = host.pane;
        host.current.state = AgentState::Waiting;
        host.current.waiting_on = Some("permission".into());
        host.current.detail = Some("Allow Bash?".into());
        host.current.wait_needs_a_person = true;
        observe(&mut agents, &host, &hook("SessionStart"));
        let state = agents.agent_lifecycle.get_mut(&pane).unwrap();
        state.claude.confirmation = confirmed;
        state.claude.activity.parent = Some(host.current.clone());
        let waits = [("worker".to_owned(), Some("question".to_owned()))].into();
        agents.agent_worker_waits.insert(
            pane,
            WorkerWaits {
                parent: host.current.clone(),
                waits,
            },
        );
        let wait = PermissionWait {
            conversation: "old".into(),
            input_revision: 4,
        };
        assert!(agents
            .advance_permission_wait(&host, pane, wait.clone())
            .is_none());
        assert_eq!(agents.claude.permission_waits.get(&pane), Some(&wait));
        host.enter_revision = 5;
        for (pane, wait) in agents.take_permission_waits() {
            let parent = agents.advance_permission_wait(&host, pane, wait).unwrap();
            assert_eq!(
                parent.state,
                if confirmed == Some(true) {
                    AgentState::Idle
                } else {
                    AgentState::Busy
                }
            );
            assert!(
                parent.detail.is_none()
                    && parent.waiting_on.is_none()
                    && !parent.wait_needs_a_person
            );
            assert_eq!(
                agents.agent_lifecycle[&pane]
                    .claude
                    .activity
                    .parent
                    .as_ref(),
                Some(&parent)
            );
            assert_eq!(agents.agent_worker_waits[&pane].parent, parent);
        }
        assert!(agents.claude.permission_waits.is_empty());
        assert_eq!(
            agents.agent_worker_waits[&pane].waits["worker"].as_deref(),
            Some("question")
        );
    }
}

#[test]
fn prompt_claim_and_event_identity_follow_delivery_acceptance_and_completion() {
    use super::{AgentEvent, HookEvent};
    use std::time::Duration;
    let mut agents = AgentController::default();
    let mut host = RecordingHost::new();
    host.live_input = true;
    host.input_revision = 0;
    let pane = host.pane;
    observe(&mut agents, &host, &hook("SessionStart"));
    let now = Instant::now();
    agents.handle(
        &mut host,
        AgentRequest::Prompt {
            pane_id: pane,
            expected_session_id: Some("old".into()),
            prompt: "review".into(),
            preserve_draft: false,
            submission_id: Some("work".into()),
            wait: true,
            timeout_ms: 30000,
        },
        Reply::Client(ClientId::new(1)),
        now,
    );
    assert!(agents.agent_draft_clears.contains_key(&pane));
    assert!(!agents.submission(pane, "work").unwrap().acknowledged);
    for offset in [0, 1, 251] {
        agents.advance_draft_clear(&mut host, pane, now + Duration::from_millis(offset));
    }
    assert!(!agents.agent_draft_clears.contains_key(&pane));
    assert_eq!(agents.agent_controls.len(), 1);
    let submit = HookEvent {
        prompt_id: Some("p".into()),
        turn_id: Some("t".into()),
        prompt: Some("review".into()),
        ..hook("UserPromptSubmit")
    };
    let event = |hook: &HookEvent| {
        AgentEvent::from_hook(
            "claude",
            TerminalId::new(7),
            hook,
            serde_json::to_string(hook).unwrap(),
        )
    };
    assert_eq!(agents.event_submission(pane, &submit), None);
    agents.advance_pending_controls(&mut host, now + Duration::from_secs(1));
    agents.advance_pending_controls(&mut host, now + Duration::from_secs(2));
    agents.correlate_agent_submissions(&host, &event(&submit), Some(&submit));
    assert_eq!(
        agents.event_submission(pane, &submit).as_deref(),
        Some("work")
    );
    assert!(agents.submission(pane, "work").unwrap().acknowledged);
    assert_eq!(agents.agent_controls.len(), 1);
    let stop = HookEvent {
        hook_event_name: "Stop".into(),
        claude_turn_complete: Some(true),
        last_assistant_message: Some("DONE".into()),
        ..submit.clone()
    };
    agents.correlate_agent_submissions(&host, &event(&stop), Some(&stop));
    assert_eq!(agents.event_submission(pane, &submit), None);
    assert!(agents.agent_controls.is_empty());
    let result = agents
        .submission(pane, "work")
        .unwrap()
        .result
        .as_ref()
        .unwrap();
    assert_eq!(result.outcome, AgentOutcome::Completed);
    assert_eq!(result.message.as_deref(), Some("DONE"));
}

#[test]
fn replacing_the_provider_retires_its_native_waits_and_blocks_old_replays() {
    use super::{ClaudeNotification, ClearInput, DialogInput, PermissionWait, WorkerWaits};
    use yinz_types::agent::claude_notifications::{Evidence, Source};
    let mut agents = AgentController::default();
    let mut host = RecordingHost::new();
    host.live_input = true;
    let pane = host.pane;
    observe(&mut agents, &host, &hook("SessionStart"));
    let old = Source { pid: 4, started: 5 };
    let new = Source { pid: 6, started: 7 };
    let state = agents.agent_lifecycle.get_mut(&pane).unwrap();
    state.claude.activity.obligations.source = Some(old.clone());
    state.last_message = Some("OLD".into());
    state.claude.notifications.insert(
        "notice".into(),
        ClaudeNotification {
            observation_unavailable: false,
            waiting: Some(host.current.clone()),
            permission_revision: Some(4),
        },
    );
    state
        .claude
        .activity
        .worker_wait_tools
        .entry("worker".into())
        .or_default()
        .insert("question".into());
    let waits = WorkerWaits {
        parent: host.current.clone(),
        waits: [("worker".into(), Some("question".into()))].into(),
    };
    agents.agent_worker_waits.insert(pane, waits.clone());
    agents.claude.permission_waits.insert(
        pane,
        PermissionWait {
            conversation: "old".into(),
            input_revision: 4,
        },
    );
    agents.agent_dialog_inputs.insert(
        pane,
        DialogInput {
            kind: "claude".into(),
            conversation: Some("old".into()),
            input_revision: 4,
            startup: false,
            fresh_waiting: false,
        },
    );
    agents.agent_clear_inputs.insert(
        pane,
        ClearInput {
            kind: "claude".into(),
            conversation: "old".into(),
            input_revision: 4,
            deadline: Instant::now(),
            session_ended: false,
        },
    );
    let resume = |source: Source| super::HookEvent {
        claude_notifications: Some(Evidence {
            source: Some(source),
            ..Default::default()
        }),
        source: Some("resume".into()),
        ..hook("SessionStart")
    };
    for (report, accepted) in [
        (resume(old.clone()), true),
        (
            super::HookEvent {
                archive_only: true,
                ..resume(new.clone())
            },
            false,
        ),
        (
            super::HookEvent {
                agent_id: Some("worker".into()),
                ..resume(new.clone())
            },
            false,
        ),
    ] {
        assert_eq!(agents.accept_claude_source(&host, pane, &report), accepted);
        let state = &agents.agent_lifecycle[&pane];
        assert_eq!(
            state.claude.activity.obligations.source.as_ref(),
            Some(&old)
        );
        assert!(state.claude.notifications.contains_key("notice"));
        assert!(state.claude.activity.worker_wait_tools["worker"].contains("question"));
        assert_eq!(state.last_message.as_deref(), Some("OLD"));
        assert_eq!(agents.agent_worker_waits.get(&pane), Some(&waits));
        assert!(agents.agent_dialog_inputs.contains_key(&pane));
        assert!(agents.agent_clear_inputs.contains_key(&pane));
        assert!(agents.claude.permission_waits.contains_key(&pane));
    }
    assert!(agents.accept_claude_source(&host, pane, &resume(new.clone())));
    assert!(!agents.accept_claude_source(&host, pane, &resume(old)));
    let state = &agents.agent_lifecycle[&pane];
    assert_eq!(
        state.claude.activity.obligations.source.as_ref(),
        Some(&new)
    );
    assert!(state.last_message.is_none());
    assert!(state.claude.notifications.is_empty());
    assert!(state.claude.activity.worker_wait_tools.is_empty());
    assert!(!agents.agent_worker_waits.contains_key(&pane));
    assert!(!agents.agent_dialog_inputs.contains_key(&pane));
    assert!(!agents.agent_clear_inputs.contains_key(&pane));
    assert!(!agents.claude.permission_waits.contains_key(&pane));
}

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
        0
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
    fn tab_for_pane(&self, _pane: PaneId) -> Option<TabId> {
        None
    }
    fn terminal_cwd(&self, _pane: PaneId) -> Option<String> {
        None
    }
    fn foreground_command(&self, _id: TerminalId) -> Option<&[String]> {
        unreachable!("unexpected host operation")
    }
    fn last_output(&self, _pane: PaneId) -> Option<Instant> {
        unreachable!("unexpected host operation")
    }
    fn pane_input(&self, _pane: PaneId) -> Option<PaneInput> {
        None
    }
    fn viewport(&self, _pane: PaneId, _ansi: bool) -> Option<(usize, PaneContents)> {
        unreachable!("unexpected host operation")
    }
    fn agent_input_revision(&self, _pane: PaneId) -> u64 {
        4
    }
    fn agent_input_at(&self, _pane: PaneId) -> Option<Instant> {
        unreachable!("unexpected host operation")
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
    fn set_agent(&mut self, _pane: PaneId, _agent: Option<AgentInfo>) -> bool {
        unreachable!("unexpected host operation")
    }
    fn write_key(
        &mut self,
        _pane: PaneId,
        _key: yinz_types::keys::KeyWithModifier,
        _completion: Option<yinz_bus::route::NotificationEnd>,
    ) -> yinz_runtime::errors::prelude::Result<()> {
        unreachable!("unexpected host operation")
    }
    fn write_bytes(
        &mut self,
        _pane: PaneId,
        _bytes: Vec<u8>,
        _completion: Option<yinz_bus::route::NotificationEnd>,
    ) -> yinz_runtime::errors::prelude::Result<()> {
        unreachable!("unexpected host operation")
    }
    fn paste(
        &mut self,
        _pane: PaneId,
        _bytes: Vec<u8>,
        _completion: Option<yinz_bus::route::NotificationEnd>,
    ) -> yinz_runtime::errors::prelude::Result<()> {
        unreachable!("unexpected host operation")
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

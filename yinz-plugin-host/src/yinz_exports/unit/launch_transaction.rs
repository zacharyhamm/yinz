use super::*;

#[test]
fn launch_transaction_journal_scopes_and_guards_the_authors_revision() {
    let directory = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(directory.path());
    let senders = ThreadSenders::default();
    let journal = Journal::new(&data, &senders, "test").unwrap();
    journal.ensure().unwrap();
    let receipt = fresh("test", "digest".into());
    let mut revision = REV_ABSENT;
    journal.write(&receipt, &mut revision).unwrap();
    let (read, read_revision) = journal.read().unwrap().unwrap();
    assert_eq!(read, receipt);
    assert_eq!(revision, read_revision);
    let mut stale = REV_ABSENT;
    assert!(journal.write(&receipt, &mut stale).is_err());
    assert_eq!(journal.read().unwrap().unwrap().0, receipt);
}

#[test]
fn launch_transaction_unknown_observation_has_no_side_effects_or_exec_permission() {
    let directory = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(directory.path());
    let senders = ThreadSenders::default();
    let response = status("lost", &senders, &data);
    let receipt = response.launch.unwrap();
    assert_eq!(receipt.outcome, LaunchOutcome::Unknown);
    assert_eq!(receipt.reason, Some(LaunchReason::HistoryUnavailable));
    assert!(senders.launches.gates.lock().unwrap().is_empty());
}

#[test]
fn launch_transaction_restart_and_conflict_never_replay_preparation() {
    let directory = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(directory.path());
    let senders = ThreadSenders::default();
    let request = LaunchRequest {
        placement: Default::default(),
        spec: Default::default(),
        cwd: None,
        project: None,
        remote_host: None,
        new_project: None,
    };
    let journal = Journal::new(&data, &senders, "old.intent").unwrap();
    journal.ensure().unwrap();
    let mut original = fresh("old.intent", request.digest().unwrap());
    original.server_instance = "retired".into();
    original.phase = LaunchPhase::Starts;
    let mut revision = REV_ABSENT;
    journal.write(&original, &mut revision).unwrap();
    let observed = launch(
        "old.intent".into(),
        request.clone(),
        crate::ClientId::new(1),
        &senders,
        &data,
    )
    .launch
    .unwrap();
    assert_eq!(observed.outcome, LaunchOutcome::Unknown);
    let mut different = request;
    different.cwd = Some("/different".into());
    let conflict = launch(
        "old.intent".into(),
        different,
        crate::ClientId::new(1),
        &senders,
        &data,
    )
    .launch
    .unwrap();
    assert_eq!(conflict.outcome, LaunchOutcome::Conflict);
    assert_eq!(journal.read().unwrap().unwrap().0, original);
    assert!(senders.launches.gates.lock().unwrap().is_empty());
}

/// Host coordinator fixture: Screen and PTY decisions are explicit, independently acknowledged
/// messages. Process non-execution itself is covered by the real-binary marker tests.
struct Host {
    senders: ThreadSenders,
    commands: yinz_test_support::Watched<Vec<yinz_types::input::command::RunCommand>>,
    threads: Vec<std::thread::JoinHandle<()>>,
}
#[derive(Clone, Copy, PartialEq)]
enum LostLaunchReply {
    None,
    Destination,
    Registration,
    Commit,
    Cleanup,
}
impl Host {
    fn new(reject: bool, register: bool, cancel: bool) -> Self {
        Self::with_lost_reply(reject, register, cancel, LostLaunchReply::None)
    }
    fn with_lost_reply(reject: bool, register: bool, cancel: bool, lost: LostLaunchReply) -> Self {
        use yinz_bus::{pty::PtyInstruction, route::AgentTabPlacement};
        use yinz_runtime::channels::{self, SenderWithContext};
        use yinz_types::{
            internal_ids::{TabId, TerminalId},
            pane_id::PaneId,
        };
        let (screen_tx, screen) = channels::unbounded();
        let (pty_tx, pty) = channels::unbounded();
        let senders = ThreadSenders {
            to_screen: Some(SenderWithContext::new(screen_tx)),
            to_pty: Some(SenderWithContext::new(pty_tx)),
            ..Default::default()
        };
        let runtime = senders.launches.clone();
        let screen_thread = std::thread::spawn(move || {
            let lifetime = Arc::new(());
            loop {
                let (instruction, _) = screen.recv().unwrap();
                match instruction {
                    ScreenInstruction::Exit => break,
                    ScreenInstruction::ResolveAgentTab {
                        reply,
                        placement,
                        launch,
                        ..
                    } => {
                        let destination = AgentTabPlacement {
                            tab_id: TabId::new(7),
                            lifetime: Arc::downgrade(&lifetime),
                            created: matches!(
                                placement,
                                yinz_types::agent::control::AgentPlacement::NewTab(_)
                            ),
                            stack_controller: None,
                        };
                        launch.destination(destination.clone());
                        if lost != LostLaunchReply::Destination {
                            let _ = reply.send(Ok(destination));
                        }
                    },
                    ScreenInstruction::RegisterAgentLaunch { spawned, reply } => {
                        if register && let PaneId::Terminal(terminal) = spawned.pane_id {
                            runtime.gate(terminal).unwrap().registered();
                        }
                        if lost != LostLaunchReply::Registration {
                            let _ = reply.send(register);
                        }
                    },
                    ScreenInstruction::CommitAgentLaunch { terminal_id, reply } => {
                        let accepted = runtime.gate(terminal_id).unwrap().handoff();
                        if lost != LostLaunchReply::Commit {
                            let _ = reply.send(accepted);
                        }
                    },
                    ScreenInstruction::ReleaseAgentLaunch { terminal_id, reply } => {
                        if let Some(gate) = runtime.gate(terminal_id) {
                            gate.layout_released();
                        }
                        let _ = reply.send(true);
                    },
                    ScreenInstruction::CleanupAgentTab(_, reply) => {
                        let _ = reply.send(true);
                    },
                    ScreenInstruction::ClosePane(..) => {},
                    ScreenInstruction::AgentControlReply(_, reply) => {
                        let _ = reply.send(AgentResponse::default());
                    },
                    other => panic!("unexpected Screen instruction: {other:?}"),
                }
            }
        });
        let runtime = senders.launches.clone();
        let commands = yinz_test_support::Watched::new(Vec::new());
        let observed = commands.clone();
        let pty_thread = std::thread::spawn(move || {
            let mut next = 0;
            loop {
                let (instruction, _) = pty.recv().unwrap();
                match instruction {
                    PtyInstruction::Exit => break,
                    PtyInstruction::SpawnTerminal(_, _, _, _, _, Some(mut completion), _) => {
                        let terminal = TerminalId::new(next);
                        next += 1;
                        let gate = completion.launch.as_ref().unwrap().clone();
                        gate.allocated(terminal);
                        runtime.gates.lock().unwrap().insert(terminal, gate.clone());
                        if reject {
                            gate.revoke(LaunchReason::PaneLimit);
                            completion.set_error_message("admission rejected".into());
                        } else {
                            gate.admit(LaunchSlot {
                                pane_id: PaneId::Terminal(terminal),
                                process_generation: terminal.get() as u64,
                                tab_id: TabId::new(7),
                                tab_incarnation: "tab".into(),
                                controller: None,
                                worker_region: None,
                                placement_generation: terminal.get() as u64,
                            });
                            completion.set_affected_pane_id(PaneId::Terminal(terminal));
                            completion.complete_pane_placement(TabId::new(7));
                        }
                    },
                    PtyInstruction::StartAgentReservation {
                        terminal_id,
                        command,
                        mut completion,
                    } => {
                        let gate = runtime.gate(terminal_id).unwrap();
                        if cancel {
                            gate.revoke(LaunchReason::ControllerMoved);
                            completion.set_error_message("controller moved".into());
                        } else {
                            assert!(gate.claim());
                            gate.running();
                            observed.lock().push(command);
                        }
                    },
                    PtyInstruction::ClosePane(PaneId::Terminal(terminal), _) => {
                        if let Some(gate) = runtime.gate(terminal) {
                            if lost == LostLaunchReply::Cleanup {
                                gate.cleanup_unknown();
                            } else {
                                gate.cleaned();
                            }
                        }
                    },
                    other => panic!("unexpected PTY instruction: {other:?}"),
                }
            }
        });
        Self {
            senders,
            commands,
            threads: vec![screen_thread, pty_thread],
        }
    }
}

#[test]
fn launch_transaction_rerunning_a_pane_cannot_reuse_the_original_worker_receipt() {
    let (_root, data, request) = repository();
    let host = Host::new(false, true, false);
    let id = format!("{}.superseded", server_instance());
    let first = launch(
        id.clone(),
        request.clone(),
        crate::ClientId::new(1),
        &host.senders,
        &data,
    )
    .launch
    .unwrap();
    let yinz_types::pane_id::PaneId::Terminal(terminal) = first.spawned.unwrap().pane_id else {
        panic!("terminal worker");
    };
    let gate = host.senders.launches.gate(terminal).unwrap();
    gate.supersede();
    let observed = launch(
        id.clone(),
        request,
        crate::ClientId::new(2),
        &host.senders,
        &data,
    );
    assert_eq!(observed.outcome, AgentOutcome::StatusUnavailable);
    let receipt = observed.launch.unwrap();
    assert_eq!(receipt.outcome, LaunchOutcome::Started);
    assert_eq!(receipt.disposition, WorkerDisposition::Unknown);
    assert_eq!(
        receipt.process,
        ProcessState::Running,
        "replacement intent is not an exit acknowledgement"
    );
    gate.exited();
    assert_eq!(
        status(&id, &host.senders, &data)
            .launch
            .unwrap()
            .disposition,
        WorkerDisposition::Closed
    );
    assert_eq!(host.commands.lock().len(), 1);
}

#[test]
fn launch_transaction_lost_registration_and_commit_replies_observe_acknowledged_shared_state() {
    for lost in [LostLaunchReply::Registration, LostLaunchReply::Commit] {
        let (_root, data, request) = repository();
        let host = Host::with_lost_reply(false, true, false, lost);
        let id = format!("{}.lost-reply-{}", server_instance(), uuid::Uuid::new_v4());
        let first = launch(
            id.clone(),
            request.clone(),
            crate::ClientId::new(1),
            &host.senders,
            &data,
        )
        .launch
        .unwrap();
        assert_eq!(first.outcome, LaunchOutcome::Started);
        for _ in 0..2 {
            let observed = launch(
                id.clone(),
                request.clone(),
                crate::ClientId::new(2),
                &host.senders,
                &data,
            )
            .launch
            .unwrap();
            assert_eq!(observed.spawned, first.spawned);
            assert_eq!(observed.outcome, LaunchOutcome::Started);
        }
        assert_eq!(host.commands.lock().len(), 1);
    }
}

#[test]
fn launch_transaction_lost_destination_reply_is_unknown_and_cannot_replay_preparation() {
    let (root, data, mut request) = repository();
    request.placement = yinz_types::agent::control::AgentPlacement::NewTab(None);
    let host = Host::with_lost_reply(false, true, false, LostLaunchReply::Destination);
    let id = format!("{}.lost-destination", server_instance());
    for _ in 0..2 {
        let receipt = launch(
            id.clone(),
            request.clone(),
            crate::ClientId::new(1),
            &host.senders,
            &data,
        )
        .launch
        .unwrap();
        assert_eq!(receipt.outcome, LaunchOutcome::Unknown);
        assert_eq!(receipt.process, ProcessState::NotStarted);
        assert!(receipt.resources.cleanup_pending);
        assert!(receipt.resources.created_tab);
        assert_eq!(
            receipt.resources.checkout.unwrap().tab_id,
            Some(yinz_types::internal_ids::TabId::new(7))
        );
    }
    assert!(!root.path().join(".worktrees").exists());
    assert!(host.commands.lock().is_empty());
}

#[test]
fn launch_transaction_cleanup_failure_never_claims_confirmed_exit_or_replays_exec() {
    let (_root, data, request) = repository();
    let host = Host::with_lost_reply(false, false, false, LostLaunchReply::Cleanup);
    let id = format!("{}.lost-cleanup", server_instance());
    for _ in 0..2 {
        let receipt = launch(
            id.clone(),
            request.clone(),
            crate::ClientId::new(1),
            &host.senders,
            &data,
        )
        .launch
        .unwrap();
        assert_eq!(receipt.outcome, LaunchOutcome::Unknown);
        assert_eq!(receipt.process, ProcessState::Unknown);
        assert!(receipt.resources.cleanup_pending);
    }
    assert_eq!(host.commands.lock().len(), 1);
}
impl Drop for Host {
    fn drop(&mut self) {
        self.senders
            .send_to_screen(ScreenInstruction::Exit)
            .unwrap();
        self.senders
            .send_to_pty(yinz_bus::pty::PtyInstruction::Exit)
            .unwrap();
        for thread in self.threads.drain(..) {
            thread.join().unwrap();
        }
    }
}
fn repository() -> (tempfile::TempDir, DataAccess, LaunchRequest) {
    let root = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "base",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(root.path())
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    let data = DataAccess::sqlite(&root.path().join("data"));
    let request = LaunchRequest {
        placement: yinz_types::agent::control::AgentPlacement::Tab(
            yinz_types::internal_ids::TabId::new(7),
        ),
        spec: yinz_types::agent::AgentSpawnSpec {
            kind: Some("codex".into()),
            branch: Some("review".into()),
            ..Default::default()
        },
        cwd: Some(root.path().to_owned()),
        project: None,
        remote_host: None,
        new_project: None,
    };
    (root, data, request)
}
#[test]
fn launch_transaction_admission_rejection_precedes_all_git_preparation() {
    let (root, data, request) = repository();
    let host = Host::new(true, true, false);
    let receipt = launch(
        format!("{}.capacity", server_instance()),
        request,
        crate::ClientId::new(1),
        &host.senders,
        &data,
    )
    .launch
    .unwrap();
    assert_eq!(receipt.outcome, LaunchOutcome::Rejected);
    assert_eq!(receipt.reason, Some(LaunchReason::PaneLimit));
    assert_eq!(receipt.process, ProcessState::NotStarted);
    assert!(!receipt.resources.cleanup_pending);
    assert!(!root.path().join(".worktrees").exists());
    assert!(host.commands.lock().is_empty());
}
#[test]
fn launch_transaction_registration_failure_retains_created_git_resources_with_provenance() {
    let (root, data, request) = repository();
    let host = Host::new(false, false, false);
    let receipt = launch(
        format!("{}.registration", server_instance()),
        request,
        crate::ClientId::new(1),
        &host.senders,
        &data,
    )
    .launch
    .unwrap();
    assert_eq!(receipt.outcome, LaunchOutcome::Failed);
    assert!(!receipt.resources.cleanup_pending);
    assert!(root.path().join(".worktrees/review/.git").exists());
    assert!(receipt
        .resources
        .git
        .iter()
        .any(|resource| resource.kind == GitResourceKind::Worktree
            && resource.ownership == ResourceOwnership::Created));
    assert_eq!(host.commands.lock().len(), 1);
}
#[test]
fn launch_transaction_matching_retries_and_conflicting_requests_do_not_repeat_checkout_or_exec() {
    let (root, data, request) = repository();
    let host = Host::new(false, true, false);
    let id = format!("{}.once", server_instance());
    let first = launch(
        id.clone(),
        request.clone(),
        crate::ClientId::new(1),
        &host.senders,
        &data,
    );
    assert_eq!(
        first.launch.as_ref().unwrap().outcome,
        LaunchOutcome::Started,
        "{first:?}"
    );
    let second = launch(
        id.clone(),
        request.clone(),
        crate::ClientId::new(2),
        &host.senders,
        &data,
    );
    assert_eq!(second.spawned, first.spawned);
    let mut different = request;
    different.spec.model = Some("different".into());
    assert_eq!(
        launch(id, different, crate::ClientId::new(1), &host.senders, &data)
            .launch
            .unwrap()
            .outcome,
        LaunchOutcome::Conflict
    );
    assert_eq!(host.commands.lock().len(), 1);
    assert!(!root.path().join(".worktrees/review-2").exists());
}

#[test]
fn launch_transaction_history_loss_in_a_live_server_does_not_repeat_exec() {
    let (_root, data, request) = repository();
    let host = Host::new(false, true, false);
    let id = format!("{}.history", server_instance());
    let first = launch(
        id.clone(),
        request.clone(),
        crate::ClientId::new(1),
        &host.senders,
        &data,
    );
    assert_eq!(first.launch.unwrap().outcome, LaunchOutcome::Started);
    data.plugin_data()
        .apply(
            NS,
            &[Mutation::delete(
                ENTITY,
                Pred::eq("uid", Journal::new(&data, &host.senders, &id).unwrap().uid),
            )],
        )
        .unwrap();
    assert_eq!(
        launch(id, request, crate::ClientId::new(1), &host.senders, &data)
            .launch
            .unwrap()
            .outcome,
        LaunchOutcome::Started
    );
    assert_eq!(host.commands.lock().len(), 1);
}

#[test]
fn launch_transaction_partial_project_failure_retains_directory_with_project_provenance() {
    let (root, data, mut request) = repository();
    let target = root.path().join("new-project");
    request.spec.branch = None;
    request.new_project = Some(("project".into(), target.clone()));
    let host = Host::new(false, true, false);
    let receipt = launch_with_prepare(
        format!("{}.partial-project", server_instance()),
        request,
        crate::ClientId::new(1),
        &host.senders,
        &data,
        None,
        None,
        Some(Box::new(|_| {
            std::fs::create_dir(&target).unwrap();
            std::fs::write(target.join("retained-user-content"), "preserve").unwrap();
            Err(AgentSpawnError::failed(
                "project materialization interrupted",
                Some(yinz_types::agent::AgentLaunchResources {
                    cwd: Some(target.clone()),
                    ..Default::default()
                }),
            ))
        })),
    )
    .launch
    .unwrap();
    assert_eq!(receipt.outcome, LaunchOutcome::Failed);
    assert_eq!(receipt.process, ProcessState::NotStarted);
    assert!(!receipt.resources.cleanup_pending);
    assert_eq!(
        receipt.resources.git[0].kind,
        GitResourceKind::ProjectDirectory
    );
    assert_eq!(
        receipt.resources.git[0].ownership,
        ResourceOwnership::RetainedAttempt
    );
    assert_eq!(receipt.resources.git[0].path, target);
    assert_eq!(
        std::fs::read_to_string(target.join("retained-user-content")).unwrap(),
        "preserve"
    );
    assert!(host.commands.lock().is_empty());
}

#[test]
fn launch_transaction_preparation_failure_retains_attempted_checkout_without_process_uncertainty() {
    let (root, data, request) = repository();
    std::fs::write(root.path().join(".worktrees"), "occupied").unwrap();
    let host = Host::new(false, true, false);
    let receipt = launch(
        format!("{}.prepare", server_instance()),
        request,
        crate::ClientId::new(1),
        &host.senders,
        &data,
    )
    .launch
    .unwrap();
    assert_eq!(receipt.outcome, LaunchOutcome::Failed);
    assert_eq!(receipt.process, ProcessState::NotStarted);
    assert!(!receipt.resources.cleanup_pending);
    assert_eq!(
        receipt.resources.git[0].ownership,
        ResourceOwnership::RetainedAttempt
    );
    assert_eq!(
        receipt.resources.git[0].path,
        root.path().join(".worktrees/review")
    );
    assert!(host.commands.lock().is_empty());
}

#[test]
fn launch_transaction_revoked_preparation_is_cancelled_without_provider_execution() {
    let (_root, data, request) = repository();
    let host = Host::new(false, true, true);
    let receipt = launch(
        format!("{}.moved", server_instance()),
        request,
        crate::ClientId::new(1),
        &host.senders,
        &data,
    )
    .launch
    .unwrap();
    assert_eq!(receipt.outcome, LaunchOutcome::Cancelled);
    assert_eq!(receipt.reason, Some(LaunchReason::ControllerMoved));
    assert_eq!(receipt.process, ProcessState::NotStarted);
    assert!(!receipt.resources.cleanup_pending);
    assert!(host.commands.lock().is_empty());
}

#[test]
fn launch_transaction_concurrent_identical_requests_observe_the_original_preparation() {
    let (root, data, request) = repository();
    let host = Host::new(false, true, false);
    let id = format!("{}.concurrent", server_instance());
    let entered = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let worker = {
        let entered = entered.clone();
        let release = release.clone();
        let data = data.clone();
        let senders = host.senders.clone();
        let request = request.clone();
        let id = id.clone();
        std::thread::spawn(move || {
            let cwd = request.cwd.clone();
            launch_with_prepare(
                id,
                request,
                crate::ClientId::new(1),
                &senders,
                &data,
                None,
                None,
                Some(Box::new(|spec| {
                    entered.wait();
                    release.wait();
                    prepare_automation_agent(
                        spec,
                        cwd,
                        None,
                        None,
                        &data,
                        &senders.session.current(),
                    )
                })),
            )
        })
    };
    entered.wait();
    let observed = launch(id, request, crate::ClientId::new(2), &host.senders, &data);
    assert_eq!(observed.launch.unwrap().outcome, LaunchOutcome::Pending);
    assert!(!root.path().join(".worktrees").exists());
    release.wait();
    assert_eq!(
        worker.join().unwrap().launch.unwrap().outcome,
        LaunchOutcome::Started
    );
    assert_eq!(host.commands.lock().len(), 1);
}

#[test]
fn launch_transaction_plugin_spawn_preserves_origin_and_legacy_response() {
    use prost::Message;
    let (root, data, request) = repository();
    let host = Host::new(false, true, false);
    let mut env = crate::test_env::test_env();
    env.senders = host.senders.clone();
    env.data_access = Arc::new(data);
    env.plugin_cwd = root.path().to_owned();
    env.response_sink = Some(Vec::new());
    // Use the permission-gated public dispatch, with precisely the existing SpawnAgent right.
    *env.permissions.lock().unwrap() = Some(std::collections::HashSet::from([
        yinz_types::data::PermissionType::RunCommands,
    ]));
    super::super::run_plugin_command(
        &mut env,
        yinz_types::data::PluginCommand::SpawnAgent {
            spec: request.spec,
            source_directory: None,
            cwd: None,
            name: Some("worker".into()),
            context: std::collections::BTreeMap::from([(
                "original-context".into(),
                "preserved".into(),
            )]),
            start_suppressed: false,
        },
    )
    .unwrap();
    let wire = yinz_plugin_api::plugin_command::ProtobufSpawnAgentResponse::decode(
        env.response_sink.take().unwrap().as_slice(),
    )
    .unwrap();
    let spawned = yinz_types::data::SpawnAgentResponse::try_from(wire)
        .unwrap()
        .unwrap();
    assert!(spawned.cwd.unwrap().join(".git").exists());
    let command = &host.commands.lock()[0];
    let origin = command.originating_plugin.as_ref().unwrap();
    assert_eq!(origin.plugin_id, env.plugin_id);
    assert_eq!(origin.context["original-context"], "preserved");
    assert_eq!(
        origin.context[yinz_types::data::AGENT_PANE_CONTEXT],
        "codex"
    );
}

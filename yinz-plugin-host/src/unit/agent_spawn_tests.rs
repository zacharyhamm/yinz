use super::agent_control::{self, spawn_agent_for_control};
use super::agent_launch::wait_for_agent_placement;
use super::pane_creation::spawn_command_pane_near_plugin;
use super::{run_plugin_command, spawn_agent_for_cli};
use crate::plugin_map::PluginEnv;
use crate::{PaneId, PluginInstruction};
use prost::Message;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;
use std::{process, thread};
use tokio::sync::oneshot;
use yinz_bus::pty::{ClientTabIndexOrPaneId, PtyInstruction};
use yinz_bus::route::NotificationEnd;
use yinz_bus::screen::ScreenInstruction;
use yinz_bus::thread_bus::ThreadSenders;
use yinz_runtime::channels::{self, SenderWithContext};
use yinz_runtime::errors::prelude::*;
use yinz_storage::data_access::DataAccess;
use yinz_types::agent::control::AgentOutcome;
use yinz_types::agent::{AgentLaunchResources, AgentSpawnSpec};
use yinz_types::command::{CommandToRun, PaneTarget};
use yinz_types::data::{
    Event, NewPanePlacement, PermissionType, PluginCommand, AGENT_PANE_CONTEXT,
};
use yinz_types::input::command::TerminalAction;
use yinz_types::internal_ids::TerminalId;

fn launch_failure_repository() -> (tempfile::TempDir, PathBuf, DataAccess) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
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
            "initial",
        ],
    ] {
        let output = process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let data = DataAccess::sqlite(&root.join("data"));
    (directory, root, data)
}

#[test]
fn checkout_hooks_receive_current_session_for_plugin_and_agent_launches() {
    let inherited_name = std::env::var_os("YINZ_SESSION_NAME");
    let (_directory, root, data) = launch_failure_repository();
    let hook_dir = root.join("hooks");
    std::fs::create_dir(&hook_dir).unwrap();
    let configured = process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["config", "core.hooksPath"])
        .arg(&hook_dir)
        .status()
        .unwrap();
    assert!(configured.success());
    let record = root.join("hook-environment");
    yinz_test_support::write_warm_script(
        &hook_dir.join("post-checkout"),
        &format!(
            "if [ \"$#\" -eq 3 ]; then printf '%s|%s\\n' \"$YINZ\" \"$YINZ_SESSION_NAME\" >> '{}'; fi\n",
            record.display().to_string().replace('\'', "'\"'\"'")
        ),
    );
    let state = yinz_runtime::envs::SessionState::new("startup-session".into());
    let mut env = crate::test_env::test_env();
    env.plugin_cwd = root.clone();
    env.server_session = crate::ServerSession::Shared(state.clone());
    state.set("plugin-renamed".into());
    super::agent_launch::prepare_git_checkout(
        &env,
        "/host".into(),
        yinz_types::command::GitCheckout::NewBranch {
            branch: "plugin-branch".into(),
            base: "main".into(),
        },
    )
    .unwrap();
    state.set("agent-renamed".into());
    super::agent_launch::prepare_agent(
        &AgentSpawnSpec {
            command: Some("/usr/bin/true".into()),
            branch: Some("agent-branch".into()),
            base: Some("main".into()),
            ..Default::default()
        },
        root,
        None,
        &data,
        &state.current(),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(record).unwrap(),
        "0|plugin-renamed\n0|agent-renamed\n"
    );
    assert_eq!(std::env::var_os("YINZ_SESSION_NAME"), inherited_name);
}

#[test]
fn failed_spawn_retains_checkout_created_before_the_pane_request() {
    let (_directory, root, data) = launch_failure_repository();
    let response = agent_control::control_agent(
        yinz_types::agent::control::AgentRequest::Spawn {
            placement: Default::default(),
            spec: AgentSpawnSpec {
                kind: Some("codex".into()),
                branch: Some("review".into()),
                ..Default::default()
            },
            cwd: Some(root.clone()),
            project: None,
            remote_host: None,
        },
        crate::ClientId::new(1),
        &ThreadSenders::default(),
        &data,
    );
    assert_eq!(response.outcome, AgentOutcome::Failed);
    assert!(response.spawned.is_none());
    let resources = response.launch_resources.unwrap();
    assert_eq!(resources.cwd, Some(root.join(".worktrees/review")));
    assert_eq!(resources.branch.as_deref(), Some("review"));
    assert!(root.join(".worktrees/review/.git").exists());
    assert!(response.message.unwrap().contains("inspect"));
}

#[test]
fn git_spawn_failures_distinguish_validation_from_attempted_checkout_changes() {
    let (_directory, root, data) = launch_failure_repository();
    std::fs::write(root.join(".worktrees"), "blocks worktree creation").unwrap();
    for (branch, outcome) in [
        ("invalid..branch", AgentOutcome::Rejected),
        ("review", AgentOutcome::Failed),
    ] {
        let response = agent_control::control_agent(
            yinz_types::agent::control::AgentRequest::Spawn {
                placement: Default::default(),
                spec: AgentSpawnSpec {
                    kind: Some("codex".into()),
                    branch: Some(branch.into()),
                    ..Default::default()
                },
                cwd: Some(root.clone()),
                project: None,
                remote_host: None,
            },
            crate::ClientId::new(1),
            &ThreadSenders::default(),
            &data,
        );
        assert_eq!(response.outcome, outcome, "{:?}", response.message);
        assert!(response.spawned.is_none());
        if outcome == AgentOutcome::Failed {
            let resources = response.launch_resources.unwrap();
            assert_eq!(resources.cwd, Some(root.join(".worktrees/review")));
            assert_eq!(resources.branch.as_deref(), Some("review"));
        } else {
            assert!(response.launch_resources.is_none());
        }
    }
}

#[test]
fn failed_or_unknown_pane_completion_keeps_known_launch_resources() {
    let (_directory, root, data) = launch_failure_repository();
    for (branch, failed_pane) in [("failed", true), ("unknown", false)] {
        let (to_pty, pty_rx) = channels::unbounded();
        let senders = ThreadSenders {
            to_pty: Some(SenderWithContext::new(to_pty)),
            ..Default::default()
        };
        let worker = thread::spawn(move || {
            let (instruction, _) = pty_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            let PtyInstruction::SpawnTerminal(_, _, _, _, _, Some(mut completion), _) = instruction
            else {
                panic!("expected spawn");
            };
            if failed_pane {
                completion.set_affected_tab_id(yinz_types::internal_ids::TabId::new(0));
                completion.set_affected_pane_id(PaneId::Terminal(TerminalId::new(17)));
                completion.set_error_message("provider failed after creating the pane".into());
            }
        });
        let response = agent_control::control_agent(
            yinz_types::agent::control::AgentRequest::Spawn {
                placement: Default::default(),
                spec: AgentSpawnSpec {
                    kind: Some("codex".into()),
                    branch: Some(branch.into()),
                    ..Default::default()
                },
                cwd: Some(root.clone()),
                project: None,
                remote_host: None,
            },
            crate::ClientId::new(1),
            &senders,
            &data,
        );
        worker.join().unwrap();
        assert_eq!(response.outcome, AgentOutcome::Failed);
        if failed_pane {
            let spawned = response.spawned.unwrap();
            assert_eq!(spawned.pane_id, PaneId::Terminal(TerminalId::new(17)));
            assert_eq!(spawned.cwd, Some(root.join(".worktrees/failed")));
            assert_eq!(spawned.branch.as_deref(), Some(branch));
            assert!(response.launch_resources.is_none());
        } else {
            assert!(response.spawned.is_none());
            assert_eq!(
                response.launch_resources.unwrap().cwd,
                Some(root.join(".worktrees/unknown"))
            );
            assert!(response.message.unwrap().contains("outcome is unknown"));
        }
    }
}

#[test]
fn agent_spawn_registration_failure_retains_created_pane_and_checkout() {
    let (tx, rx) = channels::unbounded();
    drop(rx);
    let senders = ThreadSenders {
        to_screen: Some(yinz_runtime::channels::SenderWithContext::new(tx)),
        ..Default::default()
    };
    let spawned = yinz_types::agent::SpawnedAgent {
        pane_id: yinz_types::pane_id::PaneId::Terminal(TerminalId::new(17)),
        cwd: Some("/actual/checkout".into()),
        branch: Some("actual-branch".into()),
        ..Default::default()
    };
    let response = agent_control::register_spawned_agent(&senders, spawned.clone());
    assert_eq!(
        response.outcome,
        yinz_types::agent::control::AgentOutcome::Failed
    );
    assert_eq!(response.spawned, Some(spawned));
    assert!(response
        .message
        .unwrap()
        .contains("launch registration failed"));
}

#[test]
fn plugin_agent_control_resolves_only_local_relative_directories() {
    use yinz_types::agent::control::{AgentOutcome, AgentRequest};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let child = root.join("child");
    std::fs::create_dir(&child).unwrap();
    let mut env = crate::test_env::test_env();
    env.plugin_config.location = yinz_types::input::layout::RunPluginLocation::Yinz(
        yinz_types::data::PluginTag::new("agents"),
    );
    env.plugin_cwd = root.clone();
    env.data_access = std::sync::Arc::new(DataAccess::sqlite(&root.join("data")));
    let (to_pty, pty_rx) = channels::unbounded();
    let (to_plugin, plugin_rx) = channels::unbounded();
    env.senders = ThreadSenders {
        to_pty: Some(SenderWithContext::new(to_pty)),
        to_plugin: Some(SenderWithContext::new(to_plugin)),
        ..Default::default()
    }
    .silently_fail_on_send();

    for (cwd, remote_host, expected) in [
        (Some(PathBuf::from(".")), None, Some(root.clone())),
        (Some(PathBuf::from("child")), None, Some(child)),
        (Some(root.clone()), None, Some(root.clone())),
        (None, None, Some(root.clone())),
        (
            Some(PathBuf::from("/host/remote")),
            Some("server".to_owned()),
            Some(PathBuf::from("/host/remote")),
        ),
        (Some(PathBuf::from(".")), Some("server".to_owned()), None),
    ] {
        let request = AgentRequest::Spawn {
            placement: Default::default(),
            spec: AgentSpawnSpec {
                kind: Some("codex".into()),
                allow_outside_worktree: Some(true),
                ..Default::default()
            },
            cwd,
            project: None,
            remote_host: remote_host.clone(),
        };
        run_plugin_command(
            &mut env,
            PluginCommand::AgentControl {
                id: "test".into(),
                request: serde_json::to_string(&request).unwrap(),
            },
        )
        .unwrap();
        if let Some(expected) = &expected {
            let (instruction, _) = pty_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            let PtyInstruction::SpawnTerminal(
                Some(TerminalAction::RunCommand(command)),
                _,
                _,
                _,
                _,
                Some(mut completion),
                _,
            ) = instruction
            else {
                panic!("expected agent spawn");
            };
            assert_eq!(command.cwd.as_ref(), Some(expected));
            assert_eq!(
                command.target,
                match remote_host {
                    Some(host) => yinz_types::command::PaneTarget::ssh(&host).unwrap(),
                    None => yinz_types::command::PaneTarget::Local,
                }
            );
            completion.set_affected_tab_id(yinz_types::internal_ids::TabId::new(0));
            completion.set_affected_pane_id(PaneId::Terminal(crate::TerminalId::new(7)));
        }
        let (instruction, _) = plugin_rx.recv_timeout(Duration::from_secs(30)).unwrap();
        let PluginInstruction::Update(events) = instruction else {
            panic!("expected agent response");
        };
        let Event::CustomMessage(name, payload) = &events[0].2 else {
            panic!("expected custom message");
        };
        assert_eq!(name, "agent-response");
        let payload: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(
            payload["response"]["outcome"],
            serde_json::to_value(if expected.is_some() {
                AgentOutcome::Ok
            } else {
                AgentOutcome::Rejected
            })
            .unwrap(),
            "{payload}"
        );
        if expected.is_none() {
            assert!(payload["response"]["message"]
                .as_str()
                .unwrap()
                .contains("SSH cwd must be absolute"));
            assert!(pty_rx.try_recv().is_err());
        }
    }
}

#[test]
fn agent_provider_registration_is_permissioned_owned_and_released() {
    use yinz_types::agent::provider::{provider, AgentProvider};
    let mut first = crate::test_env::test_env();
    let mut second = crate::test_env::test_env();
    // Workers share a plugin instance, but each isolate owns its own registrations.
    second.instance_id = first.instance_id;
    first.response_sink = None;
    second.response_sink = None;
    let definition = AgentProvider {
        kind: "host-fixture-agent".into(),
        executable: "host-fixture-agent".into(),
        ..Default::default()
    };
    let response = |env: &mut PluginEnv| -> Result<Vec<AgentProvider>, String> {
        let bytes: Vec<u8> = env.response_sink.take().unwrap();
        let wire = yinz_plugin_api::plugin_command::ProtobufAgentProvidersResponse::decode(
            bytes.as_slice(),
        )
        .unwrap();
        serde_json::from_str(&wire.json).unwrap()
    };
    run_plugin_command(
        &mut first,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    )
    .unwrap();
    assert!(response(&mut first)
        .unwrap_err()
        .contains("Permission denied"));
    assert!(provider(&definition.kind).is_none());
    for env in [&mut first, &mut second] {
        *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::RunCommands]));
    }
    run_plugin_command(
        &mut first,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    )
    .unwrap();
    assert!(response(&mut first).unwrap_err().contains("Scheme"));
    first.generation = Some(41);
    first.resolved_source_path = Some("fixture.js".into());
    run_plugin_command(
        &mut first,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    )
    .unwrap();
    assert!(response(&mut first).unwrap_err().contains("Scheme"));
    first.resolved_source_path = Some("fixture.scm".into());
    run_plugin_command(
        &mut first,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    )
    .unwrap();
    assert!(response(&mut first).unwrap_err().contains("shared true"));
    for (generation, env) in [(41, &mut first), (42, &mut second)] {
        env.generation = Some(generation);
        env.plugin_config.initial_userspace_configuration =
            yinz_types::input::layout::PluginUserConfiguration::new(BTreeMap::from([(
                "shared".into(),
                "true".into(),
            )]));
    }
    run_plugin_command(
        &mut first,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    )
    .unwrap();
    assert!(response(&mut first).unwrap().contains(&definition));
    run_plugin_command(
        &mut second,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    )
    .unwrap();
    assert!(response(&mut second).unwrap_err().contains("conflict"));
    second.clear_agent_providers();
    assert!(provider(&definition.kind).is_some());
    first.clear_agent_providers();
    assert!(provider(&definition.kind).is_none());
    run_plugin_command(
        &mut second,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    )
    .unwrap();
    response(&mut second).unwrap();
    first.clear_agent_providers();
    assert!(
        provider(&definition.kind).is_some(),
        "old instance cleanup cannot clear its replacement"
    );
    second.clear_agent_providers();
    for extension in ["scm", "ss", "sld"] {
        second.resolved_source_path = Some(format!("fixture.{extension}").into());
        run_plugin_command(
            &mut second,
            PluginCommand::RegisterAgentProvider(definition.clone()),
        )
        .unwrap();
        assert!(response(&mut second).unwrap().contains(&definition));
        second.clear_agent_providers();
    }
    second
        .unavailable
        .store(true, std::sync::atomic::Ordering::Release);
    run_plugin_command(
        &mut second,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    )
    .unwrap();
    assert!(response(&mut second)
        .unwrap_err()
        .contains("no longer available"));
    assert!(provider(&definition.kind).is_none());
}

#[test]
fn invalid_or_missing_agent_tab_is_rejected_before_checkout_preparation() {
    use yinz_types::agent::control::AgentPlacement;
    let (_directory, root, data) = launch_failure_repository();
    let (to_screen, screen_rx) = channels::unbounded();
    let senders = ThreadSenders {
        to_screen: Some(SenderWithContext::new(to_screen)),
        ..Default::default()
    };
    let worker = thread::spawn(move || {
        for expected in [
            AgentPlacement::Tab(yinz_types::internal_ids::TabId::new(42)),
            AgentPlacement::Suppressed(Some(yinz_types::internal_ids::TabId::new(42))),
            AgentPlacement::Suppressed(None),
        ] {
            let (instruction, _) = screen_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            let ScreenInstruction::ResolveAgentTab {
                placement, reply, ..
            } = instruction
            else {
                panic!("expected tab resolution before checkout");
            };
            assert_eq!(placement, expected);
            reply.send(Err("tab 42 does not exist".into())).unwrap();
        }
        assert!(screen_rx.try_recv().is_err());
    });
    for placement in [
        AgentPlacement::NewTab(Some("\n".into())),
        AgentPlacement::Tab(yinz_types::internal_ids::TabId::new(42)),
        AgentPlacement::Suppressed(Some(yinz_types::internal_ids::TabId::new(42))),
        AgentPlacement::Suppressed(None),
    ] {
        let response = agent_control::control_agent(
            yinz_types::agent::control::AgentRequest::Spawn {
                placement,
                spec: AgentSpawnSpec {
                    kind: Some("codex".into()),
                    branch: Some("review".into()),
                    ..Default::default()
                },
                cwd: Some(root.clone()),
                project: None,
                remote_host: None,
            },
            crate::ClientId::new(1),
            &senders,
            &data,
        );
        assert_eq!(response.outcome, AgentOutcome::Rejected, "{response:?}");
        assert!(!root.join(".worktrees/review").exists());
    }
    worker.join().unwrap();
}

#[test]
fn created_agent_tab_failure_preserves_resources_and_unknown_completion_does_not_cleanup() {
    use yinz_types::agent::control::AgentPlacement;
    use yinz_types::internal_ids::TabId;
    for (suppressed, definitive, cleaned) in [
        (false, true, true),
        (false, true, false),
        (false, false, false),
        (true, true, false),
        (true, false, false),
    ] {
        let (_directory, root, data) = launch_failure_repository();
        let checkout = root.join(".worktrees/review");
        let (to_screen, screen_rx) = channels::unbounded();
        let (to_pty, pty_rx) = channels::unbounded();
        let senders = ThreadSenders {
            to_screen: Some(SenderWithContext::new(to_screen)),
            to_pty: Some(SenderWithContext::new(to_pty)),
            ..Default::default()
        };
        let worker = thread::spawn(move || {
            let (instruction, _) = screen_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            let ScreenInstruction::ResolveAgentTab {
                reply,
                invoked_with,
                ..
            } = instruction
            else {
                panic!("expected new tab request");
            };
            assert_eq!(checkout.join(".git").exists(), !suppressed);
            assert_eq!(invoked_with.is_some(), !suppressed);
            let lifetime = std::sync::Arc::new(());
            reply
                .send(Ok(yinz_bus::route::AgentTabPlacement {
                    tab_id: TabId::new(7),
                    lifetime: std::sync::Arc::downgrade(&lifetime),
                    created: !suppressed,
                }))
                .unwrap();
            let (instruction, _) = pty_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            let PtyInstruction::SpawnTerminal(
                _,
                _,
                _,
                start_suppressed,
                _,
                Some(mut completion),
                _,
            ) = instruction
            else {
                panic!("expected spawn");
            };
            assert_eq!(start_suppressed, suppressed);
            if definitive {
                completion.set_affected_pane_id(PaneId::Terminal(TerminalId::new(17)));
                completion.set_error_message("destination closed during startup".into());
            }
            drop(completion);
            if definitive && !suppressed {
                let (instruction, _) = screen_rx.recv_timeout(Duration::from_secs(30)).unwrap();
                let ScreenInstruction::CleanupAgentTab(target, reply) = instruction else {
                    panic!("expected cleanup");
                };
                assert_eq!(target.tab_id, TabId::new(7));
                reply.send(cleaned).unwrap();
            }
        });
        let response = spawn_agent_for_control(
            if suppressed {
                AgentPlacement::Suppressed(None)
            } else {
                AgentPlacement::NewTab(Some("review".into()))
            },
            AgentSpawnSpec {
                kind: Some("codex".into()),
                branch: Some("review".into()),
                ..Default::default()
            },
            Some(root.clone()),
            None,
            None,
            crate::ClientId::new(1),
            &senders,
            &data,
        )
        .unwrap_err()
        .into_response();
        assert_eq!(response.outcome, AgentOutcome::Failed);
        let resources = response.launch_resources.unwrap();
        assert_eq!(resources.tab_id, Some(TabId::new(7)));
        assert_eq!(
            resources.created_tab_id,
            (!suppressed && !cleaned).then_some(TabId::new(7))
        );
        assert_eq!(resources.cwd, Some(root.join(".worktrees/review")));
        assert!(root.join(".worktrees/review/.git").exists());
        if definitive {
            assert_eq!(response.spawned.unwrap().tab_id, None);
        } else {
            assert!(response.spawned.is_none());
            assert!(response.message.unwrap().contains("unknown"));
        }
        worker.join().unwrap();
    }
}

#[test]
fn agent_placement_timeout_preserves_known_tab_and_checkout_without_cancelling_the_launch() {
    use yinz_types::internal_ids::TabId;
    let resources = AgentLaunchResources {
        tab_id: Some(TabId::new(7)),
        created_tab_id: Some(TabId::new(7)),
        cwd: Some("/repo/review".into()),
        branch: Some("review".into()),
        ..Default::default()
    };
    let (tx, rx) = oneshot::channel();
    let mut completion = NotificationEnd::new(tx);
    completion.wait_for_agent_start = true;
    let response = wait_for_agent_placement(rx, &resources, Duration::ZERO)
        .unwrap_err()
        .into_response();
    assert_eq!(response.outcome, AgentOutcome::TimedOut);
    assert_eq!(response.launch_resources, Some(resources));
    assert!(response.message.unwrap().contains("late placement"));
    // The pending startup/layout still owns its notification and can finish after this reply.
    completion.set_affected_pane_id(PaneId::Terminal(TerminalId::new(17)));
    completion.complete_pane_placement(TabId::new(7));
}

#[test]
fn plugin_agent_launch_waits_for_checks_and_rejects_errors_with_pane_ids() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("XDG_DATA_HOME", temp.path().join("data")) };
    let root = temp.path().join("repo");
    let cwd = root.join("task");
    std::fs::create_dir_all(&cwd).unwrap();
    let root = root.canonicalize().unwrap();
    let cwd = cwd.canonicalize().unwrap();
    let metadata = yinz_runtime::home::xdg_data_dir()
        .join("bubblewrap/environments")
        .join(yinz_config::project_config::root_key(&root));
    std::fs::create_dir_all(&metadata).unwrap();
    std::fs::write(metadata.join("accepted.json"), serde_json::to_vec(&serde_json::json!({
        "root": root, "ready": true,
        "declaration": {"mounts": [], "paths": [], "env": {}, "inherit": [], "seeds": [], "setup": [], "checks": []}
    })).unwrap()).unwrap();
    for (delay, error) in [
        (false, None),
        (true, None),
        (false, Some("sandbox check failed")),
    ] {
        let mut env = crate::test_env::test_env();
        let (pty_tx, pty_rx) = channels::unbounded();
        env.senders.to_pty = Some(SenderWithContext::new(pty_tx));
        let plugin_id = env.plugin_id;
        let expected_cwd = cwd.clone();
        let worker = std::thread::spawn(move || {
            let (
                PtyInstruction::SpawnTerminal(
                    Some(TerminalAction::RunCommand(command)),
                    title,
                    placement,
                    suppressed,
                    destination,
                    Some(mut completion),
                    _,
                ),
                _,
            ) = pty_rx.recv().unwrap()
            else {
                panic!("expected spawn")
            };
            assert!(completion.wait_for_agent_start);
            assert!(suppressed);
            assert_eq!(title.as_deref(), Some("sandbox agent"));
            assert!(matches!(placement, NewPanePlacement::Tiled { .. }));
            assert!(
                matches!(destination, ClientTabIndexOrPaneId::PaneIdOrClient(PaneId::Plugin(id), _) if id == plugin_id)
            );
            assert_eq!(command.cwd, Some(expected_cwd));
            let origin = command.originating_plugin.unwrap();
            assert_eq!(origin.plugin_id, plugin_id);
            assert_eq!(
                origin.context.get(AGENT_PANE_CONTEXT).map(String::as_str),
                Some("claude")
            );
            if delay {
                // Exercise the former one-second completion timeout.
                std::thread::sleep(Duration::from_millis(1200));
            }
            completion.set_affected_pane_id(PaneId::Terminal(TerminalId::new(42)));
            if let Some(error) = error {
                completion.set_error_message(error.to_owned());
            }
        });
        let result = spawn_command_pane_near_plugin(
            &env,
            CommandToRun {
                path: "claude".into(),
                cwd: Some(cwd.clone()),
                target: PaneTarget::bubblewrap(&root).unwrap(),
                ..Default::default()
            },
            BTreeMap::from([(AGENT_PANE_CONTEXT.into(), "claude".into())]),
            Some("sandbox agent".into()),
            true,
            true,
        );
        worker.join().unwrap();
        if let Some(error) = error {
            assert_eq!(result.unwrap_err(), error);
        } else {
            assert_eq!(result.unwrap(), PaneId::Terminal(TerminalId::new(42)));
        }
    }
}

#[test]
fn cli_agent_spawn_returns_actual_pane_checkout_and_collision_branch() {
    let (_directory, root, data_access) = launch_failure_repository();
    let (to_pty, pty_rx) = channels::unbounded();
    let senders = ThreadSenders {
        to_pty: Some(SenderWithContext::new(to_pty)),
        ..Default::default()
    };
    // Both launch preparations must reject before touching Git or the pty.
    for worktree in [false, true] {
        let response = agent_control::control_agent(
            yinz_types::agent::control::AgentRequest::Spawn {
                placement: Default::default(),
                spec: AgentSpawnSpec {
                    kind: Some("codex".into()),
                    branch: Some("rejected".into()),
                    worktree: Some(worktree),
                    resume: Some("last".into()),
                    permission_mode: Some("skip".into()),
                    ..Default::default()
                },
                cwd: Some(root.clone()),
                project: None,
                remote_host: None,
            },
            crate::ClientId::new(99),
            &senders,
            &data_access,
        );
        assert_eq!(
            response.outcome,
            yinz_types::agent::control::AgentOutcome::Rejected
        );
        assert!(response.message.unwrap().contains("omit --permission-mode"));
        assert!(response.spawned.is_none());
        assert!(pty_rx.try_recv().is_err());
        assert!(!root.join(".worktrees").exists());
        let refs = process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["branch", "--format=%(refname:short)"])
            .output()
            .unwrap();
        assert!(refs.status.success());
        assert_eq!(String::from_utf8(refs.stdout).unwrap(), "main\n");
    }
    let worker_root = root.clone();
    let worker = thread::spawn(move || {
        for (pane, branch) in [(7, "review"), (8, "review-2")] {
            let (instruction, _) = pty_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            let PtyInstruction::SpawnTerminal(
                Some(TerminalAction::RunCommand(command)),
                title,
                _,
                _,
                placement,
                Some(mut completion),
                _,
            ) = instruction
            else {
                panic!("expected an agent command pane");
            };
            assert_eq!(
                placement,
                ClientTabIndexOrPaneId::ClientIdNoFocus(crate::ClientId::new(1))
            );
            assert_eq!(title.as_deref(), Some("codex (gpt-test)"));
            assert_eq!(
                command.cwd,
                Some(worker_root.join(".worktrees").join(branch))
            );
            assert!(command.originating_plugin.is_none());
            assert_eq!(command.command, PathBuf::from("env"));
            assert!(command
                .args
                .contains(&format!("{}=1", yinz_types::agent::AGENT_PANE_ENV)));
            assert_eq!(
                yinz_types::agent::agent_kind_of(&command.command, &command.args).as_deref(),
                Some("codex")
            );
            completion.set_affected_tab_id(yinz_types::internal_ids::TabId::new(0));
            completion.set_affected_pane_id(PaneId::Terminal(crate::TerminalId::new(pane)));
        }
    });
    for (pane, branch) in [(7, "review"), (8, "review-2")] {
        let spawned = spawn_agent_for_cli(
            AgentSpawnSpec {
                kind: Some(" Codex ".into()),
                model: Some("gpt-test".into()),
                branch: Some("review".into()),
                ..Default::default()
            },
            Some(root.clone()),
            None,
            None,
            crate::ClientId::new(1),
            &senders,
            &data_access,
        )
        .unwrap();
        assert_eq!(
            spawned.pane_id,
            PaneId::Terminal(crate::TerminalId::new(pane))
        );
        assert_eq!(spawned.branch.as_deref(), Some(branch));
        assert_eq!(spawned.cwd, Some(root.join(".worktrees").join(branch)));
    }
    worker.join().unwrap();
    let refused = spawn_agent_for_cli(
        AgentSpawnSpec {
            kind: Some("codex".into()),
            ..Default::default()
        },
        Some(root.clone()),
        None,
        None,
        crate::ClientId::new(1),
        &senders,
        &data_access,
    )
    .unwrap_err();
    assert!(refused.contains("not a linked git worktree"));
    let refused = spawn_agent_for_cli(
        AgentSpawnSpec {
            kind: Some("zsh".into()),
            branch: Some("invalid".into()),
            ..Default::default()
        },
        Some(root.clone()),
        None,
        None,
        crate::ClientId::new(1),
        &senders,
        &data_access,
    )
    .unwrap_err();
    assert!(refused.contains("recognized agent command"));
    assert!(!root.join(".worktrees/invalid").exists());
}

#[test]
fn automation_spawn_rejects_prompts_and_local_options_for_ssh_before_opening_a_pane() {
    let directory = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(directory.path());
    for field in ["prompt", "reuse_prompt"] {
        let spec =
            serde_json::from_value(serde_json::json!({"kind":"codex",field:"work"})).unwrap();
        let error = spawn_agent_for_cli(
            spec,
            None,
            None,
            None,
            crate::ClientId::new(1),
            &ThreadSenders::default(),
            &data,
        )
        .unwrap_err();
        assert!(error.contains("spawn does not accept"));
    }
    for input in [
        serde_json::json!({"kind":"codex"}),
        serde_json::json!({"kind":"codex","allow_outside_worktree":true,"branch":"topic"}),
    ] {
        let mut spec = serde_json::from_value(input).unwrap();
        assert!(agent_control::prepare_automation_agent(
            &mut spec,
            Some("/remote".into()),
            None,
            Some("server".into()),
            &data,
            &Default::default()
        )
        .is_err());
    }
    let mut spec = AgentSpawnSpec {
        kind: Some("codex".into()),
        allow_outside_worktree: Some(true),
        ..Default::default()
    };
    assert!(agent_control::prepare_automation_agent(
        &mut spec,
        Some("relative".into()),
        None,
        Some("server".into()),
        &data,
        &Default::default()
    )
    .is_err());
    let prepared = agent_control::prepare_automation_agent(
        &mut spec,
        Some("/remote/not-on-this-host".into()),
        None,
        Some("server".into()),
        &data,
        &Default::default(),
    )
    .unwrap();
    assert!(prepared.command.target.is_remote());
    assert_eq!(
        prepared.command.cwd,
        Some("/remote/not-on-this-host".into())
    );
}

#[test]
fn agent_destination_is_carried_for_local_and_ssh_spawns() {
    use yinz_types::agent::control::AgentPlacement;
    use yinz_types::internal_ids::TabId;
    for target in ["local", "ssh"] {
        for placement in [
            AgentPlacement::Tab(TabId::new(7)),
            AgentPlacement::Suppressed(None),
            AgentPlacement::Suppressed(Some(TabId::new(7))),
        ] {
            let suppressed = matches!(placement, AgentPlacement::Suppressed(_));
            let (_directory, root, data) = launch_failure_repository();
            for def in project_data::store::shared_defs() {
                data.plugin_data()
                    .define_entity(project_data::store::SHARED, "test", &def, false)
                    .unwrap();
            }
            let project = project_data::projects::validate("work", root.to_str().unwrap()).unwrap();
            data.plugin_data()
                .apply(
                    project_data::store::SHARED,
                    &[project_data::projects::to_mutation(&project)],
                )
                .unwrap();
            let (to_screen, screen_rx) = channels::unbounded();
            let (to_pty, pty_rx) = channels::unbounded();
            let senders = ThreadSenders {
                to_screen: Some(SenderWithContext::new(to_screen)),
                to_pty: Some(SenderWithContext::new(to_pty)),
                ..Default::default()
            };
            let worker = thread::spawn(move || {
                let (instruction, _) = screen_rx.recv_timeout(Duration::from_secs(30)).unwrap();
                let ScreenInstruction::ResolveAgentTab {
                    reply,
                    invoked_with,
                    ..
                } = instruction
                else {
                    panic!("expected resolution");
                };
                assert!(invoked_with.is_none());
                let lifetime = std::sync::Arc::new(());
                reply
                    .send(Ok(yinz_bus::route::AgentTabPlacement {
                        tab_id: TabId::new(7),
                        lifetime: std::sync::Arc::downgrade(&lifetime),
                        created: false,
                    }))
                    .unwrap();
                let (instruction, _) = pty_rx.recv_timeout(Duration::from_secs(30)).unwrap();
                let PtyInstruction::SpawnTerminal(
                    Some(TerminalAction::RunCommand(command)),
                    _,
                    _,
                    start_suppressed,
                    destination,
                    Some(mut completion),
                    false,
                ) = instruction
                else {
                    panic!("expected nonblocking spawn");
                };
                assert_eq!(start_suppressed, suppressed);
                assert_eq!(
                    destination,
                    ClientTabIndexOrPaneId::TabIndexNoFocus(TabId::new(7))
                );
                assert!(completion
                    .agent_tab
                    .as_ref()
                    .unwrap()
                    .lifetime
                    .upgrade()
                    .is_some());
                match target {
                    "ssh" => assert!(matches!(command.target, PaneTarget::Ssh { .. })),
                    _ => assert!(matches!(command.target, PaneTarget::Local)),
                }
                completion.set_affected_pane_id(PaneId::Terminal(TerminalId::new(17)));
                completion.set_affected_tab_id(TabId::new(7));
            });
            let result = spawn_agent_for_control(
                placement,
                AgentSpawnSpec {
                    kind: Some("codex".into()),
                    allow_outside_worktree: Some(true),
                    ..Default::default()
                },
                (target == "ssh").then(|| root.clone()),
                (target == "local").then(|| "work".into()),
                (target == "ssh").then(|| "host".into()),
                crate::ClientId::new(1),
                &senders,
                &data,
            )
            .unwrap();
            assert_eq!(result.tab_id, Some(TabId::new(7)));
            assert_eq!(result.cwd, Some(root));
            assert_eq!(result.pane_id, PaneId::Terminal(TerminalId::new(17)));
            worker.join().unwrap();
        }
    }
}

#[test]
fn project_provider_registration_requires_permissions_shared_scheme_and_owned_generation() {
    use yinz_types::project_provider::{providers, ProjectProvider};
    let mut env = crate::test_env::test_env();
    env.response_sink = None;
    let definition = ProjectProvider {
        id: "host-project-fixture".into(),
        priority: 7,
        plugin_id: 999,
        generation: 999,
    };
    let response = |env: &mut PluginEnv| -> Result<serde_json::Value, String> {
        let bytes: Vec<u8> = env.response_sink.take().unwrap();
        let wire = yinz_plugin_api::plugin_command::ProtobufAgentProvidersResponse::decode(
            bytes.as_slice(),
        )
        .unwrap();
        serde_json::from_str(&wire.json).unwrap()
    };
    run_plugin_command(
        &mut env,
        PluginCommand::RegisterProjectProvider(definition.clone()),
    )
    .unwrap();
    assert!(response(&mut env)
        .unwrap_err()
        .contains("Permission denied"));
    *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::RunCommands]));
    run_plugin_command(
        &mut env,
        PluginCommand::RegisterProjectProvider(definition.clone()),
    )
    .unwrap();
    assert!(response(&mut env).unwrap_err().contains("Scheme"));
    env.generation = Some(51);
    env.resolved_source_path = Some("fixture.scm".into());
    run_plugin_command(
        &mut env,
        PluginCommand::RegisterProjectProvider(definition.clone()),
    )
    .unwrap();
    assert!(response(&mut env).unwrap_err().contains("shared true"));
    env.plugin_config.initial_userspace_configuration =
        yinz_types::input::layout::PluginUserConfiguration::new(BTreeMap::from([(
            "shared".into(),
            "true".into(),
        )]));
    run_plugin_command(&mut env, PluginCommand::RegisterProjectProvider(definition)).unwrap();
    response(&mut env).unwrap();
    let provider = providers()
        .into_iter()
        .find(|p| p.id == "host-project-fixture")
        .unwrap();
    assert_eq!(provider.generation, 51);
    assert_eq!(provider.plugin_id, env.plugin_id.get());
    run_plugin_command(
        &mut env,
        PluginCommand::PublishProjectLocations(serde_json::json!({"providers":[],"locations":{}})),
    )
    .unwrap();
    assert!(response(&mut env).unwrap_err().contains("only the builtin"));
    env.clear_agent_providers();
    assert!(!providers().iter().any(|p| p.id == "host-project-fixture"));
}

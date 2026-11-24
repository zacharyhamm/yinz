use super::agent_control;
use super::agent_launch::wait_for_agent_placement;
use super::run_plugin_command;
use crate::PaneId;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::process;
use std::time::Duration;
use tokio::sync::oneshot;
use yinz_bus::route::NotificationEnd;
use yinz_bus::thread_bus::ThreadSenders;
use yinz_runtime::channels;
use yinz_storage::data_access::DataAccess;
use yinz_types::agent::control::AgentOutcome;
use yinz_types::agent::{AgentLaunchResources, AgentSpawnSpec};
use yinz_types::data::{PermissionType, PluginCommand};
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
        .contains("launch registration was not acknowledged"));
}

#[test]
fn agent_provider_registration_is_permissioned_owned_and_released() {
    use yinz_types::agent::provider::{provider, AgentProvider};
    let mut first = crate::test_env::test_env();
    let mut second = crate::test_env::test_env();
    // Workers share a plugin instance, but each isolate owns its own registrations.
    second.instance_id = first.instance_id;

    let definition = AgentProvider {
        kind: "host-fixture-agent".into(),
        executable: "host-fixture-agent".into(),
        ..Default::default()
    };
    let response = run_plugin_command(
        &mut first,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    );
    assert!(response
        .unwrap_err()
        .to_string()
        .contains("Permission denied"));
    assert!(provider(&definition.kind).is_none());
    for env in [&mut first, &mut second] {
        *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::RunCommands]));
    }
    let response = run_plugin_command(
        &mut first,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    );
    assert!(response.unwrap_err().to_string().contains("Scheme"));
    first.generation = Some(41);
    first.resolved_source_path = Some("fixture.js".into());
    let response = run_plugin_command(
        &mut first,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    );
    assert!(response.unwrap_err().to_string().contains("Scheme"));
    first.resolved_source_path = Some("fixture.scm".into());
    let response = run_plugin_command(
        &mut first,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    );
    assert!(response.unwrap_err().to_string().contains("shared true"));
    for (generation, env) in [(41, &mut first), (42, &mut second)] {
        env.generation = Some(generation);
        env.plugin_config.initial_userspace_configuration =
            yinz_types::input::layout::PluginUserConfiguration::new(BTreeMap::from([(
                "shared".into(),
                "true".into(),
            )]));
    }
    let response = run_plugin_command(
        &mut first,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    );
    assert!(response
        .unwrap()
        .expect_json()
        .as_array()
        .unwrap()
        .contains(&serde_json::to_value(&definition).unwrap()));
    let response = run_plugin_command(
        &mut second,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    );
    assert!(response.unwrap_err().to_string().contains("conflict"));
    second.clear_agent_providers();
    assert!(provider(&definition.kind).is_some());
    first.clear_agent_providers();
    assert!(provider(&definition.kind).is_none());
    let response = run_plugin_command(
        &mut second,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    );
    response.unwrap();
    first.clear_agent_providers();
    assert!(
        provider(&definition.kind).is_some(),
        "old instance cleanup cannot clear its replacement"
    );
    second.clear_agent_providers();
    for extension in ["scm", "ss", "sld"] {
        second.resolved_source_path = Some(format!("fixture.{extension}").into());
        let response = run_plugin_command(
            &mut second,
            PluginCommand::RegisterAgentProvider(definition.clone()),
        );
        assert!(response
            .unwrap()
            .expect_json()
            .as_array()
            .unwrap()
            .contains(&serde_json::to_value(&definition).unwrap()));
        second.clear_agent_providers();
    }
    second
        .unavailable
        .store(true, std::sync::atomic::Ordering::Release);
    let response = run_plugin_command(
        &mut second,
        PluginCommand::RegisterAgentProvider(definition.clone()),
    );
    assert!(response
        .unwrap_err()
        .to_string()
        .contains("no longer available"));
    assert!(provider(&definition.kind).is_none());
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
fn project_provider_registration_requires_permissions_shared_scheme_and_owned_generation() {
    use yinz_types::project_provider::{providers, ProjectProvider};
    let mut env = crate::test_env::test_env();

    let definition = ProjectProvider {
        id: "host-project-fixture".into(),
        priority: 7,
        plugin_id: 999,
        generation: 999,
    };
    let response = run_plugin_command(
        &mut env,
        PluginCommand::RegisterProjectProvider(definition.clone()),
    );
    assert!(response
        .unwrap_err()
        .to_string()
        .contains("Permission denied"));
    *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::RunCommands]));
    let response = run_plugin_command(
        &mut env,
        PluginCommand::RegisterProjectProvider(definition.clone()),
    );
    assert!(response.unwrap_err().to_string().contains("Scheme"));
    env.generation = Some(51);
    env.resolved_source_path = Some("fixture.scm".into());
    let response = run_plugin_command(
        &mut env,
        PluginCommand::RegisterProjectProvider(definition.clone()),
    );
    assert!(response.unwrap_err().to_string().contains("shared true"));
    env.plugin_config.initial_userspace_configuration =
        yinz_types::input::layout::PluginUserConfiguration::new(BTreeMap::from([(
            "shared".into(),
            "true".into(),
        )]));
    let response = run_plugin_command(&mut env, PluginCommand::RegisterProjectProvider(definition));
    response.unwrap();
    let provider = providers()
        .into_iter()
        .find(|p| p.id == "host-project-fixture")
        .unwrap();
    assert_eq!(provider.generation, 51);
    assert_eq!(provider.plugin_id, env.plugin_id.get());
    let response = run_plugin_command(
        &mut env,
        PluginCommand::PublishProjectLocations(serde_json::json!({"providers":[],"locations":{}})),
    );
    assert!(response
        .unwrap_err()
        .to_string()
        .contains("only the builtin"));
    env.clear_agent_providers();
    assert!(!providers().iter().any(|p| p.id == "host-project-fixture"));
}

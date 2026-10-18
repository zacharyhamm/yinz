use super::{
    data_parity::DataHost,
    runtime_parity::{default_reply, Pair, Responder},
};
use prost::Message;
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};
use yinz_plugin_api::plugin_command::*;
use yinz_types::{
    data::*,
    internal_ids::{PluginId, TabPosition, TerminalId},
};

fn responder() -> Responder {
    configured_responder(false, false)
}
fn configured_responder(projects: bool, fail_open: bool) -> Responder {
    let data = DataHost::new("new-agent-parity");
    let responder = data.responder(Arc::new(move |command| {
        Some(match command {
            PluginCommand::GetAgentProviders => ProtobufAgentProvidersResponse {
                json: "{\"Ok\":[]}".into(),
            }
            .encode_to_vec(),
            PluginCommand::SpawnAgent { .. } => ProtobufSpawnAgentResponse::from(if fail_open {
                Err("fixture launch failed".into())
            } else {
                Ok(SpawnedAgent {
                    pane_id: PaneId::Terminal(TerminalId::new(11)),
                    branch: Some("topic".into()),
                    cwd: Some("/work/.worktrees/topic".into()),
                    ..Default::default()
                })
            })
            .encode_to_vec(),
            PluginCommand::OpenCommandPaneNearPlugin(..) => {
                ProtobufOpenCommandPaneNearPluginResponse {
                    pane_id: (!fail_open).then_some(ProtobufPaneId {
                        pane_type: 0,
                        id: 12,
                    }),
                }
                .encode_to_vec()
            },
            PluginCommand::GetPaneInfo(..) => {
                ProtobufGetPaneInfoResponse { pane_info: None }.encode_to_vec()
            },
            _ => return default_reply(command),
        })
    }));
    if projects {
        for def in agent_proto::store::shared_defs() {
            responder(&PluginCommand::DataDefineEntity {
                namespace: Some(agent_proto::store::SHARED.into()),
                def,
                allow_destructive: false,
            })
            .unwrap();
        }
        data.apply(
            agent_proto::store::SHARED,
            &[
                Mutation::put("project")
                    .set("name", "alpha")
                    .set("root", "/work/alpha")
                    .set("worktree_dir", "../alpha trees")
                    .build(),
                Mutation::put("project")
                    .set("name", "beta")
                    .set("root", "/work/beta")
                    .set("worktree_dir", "/tmp/beta trees")
                    .build(),
            ],
        );
    }
    responder
}
fn pair(config: BTreeMap<String, String>) -> Pair {
    Pair::with_hosts("new-agent", config, responder(), responder(), None)
}
fn snapshot(pair: &mut Pair, command: Option<&str>, host: Option<&str>) -> Vec<serde_json::Value> {
    let payload = json!({"panes":[{"pane_id":7,"title":"agent","command":command,"cwd":"/work/topic","host":host,"status":"Running","tab":[1,"work"]}],"activity":{},"manual_kinds":{}}).to_string();
    let message = PipeMessage::new(
        PipeSource::Plugin(PluginId::new(2)),
        "agent_snapshot",
        &Some(payload),
        &None,
        false,
    );
    assert_eq!(pair.steel.pipe(&message), pair.wasm.pipe(&message));
    pair.commands()
}
#[test]
fn new_agent_actual_runtimes_match_form_navigation_probe_suggestions_and_submit() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "new_agent_actual_runtimes_match_form_navigation_probe_suggestions_and_submit",
    );
    let mut pair = pair(BTreeMap::from([("caller_cwd".into(), "/work".into())]));
    pair.viewports();
    for (rows, cols) in [(30, 120), (14, 80), (60, 240)] {
        pair.event(Event::TabUpdate(vec![TabInfo {
            active: true,
            display_area_columns: yinz_types::position::ColumnCount::new(cols),
            viewport_rows: yinz_types::position::RowCount::new(rows),
            ..Default::default()
        }]));
    }
    for kind in ["branches", "worktrees"] {
        pair.event(Event::RunCommandResult(
            Some(0),
            if kind == "branches" {
                b"main\ntopic\nother\n".to_vec()
            } else {
                b"worktree /work\n\nworktree /work/.worktrees/topic\n\nworktree /outside/other\n"
                    .to_vec()
            },
            vec![],
            BTreeMap::from([
                ("git_probe".into(), kind.into()),
                ("git_probe_id".into(), "1".into()),
            ]),
        ));
    }
    for _ in 0..12 {
        pair.key("Tab");
        pair.render(25, 60);
    }
    pair.key("Enter");
    pair.viewports();
    // The create button wraps to project; branch is the ninth field.
    for _ in 0..9 {
        pair.key("Tab");
    }
    pair.key("Enter");
    pair.event(Event::PastedText("topic".into()));
    pair.key("Enter");
    for _ in 0..4 {
        pair.key("Tab");
    }
    let commands = pair.key("Enter");
    assert!(commands.iter().any(|c| c.get("SpawnAgent").is_some()));
}
#[test]
fn new_agent_actual_runtimes_match_prefilled_prompt_and_missing_source_modes() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "new_agent_actual_runtimes_match_prefilled_prompt_and_missing_source_modes",
    );
    for config in [
        BTreeMap::from([(
            "initial_prompt".into(),
            "Todo #1: multiline\n\nKeep **all** this prompt, including 雪.".into(),
        )]),
        BTreeMap::from([("duplicate_focused".into(), "true".into())]),
        BTreeMap::from([("fork_focused".into(), "true".into())]),
    ] {
        let mut pair = pair(config);
        pair.viewports();
        pair.key("Esc");
    }
}
#[test]
fn new_agent_actual_runtimes_match_fork_waiting_ready_missing_and_duplicate() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "new_agent_actual_runtimes_match_fork_waiting_ready_missing_and_duplicate",
    );
    for (duplicate, command, host) in [
        (false, Some("codex"), None),
        (false, None, None),
        (true, Some("claude"), Some("build-box")),
        (true, None, None),
    ] {
        let mut pair = pair(BTreeMap::from([
            (
                if duplicate {
                    "duplicate_focused"
                } else {
                    "fork_focused"
                }
                .into(),
                "true".into(),
            ),
            ("caller_terminal_pane_id".into(), "7".into()),
        ]));
        pair.viewports();
        if !duplicate {
            pair.key("Tab");
            pair.key("Enter");
            pair.viewports();
        }
        let commands = snapshot(&mut pair, command, host);
        if duplicate && command.is_some() {
            assert!(commands
                .iter()
                .any(|c| c.get("OpenCommandPaneNearPlugin").is_some()));
        }
        pair.viewports();
        if !duplicate {
            let commands = pair.key("Enter");
            if command.is_some() {
                assert!(commands
                    .iter()
                    .any(|c| c.get("OpenCommandPaneNearPlugin").is_some()));
            }
        }
    }
}

fn tabs(pair: &mut Pair, count: usize) {
    for _ in 0..count {
        pair.key("Tab");
    }
}
fn probe(pair: &mut Pair, kind: &str, id: &str, code: Option<i32>, stdout: &str) {
    pair.event(Event::RunCommandResult(
        code,
        stdout.as_bytes().to_vec(),
        vec![],
        BTreeMap::from([
            ("git_probe".into(), kind.into()),
            ("git_probe_id".into(), id.into()),
        ]),
    ));
}
#[test]
fn new_agent_actual_runtimes_match_provider_refresh_and_launch_errors() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "new_agent_actual_runtimes_match_provider_refresh_and_launch_errors",
    );
    let mut pair = Pair::with_hosts(
        "new-agent",
        BTreeMap::new(),
        configured_responder(false, true),
        configured_responder(false, true),
        None,
    );
    let descriptor = json!({"kind":"fixture-agent","executable":"fixture-agent","launch":{
        "efforts":{"high":["--high"]},"permissions":{"skip":["--skip"]},
        "permission_descriptions":{"skip":"Fixture skip description"}
    }});
    let providers = || serde_json::from_value(json!([descriptor.clone()])).unwrap();
    pair.event(Event::AgentProvidersChanged(providers()));
    pair.key("Tab");
    pair.key("Enter");
    for _ in 0..6 {
        pair.key("Down");
    }
    pair.key("Enter");
    tabs(&mut pair, 2);
    pair.key("Enter");
    pair.key("Down");
    pair.key("Enter");
    pair.key("Tab");
    pair.key("Enter");
    pair.key("Down");
    pair.key("Enter");
    pair.viewports();
    // Disable branch creation, then submit the registered provider's launch.
    tabs(&mut pair, 2);
    pair.key("Enter");
    tabs(&mut pair, 6);
    let commands = pair.key("Enter");
    let spawn = commands
        .iter()
        .find_map(|command| command.get("SpawnAgent"))
        .unwrap();
    assert_eq!(spawn["spec"]["kind"], "Fixture-agent");
    assert_eq!(spawn["spec"]["effort"], "high");
    assert_eq!(spawn["spec"]["permission_mode"], "skip");
    pair.viewports();
    pair.event(Event::AgentProvidersChanged(providers()));
    pair.viewports();
    let mut changed = descriptor.clone();
    changed["launch"]["efforts"] = json!({});
    changed["launch"]["permissions"] = json!({});
    changed["launch"]["permission_descriptions"] = json!({});
    pair.event(Event::AgentProvidersChanged(
        serde_json::from_value(json!([changed])).unwrap(),
    ));
    pair.viewports();
    pair.event(Event::AgentProvidersChanged(vec![]));
    pair.viewports();
    pair.key("Enter");
    pair.viewports();
}
#[test]
fn new_agent_actual_runtimes_match_project_probe_defaults_stale_replies_and_manual_edits() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "new_agent_actual_runtimes_match_project_probe_defaults_stale_replies_and_manual_edits",
    );
    let mut pair = Pair::with_hosts(
        "new-agent",
        BTreeMap::from([("caller_cwd".into(), "/outside/linked".into())]),
        configured_responder(true, false),
        configured_responder(true, false),
        None,
    );
    probe(
        &mut pair,
        "worktrees",
        "1",
        Some(0),
        "worktree /work/alpha\n\nworktree /outside/linked\n\nworktree /work/alpha/.trees/topic\n",
    );
    pair.viewports();
    tabs(&mut pair, 10);
    pair.key("Enter");
    pair.key("Ctrl a");
    pair.key("Ctrl k");
    pair.event(Event::PastedText("/manual".into()));
    pair.key("Enter");
    probe(
        &mut pair,
        "worktrees",
        "1",
        None,
        "worktree /work/alpha\n\nworktree /outside/linked\n",
    );
    pair.viewports();
    tabs(&mut pair, 3);
    pair.key("Enter");
    pair.key("Down");
    pair.key("Down");
    pair.key("Enter");
    pair.viewports();
    probe(&mut pair, "branches", "1", Some(0), "stale-branch\n");
    probe(&mut pair, "branches", "2", Some(1), "failed-branch\n");
    probe(
        &mut pair,
        "branches",
        "2",
        Some(0),
        " main\n\n beta-topic \n",
    );
    tabs(&mut pair, 8);
    pair.key("Enter");
    pair.event(Event::PastedText("topic".into()));
    pair.key("Enter");
    tabs(&mut pair, 4);
    let commands = pair.key("Enter");
    let spawn = commands
        .iter()
        .find_map(|command| command.get("SpawnAgent"))
        .unwrap();
    assert_eq!(spawn["source_directory"], "/work/beta");
    assert_eq!(spawn["spec"]["worktree_dir"], "/tmp/beta trees");
}
#[test]
fn new_agent_actual_runtimes_match_failed_fork_and_duplicate_open() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "new_agent_actual_runtimes_match_failed_fork_and_duplicate_open",
    );
    for duplicate in [false, true] {
        let mut pair = Pair::with_hosts(
            "new-agent",
            BTreeMap::from([
                (
                    (if duplicate {
                        "duplicate_focused"
                    } else {
                        "fork_focused"
                    })
                    .into(),
                    "true".into(),
                ),
                ("caller_terminal_pane_id".into(), "7".into()),
            ]),
            configured_responder(false, true),
            configured_responder(false, true),
            None,
        );
        let commands = snapshot(&mut pair, Some("codex"), None);
        if duplicate {
            assert!(commands
                .iter()
                .any(|command| command.get("Notify").is_some()));
        } else {
            pair.key("Tab");
            let commands = pair.key("Enter");
            assert!(commands
                .iter()
                .any(|command| command.get("OpenCommandPaneNearPlugin").is_some()));
        }
        pair.viewports();
    }
}

#[test]
fn new_agent_actual_runtimes_hide_agents_panel_and_focus_created_pane() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "new_agent_actual_runtimes_hide_agents_panel_and_focus_created_pane",
    );
    let mut pair = pair(BTreeMap::new());
    let mut event = Event::PaneUpdate(PaneManifest {
        panes: std::collections::HashMap::from([(
            TabPosition::new(0),
            vec![
                PaneInfo {
                    id: PaneId::Plugin(PluginId::new(1)),
                    ..Default::default()
                },
                PaneInfo {
                    id: PaneId::Plugin(PluginId::new(20)),
                    plugin_url: Some("yinz:agents".into()),
                    ..Default::default()
                },
            ],
        )]),
    });
    pair.wasm.update(&event);
    // The recorded panel used Agents identity; the current host supplies container identity.
    if let Event::PaneUpdate(manifest) = &mut event {
        let pane = &mut manifest.panes.get_mut(&TabPosition::new(0)).unwrap()[1];
        pane.plugin_url = Some("file:/custom.scm".into());
        pane.pane_tabs = Some(serde_json::from_value(json!({
            "id":7, "pane_id":{"Plugin":20}, "active":0, "bar_focused":[], "overlay_group":true,
            "tabs":[{"descriptor":{"label":"Custom","location":"file:/custom.scm"},"plugin_id":20,"state":"Ready"}]
        })).unwrap());
    }
    pair.steel.update(&event);
    pair.commands();
    tabs(&mut pair, 6);
    pair.key("Enter");
    tabs(&mut pair, 6);
    let commands = pair.key("Enter");
    assert!(commands.contains(&json!({"HidePaneWithId":{"Plugin":20}})));
    assert!(commands.contains(&json!({"FocusTerminalPane":[11,false,false]})));
}
#[test]
fn new_agent_actual_runtimes_duplicate_explicit_hidden_sandbox_source() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "new_agent_actual_runtimes_duplicate_explicit_hidden_sandbox_source",
    );
    let lookup = || {
        let fallback = responder();
        Arc::new(move |command: &PluginCommand| {
            if matches!(command, PluginCommand::GetPaneInfo(..)) {
                Some(
                    ProtobufGetPaneInfoResponse {
                        pane_info: Some(
                            PaneInfo {
                                id: PaneId::Terminal(TerminalId::new(7)),
                                terminal_command: Some("codex".into()),
                                cwd: Some("/sandbox/work".into()),
                                is_suppressed: true,
                                target: yinz_types::command::PaneTarget::bubblewrap(
                                    std::path::Path::new("/sandbox"),
                                )
                                .unwrap(),
                                ..Default::default()
                            }
                            .try_into()
                            .unwrap(),
                        ),
                    }
                    .encode_to_vec(),
                )
            } else {
                fallback(command)
            }
        }) as Responder
    };
    let mut pair = Pair::with_hosts(
        "new-agent",
        BTreeMap::from([
            ("duplicate_focused".into(), "true".into()),
            ("caller_terminal_pane_id".into(), "7".into()),
        ]),
        lookup(),
        lookup(),
        None,
    );
    let message = PipeMessage::new(
        PipeSource::Plugin(PluginId::new(2)),
        "agent_snapshot",
        &Some(json!({"panes":[],"activity":{},"manual_kinds":{}}).to_string()),
        &None,
        false,
    );
    assert_eq!(pair.steel.pipe(&message), pair.wasm.pipe(&message));
    let commands = pair.commands();
    assert!(commands
        .iter()
        .any(|command| command.get("GetPaneInfo").is_some()));
    let open = commands
        .iter()
        .find_map(|command| command.get("OpenCommandPaneNearPlugin"))
        .unwrap();
    assert_eq!(open[0]["cwd"], "/sandbox/work");
    assert_eq!(open[0]["target"]["Bubblewrap"]["root"], "/sandbox");
}

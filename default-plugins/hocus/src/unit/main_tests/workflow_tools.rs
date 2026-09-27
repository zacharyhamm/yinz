use super::*;
use crate::workflows;
use serde_json::json;
use zellij_tile::prelude::PluginId;

fn workflow_state() -> State {
    let mut state = project_state();
    state.plugin_instances.insert(
        PluginId::new(7),
        PluginInstance {
            configuration: BTreeMap::new(),
            location: "zellij:swamp-bridge".into(),
            has_pane: true,
        },
    );
    state
}

#[test]
fn workflow_registration_confirmation_and_planning_policy_agree() {
    for name in workflows::TOOLS {
        assert!(tools::names().contains(&name.to_string()));
        assert_eq!(tools::family_of(name).unwrap().name, "workflow");
        assert_eq!(
            tools::READ_ONLY.contains(name),
            workflows::READ_ONLY.contains(name)
        );
        let call = ToolCall {
            name: name.to_string(),
            arguments: "{}".into(),
            ..Default::default()
        };
        assert_eq!(
            tools::call_needs_confirmation(&call),
            workflows::mutates(name)
        );
        assert_eq!(
            tools::consequence(&call).is_some(),
            workflows::mutates(name)
        );
    }
    for (tool, args) in [
        ("workflow_evaluate", r#"{"workflow":"test"}"#),
        ("workflow_launch", r#"{"workflow":"test"}"#),
    ] {
        let mut state = workflow_state();
        state.plan = Plan::Planning;
        state.on_chunk("hocus-1", &tool_call_frame("call", tool, args), false, None);
        state.on_chunk("hocus-1", b"", true, None);
        assert!(tool_results(&state)[0].content.contains("planning mode"));
        assert!(state.workflow_call.is_none());
    }
    let mut state = workflow_state();
    state.on_chunk(
        "hocus-1",
        &tool_call_frame("call", "workflow_create", r#"{"name":"test"}"#),
        false,
        None,
    );
    state.on_chunk("hocus-1", b"", true, None);
    assert!(matches!(state.status, Status::ConfirmingTool));
    assert!(state.workflow_call.is_none());
}

#[test]
fn workflow_arguments_reject_unscoped_controls_and_unexpected_fields() {
    for (name, args) in [
        ("workflow_resume", json!({"run":"latest"})),
        ("workflow_cancel", json!({"workflow":"name"})),
        (
            "workflow_get",
            json!({"workflow":"name","directory":"/elsewhere"}),
        ),
        (
            "workflow_edit",
            json!({"workflow":"name","definition":{},"expected":null}),
        ),
        ("workflow_data", json!({"predicate":"true","limit":-1})),
        (
            "workflow_logs",
            json!({"run":"11111111-1111-4111-8111-111111111111","tail":10001}),
        ),
    ] {
        assert!(workflows::arguments(&ToolCall {
            name: name.into(),
            arguments: args.to_string(),
            ..Default::default()
        })
        .is_err());
    }
    let mut state = workflow_state();
    state.on_chunk(
        "hocus-1",
        &tool_call_frame("call", "workflow_delete", "{}"),
        false,
        None,
    );
    state.on_chunk("hocus-1", b"", true, None);
    assert!(tool_results(&state)[0].content.contains("required"));
    assert!(!matches!(state.status, Status::ConfirmingTool));
}

#[test]
fn workflow_rpc_pins_owner_and_correlation_retains_results_and_never_replays_timeouts() {
    let mut state = workflow_state();
    assert!(!run_file_tool(
        &mut state,
        "workflow_get",
        r#"{"workflow":"test"}"#
    ));
    let pending = state.workflow_call.clone().unwrap();
    let result =
        json!({"version":1,"call_id":pending.rpc_id,"result":{"output":"x".repeat(100000)}})
            .to_string();
    assert!(!state.on_workflow_reply(PluginId::new(8), &result, false));
    assert!(!state.on_workflow_reply(
        pending.owner.unwrap(),
        &json!({"version":1,"call_id":"other","result":{}}).to_string(),
        false
    ));
    state.mono_ms = 1000;
    assert!(state.on_workflow_reply(
        pending.owner.unwrap(),
        &json!({"version":1,"call_id":pending.rpc_id,"payload":{"status":"waiting"}}).to_string(),
        true
    ));
    assert_eq!(state.extension_call_started_ms, Some(1000));
    assert!(state.on_workflow_reply(pending.owner.unwrap(), &result, false));
    assert!(tool_results(&state)[0].content.len() > 100000);
    assert!(!state.on_workflow_reply(pending.owner.unwrap(), &result, false));

    assert!(!run_file_tool(
        &mut state,
        "workflow_launch",
        r#"{"workflow":"test","inputs":{"launch":"supplied-launch","session":"session"}}"#
    ));
    let seq = state.workflow_seq;
    state.on_timer(121.0);
    assert_eq!(state.workflow_seq, seq);
    assert!(state.workflow_call.is_none());
    let uncertain: serde_json::Value =
        serde_json::from_str(&tool_results(&state).last().unwrap().content).unwrap();
    assert_eq!(uncertain["error"]["code"], "transport_unknown");
    assert!(uncertain["history_filter"]
        .as_str()
        .unwrap()
        .contains(uncertain["request_id"].as_str().unwrap()));
    assert!(uncertain["history_filter"]
        .as_str()
        .unwrap()
        .contains("inputs.launch == \"supplied-launch\""));
}

#[test]
fn workflow_calls_follow_parked_chats_and_require_the_active_project_and_bridge() {
    let mut state = workflow_state();
    assert!(!run_file_tool(
        &mut state,
        "workflow_get",
        r#"{"workflow":"test"}"#
    ));
    let pending = state.workflow_call.clone().unwrap();
    let chat = state.take_chat();
    state.parked.push(chat);
    let reply = json!({"version":1,"call_id":pending.rpc_id,"result":{"id":"test"}}).to_string();
    assert!(state.on_workflow_reply(pending.owner.unwrap(), &reply, false));
    assert!(state.parked[0].workflow_call.is_none());
    assert!(state.parked[0]
        .messages
        .iter()
        .any(|m| m.role == Role::Tool && m.content.contains("test")));
    state.active_project = None;
    assert!(run_file_tool(
        &mut state,
        "workflow_get",
        r#"{"workflow":"test"}"#
    ));
    assert!(tool_results(&state)
        .last()
        .unwrap()
        .content
        .contains("no active project"));
    let mut state = project_state();
    assert!(!run_file_tool(
        &mut state,
        "workflow_get",
        r#"{"workflow":"test"}"#
    ));
    let pending = state.workflow_call.clone().unwrap();
    assert!(pending.owner.is_none());
    let reply = json!({"version":1,"call_id":pending.rpc_id,"result":{}}).to_string();
    assert!(!state.on_workflow_reply(PluginId::new(9), &reply, false));
    state.plugin_instances.insert(
        PluginId::new(9),
        PluginInstance {
            configuration: BTreeMap::new(),
            location: "zellij:swamp-bridge".into(),
            has_pane: false,
        },
    );
    assert!(state.on_workflow_reply(PluginId::new(9), &reply, false));
}

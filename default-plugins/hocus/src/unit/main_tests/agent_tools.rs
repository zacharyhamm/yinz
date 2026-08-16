use super::*;

/// A global pane is in no tab, so it is in its own field of the manifest. Missing
/// it is how an agent beside a global Hocus would stay invisible.
#[test]
fn a_snapshot_keeps_terminal_panes_of_every_scope() {
    let plugin = PaneInfo {
        is_plugin: true,
        ..pane_info(1, "hocus", None)
    };
    let suppressed = PaneInfo {
        is_suppressed: true,
        ..pane_info(2, "hidden", Some("codex"))
    };
    let held = PaneInfo {
        is_held: true,
        exited: true,
        ..pane_info(3, "codex", Some("codex"))
    };
    let global = PaneManifest {
        global_panes: vec![pane_info(5, "aider", Some("aider"))],
        ..manifest(vec![plugin, suppressed, held])
    };

    let panes = terminal_panes(&global, &[tab_info(0, 9, "work")]);
    assert_eq!(
        panes.iter().map(|pane| pane.pane_id).collect::<Vec<_>>(),
        vec![3, 5],
    );
    assert_eq!(
        panes[0].status,
        agents::AgentStatus::Held,
        "held beats exited"
    );
    assert_eq!(panes[0].tab, Some((9, "work".to_owned())));
    assert_eq!(panes[1].tab, None, "a global pane is in no tab");
}

#[test]
fn agent_register_rejects_a_pane_that_is_not_there_and_a_kind_that_is_not_a_word() {
    let mut state = state_with_panes();
    assert_eq!(
        state.run_local(&agent_call(
            "agent_register",
            r#"{"pane_id":9,"kind":"codex"}"#
        )),
        Some("pane 9 is not available".into()),
    );
    assert_eq!(
        state.run_local(&agent_call(
            "agent_register",
            r#"{"pane_id":4,"kind":"code review"}"#
        )),
        Some("kind must be 1 to 32 characters of a-z, 0-9, _ or -".into()),
    );
    assert!(state.manual_kinds.is_empty());
}

#[test]
fn a_manual_kind_lasts_until_its_pane_closes() {
    let mut state = state_with_panes();
    assert_eq!(
        state.run_local(&agent_call(
            "agent_register",
            r#"{"pane_id":4,"kind":"reviewer"}"#
        )),
        Some("registered pane 4 as reviewer".into()),
    );

    // A retitled pane is still the same pane, so the kind stays.
    state.set_panes(terminal_panes(
        &manifest(vec![pane_info(4, "new title", Some("zsh"))]),
        &[],
    ));
    assert_eq!(
        state.manual_kinds.get(&4).map(String::as_str),
        Some("reviewer")
    );

    state.set_panes(Vec::new());
    assert!(state.manual_kinds.is_empty(), "the pane closed, so it goes");
}

/// Off wasm every host call is a stub, so this checks which panes were listed and
/// what was said about them, not what they were showing.
#[test]
fn agent_list_covers_detected_and_registered_panes_only() {
    let mut state = state_with_panes();
    state.run_local(&agent_call(
        "agent_register",
        r#"{"pane_id":4,"kind":"reviewer"}"#,
    ));

    let list = state.run_local(&agent_call("agent_list", "{}")).unwrap();
    let list: serde_json::Value = serde_json::from_str(&list).unwrap();
    let list = list.as_array().unwrap();

    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["pane_id"], 3);
    assert_eq!(list[0]["kind"], "codex");
    assert_eq!(list[0]["source"], "auto");
    assert_eq!(list[0]["status"], "running");
    assert_eq!(list[1]["pane_id"], 4);
    assert_eq!(list[1]["kind"], "reviewer");
    assert_eq!(list[1]["source"], "manual");
}

#[test]
fn agent_list_is_empty_before_the_first_pane_update() {
    let mut state = idle_state();
    assert_eq!(
        state.run_local(&agent_call("agent_list", "{}")),
        Some("[]".into()),
    );
}

#[test]
fn agent_spawn_rejects_no_command_and_a_kind_that_is_not_a_word() {
    let mut state = idle_state();
    assert_eq!(
        state.run_local(&agent_call("agent_spawn", "{}")),
        Some("command is required".into()),
    );
    assert_eq!(
        state.run_local(&agent_call(
            "agent_spawn",
            r#"{"command":"zsh","kind":"code review"}"#
        )),
        Some("kind must be 1 to 32 characters of a-z, 0-9, _ or -".into()),
    );
    assert!(state.manual_kinds.is_empty());
}

/// The dispatch itself needs the host, so what is tested here is the half that
/// does not: folding `new_pane`'s answer back into the kinds.
#[test]
fn a_spawned_kind_is_registered_against_the_pane_new_pane_reported() {
    let mut state = idle_state();
    assert_eq!(
        state.register_spawned(
            Some("reviewer"),
            r#"{"opened":true,"pane_id":7,"is_plugin":false}"#
        ),
        r#"{"opened":true,"pane_id":7,"is_plugin":false} (registered as reviewer)"#,
    );
    assert_eq!(
        state.manual_kinds.get(&7).map(String::as_str),
        Some("reviewer")
    );
    assert!(state.pending_agent_panes.contains(&7));

    // The subscribe reply can beat the pane-created event back to hocus.
    // That early snapshot must not discard the override it just sent.
    assert!(!state.set_panes(Vec::new()));
    assert_eq!(
        state.manual_kinds.get(&7).map(String::as_str),
        Some("reviewer")
    );

    state.set_panes(terminal_panes(
        &manifest(vec![pane_info(7, "zsh", Some("zsh"))]),
        &[],
    ));
    assert!(!state.pending_agent_panes.contains(&7));
    assert_eq!(
        state.manual_kinds.get(&7).map(String::as_str),
        Some("reviewer")
    );

    assert!(state.set_panes(Vec::new()));
    assert!(!state.manual_kinds.contains_key(&7));

    // No kind, or no pane in the answer: the result passes through untouched.
    assert_eq!(
        state.register_spawned(None, r#"{"opened":true,"pane_id":8,"is_plugin":false}"#),
        r#"{"opened":true,"pane_id":8,"is_plugin":false}"#,
    );
    assert_eq!(
        state.register_spawned(Some("scout"), "zellij did not open the pane"),
        "zellij did not open the pane",
    );
    assert!(state.manual_kinds.is_empty());
}

/// The whole of `agent_control`, compared as the complete host call it becomes.
/// A wrong pane kind or a wrong tool here is a killed pane.
#[test]
fn every_agent_action_maps_onto_the_tool_that_already_does_it() {
    let state = state_with_panes();
    for (arguments, name, mapped) in [
        (
            r#"{"pane_id":3,"text":"go","keys":["enter"]}"#,
            "send_input",
            r#"{"is_plugin":false,"keys":["enter"],"pane_id":3,"text":"go"}"#,
        ),
        (
            r#"{"pane_id":3,"action":"interrupt"}"#,
            "send_input",
            r#"{"is_plugin":false,"keys":["ctrl_c"],"pane_id":3}"#,
        ),
        (
            r#"{"pane_id":3,"action":"restart"}"#,
            "rerun_pane",
            r#"{"is_plugin":false,"pane_id":3}"#,
        ),
        (
            r#"{"pane_id":3,"action":"close"}"#,
            "close_pane",
            r#"{"is_plugin":false,"pane_id":3}"#,
        ),
    ] {
        let call = agent_call(
            if arguments.contains("action") {
                "agent_control"
            } else {
                "agent_send"
            },
            arguments,
        );
        let args = tools::Args::parse(arguments).unwrap();
        let host = state.agent_host_call(&call, &args).unwrap();
        assert_eq!(host.name, name, "{arguments}");
        assert_eq!(host.arguments, mapped, "{arguments}");
        assert_eq!(host.id, call.id, "the result still answers the call");
    }
}

#[test]
fn agent_tools_refuse_a_pane_that_is_not_an_agent_and_an_action_that_is_not_one() {
    let mut state = state_with_panes();
    assert_eq!(
        state.run_local(&agent_call(
            "agent_control",
            r#"{"pane_id":3,"action":"pause"}"#
        )),
        Some("unknown agent action: pause".into()),
    );
    // Pane 4 is a plain shell that nobody registered.
    assert_eq!(
        state.run_local(&agent_call("agent_send", r#"{"pane_id":4,"text":"go"}"#)),
        Some("pane 4 is not an agent; call agent_list, or agent_register it first".into()),
    );
    assert_eq!(
        state.run_local(&agent_call("agent_send", r#"{"text":"go"}"#)),
        Some("pane_id is required".into()),
    );
}

/// A closed pane leaves the snapshot, and with it every way to send to it.
#[test]
fn a_closed_agent_can_no_longer_be_sent_to() {
    let mut state = state_with_panes();
    assert!(state.is_agent(3));
    state.set_panes(Vec::new());
    assert!(!state.is_agent(3));
}

/// What the latch changes for the tools, stated on purpose: an agent that is
/// busy shelling out is still an agent, so `agent_send` reaches it and the
/// keys land in whatever it is running. That is what a human at the keyboard
/// would get, and it is the only way to interrupt a tool run — losing that
/// would be worse than the surprise.
#[test]
fn a_shelled_out_agent_can_still_be_sent_to() {
    let mut state = state_with_panes();
    let shelled_out = agents::AgentPane {
        title: "zsh".to_owned(),
        command: Some("git status".to_owned()),
        detected: Some("codex".to_owned()),
        ..panes_with_agent()[0].clone()
    };

    state.set_panes(vec![shelled_out]);

    assert!(state.is_agent(3));
}

#[test]
fn agent_changes_do_not_post_to_chat() {
    let mut state = state_with_panes();
    let mut panes = panes_with_agent();
    panes[0].status = agents::AgentStatus::Exited;

    state.apply_poll(
        panes,
        agents::Activity::new(),
        agents::Summaries::new(),
        None,
    );
    state.apply_poll(
        Vec::new(),
        agents::Activity::new(),
        agents::Summaries::new(),
        None,
    );

    assert!(state.messages.is_empty());
}

#[test]
fn a_model_agent_action_is_confirmed_and_a_typed_one_is_not() {
    for (name, arguments) in [
        ("agent_send", r#"{"pane_id":3,"text":"go"}"#),
        ("agent_control", r#"{"pane_id":3,"action":"close"}"#),
    ] {
        let mut state = State {
            stream: state_streaming(Vec::new(), "hocus-1").stream,
            reply_at: Some(1),
            messages: vec![Message::user("hi"), Message::assistant("")],
            ..state_with_panes()
        };
        let frame = tool_call_frame("call_1", name, arguments);
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(matches!(state.status, Status::ConfirmingTool), "{name}");
        assert!(tool_results(&state).is_empty(), "{name} ran unasked");
    }

    // Typing the command is the confirmation, so it runs on the spot.
    let mut state = state_with_panes();
    state.run_manual(r#"/agent_control pane_id=3 action=close"#);
    assert!(!matches!(state.status, Status::ConfirmingTool));
    assert_eq!(tool_results(&state).len(), 1);
}

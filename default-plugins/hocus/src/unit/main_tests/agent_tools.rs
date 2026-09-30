use super::*;
use yinz_tile::prelude::TabId;

#[test]
fn agent_runs_forwards_empty_arguments_and_rejects_malformed_json() {
    for arguments in ["", " \t\r\n", "{}", "{"] {
        let mut state = idle_state();
        add_background_store(&mut state);
        let call = agent_call("agent_runs", arguments);
        state.pending.push_back(call.clone());

        if arguments == "{" {
            assert!(state.run_head(&call));
            assert!(state.workflow_call.is_none());
            assert!(tool_results(&state)[0].content.contains("bad arguments:"));
        } else {
            assert!(!state.run_head(&call), "{arguments:?}");
            assert!(matches!(state.status, Status::AwaitingExtension));
            assert_eq!(
                state.workflow_call.as_ref().unwrap().owner,
                Some(yinz_tile::prelude::PluginId::new(90))
            );
            assert!(tool_results(&state).is_empty());
        }
    }
}

/// A plugin pane is not an agent, a suppressed one is not on screen, and a held
/// one still is - the three cases the snapshot has to tell apart.
#[test]
fn a_snapshot_keeps_the_terminal_panes_that_are_agents() {
    let plugin = PaneInfo {
        id: PaneId::from_legacy(1, true),
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
    let manifest = manifest(vec![plugin, suppressed, held]);

    let panes = terminal_panes(&manifest, &[tab_info(0, TabId::new(9), "work")]);
    assert_eq!(
        panes.iter().map(|pane| pane.pane_id).collect::<Vec<_>>(),
        vec![3],
    );
    assert_eq!(
        panes[0].status,
        agents::AgentStatus::Held,
        "held beats exited"
    );
    assert_eq!(panes[0].tab, Some((TabId::new(9), "work".to_owned())));
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

/// A closed pane leaves the snapshot, and with it every way to send to it.
#[test]
fn a_closed_agent_can_no_longer_be_sent_to() {
    let mut state = state_with_panes();
    assert!(state.is_agent(3));
    state.set_panes(Vec::new());
    assert!(!state.is_agent(3));
}

/// A latched agent stays visible while it runs a shell command.
#[test]
fn a_shelled_out_agent_stays_visible() {
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
        agents::HookFeeds::new(),
        None,
    );
    state.apply_poll(
        Vec::new(),
        agents::Activity::new(),
        agents::Summaries::new(),
        agents::HookFeeds::new(),
        None,
    );

    assert!(state.messages.is_empty());
}

#[test]
fn a_model_agent_action_is_confirmed_and_a_typed_one_is_not() {
    for (name, arguments) in [
        ("agent_prompt", r#"{"pane_id":3,"text":"go"}"#),
        ("agent_send_keys", r#"{"pane_id":3,"keys":["Esc"]}"#),
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
    state.run_manual(r#"/agent_send_keys pane_id=3 keys=["Esc"]"#);
    assert!(!matches!(state.status, Status::ConfirmingTool));
    assert!(matches!(state.status, Status::AwaitingExtension));
}

#[test]
fn project_images_apply_only_to_firecracker_spawns_and_explicit_images_win() {
    let mut local = launch::Spec {
        kind: Some("codex".to_owned()),
        ..Default::default()
    };
    crate::tool_loop::apply_project_firecracker_image(&mut local, Some("rust".to_owned()));
    assert_eq!(local.firecracker_image, None);

    let mut firecracker = launch::Spec {
        firecracker: Some("review".to_owned()),
        ..local.clone()
    };
    crate::tool_loop::apply_project_firecracker_image(&mut firecracker, Some("rust".to_owned()));
    assert_eq!(firecracker.firecracker_image.as_deref(), Some("rust"));

    firecracker.firecracker_image = Some("custom".to_owned());
    crate::tool_loop::apply_project_firecracker_image(&mut firecracker, Some("ignored".to_owned()));
    assert_eq!(firecracker.firecracker_image.as_deref(), Some("custom"));
}

/// The image is one field of a project and answers to the project tools like the
/// rest: a call that says nothing about it keeps what is stored, an empty one
/// clears it, and one with no root updates a project already known in place.
#[test]
fn a_projects_firecracker_image_is_kept_cleared_or_updated_by_the_project_tools() {
    let mut state = idle_state();
    let remember = |state: &mut State, arguments: &str| {
        state.remember_project(&tools::Args::parse(arguments).unwrap())
    };
    let stored_image = |state: &mut State| {
        let rows = state
            .store()
            .fetch(&FetchRequest::entity("project"))
            .expect("fetched");
        projects::from_rows(&rows)["hocus"]
            .firecracker_image
            .clone()
    };

    let project = remember(
        &mut state,
        r#"{"name":"hocus","root":"/work/hocus","firecracker_image":"rust"}"#,
    )
    .unwrap();
    assert_eq!(project.firecracker_image.as_deref(), Some("rust"));
    assert_eq!(stored_image(&mut state).as_deref(), Some("rust"));

    // Remembered again with its root and nothing said about the image.
    let project = remember(&mut state, r#"{"name":"hocus","root":"/work/hocus"}"#).unwrap();
    assert_eq!(project.firecracker_image.as_deref(), Some("rust"));
    assert_eq!(
        state.projects["hocus"].firecracker_image.as_deref(),
        Some("rust")
    );

    // The image alone, on a project already known.
    let project = remember(&mut state, r#"{"name":"hocus","firecracker_image":"ruby"}"#).unwrap();
    assert_eq!(project.firecracker_image.as_deref(), Some("ruby"));
    assert_eq!(stored_image(&mut state).as_deref(), Some("ruby"));

    // Empty clears it, with or without the root.
    let project = remember(&mut state, r#"{"name":"hocus","firecracker_image":""}"#).unwrap();
    assert_eq!(project.firecracker_image, None);
    assert_eq!(stored_image(&mut state), None);
    remember(
        &mut state,
        r#"{"name":"hocus","root":"/work/hocus","firecracker_image":"rust"}"#,
    )
    .unwrap();
    let project = remember(
        &mut state,
        r#"{"name":"hocus","root":"/work/hocus","firecracker_image":" "}"#,
    )
    .unwrap();
    assert_eq!(project.firecracker_image, None);
    assert_eq!(stored_image(&mut state), None);

    // Renaming a discovered project and clearing its image in one call: the
    // clear wins over the discovered image, and the answer is what is stored.
    let discovered = |state: &mut State, root: &str, image: &str| {
        let mut registry = state.projects.clone();
        projects::discover(
            &mut registry,
            &[projects::Repository {
                root: root.to_owned(),
                worktree_status: Some(projects::WorktreeStatus::Primary),
                ..projects::Repository::default()
            }],
        );
        let name = registry
            .values()
            .find(|project| project.root == root)
            .map(|project| project.name.clone())
            .expect("discovered");
        registry.get_mut(&name).unwrap().firecracker_image = Some(image.to_owned());
        give_projects(state, registry);
        name
    };
    let found = discovered(&mut state, "/work/found", "rust");
    let project = remember(
        &mut state,
        r#"{"name":"renamed","root":"/work/found","firecracker_image":""}"#,
    )
    .unwrap();
    assert_eq!(project.firecracker_image, None);
    assert_eq!(state.projects["renamed"].firecracker_image, None);
    assert!(!state.projects.contains_key(&found));
    // With nothing said about the image, the discovered one comes along and
    // is what the tool reports.
    let other = discovered(&mut state, "/work/other", "go");
    let project = remember(&mut state, r#"{"name":"named","root":"/work/other"}"#).unwrap();
    assert_eq!(project.firecracker_image.as_deref(), Some("go"));
    assert_eq!(
        state.projects["named"].firecracker_image.as_deref(),
        Some("go")
    );
    assert!(!state.projects.contains_key(&other));

    // Neither a bad name nor an image for a project nobody has named yet.
    let error = remember(
        &mut state,
        r#"{"name":"hocus","firecracker_image":"no spaces"}"#,
    )
    .expect_err("a space is not an image name");
    assert!(error.contains("invalid Firecracker VM name"), "{error}");
    assert!(remember(&mut state, r#"{"name":"other","firecracker_image":"rust"}"#).is_err());
}

/// The prompt and the model are the agent's own arguments, not hocus's: they reach
/// the pane as argv, in the order `launch` puts them in.
#[test]
fn a_prompt_and_a_model_reach_the_agent_as_its_own_arguments() {
    let args =
        tools::Args::parse(r#"{"agent":"claude","model":"opus","prompt":"fix it","hooks":false}"#)
            .unwrap();
    let (command, _) = launch::plan(&args.launch_spec()).unwrap();
    assert_eq!(command.path, std::path::PathBuf::from("env"));
    assert_eq!(
        command.args,
        [
            "YINZ_CLAUDE_INLINE_HOOKS=0",
            "claude",
            "--model",
            "opus",
            "fix it"
        ]
    );

    // Left to its default, a claude pane is also handed its hook wiring, which the model
    // may pass through untouched as `env`, `permission_mode` and `resume` too.
    let args = tools::Args::parse(
        r#"{"agent":"claude","permission_mode":"skip","resume":"last","env":{"A":"1"}}"#,
    )
    .unwrap();
    let (command, _) = launch::plan(&args.launch_spec()).unwrap();
    assert_eq!(command.path, std::path::PathBuf::from("env"));
    assert_eq!(
        &command.args[..5],
        [
            "A=1",
            "YINZ_CLAUDE_INLINE_HOOKS=1",
            "claude",
            "--dangerously-skip-permissions",
            "--continue"
        ]
    );
    assert_eq!(command.args[5], "--settings");
}

/// Chat reloads read daemon snapshots; opening chat must never write or claim latches.
#[test]
fn registration_metadata_is_owned_by_the_daemon() {
    let mut state = state_with_panes();
    state
        .store()
        .apply(vec![agents::rows::latch_mutation(
            &state.owner(),
            4,
            "reviewer",
        )])
        .unwrap();
    state.open_store();
    assert!(state.manual_kinds.is_empty());
    let mut message = worktree_snapshot(1, "main");
    let mut reply: PollReply = serde_json::from_str(message.payload.as_deref().unwrap()).unwrap();
    reply.panes = Some(state.panes.clone());
    reply.manual_kinds.insert(4, "reviewer".into());
    message.payload = Some(serde_json::to_string(&reply).unwrap());
    state.pipe(message);
    assert_eq!(
        state.manual_kinds.get(&4).map(String::as_str),
        Some("reviewer")
    );
    state.set_panes(vec![]);
    let rows = state
        .store()
        .fetch(&FetchRequest::entity(agents::rows::LATCH_ENTITY))
        .unwrap();
    assert_eq!(
        agents::rows::latches_from_rows(&rows)
            .get(&4)
            .map(String::as_str),
        Some("reviewer")
    );
}

#[test]
fn live_agent_tools_build_guarded_requests_and_reject_spawn_prompts() {
    use yinz_tile::agent::control::AgentRequest;
    let state = idle_state();
    let request = state.agent_request(&agent_call("agent_prompt", r#"{"pane_id":"terminal_7","text":"review","conversation":"thread","wait":true,"timeout_ms":1234}"#)).unwrap();
    assert!(
        matches!(request, AgentRequest::Prompt { expected_session_id: Some(id), prompt, wait: true, timeout_ms: 1234, .. } if id == "thread" && prompt == "review")
    );
    for field in ["prompt", "reuse_prompt"] {
        for value in [serde_json::Value::Null, "act now".into()] {
            let call = agent_call(
                "agent_spawn",
                &serde_json::json!({"agent":"codex",field:value}).to_string(),
            );
            assert!(state
                .agent_request(&call)
                .unwrap_err()
                .contains("spawn does not accept"));
        }
    }
    assert!(state
        .agent_request(&agent_call("agent_send", r#"{"pane_id":7}"#))
        .is_err());
    assert!(state
        .agent_request(&agent_call("agent_control", r#"{"pane_id":7}"#))
        .is_err());
}

#[test]
fn live_agent_reply_requires_the_pending_correlation_id() {
    let mut state = idle_state();
    let call = agent_call("agent_status", r#"{"pane_id":7}"#);
    state.pending.push_back(call.clone());
    assert!(!state.start_agent_tool(&call));
    assert!(matches!(state.status, Status::AwaitingExtension));
    assert!(!state.on_agent_response(r#"{"id":"unrelated","response":{"outcome":"ok"}}"#));
    assert!(state.workflow_call.is_some());
    let id = state.workflow_call.as_ref().unwrap().rpc_id.clone();
    assert!(state.on_agent_response(
        &serde_json::json!({"id":id,"response":{"outcome":"target_changed","acknowledged":false}})
            .to_string()
    ));
    assert!(state.workflow_call.is_none());
    assert!(tool_results(&state)
        .iter()
        .any(|result| result.content.contains("target_changed")));
}

#[test]
fn lost_agent_reply_is_an_uncertain_agent_outcome() {
    let call = agent_call(
        "agent_prompt",
        r#"{"pane_id":"terminal_7","text":"review"}"#,
    );
    let response: yinz_tile::agent::control::AgentResponse = serde_json::from_str(
        &workflows::uncertain_result(Some(&call), "request", "Response lost"),
    )
    .unwrap();
    assert_eq!(
        response.outcome,
        yinz_tile::agent::control::AgentOutcome::TimedOut
    );
    assert!(response.message.unwrap().contains("outcome unknown"));
}

#[test]
fn spawn_cwd_uses_focus_only_when_a_host_checkout_is_needed() {
    use yinz_tile::agent::control::AgentRequest;
    let mut state = idle_state();
    for (arguments, expected, consult_focus) in [
        (r#"{"agent":"codex"}"#, Some("/focused/checkout"), true),
        (
            r#"{"agent":"codex","branch":"topic","firecracker":true}"#,
            Some("/focused/checkout"),
            true,
        ),
        (
            r#"{"agent":"codex","cwd":"/explicit"}"#,
            Some("/explicit"),
            false,
        ),
        (r#"{"agent":"codex","project":"work"}"#, None, false),
        (r#"{"agent":"codex","remote_host":"server"}"#, None, false),
        (r#"{"agent":"codex","firecracker":true}"#, None, false),
    ] {
        let mut consulted = false;
        let request = state
            .agent_request_with_focus(&agent_call("agent_spawn", arguments), || {
                consulted = true;
                Some("/focused/checkout".into())
            })
            .unwrap();
        let AgentRequest::Spawn {
            cwd,
            automatic_firecracker_vm,
            spec,
            ..
        } = request
        else {
            panic!("spawn");
        };
        assert_eq!(cwd, expected.map(Into::into), "{arguments}");
        assert_eq!(consulted, consult_focus, "{arguments}");
        assert_eq!(automatic_firecracker_vm, spec.firecracker.is_some());
    }
    let call = agent_call("agent_spawn", r#"{"agent":"codex"}"#);
    // None preserves the host's plugin-startup-directory fallback on lookup failure.
    assert!(matches!(
        state.agent_request_with_focus(&call, || None).unwrap(),
        AgentRequest::Spawn { cwd: None, .. }
    ));
    state.active_project = Some("selected".into());
    assert!(
        matches!(state.agent_request_with_focus(&call, || panic!("project takes precedence")).unwrap(), AgentRequest::Spawn { cwd: None, project: Some(project), .. } if project == "selected")
    );
}

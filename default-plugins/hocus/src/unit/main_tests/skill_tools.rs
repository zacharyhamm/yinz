use super::*;

#[test]
fn a_successful_toggle_refreshes_the_tier_before_answering() {
    let mut state = project_state();
    settle(&mut state);
    state.skills_project_root = Some("/work/hocus".to_owned());
    load_skill(&mut state, skills::Source::User, "bench", "User.");
    load_skill(&mut state, skills::Source::Project, "bench", "Project.");
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "skill_disable".into(),
        arguments: r#"{"name":"bench","scope":"project"}"#.into(),
    }]
    .into();
    state.status = Status::AwaitingCommand;

    state.on_command_result(
        Some(0),
        b"\x01/work/hocus/.hocus/skills/bench/SKILL.md\x03\x02Project.\n",
        b"",
        &BTreeMap::from([
            ("tool_call_id".to_owned(), "mine".to_owned()),
            (skills::SOURCE_CONTEXT.to_owned(), "project".to_owned()),
            (
                skills::ROOT_CONTEXT.to_owned(),
                "/work/hocus/.hocus/skills".to_owned(),
            ),
        ]),
    );

    assert_eq!(state.skills.get("bench").unwrap().body, "User.");
    assert!(tool_results(&state)[0]
        .content
        .contains("disabled the project skill bench"));
}

#[test]
fn a_mutation_result_from_an_obsolete_project_root_is_ignored() {
    let mut state = project_state();
    settle(&mut state);
    state.skills_project_root = Some("/work/other".to_owned());
    load_skill(&mut state, skills::Source::Project, "current", "Current.");
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "skill_write".into(),
        arguments: r#"{"name":"old","scope":"project","source":"Old."}"#.into(),
    }]
    .into();
    state.status = Status::AwaitingCommand;

    state.on_command_result(
        Some(0),
        b"\x01/work/hocus/.hocus/skills/old/SKILL.md\x02Old.\n",
        b"",
        &BTreeMap::from([
            ("tool_call_id".to_owned(), "mine".to_owned()),
            (skills::SOURCE_CONTEXT.to_owned(), "project".to_owned()),
            (
                skills::ROOT_CONTEXT.to_owned(),
                "/work/hocus/.hocus/skills".to_owned(),
            ),
        ]),
    );

    assert!(state.skills.get("current").is_some());
    assert!(state.skills.get("old").is_none());
}

#[test]
fn slash_skills_toggle_runs_the_same_tool_as_hocus() {
    let mut state = project_state();
    settle(&mut state);

    assert!(state.run_manual("/skills disable project bench"));
    let call = state.pending.front().expect("the host command is pending");
    assert_eq!(call.name, "skill_disable");
    assert_eq!(
        tools::Args::parse(&call.arguments)
            .unwrap()
            .scope
            .as_deref(),
        Some("project")
    );
    assert!(state.manual_head, "typing the command is its confirmation");
}

#[test]
fn skill_list_and_read_are_available_while_planning_but_mutations_are_not() {
    for name in ["skill_list", "skill_read"] {
        assert!(tools::READ_ONLY.contains(&name), "{name}");
    }
    for name in ["skill_write", "skill_enable", "skill_disable"] {
        assert!(!tools::READ_ONLY.contains(&name), "{name}");
        assert!(tools::needs_confirmation(name), "{name}");
    }
}

#[test]
fn the_listing_names_disabled_and_shadowed_copies() {
    let mut state = State::default();
    load_skill(&mut state, skills::Source::Project, "bench", "Project.");
    load_skill(&mut state, skills::Source::User, "bench", "User.");
    let user = skills::parse_listing(
        skills::Source::User,
        "\x01/skills/bench/SKILL.md\x02---\ndescription: User.\n---\nBody.\n\x01/skills/off/SKILL.md\x03\x02---\ndescription: Off.\n---\nBody.\n",
    );
    state
        .skills
        .replace(skills::Source::User, user, &BTreeSet::new());

    state.run_manual("/skills");
    let listing = &state.messages.last().unwrap().content;
    assert!(listing.contains("(user, shadowed)"), "{listing}");
    assert!(listing.contains("(user, disabled)"), "{listing}");
}

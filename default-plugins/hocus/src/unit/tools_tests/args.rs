use super::*;

/// A tab is addressed one way only. A display position is not a `tab_id`, so
/// there is no second field for the model to reach for and no pair of
/// arguments that can disagree.
#[test]
fn a_tab_position_is_not_an_argument_any_tool_accepts() {
    let by_position = Args::parse(r#"{"tab_index":2,"tab_number":3}"#).unwrap();
    assert_eq!(by_position.tab_id, None);
    assert!(by_position.tab_target().is_err());
    assert!(by_position.tab_or_pane_target().is_none());
}

#[test]
fn an_absent_placement_lands_the_pane_in_a_tab_of_its_own() {
    assert_eq!(Args::default().placement(), Ok(WarpPlacement::NewTab));
}

#[test]
fn every_advertised_placement_parses() {
    for (name, expected) in [
        ("new_tab", WarpPlacement::NewTab),
        ("focused_tab", WarpPlacement::FocusedTab),
        ("floating", WarpPlacement::Floating),
    ] {
        let args = Args {
            placement: Some(name.to_owned()),
            ..Args::default()
        };
        assert_eq!(args.placement(), Ok(expected), "{name} should parse");
    }
}

#[test]
fn an_invented_placement_is_answered_with_the_ones_that_exist() {
    let args = Args {
        placement: Some("float".to_owned()),
        ..Args::default()
    };
    let error = args.placement().expect_err("float is not a placement");
    for legal in ["new_tab", "focused_tab", "floating"] {
        assert!(
            error.contains(legal),
            "the error should name {legal}, got: {error}"
        );
    }
}

/// `from` belongs to the Slack tools; a warp that quietly read it would take
/// the pane from a channel name.
#[test]
fn the_warp_arguments_do_not_collide_with_the_slack_ones() {
    let args = Args::parse(r##"{"from":"#general","from_session":"other"}"##).unwrap();
    assert_eq!(args.from, Some("#general".to_owned()));
    assert_eq!(args.from_session, Some("other".to_owned()));
}

#[test]
fn empty_arguments_parse_as_defaults() {
    assert!(Args::parse("").unwrap().pane_id.is_none());
    assert!(Args::parse("{}").unwrap().pane_id.is_none());
}

#[test]
fn unknown_argument_keys_are_ignored_rather_than_failing_the_call() {
    let args = Args::parse(r#"{"pane_id":3,"invented_by_the_model":true}"#).unwrap();
    assert_eq!(args.pane_id, Some(3));
}

#[test]
fn malformed_arguments_report_an_error_instead_of_panicking() {
    assert!(Args::parse("{not json").is_err());
}

#[test]
fn a_string_command_absorbs_the_args_array() {
    let args = Args::parse(r#"{"command":"git","args":["status","--short"]}"#).unwrap();
    assert_eq!(args.command_words(), ["git", "status", "--short"]);
}

#[test]
fn an_array_command_is_taken_whole() {
    let args = Args::parse(r#"{"command":["git","status"]}"#).unwrap();
    assert_eq!(args.command_words(), ["git", "status"]);
}

#[test]
fn a_missing_command_yields_no_words() {
    assert!(Args::parse(r#"{"floating":true}"#)
        .unwrap()
        .command_words()
        .is_empty());
}

/// `program_and_args` is the one reading the confirmation prompt and both pane
/// dispatchers share: a leading empty word is skipped, an empty command is no
/// command, and an empty word *after* the program is an argument.
#[test]
fn program_and_args_skips_only_a_leading_empty_word() {
    let program = |json: &str| Args::parse(json).unwrap().program_and_args();
    assert_eq!(
        program(r#"{"command":"","args":["htop"]}"#),
        Some(("htop".to_owned(), vec![]))
    );
    assert_eq!(
        program(r#"{"command":["","htop","-d","5"]}"#),
        Some(("htop".to_owned(), vec!["-d".to_owned(), "5".to_owned()]))
    );
    assert_eq!(
        program(r#"{"command":["printf",""]}"#),
        Some(("printf".to_owned(), vec![String::new()]))
    );
    assert_eq!(program(r#"{"command":""}"#), None);
    assert_eq!(program(r#"{"command":["",""]}"#), None);
    assert_eq!(program(r#"{"floating":true}"#), None);
}

#[test]
fn multi_pane_arguments_require_complete_pane_pairs() {
    let error = Args::parse(r#"{"panes":[{"pane_id":3}]}"#).unwrap_err();
    assert!(error.contains("is_plugin"), "{error}");
}

#[test]
fn moving_panes_requires_exactly_one_destination() {
    let neither = Args::parse(r#"{"panes":[{"pane_id":3,"is_plugin":false}]}"#).unwrap();
    assert!(neither.move_destination().is_err());

    let both =
        Args::parse(r#"{"panes":[{"pane_id":3,"is_plugin":false}],"tab_id":1,"new_tab":true}"#)
            .unwrap();
    assert!(both.move_destination().is_err());

    let new = Args::parse(
        r#"{"panes":[{"pane_id":3,"is_plugin":false}],"new_tab":true,"new_tab_name":"logs"}"#,
    )
    .unwrap();
    assert_eq!(
        new.move_destination().unwrap(),
        MoveDestination::New(Some("logs".into()))
    );

    let by_id = Args::parse(r#"{"panes":[{"pane_id":3,"is_plugin":false}],"tab_id":47}"#).unwrap();
    assert_eq!(
        by_id.move_destination().unwrap(),
        MoveDestination::ExistingId(47)
    );

    let id_and_new =
        Args::parse(r#"{"panes":[{"pane_id":3,"is_plugin":false}],"tab_id":47,"new_tab":true}"#)
            .unwrap();
    assert!(id_and_new.move_destination().is_err());
}

#[test]
fn a_tab_target_is_a_tab_id() {
    assert_eq!(
        Args::parse(r#"{"tab_id":47}"#)
            .unwrap()
            .tab_target()
            .unwrap(),
        47
    );
    assert!(Args::parse("{}").unwrap().tab_target().is_err());
}

#[test]
fn naming_no_tab_leaves_focus_and_rename_on_the_pane_path() {
    assert!(Args::parse(r#"{"pane_id":3,"is_plugin":false}"#)
        .unwrap()
        .tab_or_pane_target()
        .is_none());

    let both = Args::parse(r#"{"pane_id":3,"is_plugin":false,"tab_id":47}"#).unwrap();
    assert_eq!(
        both.tab_or_pane_target().unwrap().unwrap_err(),
        "give exactly one of pane pair or tab_id"
    );

    // A half-supplied pane pair still counts as naming a pane.
    let half = Args::parse(r#"{"is_plugin":false,"tab_id":47}"#).unwrap();
    assert!(half.tab_or_pane_target().unwrap().is_err());

    assert_eq!(
        Args::parse(r#"{"tab_id":47}"#)
            .unwrap()
            .tab_or_pane_target()
            .unwrap()
            .unwrap(),
        47
    );
}

#[test]
fn stacking_requires_two_panes() {
    let args = Args::parse(r#"{"panes":[{"pane_id":3,"is_plugin":false}]}"#).unwrap();
    assert_eq!(
        args.pane_ids(2).unwrap_err(),
        "at least 2 panes are required"
    );
}

#[test]
fn swapping_requires_exactly_two_panes() {
    let one = Args::parse(r#"{"panes":[{"pane_id":3,"is_plugin":false}]}"#).unwrap();
    assert_eq!(
        one.swap_pane_ids().unwrap_err(),
        "swap_panes needs exactly two panes"
    );

    let two = Args::parse(
        r#"{"panes":[{"pane_id":3,"is_plugin":false},{"pane_id":4,"is_plugin":true}]}"#,
    )
    .unwrap();
    assert_eq!(
        two.swap_pane_ids().unwrap(),
        [PaneId::Terminal(3), PaneId::Plugin(4)]
    );

    let three = Args::parse(
        r#"{"panes":[{"pane_id":3,"is_plugin":false},{"pane_id":4,"is_plugin":false},{"pane_id":5,"is_plugin":false}]}"#,
    )
    .unwrap();
    assert_eq!(
        three.swap_pane_ids().unwrap_err(),
        "swap_panes needs exactly two panes"
    );
}

#[test]
fn floating_geometry_accepts_cells_and_percentages() {
    let args = Args::parse(r#"{"x":"2","y":"10%","width":"50%","height":"12"}"#).unwrap();
    let geometry = args.floating_geometry().unwrap();
    assert!(geometry.x.is_some());
    assert!(geometry.y.is_some());
    assert!(geometry.width.is_some());
    assert!(geometry.height.is_some());
}

#[test]
fn floating_geometry_rejects_empty_invalid_and_zero_sizes() {
    assert!(Args::parse("{}").unwrap().floating_geometry().is_err());
    assert!(Args::parse(r#"{"x":"101%"}"#)
        .unwrap()
        .floating_geometry()
        .is_err());
    assert!(Args::parse(r#"{"width":"0"}"#)
        .unwrap()
        .floating_geometry()
        .is_err());
}

#[test]
fn rerun_rejects_plugin_panes() {
    let args = Args::parse(r#"{"pane_id":3,"is_plugin":true}"#).unwrap();
    assert_eq!(
        args.terminal_pane_id().unwrap_err(),
        "rerun_pane only supports terminal command panes"
    );
}

/// The one argument each todo tool cannot work without. `todo_list` needs
/// none: it falls back to the active project.
#[test]
fn the_todo_tools_require_their_key_argument() {
    let specs = specs();
    let required = |name: &str| -> Vec<String> {
        specs
            .as_array()
            .unwrap()
            .iter()
            .find(|spec| spec["function"]["name"] == name)
            .map(|spec| {
                spec["function"]["parameters"]["required"]
                    .as_array()
                    .map(|names| {
                        names
                            .iter()
                            .map(|name| name.as_str().unwrap().to_owned())
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .expect("the tool is advertised")
    };
    assert_eq!(required("todo_add"), vec!["text".to_owned()]);
    assert_eq!(required("todo_search"), vec!["query".to_owned()]);
    assert_eq!(required("todo_done"), vec!["id".to_owned()]);
    assert_eq!(
        required("todo_edit"),
        vec!["id".to_owned(), "text".to_owned()]
    );
    assert!(required("todo_list").is_empty());
}

#[test]
fn args_parse_reads_the_todo_fields() {
    let args = Args::parse(
        r#"{"id":7,"parent_id":3,"include_done":true,"all_projects":true,"undo":true}"#,
    )
    .unwrap();

    assert_eq!(args.id, Some(7));
    assert_eq!(args.parent_id, Some(3));
    assert!(args.include_done);
    assert!(args.all_projects);
    assert!(args.undo);
}

/// The one argument each note tool cannot work without. `note_list` needs
/// none: it falls back to the active project.
#[test]
fn the_note_tools_require_their_key_argument() {
    let specs = specs();
    let required = |name: &str| -> Vec<String> {
        specs
            .as_array()
            .unwrap()
            .iter()
            .find(|spec| spec["function"]["name"] == name)
            .map(|spec| {
                spec["function"]["parameters"]["required"]
                    .as_array()
                    .map(|names| {
                        names
                            .iter()
                            .map(|name| name.as_str().unwrap().to_owned())
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .expect("the tool is advertised")
    };
    assert_eq!(
        required("note_add"),
        vec!["title".to_owned(), "body".to_owned()]
    );
    for name in ["note_get", "note_edit", "note_archive", "note_delete"] {
        assert_eq!(required(name), vec!["id".to_owned()], "{name}");
    }
    assert_eq!(
        required("note_link"),
        vec!["note_id".to_owned(), "todo_id".to_owned()]
    );
    assert_eq!(required("note_search"), vec!["query".to_owned()]);
    assert!(required("note_list").is_empty());
}

#[test]
fn args_parse_reads_the_note_fields() {
    let args = Args::parse(
        r#"{"title":"t","body":"b","global":true,"include_archived":true,"tag":"x",
            "tags":["a","b"],"todo_ids":[1,2],"todo_id":3,"note_id":4,"unlink":true}"#,
    )
    .unwrap();
    assert_eq!(args.title.as_deref(), Some("t"));
    assert_eq!(args.body.as_deref(), Some("b"));
    assert!(args.global);
    assert!(args.include_archived);
    assert_eq!(args.tag.as_deref(), Some("x"));
    assert_eq!(args.tags, Some(vec!["a".to_owned(), "b".to_owned()]));
    assert_eq!(args.todo_ids, vec![1, 2]);
    assert_eq!(args.todo_id, Some(3));
    assert_eq!(args.note_id, Some(4));
    assert!(args.unlink);
    // Absent tags are not empty tags: the former leaves a note alone.
    assert_eq!(Args::parse("{}").unwrap().tags, None);
}

/// The shared parser is also `set_floating_pane_geometry`'s, whose messages several
/// callers already depend on.
#[test]
fn the_floating_geometry_messages_survive_the_shared_parser() {
    let geometry = |arguments: &str| Args::parse(arguments).unwrap().floating_geometry();
    assert_eq!(
        geometry(r#"{"width":"0"}"#).unwrap_err(),
        "width must be greater than zero"
    );
    assert_eq!(
        geometry(r#"{"x":"200%"}"#).unwrap_err(),
        "x percentage must be from 0% to 100%"
    );
    // x and y may be zero; width and height may not.
    assert!(geometry(r#"{"x":"0","y":"0"}"#).is_ok());
}

/// `agent_spawn`'s arguments as a `launch::Spec` — the struct the New agent modal
/// fills in from its form. This mapping is the whole of what makes the two entry
/// points one feature, so it is checked field by field.
#[test]
fn agent_spawn_arguments_become_the_spec_the_new_agent_modal_builds() {
    let args = Args::parse(
        r#"{"agent":"cursor","model":"opus","prompt":"fix it","branch":"topic",
            "base":"develop","worktree":false,"worktree_path":"/elsewhere"}"#,
    )
    .unwrap();
    assert_eq!(
        args.launch_spec(),
        launch::Spec {
            kind: Some("cursor".to_owned()),
            command: None,
            args: Vec::new(),
            model: Some("opus".to_owned()),
            prompt: Some("fix it".to_owned()),
            branch: Some("topic".to_owned()),
            base: Some("develop".to_owned()),
            worktree: Some(false),
            worktree_path: Some("/elsewhere".to_owned()),
            ..Default::default()
        }
    );
}

/// `command_words` folds `args` in behind the program, which is what `new_pane`
/// wants and not what `launch` does. Getting this backwards would run the program
/// with its own name as the first argument.
#[test]
fn a_command_and_its_arguments_come_apart_again_for_launch() {
    let spec = Args::parse(r#"{"command":"zsh","args":["-l","-c","echo hi"]}"#)
        .unwrap()
        .launch_spec();
    assert_eq!(spec.command.as_deref(), Some("zsh"));
    assert_eq!(spec.args, ["-l", "-c", "echo hi"]);
    assert!(spec.kind.is_none());
}

/// An `agent_spawn` call from before any of this still means what it meant.
#[test]
fn a_bare_command_spec_is_unchanged_by_the_new_arguments() {
    let spec = Args::parse(r#"{"command":"claude"}"#)
        .unwrap()
        .launch_spec();
    assert_eq!(
        spec,
        launch::Spec {
            command: Some("claude".to_owned()),
            ..Default::default()
        }
    );
}

/// `command_words` answers with nothing when there is no `command`, so an `agent`
/// call has to take its `args` from the field directly or they vanish on the way
/// to the pane.
#[test]
fn an_agents_own_arguments_survive_having_no_command_to_hang_off() {
    let spec = Args::parse(
        r#"{"agent":"claude","args":["--dangerously-skip-permissions"],"hooks":false}"#,
    )
    .unwrap()
    .launch_spec();
    assert_eq!(spec.args, ["--dangerously-skip-permissions"]);
    let (command, _) = launch::plan(&spec).unwrap();
    assert_eq!(command.args, ["--dangerously-skip-permissions"]);
}

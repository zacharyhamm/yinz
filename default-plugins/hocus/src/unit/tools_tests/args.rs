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

/// `to` belongs to the Slack tools; a warp that quietly read it would send
/// the pane to a channel name.
#[test]
fn the_warp_arguments_do_not_collide_with_the_slack_ones() {
    let args = Args::parse(r##"{"to":"#general","to_session":"other"}"##).unwrap();
    assert_eq!(args.to, Some("#general".to_owned()));
    assert_eq!(args.to_session, Some("other".to_owned()));
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

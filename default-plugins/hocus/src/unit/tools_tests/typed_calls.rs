use super::*;

#[test]
fn summarize_omits_an_empty_argument_object() {
    let call = ToolCall {
        id: "1".into(),
        name: "list_session".into(),
        arguments: "{}".into(),
    };
    assert_eq!(summarize(&call), "list_session");
}

#[test]
fn summarize_shows_the_arguments_verbatim() {
    let call = ToolCall {
        id: "1".into(),
        name: "close_pane".into(),
        arguments: r#"{"pane_id":3,"is_plugin":false}"#.into(),
    };
    assert_eq!(
        summarize(&call),
        r#"close_pane {"pane_id":3,"is_plugin":false}"#
    );
}

#[test]
fn a_bare_command_takes_no_arguments() {
    assert_eq!(
        parse_command("/list_session"),
        Ok(("list_session".to_owned(), "{}".to_owned()))
    );
}

/// The point of the pairs: values must come out typed, under the names `Args`
/// actually deserializes.
#[test]
fn pairs_are_typed_and_land_on_the_right_args_fields() {
    let (name, arguments) = parse_command("/read_pane pane_id=3 is_plugin=false").unwrap();
    assert_eq!(name, "read_pane");
    let args = Args::parse(&arguments).unwrap();
    assert_eq!(args.pane_id, Some(3));
    assert_eq!(args.is_plugin, Some(false));

    let (_, arguments) = parse_command("/new_pane name=logs floating=true").unwrap();
    let args = Args::parse(&arguments).unwrap();
    assert_eq!(args.name.as_deref(), Some("logs"));
    assert!(args.floating);

    let (name, arguments) = parse_command("/new_remote_pane remote_host=box command=htop").unwrap();
    assert_eq!(name, "new_remote_pane");
    let args = Args::parse(&arguments).unwrap();
    assert_eq!(args.remote_host.as_deref(), Some("box"));
    assert_eq!(args.command, Some(Value::String("htop".to_owned())));

    // A bare host is a whole call: the command is what may be left out.
    let (_, arguments) = parse_command("/new_remote_pane remote_host=box").unwrap();
    let args = Args::parse(&arguments).unwrap();
    assert_eq!(args.remote_host.as_deref(), Some("box"));
    assert_eq!(args.command, None);

    let json = r#"/stack_panes panes=[{"pane_id":1,"is_plugin":false}]"#;
    let (_, arguments) = parse_command(json).unwrap();
    assert_eq!(Args::parse(&arguments).unwrap().panes.len(), 1);
}

#[test]
fn a_json_rest_is_passed_through_verbatim() {
    assert_eq!(
        parse_command(r#"/rename {"name": "my tab"}"#),
        Ok(("rename".to_owned(), r#"{"name": "my tab"}"#.to_owned()))
    );
}

#[test]
fn a_token_without_an_equals_is_rejected() {
    assert!(parse_command("/read_pane 3").is_err());
    assert!(parse_command("/").is_err());
}

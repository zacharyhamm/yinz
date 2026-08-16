use super::*;

#[test]
fn slack_argv_maps_each_tool_onto_its_dobbs_command() {
    assert_eq!(argv("slack_status", "{}").unwrap(), ["status"]);
    assert_eq!(
        argv("slack_search", r#"{"query": "deploy NOT staging"}"#).unwrap(),
        ["search", "deploy NOT staging"]
    );
    // A channel name is passed through as written; dobbs resolves it.
    assert_eq!(
        argv(
            "slack_read",
            r##"{"channel": "#general", "from": "2026-08-01"}"##
        )
        .unwrap(),
        [
            "export-channel",
            "-channel",
            "#general",
            "-from",
            "2026-08-01"
        ]
    );
    assert_eq!(
        argv("slack_backfill", r#"{"channel": "C1", "to": "2026-08-08"}"#).unwrap(),
        ["backfill-channel", "-channel", "C1", "-to", "2026-08-08"]
    );
    assert_eq!(
        argv(
            "slack_post",
            r#"{"channel": "C1", "text": "hi", "thread": "1.2"}"#
        )
        .unwrap(),
        ["post", "-channel", "C1", "-text", "hi", "-thread", "1.2"]
    );
}

/// An absent optional flag must be left out entirely rather than passed
/// empty: dobbs reads `-from ""` as open, but `-channel ""` as an error.
#[test]
fn slack_argv_omits_absent_and_blank_options() {
    assert_eq!(
        argv(
            "slack_read",
            r#"{"channel": "eng", "workspace": "  ", "to": ""}"#
        )
        .unwrap(),
        ["export-channel", "-channel", "eng"]
    );
}

#[test]
fn slack_argv_rejects_missing_required_arguments() {
    for (name, arguments, field) in [
        ("slack_search", "{}", "query"),
        ("slack_read", r#"{"from": "2026-08-01"}"#, "channel"),
        ("slack_backfill", "{}", "channel"),
        ("slack_post", r#"{"text": "hi"}"#, "channel"),
        ("slack_post", r#"{"channel": "C1", "text": "  "}"#, "text"),
    ] {
        let error = argv(name, arguments).unwrap_err();
        assert!(error.starts_with(field), "{name}: {error}");
    }
    assert!(argv("slack_nope", "{}")
        .unwrap_err()
        .contains("no such tool"));
}

/// The four Slack tools are intercepted in `run_head` by name, so the list
/// and the advertised schemas have to stay in step.
#[test]
fn every_slack_tool_is_advertised() {
    let specs = specs();
    let names: Vec<&str> = specs
        .as_array()
        .unwrap()
        .iter()
        .map(|spec| spec["function"]["name"].as_str().unwrap())
        .collect();
    for name in SLACK_TOOLS {
        assert!(names.contains(&name), "{name} is not advertised");
    }
}

#[test]
fn schedule_advertises_optional_automatic_confirmation() {
    let specs = specs();
    let schedule = specs
        .as_array()
        .unwrap()
        .iter()
        .find(|spec| spec["function"]["name"] == "schedule")
        .unwrap();
    let field = &schedule["function"]["parameters"]["properties"]["auto_confirm"];

    assert_eq!(field["type"], "boolean");
    assert!(!schedule["function"]["parameters"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|name| name == "auto_confirm"));
}

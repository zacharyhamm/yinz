use super::*;

#[test]
fn swamp_arguments_are_fixed_and_searches_default_to_all_statuses() {
    assert_eq!(
        swamp_argv("swamp_issue_search", "{}").unwrap(),
        ["swamp", "issue", "search", "--json", "--limit", "100"]
    );
    assert_eq!(
        swamp_argv(
            "swamp_issue_search",
            r#"{"query":"vault $(id)","type":"bug","status":"closed","source":"swamp","limit":1}"#
        )
        .unwrap(),
        [
            "swamp",
            "issue",
            "search",
            "--json",
            "--limit",
            "1",
            "--type",
            "bug",
            "--status",
            "closed",
            "--source",
            "swamp",
            "vault $(id)"
        ]
    );
    assert_eq!(
        swamp_argv("swamp_issue_get", r#"{"number":42}"#).unwrap(),
        ["swamp", "issue", "get", "42", "--json"]
    );
    for status in ["open", "triaged", "in_progress", "shipped", "closed"] {
        assert!(swamp_argv("swamp_issue_search", &json!({"status":status}).to_string()).is_ok());
    }
    for kind in ["bug", "feature", "security"] {
        assert!(swamp_argv("swamp_issue_search", &json!({"type":kind}).to_string()).is_ok());
    }
}

#[test]
fn swamp_rejects_invalid_numbers_limits_and_option_injection() {
    for arguments in [
        "{}",
        r#"{"number":0}"#,
        r#"{"number":-1}"#,
        r#"{"number":1.5}"#,
        r#"{"number":"--help"}"#,
    ] {
        assert!(
            swamp_argv("swamp_issue_get", arguments).is_err(),
            "{arguments}"
        );
    }
    for arguments in [
        r#"{"limit":0}"#,
        r#"{"limit":101}"#,
        r#"{"limit":-1}"#,
        r#"{"limit":1.5}"#,
        r#"{"type":"--help"}"#,
        r#"{"status":"all"}"#,
        r#"{"query":" --help"}"#,
        r#"{"source":"--log-level"}"#,
        r#"{"query":"a\u0000b"}"#,
        r#"{"query":42}"#,
    ] {
        assert!(
            swamp_argv("swamp_issue_search", arguments).is_err(),
            "{arguments}"
        );
    }
}

#[test]
fn swamp_tools_are_deferred_and_available_in_planning() {
    let loaded = BTreeSet::from(["swamp"]);
    let before = read_only_specs_for(&BTreeSet::new()).to_string();
    let after = read_only_specs_for(&loaded).to_string();
    for name in SWAMP_TOOLS {
        assert!(READ_ONLY.contains(&name));
        assert_eq!(family_of(name).unwrap().name, "swamp");
        assert!(family_of(name).unwrap().deferred);
        // The loader description mentions the family, not individual tools.
        assert!(!before.contains(name));
        assert!(after.contains(name));
        assert!(load_family(Some("swamp"), true).contains(name));
    }
}

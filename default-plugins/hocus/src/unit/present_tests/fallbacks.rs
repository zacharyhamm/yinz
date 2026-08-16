use super::*;

#[test]
fn malformed_json_falls_back_rather_than_erroring() {
    assert_eq!(present(&call("list_session", "{}"), "{not json"), None);
}

/// A bare scalar is valid JSON but not a structure worth rendering.
#[test]
fn a_scalar_result_is_not_treated_as_structure() {
    assert_eq!(present(&call("whatever", "{}"), "42"), None);
    assert_eq!(present(&call("whatever", "{}"), "\"hello\""), None);
}

// -- the generic renderer ----------------------------------------------

#[test]
fn an_unknown_tool_still_gets_a_readable_list() {
    let result = present(
        &call("some_future_tool", "{}"),
        r#"{"opened":true,"pane_id":7}"#,
    );
    assert_eq!(
        result,
        Some("- **opened**: true\n- **pane_id**: 7\n".to_owned())
    );
}

#[test]
fn nested_objects_become_nested_lists() {
    let raw = r#"{"tabs":[{"name":"one","panes":[]}]}"#;
    assert_eq!(
        present(&call("some_future_tool", "{}"), raw),
        Some(
            "- **tabs**:\n  -\n    - **name**: one\n    - **panes**:\n      - _none_\n".to_owned()
        )
    );
}

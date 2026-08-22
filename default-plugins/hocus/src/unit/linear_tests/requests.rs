use super::*;

// -- headers ------------------------------------------------------------

/// The single easiest thing to get wrong about Linear. Every other endpoint
/// hocus talks to takes `Bearer`; a personal API key sent that way is a 401
/// whose body does not explain itself.
#[test]
fn the_api_key_is_sent_raw_with_no_bearer_prefix() {
    let headers = headers("lin_api_secret");
    assert_eq!(headers.get("Authorization").unwrap(), "lin_api_secret");
    assert!(!headers["Authorization"].contains("Bearer"));
}

#[test]
fn headers_ask_for_json_in_both_directions() {
    let headers = headers("k");
    assert_eq!(headers.get("Content-Type").unwrap(), "application/json");
    assert_eq!(headers.get("Accept").unwrap(), "application/json");
}

// -- identifiers --------------------------------------------------------

#[test]
fn parse_identifier_takes_the_key_and_the_number() {
    assert_eq!(parse_identifier("ENG-123"), Some(("ENG".to_owned(), 123)));
    assert_eq!(
        parse_identifier("  ENG-123  "),
        Some(("ENG".to_owned(), 123))
    );
    // The model will lowercase it sooner or later; Linear's keys are uppercase.
    assert_eq!(parse_identifier("eng-123"), Some(("ENG".to_owned(), 123)));
    assert_eq!(parse_identifier("ENG-0"), Some(("ENG".to_owned(), 0)));
    assert_eq!(parse_identifier("A1-7"), Some(("A1".to_owned(), 7)));
}

#[test]
fn parse_identifier_rejects_what_is_not_one() {
    for raw in [
        "ENG",
        "123",
        "ENG-12A",
        "ENG-",
        "-123",
        "",
        // Team keys never contain a hyphen, so a leftover one means this was
        // never an identifier — splitting on the last hyphen alone would have
        // read this as key "A-B".
        "A-B-1",
        "8bf0b3a4-1c2d-4e5f-8a9b-0c1d2e3f4a5b",
    ] {
        assert_eq!(parse_identifier(raw), None, "{raw:?}");
    }
}

#[test]
fn is_uuid_recognises_the_eight_four_four_four_twelve_form() {
    assert!(is_uuid("8bf0b3a4-1c2d-4e5f-8a9b-0c1d2e3f4a5b"));
    assert!(!is_uuid("ENG-123"));
    assert!(!is_uuid("8bf0b3a4-1c2d-4e5f-8a9b"));
    assert!(!is_uuid("zzzzzzzz-1c2d-4e5f-8a9b-0c1d2e3f4a5b"));
}

// -- limits -------------------------------------------------------------

#[test]
fn clamp_limit_bounds_the_ask_at_both_ends() {
    assert_eq!(clamp_limit(None), DEFAULT_LIMIT);
    // Zero would be a call that cannot answer anything.
    assert_eq!(clamp_limit(Some(0)), 1);
    assert_eq!(clamp_limit(Some(7)), 7);
    assert_eq!(clamp_limit(Some(500)), 50);
}

// -- request bodies -----------------------------------------------------

#[test]
fn every_request_is_a_document_plus_variables() {
    let body = request_for("linear_search", &args(r#"{"query":"login"}"#)).unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert!(value["query"].is_string());
    assert!(value["variables"].is_object());
}

/// The whole of the escaping story: nothing the model typed is ever spliced
/// into the GraphQL document, so there is nothing to escape.
#[test]
fn user_values_never_reach_the_document() {
    let nasty = r#"") { id } } # "#;
    let body = request_for(
        "linear_search",
        &args(&serde_json::json!({ "query": nasty }).to_string()),
    )
    .unwrap();
    assert!(!document(&body).contains("# "));
    // Trimmed on the way in, like every other free-text argument.
    assert_eq!(
        variables(&body)["term"],
        Value::String(nasty.trim().to_owned())
    );
}

#[test]
fn search_requires_a_query() {
    assert!(request_for("linear_search", &args("{}")).is_err());
    assert!(request_for("linear_search", &args(r#"{"query":"   "}"#)).is_err());
}

#[test]
fn search_passes_the_term_and_the_clamped_limit() {
    let body = request_for("linear_search", &args(r#"{"query":"login","limit":900}"#)).unwrap();
    assert_eq!(variables(&body)["term"], "login");
    assert_eq!(variables(&body)["first"], 50);
}

/// "me" is asked as the viewer rather than as an assignee filter, because
/// `viewer.assignedIssues` cannot be wrong about who "me" is.
#[test]
fn my_issues_go_through_the_viewer() {
    for arguments in ["{}", r#"{"assignee":"me"}"#, r#"{"assignee":"ME"}"#] {
        let body = request_for("linear_issues", &args(arguments)).unwrap();
        assert!(document(&body).contains("viewer"), "{arguments}");
        assert!(
            variables(&body)["filter"].get("assignee").is_none(),
            "{arguments}"
        );
    }
}

#[test]
fn a_named_assignee_goes_through_the_plain_issues_query() {
    let body = request_for("linear_issues", &args(r#"{"assignee":"Ada"}"#)).unwrap();
    assert!(!document(&body).contains("viewer"));
    assert_eq!(
        variables(&body)["filter"]["assignee"],
        serde_json::json!({ "displayName": { "eqIgnoreCase": "Ada" } })
    );
}

#[test]
fn an_identifier_is_looked_up_by_team_key_and_number() {
    let body = request_for("linear_issue", &args(r#"{"identifier":"ENG-123"}"#)).unwrap();
    assert!(document(&body).contains("IssueByKey"));
    assert_eq!(variables(&body)["key"], "ENG");
    assert_eq!(variables(&body)["number"], 123);
}

#[test]
fn a_uuid_is_looked_up_by_id() {
    let uuid = "8bf0b3a4-1c2d-4e5f-8a9b-0c1d2e3f4a5b";
    let body = request_for(
        "linear_issue",
        &args(&serde_json::json!({ "identifier": uuid }).to_string()),
    )
    .unwrap();
    assert!(document(&body).contains("IssueById"));
    assert_eq!(variables(&body)["id"], uuid);
}

#[test]
fn an_issue_needs_an_identifier_it_can_recognise() {
    assert!(request_for("linear_issue", &args("{}")).is_err());
    let error = request_for("linear_issue", &args(r#"{"identifier":"nonsense"}"#)).unwrap_err();
    // The sentence is the model's only feedback loop, so it has to say what the
    // accepted form is.
    assert!(error.contains("ENG-123"), "{error}");
}

/// Absent means yes: a bare `linear_issue` should read the whole thing.
#[test]
fn comments_are_included_unless_they_are_refused() {
    let with = request_for("linear_issue", &args(r#"{"identifier":"ENG-1"}"#)).unwrap();
    assert!(document(&with).contains("comments("));
    let without = request_for(
        "linear_issue",
        &args(r#"{"identifier":"ENG-1","comments":false}"#),
    )
    .unwrap();
    assert!(!document(&without).contains("comments("));
}

#[test]
fn no_such_tool_is_an_error_rather_than_a_request() {
    assert!(request_for("linear_nope", &args("{}")).is_err());
}

// -- filters ------------------------------------------------------------
//
// The highest-value tests here: a filter mistake shows up as a silently empty
// list, not as an error.

#[test]
fn the_default_listing_is_open_issues() {
    let body = request_for("linear_issues", &args("{}")).unwrap();
    assert_eq!(
        variables(&body)["filter"]["state"],
        serde_json::json!({ "type": { "nin": ["completed", "canceled"] } })
    );
}

#[test]
fn a_named_state_is_matched_exactly() {
    let body = request_for("linear_issues", &args(r#"{"state":"started"}"#)).unwrap();
    assert_eq!(
        variables(&body)["filter"]["state"],
        serde_json::json!({ "type": { "eq": "started" } })
    );
}

#[test]
fn an_unknown_state_is_refused_with_the_list_of_real_ones() {
    let error = request_for("linear_issues", &args(r#"{"state":"In Progress"}"#)).unwrap_err();
    assert!(error.contains("started"), "{error}");
}

#[test]
fn team_project_and_label_each_map_to_their_own_member() {
    let body = request_for(
        "linear_issues",
        &args(r#"{"team":"ENG","project":"Platform","label":"bug"}"#),
    )
    .unwrap();
    let filter = &variables(&body)["filter"];
    assert_eq!(
        filter["team"],
        serde_json::json!({ "key": { "eq": "ENG" } })
    );
    assert_eq!(
        filter["project"],
        serde_json::json!({ "name": { "containsIgnoreCase": "Platform" } })
    );
    assert_eq!(
        filter["labels"],
        serde_json::json!({ "name": { "eqIgnoreCase": "bug" } })
    );
}

#[test]
fn updated_since_must_look_like_a_date() {
    let body = request_for("linear_issues", &args(r#"{"updated_since":"2026-08-01"}"#)).unwrap();
    assert_eq!(
        variables(&body)["filter"]["updatedAt"],
        serde_json::json!({ "gte": "2026-08-01" })
    );
    let error =
        request_for("linear_issues", &args(r#"{"updated_since":"last week"}"#)).unwrap_err();
    assert!(error.contains("2026-08-01"), "{error}");
}

/// Absent members are omitted rather than nulled: Linear reads a null member as
/// a filter on absence, not as "no filter".
#[test]
fn absent_filters_are_left_out_entirely() {
    let body = request_for("linear_issues", &args("{}")).unwrap();
    let filter = variables(&body)["filter"].clone();
    let object = filter.as_object().unwrap();
    assert!(!object.contains_key("team"));
    assert!(!object.contains_key("project"));
    assert!(!object.contains_key("labels"));
    assert!(!object.contains_key("updatedAt"));
}

use super::*;

const ENV: &str = "LINEAR_API_KEY";

fn body(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).unwrap()
}

fn row(identifier: &str, title: &str) -> Value {
    serde_json::json!({
        "identifier": identifier,
        "title": title,
        "priority": 2,
        "updatedAt": "2026-08-01T09:30:00.000Z",
        "state": { "name": "In Progress", "type": "started" },
        "assignee": { "displayName": "Ada" },
        "team": { "key": "ENG" },
    })
}

fn listing(nodes: Vec<Value>) -> Vec<u8> {
    body(serde_json::json!({ "data": { "issues": { "nodes": nodes } } }))
}

// -- rows ---------------------------------------------------------------

/// Asserted whole, so the token cost of a row is visible here rather than only
/// on the bill. In particular there is no URL: at ~60 characters each they are
/// most of a page across a listing, re-sent on every later turn.
#[test]
fn a_row_is_one_line_with_no_url() {
    let out = format(
        "linear_search",
        200,
        &listing(vec![row("ENG-123", "Fix the login bug")]),
        ENV,
    );
    assert_eq!(
        out,
        "ENG-123 [ENG] In Progress High Ada 2026-08-01 — Fix the login bug"
    );
}

#[test]
fn rows_are_one_per_line() {
    let out = format(
        "linear_issues",
        200,
        &listing(vec![row("ENG-1", "One"), row("ENG-2", "Two")]),
        ENV,
    );
    assert_eq!(out.lines().count(), 2);
}

/// `assignee`, `project` and `parent` are all nullable in Linear, and a listing
/// full of unassigned issues is entirely ordinary.
#[test]
fn a_row_survives_every_nullable_field_being_null() {
    let bare = serde_json::json!({
        "identifier": "ENG-9",
        "title": "Unassigned",
        "priority": null,
        "updatedAt": null,
        "state": null,
        "assignee": null,
        "team": null,
    });
    let out = format("linear_issues", 200, &listing(vec![bare]), ENV);
    assert_eq!(out, "ENG-9 — Unassigned");
}

/// Linear's priority is 0-4 and 0 means *none*, so printing the bare number
/// would read as the most urgent thing on the list.
#[test]
fn priority_zero_is_no_priority_rather_than_the_highest() {
    let mut node = row("ENG-1", "Thing");
    node["priority"] = serde_json::json!(0);
    let out = format("linear_issues", 200, &listing(vec![node]), ENV);
    assert!(!out.contains("Urgent"), "{out}");
    assert!(!out.contains(" 0 "), "{out}");
}

#[test]
fn priorities_are_named() {
    for (value, label) in [(1, "Urgent"), (2, "High"), (3, "Medium"), (4, "Low")] {
        let mut node = row("ENG-1", "Thing");
        node["priority"] = serde_json::json!(value);
        let out = format("linear_issues", 200, &listing(vec![node]), ENV);
        assert!(out.contains(label), "{value} -> {out}");
    }
}

/// An empty string reads to the model as a broken tool, and it retries the same
/// call.
#[test]
fn an_empty_listing_says_so() {
    assert_eq!(
        format("linear_issues", 200, &listing(vec![]), ENV),
        "no issues match"
    );
}

/// The nodes are found wherever the document put them, which is what lets the
/// search document be swapped for the ranked form without touching this.
#[test]
fn nodes_are_found_under_any_of_the_query_shapes() {
    for pointer in ["searchIssues", "issueSearch"] {
        let value = serde_json::json!({
            "data": { pointer: { "nodes": [row("ENG-1", "Found")] } }
        });
        let out = format("linear_search", 200, &body(value), ENV);
        assert!(out.contains("ENG-1"), "{pointer}: {out}");
    }
    let viewer = serde_json::json!({
        "data": { "viewer": { "assignedIssues": { "nodes": [row("ENG-2", "Mine")] } } }
    });
    assert!(format("linear_issues", 200, &body(viewer), ENV).contains("ENG-2"));
}

// -- detail -------------------------------------------------------------

fn detail() -> Value {
    serde_json::json!({
        "data": { "issue": {
            "identifier": "ENG-123",
            "title": "Fix the login bug",
            "url": "https://linear.app/acme/issue/ENG-123",
            "priority": 1,
            "estimate": 3,
            "createdAt": "2026-07-01T00:00:00.000Z",
            "updatedAt": "2026-08-01T00:00:00.000Z",
            "state": { "name": "In Progress", "type": "started" },
            "assignee": { "displayName": "Ada" },
            "creator": { "displayName": "Grace" },
            "team": { "key": "ENG", "name": "Engineering" },
            "project": { "name": "Platform" },
            "parent": { "identifier": "ENG-100", "title": "Auth" },
            "labels": { "nodes": [{ "name": "bug" }, { "name": "p1" }] },
            "description": "It logs you out.",
            "comments": { "nodes": [
                { "createdAt": "2026-08-01T00:00:00.000Z", "body": "Looking.", "user": { "displayName": "Ada" } }
            ] },
        } }
    })
}

#[test]
fn detail_carries_the_fields_a_listing_leaves_out() {
    let out = format("linear_issue", 200, &body(detail()), ENV);
    for expected in [
        "ENG-123 Fix the login bug",
        "state: In Progress",
        "team: ENG (Engineering)",
        "assignee: Ada",
        "creator: Grace",
        "priority: Urgent",
        "project: Platform",
        "parent: ENG-100 Auth",
        "labels: bug, p1",
        // Unlike a listing, the detail view is one issue and can afford it.
        "url: https://linear.app/acme/issue/ENG-123",
        "It logs you out.",
        "comments:",
        "Ada 2026-08-01",
        "Looking.",
    ] {
        assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
    }
}

#[test]
fn detail_reads_the_by_key_lookups_single_node() {
    let value = serde_json::json!({
        "data": { "issues": { "nodes": [{ "identifier": "ENG-7", "title": "Via filter" }] } }
    });
    let out = format("linear_issue", 200, &body(value), ENV);
    assert!(out.starts_with("ENG-7 Via filter"), "{out}");
}

#[test]
fn a_missing_issue_says_so() {
    let value = serde_json::json!({ "data": { "issue": null } });
    assert_eq!(
        format("linear_issue", 200, &body(value), ENV),
        "no such issue"
    );
    let empty = serde_json::json!({ "data": { "issues": { "nodes": [] } } });
    assert_eq!(
        format("linear_issue", 200, &body(empty), ENV),
        "no such issue"
    );
}

// -- caps ---------------------------------------------------------------
//
// `finish_head` does not truncate — `MAX_TOOL_OUTPUT` applies only to the
// command path — so these caps are the only thing bounding a tool result.

#[test]
fn a_long_description_is_cut_and_says_so() {
    let mut value = detail();
    value["data"]["issue"]["description"] = serde_json::json!("x".repeat(10_000));
    let out = format("linear_issue", 200, &body(value), ENV);
    assert!(out.contains("… (truncated)"), "not truncated");
    assert!(out.len() < 10_000, "still {} bytes", out.len());
}

#[test]
fn comments_beyond_the_cap_are_counted_rather_than_dropped_silently() {
    let mut value = detail();
    let many: Vec<Value> = (0..25)
        .map(|n| serde_json::json!({ "body": format!("comment {n}"), "user": { "displayName": "Ada" } }))
        .collect();
    value["data"]["issue"]["comments"]["nodes"] = Value::Array(many);
    let out = format("linear_issue", 200, &body(value), ENV);
    assert!(out.contains("15 more comments not shown"), "{out}");
}

#[test]
fn a_long_comment_is_cut_too() {
    let mut value = detail();
    value["data"]["issue"]["comments"]["nodes"] = serde_json::json!([
        { "body": "y".repeat(5_000), "user": { "displayName": "Ada" } }
    ]);
    let out = format("linear_issue", 200, &body(value), ENV);
    assert!(out.contains("… (truncated)"));
}

use super::*;
use crate::http::{parse, parse_empty};
use crate::types::*;

fn logged_in() -> Credentials {
    Credentials {
        server_url: "https://lab.example.test".into(),
        api_key: Some("swamp_secret".into()),
        username: Some("keeper".into()),
    }
}

fn body(request: &Request) -> Value {
    serde_json::from_slice(&request.body).unwrap()
}

fn server() -> BTreeMap<String, String> {
    BTreeMap::from([("content-type".into(), "application/json".into())])
}

const ISSUE: &str = r#"{
  "id": "7c1d", "number": 42, "type": "bug", "status": "in_progress",
  "title": "Checkout breaks", "body": "It **breaks**.",
  "authorId": "u1", "authorUsername": "keeper", "source": "swamp",
  "assignees": [{"userId": "u2", "username": "fixer"}],
  "githubPrUrl": "https://github.com/o/r/pull/9", "githubPrNumber": 9,
  "isRegression": true, "regressionIntroducedIn": "20260901.1",
  "parentIssueNumber": 40, "authorPlan": "free",
  "createdAt": "2026-09-01T10:00:00.000Z", "updatedAt": "2026-09-02T11:00:00.000Z",
  "somethingNew": {"the": "server grew a field"}
}"#;

const AUDIENCE: &str = r#"{"visibility": "public", "reasons": []}"#;

#[test]
fn every_request_is_addressed_and_authenticated() {
    let creds = logged_in();
    let request = Client::new(&creds).get(42).unwrap();
    assert_eq!(request.method, Method::Get);
    assert_eq!(request.url, "https://lab.example.test/api/v1/lab/issues/42");
    assert_eq!(
        request.headers.get("x-api-key").map(String::as_str),
        Some("swamp_secret")
    );
    assert_eq!(
        request.headers.get("Accept").map(String::as_str),
        Some("application/json")
    );
    assert!(request.body.is_empty());
    assert!(!request.headers.contains_key("Content-Type"));
    assert!(!format!("{request:?}").contains("swamp_secret"));
}

#[test]
fn an_anonymous_caller_may_read_but_not_write() {
    let creds = Credentials::anonymous();
    let client = Client::new(&creds);
    let read = client.search(&SearchQuery::default()).unwrap();
    assert!(!read.headers.contains_key("x-api-key"));
    assert!(client.get(1).is_ok());
    assert!(client.relationships(1).is_ok());
    assert!(client.stream().is_ok());
    let new = NewIssue {
        kind: IssueType::Bug,
        title: "t".into(),
        body: "b".into(),
        source: None,
    };
    assert_eq!(client.create(&new), Err(Error::NotLoggedIn));
    assert_eq!(client.comment(1, "hi"), Err(Error::NotLoggedIn));
    assert_eq!(client.delete(1), Err(Error::NotLoggedIn));
    assert_eq!(client.assignees(), Err(Error::NotLoggedIn));
}

#[test]
fn search_always_bounds_the_page_and_encodes_its_filters() {
    let creds = logged_in();
    let client = Client::new(&creds);
    let plain = client.search(&SearchQuery::default()).unwrap();
    assert_eq!(
        plain.url,
        "https://lab.example.test/api/v1/lab/issues?limit=50"
    );
    let full = client
        .search(&SearchQuery {
            text: Some(" checkout & pay ".into()),
            kind: Some(IssueType::Bug),
            status: Some(IssueStatus::InProgress),
            source: Some("swamp".into()),
            author: Some("keeper".into()),
            shipped_since: Some("20260901.120000.0".into()),
            limit: 200,
            offset: 400,
        })
        .unwrap();
    assert_eq!(
        full.url,
        "https://lab.example.test/api/v1/lab/issues?q=checkout+%26+pay&type=bug&status=in_progress\
         &source=swamp&author=keeper&shippedSince=20260901.120000.0&limit=200&offset=400"
    );
    for limit in [0, 201] {
        let query = SearchQuery {
            limit,
            ..SearchQuery::default()
        };
        assert!(matches!(client.search(&query), Err(Error::Invalid(_))));
    }
    let bad_source = SearchQuery {
        source: Some("no spaces".into()),
        ..SearchQuery::default()
    };
    assert!(matches!(client.search(&bad_source), Err(Error::Invalid(_))));
    let long = SearchQuery {
        text: Some("q".repeat(201)),
        ..SearchQuery::default()
    };
    assert!(matches!(client.search(&long), Err(Error::Invalid(_))));
}

#[test]
fn create_trims_and_checks_what_the_server_would() {
    let creds = logged_in();
    let client = Client::new(&creds);
    let request = client
        .create(&NewIssue {
            kind: IssueType::Feature,
            title: "  Dark mode  ".into(),
            body: "\nPlease.\n".into(),
            source: Some("agents".into()),
        })
        .unwrap();
    assert_eq!(request.method, Method::Post);
    assert_eq!(request.url, "https://lab.example.test/api/v1/lab/issues");
    assert_eq!(
        request.headers.get("Content-Type").map(String::as_str),
        Some("application/json")
    );
    assert_eq!(
        body(&request),
        json!({"type": "feature", "title": "Dark mode", "body": "Please.", "source": "agents"})
    );
    let issue = |title: &str, body: &str| NewIssue {
        kind: IssueType::Bug,
        title: title.into(),
        body: body.into(),
        source: None,
    };
    assert!(client.create(&issue(" ", "b")).is_err());
    assert!(client.create(&issue("t", " ")).is_err());
    assert!(client.create(&issue(&"t".repeat(257), "b")).is_err());
    assert!(client.create(&issue(&"t".repeat(256), "b")).is_ok());
    // The server counts UTF-16 units: 129 astral characters are 258 of them.
    assert!(client.create(&issue(&"🐸".repeat(129), "b")).is_err());
    assert!(client.create(&issue("t", &"b".repeat(65_537))).is_err());
    assert!(!body(&client.create(&issue("t", "b")).unwrap())
        .as_object()
        .unwrap()
        .contains_key("source"));
}

#[test]
fn update_sends_only_what_changes_and_can_clear_a_version() {
    let creds = logged_in();
    let client = Client::new(&creds);
    let request = client
        .update(
            42,
            &IssueUpdate {
                status: Some(IssueStatus::Closed),
                kind: Some(IssueType::Feature),
                resolved_in_version: Some(None),
                assignees: Some(vec!["u2".into()]),
                ..IssueUpdate::default()
            },
        )
        .unwrap();
    assert_eq!(request.method, Method::Patch);
    assert_eq!(request.url, "https://lab.example.test/api/v1/lab/issues/42");
    assert_eq!(
        body(&request),
        json!({"status": "closed", "type": "feature", "resolvedInVersion": null, "assignees": ["u2"]})
    );
    assert!(matches!(
        client.update(42, &IssueUpdate::default()),
        Err(Error::Invalid(_))
    ));
    assert!(client
        .update(
            0,
            &IssueUpdate {
                blocked: Some(true),
                ..IssueUpdate::default()
            }
        )
        .is_err());
}

#[test]
fn comment_endpoints() {
    let creds = logged_in();
    let client = Client::new(&creds);
    let post = client.comment(42, " Looks fixed. ").unwrap();
    assert_eq!(
        (post.method, post.url.as_str()),
        (
            Method::Post,
            "https://lab.example.test/api/v1/lab/issues/42/comments"
        )
    );
    assert_eq!(body(&post), json!({"body": "Looks fixed."}));
    let edit = client.edit_comment(42, "c 1", "Still fixed.").unwrap();
    assert_eq!(edit.method, Method::Patch);
    assert_eq!(
        edit.url,
        "https://lab.example.test/api/v1/lab/issues/42/comments?commentId=c+1"
    );
    let delete = client.delete_comment(42, "c1").unwrap();
    assert_eq!(delete.method, Method::Delete);
    assert!(delete.body.is_empty());
    assert!(client.delete_comment(42, " ").is_err());
    assert!(client.comment(42, "").is_err());
}

#[test]
fn relationship_endpoints() {
    let creds = logged_in();
    let client = Client::new(&creds);
    let relate = client.relate(42, &RelationshipType::BlockedBy, 41).unwrap();
    assert_eq!(
        relate.url,
        "https://lab.example.test/api/v1/lab/issues/42/relationships"
    );
    assert_eq!(
        body(&relate),
        json!({"type": "blocked_by", "targetIssueNumber": 41})
    );
    assert!(client.relate(42, &RelationshipType::RelatedTo, 42).is_err());
    assert!(client.relate(42, &RelationshipType::RelatedTo, 0).is_err());
    let unrelate = client.unrelate(42, "r1").unwrap();
    assert_eq!(unrelate.method, Method::Delete);
    assert_eq!(body(&unrelate), json!({"relationshipId": "r1"}));
    assert_eq!(client.relationships(42).unwrap().method, Method::Get);
}

#[test]
fn audience_lifecycle_ensure_assignees_and_stream() {
    let creds = logged_in();
    let client = Client::new(&creds);
    assert_eq!(
        body(&client.set_audience(42, None).unwrap()),
        json!({"collectiveId": null})
    );
    assert_eq!(
        body(&client.set_audience(42, Some(" ")).unwrap()),
        json!({"collectiveId": null})
    );
    let private = client.set_audience(42, Some("col-1")).unwrap();
    assert_eq!(
        private.url,
        "https://lab.example.test/api/v1/lab/issues/42/audience"
    );
    assert_eq!(body(&private), json!({"collectiveId": "col-1"}));

    let mut payload = Map::new();
    payload.insert("isRegression".into(), true.into());
    let entry = LifecycleInput {
        step: "classified".into(),
        target_status: IssueStatus::Triaged,
        summary: "A regression".into(),
        emoji: "🔎".into(),
        payload: payload.clone(),
        body: None,
        is_verbose: None,
    };
    let lifecycle = client.lifecycle(42, &entry).unwrap();
    assert_eq!(
        lifecycle.url,
        "https://lab.example.test/api/v1/lab/issues/42/lifecycle"
    );
    assert_eq!(
        body(&lifecycle),
        json!({"step": "classified", "targetStatus": "triaged", "summary": "A regression",
               "emoji": "🔎", "payload": {"isRegression": true}})
    );
    payload.insert("$where".into(), "1".into());
    assert!(client
        .lifecycle(42, &LifecycleInput { payload, ..entry })
        .is_err());

    let github = EnsureGithub {
        github_repo_full_name: "swamp-club/swamp".into(),
        github_issue_number: 12,
        title: "From GitHub".into(),
        body: "Body".into(),
        kind: Some(IssueType::Bug),
        github_issue_id: None,
        github_author_login: Some("octo".into()),
    };
    let ensure = client.ensure(&github).unwrap();
    assert_eq!(
        ensure.url,
        "https://lab.example.test/api/v1/lab/issues/ensure"
    );
    assert_eq!(
        body(&ensure),
        json!({"githubRepoFullName": "swamp-club/swamp", "githubIssueNumber": 12,
               "title": "From GitHub", "body": "Body", "type": "bug", "githubAuthorLogin": "octo"})
    );
    let no_owner = EnsureGithub {
        github_repo_full_name: "swamp".into(),
        ..github
    };
    assert!(client.ensure(&no_owner).is_err());

    assert_eq!(
        client.assignees().unwrap().url,
        "https://lab.example.test/api/v1/lab/assignees"
    );
    let stream = client.stream().unwrap();
    assert_eq!(stream.url, "https://lab.example.test/api/v1/lab/stream");
    assert_eq!(
        stream.headers.get("Accept").map(String::as_str),
        Some("text/event-stream")
    );
}

#[test]
fn a_search_reply_decodes_with_its_summaries() {
    let reply = format!(
        r#"{{"issues": [{summary}, {{"number": 7, "type": "platform", "status": "under_review", "title": "New status"}}],
            "total": 9, "tierByUsername": {{"keeper": "gold"}},
            "relationshipSummaries": {{"7c1d": {{"parentIssueNumber": 40, "childCount": 0,
              "siblingCount": 2, "blockedByCount": 1, "blocksCount": 0}}}}}}"#,
        summary = ISSUE.replace(r#""body": "It **breaks**.","#, "")
    );
    let search: SearchResponse = parse(200, &server(), reply.as_bytes()).unwrap();
    assert_eq!(search.total, 9);
    let first = &search.issues[0];
    assert_eq!(
        (first.number, &first.kind, &first.status),
        (42, &IssueType::Bug, &IssueStatus::InProgress)
    );
    assert_eq!(first.body, "", "a list leaves the body out");
    assert_eq!(first.assignees[0].username, "fixer");
    assert_eq!(first.github_pr_number, Some(9));
    assert!(first.is_regression);
    // An unknown status keeps the list, and the value.
    assert_eq!(
        search.issues[1].status,
        IssueStatus::Other("under_review".into())
    );
    assert_eq!(search.issues[1].kind, IssueType::Platform);
    assert_eq!(search.relationship_summaries["7c1d"].sibling_count, 2);
}

#[test]
fn a_detail_reply_decodes_comments_events_and_relationships() {
    let reply = format!(
        r#"{{"issue": {ISSUE}, "audience": {{"visibility": "collective", "reasons": [{{"kind": "private_collective", "collectiveId": "c"}}],
             "collective": {{"id": "c", "slug": "swamp"}}}},
           "tierByUsername": {{}}, "bodyHtml": "<p>It <strong>breaks</strong>.</p>",
           "comments": [{{"id": "c1", "issueId": "7c1d", "authorId": "u2", "authorUsername": "fixer",
                          "body": "On it", "createdAt": "2026-09-02T09:00:00.000Z", "bodyHtml": "<p>On it</p>"}}],
           "events": [{{"id": "e1", "issueId": "7c1d", "actorId": "u2", "actorUsername": "fixer",
                        "type": "status_changed", "from": "triaged", "to": "in_progress",
                        "createdAt": "2026-09-02T09:01:00.000Z"}}],
           "lifecycleEntries": [{{"id": "l1", "issueId": "7c1d", "actorId": "a", "actorUsername": "bot",
                                  "targetStatus": "triaged", "step": "classified", "summary": "bug",
                                  "emoji": "🔎", "payload": {{"isRegression": true}}, "isVerbose": false,
                                  "createdAt": "2026-09-01T10:05:00.000Z"}}],
           "relationships": [{{"id": "r1", "type": "parent_of", "sourceIssueId": "p", "sourceIssueNumber": 40,
                               "targetIssueId": "7c1d", "targetIssueNumber": 42, "createdByUserId": "u1",
                               "createdByUsername": "keeper", "createdAt": "2026-09-01T10:00:00.000Z",
                               "direction": "incoming"}}],
           "isAdmin": false, "canEdit": true, "currentUserId": "u1"}}"#
    );
    let detail: IssueDetail = parse(200, &server(), reply.as_bytes()).unwrap();
    assert_eq!(detail.issue.body, "It **breaks**.");
    assert_eq!(detail.audience.visibility, Visibility::Collective);
    assert_eq!(
        detail.audience.collective.as_ref().unwrap().slug.as_deref(),
        Some("swamp")
    );
    assert_eq!(detail.comments[0].author_username, "fixer");
    assert_eq!(detail.events[0].kind, "status_changed");
    assert_eq!(detail.events[0].detail["to"], "in_progress");
    assert_eq!(detail.lifecycle_entries[0].payload["isRegression"], true);
    assert_eq!(detail.relationships[0].kind, RelationshipType::ParentOf);
    assert_eq!(detail.relationships[0].direction, Some(Direction::Incoming));
    assert!(detail.can_edit && !detail.is_admin);
}

#[test]
fn write_replies_decode() {
    let created: Created = parse(
        201,
        &server(),
        format!(r#"{{"issue": {ISSUE}, "audience": {AUDIENCE}}}"#).as_bytes(),
    )
    .unwrap();
    assert_eq!(created.issue.number, 42);
    assert_eq!(created.audience.visibility, Visibility::Public);

    let updated: Updated = parse(
        200,
        &server(),
        format!(r#"{{"issue": {ISSUE}, "events": []}}"#).as_bytes(),
    )
    .unwrap();
    assert!(updated.events.is_empty());

    // The comment comes back bare, not wrapped in `comment`.
    let comment: Comment = parse(
        201,
        &server(),
        br#"{"id": "c9", "issueId": "7c1d", "authorId": "u1", "authorUsername": "keeper",
             "body": "Thanks", "createdAt": "2026-09-03T00:00:00.000Z", "bodyHtml": "<p>Thanks</p>"}"#,
    )
    .unwrap();
    assert_eq!(comment.id, "c9");

    let related: Related = parse(
        201,
        &server(),
        br#"{"relationship": {"id": "r2", "type": "related_to", "sourceIssueId": "a", "sourceIssueNumber": 42,
              "targetIssueId": "b", "targetIssueNumber": 43, "createdByUserId": "u1",
              "createdByUsername": "keeper", "createdAt": "2026-09-03T00:00:00.000Z"}, "events": []}"#,
    )
    .unwrap();
    assert_eq!(related.relationship.target_issue_number, 43);
    assert_eq!(related.relationship.direction, None);

    let moved: AudienceChanged = parse(
        200,
        &server(),
        format!(r#"{{"audience": {AUDIENCE}, "issue": {ISSUE}}}"#).as_bytes(),
    )
    .unwrap();
    assert_eq!(moved.issue.author_username, "keeper");

    let ensured: Ensured = parse(
        200,
        &server(),
        format!(r#"{{"issue": {ISSUE}, "created": false}}"#).as_bytes(),
    )
    .unwrap();
    assert!(!ensured.created);

    let assignees: Assignees = parse(
        200,
        &server(),
        br#"{"assignees": [{"userId": "u2", "username": "fixer"}]}"#,
    )
    .unwrap();
    assert_eq!(assignees.assignees[0].user_id, "u2");

    let entry: LifecycleEntry = parse(
        201,
        &server(),
        r#"{"id": "l2", "targetStatus": "shipped", "step": "released", "summary": "out", "emoji": "🚀",
             "payload": {}, "isVerbose": false, "createdAt": "2026-09-04T00:00:00.000Z"}"#
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(entry.target_status, IssueStatus::Shipped);

    assert_eq!(parse_empty(204, &server(), b""), Ok(()));
}

#[test]
fn known_enum_values_round_trip() {
    for kind in IssueType::KNOWN {
        assert_eq!(IssueType::parse(kind.as_str()).as_ref(), Some(kind));
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.as_str()));
    }
    assert_eq!(
        IssueStatus::parse("in_progress"),
        Some(IssueStatus::InProgress)
    );
    assert_eq!(IssueStatus::parse("nope"), None);
    assert_eq!(
        serde_json::to_value(IssueStatus::Other("later".into())).unwrap(),
        json!("later")
    );
}

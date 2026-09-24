use super::*;

fn call(name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: "c".into(),
        name: name.into(),
        arguments: arguments.into(),
    }
}

fn server() -> BTreeMap<String, String> {
    BTreeMap::from([("content-type".into(), "application/json".into())])
}

#[test]
fn searches_default_to_every_status_and_a_bounded_page() {
    let Ok(Call::Search(query)) = parse(SEARCH, "{}") else {
        panic!("a search");
    };
    assert_eq!(
        query,
        SearchQuery {
            limit: 100,
            ..SearchQuery::default()
        }
    );
    let Ok(Call::Search(query)) = parse(
        SEARCH,
        r#"{"query":" vault $(id) --help ","type":"bug","status":"in_progress","source":"swamp","author":"ada","limit":5,"offset":10}"#,
    ) else {
        panic!("a search");
    };
    assert_eq!(query.text.as_deref(), Some("vault $(id) --help"));
    assert_eq!(query.kind, Some(lab::IssueType::Bug));
    assert_eq!(query.status, Some(lab::IssueStatus::InProgress));
    assert_eq!((query.limit, query.offset), (5, 10));
    let request = query_request(&query);
    assert!(
        request.url.contains("q=vault+%24%28id%29+--help"),
        "{}",
        request.url
    );
}

fn query_request(query: &SearchQuery) -> swamp_lab::Request {
    Client::new(&Credentials::anonymous())
        .search(query)
        .unwrap()
}

#[test]
fn bad_arguments_are_refused_before_anything_runs() {
    for (name, arguments) in [
        (GET, "{}"),
        (GET, r#"{"number":0}"#),
        (GET, r#"{"number":-1}"#),
        (GET, r#"{"number":"7"}"#),
        (SEARCH, r#"{"limit":0}"#),
        (SEARCH, r#"{"limit":101}"#),
        (SEARCH, r#"{"type":"platform"}"#),
        (SEARCH, r#"{"status":"all"}"#),
        (SEARCH, r#"{"source":"two words"}"#),
        (SEARCH, r#"{"query":"a\u0000b"}"#),
        (SEARCH, r#"{"unknown":1}"#),
        (CREATE, r#"{"type":"bug","title":"t"}"#),
        (CREATE, r#"{"type":"bug","body":"b"}"#),
        (CREATE, r#"{"title":"t","body":"b"}"#),
        (CREATE, r#"{"type":"platform","title":"t","body":"b"}"#),
        (COMMENT, r#"{"body":"b"}"#),
        (COMMENT, r#"{"number":3,"body":"  "}"#),
        ("swamp_issue_delete", "{}"),
    ] {
        assert!(parse(name, arguments).is_err(), "{name} {arguments}");
    }
}

#[test]
fn writes_are_redacted_before_anyone_sees_them() {
    let Ok(Call::Create { issue, redacted }) = parse(
        CREATE,
        r#"{"type":"bug","title":"Crash for ada@example.com","body":"Token ghp_abcdefghijklmnopqrstuvwxyz0123456789 leaked","source":"swamp"}"#,
    ) else {
        panic!("a create");
    };
    assert_eq!(issue.title, "Crash for [REDACTED-EMAIL]");
    assert!(!issue.body.contains("ghp_"), "{}", issue.body);
    assert_eq!(redacted.total, 2);
    let Ok(Call::Comment {
        number,
        body,
        redacted,
    }) = parse(COMMENT, r#"{"number":9,"body":"Seen on 10.1.2.3"}"#)
    else {
        panic!("a comment");
    };
    assert_eq!(number, 9);
    assert!(!body.contains("10.1.2.3"), "{body}");
    assert!(!redacted.is_empty());
}

#[test]
fn the_confirmation_shows_what_will_be_posted() {
    let create = call(
        CREATE,
        r#"{"type":"security","title":"Leak","body":"mail ada@example.com\nsecond line"}"#,
    );
    let sentence = consequence(&create).unwrap();
    assert!(sentence.contains("files a security issue"), "{sentence}");
    assert!(
        sentence.contains("you and the swamp-club admins"),
        "{sentence}"
    );
    assert!(sentence.contains("redacted 1 email first"), "{sentence}");
    let lines = crate::tools::argument_lines(&create);
    assert!(
        lines.contains(&"  mail [REDACTED-EMAIL]".to_owned()),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("ada@example.com")),
        "{lines:?}"
    );

    let public = consequence(&call(CREATE, r#"{"type":"bug","title":"t","body":"b"}"#)).unwrap();
    assert!(
        public.contains("publicly visible") && !public.contains("redacted"),
        "{public}"
    );
    let comment = consequence(&call(COMMENT, r#"{"number":4,"body":"ok"}"#)).unwrap();
    assert!(comment.contains("issue #4"), "{comment}");
    // Unreadable arguments still say what kind of thing would happen.
    assert!(consequence(&call(COMMENT, "{")).is_some());
    assert!(consequence(&call(SEARCH, "{}")).is_none());
    assert!(crate::tools::call_needs_confirmation(&call(CREATE, "{}")));
    assert!(crate::tools::call_needs_confirmation(&call(COMMENT, "{}")));
    assert!(!crate::tools::call_needs_confirmation(&call(SEARCH, "{}")));
    assert!(!crate::tools::call_needs_confirmation(&call(GET, "{}")));
}

const ISSUE: &str = r#"{"id":"i","number":42,"type":"bug","status":"closed","title":"Fix vault","body":"Full issue body",
    "authorId":"u","authorUsername":"Ada","source":"swamp","assignees":[{"userId":"u","username":"Ada"}],
    "githubPrUrl":"https://github.com/o/r/pull/1","isRegression":true,"regressionIntroducedIn":"20260901.1",
    "createdAt":"2026-09-17T00:00:00Z","updatedAt":"2026-09-18T00:00:00Z"}"#;

#[test]
fn replies_are_reduced_to_what_the_model_can_use() {
    let credentials = Credentials::anonymous();
    let search = parse(SEARCH, "{}").unwrap();
    let reply = format!(r#"{{"issues":[{ISSUE}],"total":7}}"#);
    let value: Value =
        serde_json::from_str(&search.result(&credentials, 200, &server(), reply.as_bytes()))
            .unwrap();
    assert_eq!(value["total"], 7);
    assert_eq!(value["issues"][0]["url"], "https://swamp-club.com/lab/42");
    assert_eq!(value["issues"][0]["author"], "Ada");
    assert!(value["issues"][0].get("body").is_none());

    let get = parse(GET, r#"{"number":42}"#).unwrap();
    let reply = format!(
        r#"{{"issue":{ISSUE},"audience":{{"visibility":"public","reasons":[]}},
            "comments":[{{"id":"c","authorUsername":"Bob","body":"Fixed now","createdAt":"2026-09-18T00:00:00Z"}}],
            "relationships":[{{"id":"r","type":"blocked_by","sourceIssueNumber":42,"targetIssueNumber":41,"direction":"outgoing"}}]}}"#
    );
    let value: Value =
        serde_json::from_str(&get.result(&credentials, 200, &server(), reply.as_bytes())).unwrap();
    assert_eq!(value["body"], "Full issue body");
    assert_eq!(value["assignees"], json!(["Ada"]));
    assert_eq!(value["comments"][0]["author"], "Bob");
    assert_eq!(value["relationships"][0]["target"], 41);
    assert_eq!(value["prUrl"], "https://github.com/o/r/pull/1");
    assert_eq!(value["regression"], "20260901.1");
    assert_eq!(value["visibility"], "public");

    let create = parse(
        CREATE,
        r#"{"type":"bug","title":"t","body":"ada@example.com"}"#,
    )
    .unwrap();
    let reply = format!(r#"{{"issue":{ISSUE},"audience":{{"visibility":"public","reasons":[]}}}}"#);
    let value: Value =
        serde_json::from_str(&create.result(&credentials, 201, &server(), reply.as_bytes()))
            .unwrap();
    assert_eq!(value["created"], 42);
    assert_eq!(value["redacted"], "1 email");

    let comment = parse(COMMENT, r#"{"number":42,"body":"thanks"}"#).unwrap();
    let value: Value = serde_json::from_str(&comment.result(
        &credentials,
        201,
        &server(),
        br#"{"id":"c9","authorUsername":"Ada","body":"thanks","createdAt":"2026-09-18T00:00:00Z"}"#,
    ))
    .unwrap();
    assert_eq!(value["comment_id"], "c9");
    assert_eq!(value["redacted"], Value::Null);
}

#[test]
fn failures_are_explained_in_words() {
    let credentials = Credentials::anonymous();
    let get = parse(GET, r#"{"number":42}"#).unwrap();
    assert_eq!(
        get.result(
            &credentials,
            404,
            &server(),
            br#"{"error":"Issue not found"}"#
        ),
        "Issue not found"
    );
    assert!(get
        .result(&credentials, 401, &server(), br#"{"error":"Unauthorized"}"#)
        .contains("swamp auth login"));
    assert!(get
        .result(
            &credentials,
            429,
            &BTreeMap::from([("Retry-After".into(), "12".into())]),
            br#"{"error":"Rate limit exceeded"}"#
        )
        .contains("retry in 12s"));
    assert!(get
        .result(&credentials, 400, &BTreeMap::new(), b"Connection refused")
        .contains("could not reach swamp-club"));
    let create = parse(CREATE, r#"{"type":"bug","title":"t","body":"darn"}"#).unwrap();
    assert!(create
        .result(
            &credentials,
            422,
            &server(),
            br#"{"error":"Content flagged","flagged":["darn"]}"#
        )
        .contains("flagged: darn"));
    assert!(matches!(
        create.request(&credentials),
        Err(swamp_lab::Error::NotLoggedIn)
    ));
}

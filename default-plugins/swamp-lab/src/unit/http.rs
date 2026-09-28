use super::*;
use serde_json::Value;

fn headers(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn server() -> BTreeMap<String, String> {
    headers(&[("content-type", "application/json")])
}

#[test]
fn a_success_decodes_and_a_204_needs_no_body() {
    let value: Value = parse(200, &server(), br#"{"ok":true}"#).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(parse_empty(204, &server(), b""), Ok(()));
    assert!(matches!(
        parse::<Value>(200, &server(), b"<html>"),
        Err(Error::Decode(_))
    ));
}

#[test]
fn failures_are_classified_by_status() {
    let body = |message: &str| format!(r#"{{"error":"{message}"}}"#).into_bytes();
    assert_eq!(
        parse_empty(401, &server(), &body("Unauthorized")),
        Err(Error::Unauthorized("Unauthorized".into()))
    );
    assert_eq!(
        parse_empty(403, &server(), &body("Comments are locked on this issue")),
        Err(Error::Forbidden("Comments are locked on this issue".into()))
    );
    assert_eq!(
        parse_empty(404, &server(), &body("Issue not found")),
        Err(Error::NotFound("Issue not found".into()))
    );
    assert_eq!(
        parse_empty(400, &server(), &body("Invalid status filter")),
        Err(Error::Rejected {
            status: 400,
            message: "Invalid status filter".into(),
            flagged: vec![],
        })
    );
}

#[test]
fn a_profanity_refusal_names_what_it_flagged() {
    let error = parse_empty(
        422,
        &server(),
        br#"{"error":"Content contains inappropriate language","flagged":["darn"]}"#,
    )
    .unwrap_err();
    assert_eq!(
        error,
        Error::Rejected {
            status: 422,
            message: "Content contains inappropriate language".into(),
            flagged: vec!["darn".into()],
        }
    );
    assert!(error.to_string().contains("flagged: darn"));
}

#[test]
fn a_rate_limit_carries_its_retry_after() {
    let error = parse_empty(
        429,
        &headers(&[("Retry-After", "17"), ("X-RateLimit-Scope", "lab-read")]),
        br#"{"error":"Rate limit exceeded","scope":"lab-read"}"#,
    )
    .unwrap_err();
    assert_eq!(
        error,
        Error::RateLimited {
            retry_after: Some(17),
            scope: Some("lab-read".into()),
        }
    );
    assert_eq!(
        error.to_string(),
        "rate limited by swamp-club; retry in 17s"
    );
}

#[test]
fn a_request_that_never_reached_the_server_is_a_transport_error() {
    // What yinz reports when the connection itself failed.
    assert_eq!(
        parse_empty(400, &BTreeMap::new(), b"Connection refused (os error 111)"),
        Err(Error::Transport("Connection refused (os error 111)".into()))
    );
}

#[test]
fn a_body_that_is_not_json_is_quoted_and_bounded() {
    let long = "x".repeat(2000);
    match parse_empty(502, &server(), long.as_bytes()) {
        Err(Error::Rejected { message, .. }) => assert_eq!(message.len(), 500),
        other => panic!("{other:?}"),
    }
    match parse_empty(500, &server(), b"") {
        Err(Error::Rejected { message, .. }) => assert_eq!(message, "HTTP 500"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn headers_are_found_whatever_their_case() {
    let map = headers(&[("RETRY-after", "3")]);
    assert_eq!(header(&map, "retry-after"), Some("3"));
    assert_eq!(header(&map, "x-missing"), None);
}

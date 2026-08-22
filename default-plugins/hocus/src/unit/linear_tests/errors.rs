use super::*;

const ENV: &str = "MY_LINEAR_KEY";

fn body(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).unwrap()
}

/// The trap. Linear answers a validation failure with HTTP 200 and the failure
/// in `errors`, so anything that branches on the status first reports success.
#[test]
fn errors_inside_a_200_are_still_errors() {
    let value = serde_json::json!({
        "data": null,
        "errors": [{ "message": "Field 'nope' doesn't exist" }],
    });
    let out = format("linear_search", 200, &body(value), ENV);
    assert!(out.starts_with("Linear error:"), "{out}");
    assert!(out.contains("doesn't exist"), "{out}");
}

/// And the same body shape arrives with a 400 or a 401, which is the other half
/// of why the status is not what decides.
#[test]
fn errors_alongside_a_non_2xx_read_the_same_way() {
    let value = serde_json::json!({
        "data": null,
        "errors": [{ "message": "Authentication required" }],
    });
    let out = format("linear_issues", 401, &body(value), ENV);
    assert!(out.starts_with("Linear error:"), "{out}");
}

/// A partial success — data *and* errors — is a failure as far as a tool result
/// is concerned. Reporting the half that worked would be reporting a lie.
#[test]
fn a_partial_success_is_reported_as_the_error() {
    let value = serde_json::json!({
        "data": { "issues": { "nodes": [] } },
        "errors": [{ "message": "Something went wrong" }],
    });
    let out = format("linear_issues", 200, &body(value), ENV);
    assert!(out.starts_with("Linear error:"), "{out}");
}

#[test]
fn an_auth_code_names_the_configured_variable() {
    let value = serde_json::json!({
        "errors": [{
            "message": "Authentication required",
            "extensions": { "code": "AUTHENTICATION_ERROR" },
        }],
    });
    let out = format("linear_issues", 200, &body(value), ENV);
    assert!(out.contains("AUTHENTICATION_ERROR"), "{out}");
    // The variable actually configured, not the default — the whole point of
    // making the name configurable.
    assert!(out.contains("$MY_LINEAR_KEY"), "{out}");
}

#[test]
fn a_rate_limit_code_says_to_wait() {
    let value = serde_json::json!({
        "errors": [{ "message": "Too many requests", "extensions": { "code": "RATELIMITED" } }],
    });
    let out = format("linear_search", 200, &body(value), ENV);
    assert!(out.contains("Rate limited"), "{out}");
}

#[test]
fn several_errors_are_joined() {
    let value = serde_json::json!({
        "errors": [{ "message": "first" }, { "message": "second" }],
    });
    let out = format("linear_search", 200, &body(value), ENV);
    assert!(out.contains("first"), "{out}");
    assert!(out.contains("second"), "{out}");
}

/// An error can carry the whole query back in its message.
#[test]
fn a_huge_error_is_capped() {
    let value = serde_json::json!({ "errors": [{ "message": "e".repeat(5_000) }] });
    let out = format("linear_search", 200, &body(value), ENV);
    assert!(out.contains("… (truncated)"), "not truncated");
    assert!(out.len() < 1_000, "still {} bytes", out.len());
}

// -- bodies that are not GraphQL at all ---------------------------------

/// An HTML error page from a proxy in front of the API.
#[test]
fn a_non_json_body_falls_back_to_the_shared_error_renderer() {
    let out = format("linear_search", 502, b"<html>bad gateway</html>", ENV);
    assert!(out.contains("502"), "{out}");
}

#[test]
fn a_non_json_401_still_points_at_the_key() {
    let out = format("linear_search", 401, b"nope", ENV);
    assert!(out.contains("$MY_LINEAR_KEY"), "{out}");
}

#[test]
fn a_non_json_429_says_to_wait() {
    let out = format("linear_search", 429, b"slow down", ENV);
    assert!(out.contains("Rate limited"), "{out}");
}

/// Valid JSON, no errors, but nothing recognisable in it. Better to say so than
/// to render an empty listing as "no issues match", which is a different fact.
#[test]
fn an_unrecognisable_payload_says_it_could_not_be_read() {
    let value = serde_json::json!({ "data": { "somethingElse": 1 } });
    let out = format("linear_search", 200, &body(value), ENV);
    assert_eq!(out, "could not read Linear's response");
}

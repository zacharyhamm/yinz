use super::*;

// -- api_error_message --------------------------------------------------

#[test]
fn api_error_message_extracts_the_openai_error_field() {
    let body =
        br#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}"#;
    assert_eq!(
        api_error_message(401, body),
        "HTTP 401: Incorrect API key provided"
    );
}

#[test]
fn api_error_message_falls_back_to_the_raw_body() {
    assert_eq!(
        api_error_message(502, b"<html>bad gateway</html>"),
        "HTTP 502: <html>bad gateway</html>"
    );
}

#[test]
fn api_error_message_handles_an_empty_body() {
    assert_eq!(api_error_message(500, b""), "HTTP 500");
}

#[test]
fn api_error_message_truncates_a_huge_body() {
    let body = "x".repeat(5000);
    let message = api_error_message(500, body.as_bytes());
    assert_eq!(message.chars().count(), "HTTP 500: ".len() + 500);
}

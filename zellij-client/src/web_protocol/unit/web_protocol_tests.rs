//! The wire format is shared with `assets/*.js`, so these tests pin the JSON
//! shape rather than just the Rust round-trip.

use crate::web_protocol::control_message::{SetConfigPayload, SetConfigPayloadTheme};
use crate::web_protocol::endpoints;
use crate::web_protocol::http::{CreateClientIdResponse, LoginRequest};

#[test]
fn login_request_round_trips() {
    let sent = LoginRequest {
        auth_token: "token".to_string(),
        remember_me: Some(true),
    };
    let received: LoginRequest = serde_json::from_str(&serde_json::to_string(&sent).unwrap())
        .expect("the server parses what the remote client sends");

    assert_eq!(received.auth_token, "token");
    assert_eq!(received.remember_me, Some(true));
}

#[test]
fn login_request_without_remember_me_parses() {
    let received: LoginRequest =
        serde_json::from_str(r#"{"auth_token":"token"}"#).expect("remember_me is optional");

    assert_eq!(received.remember_me, None);
}

#[test]
fn create_client_id_response_round_trips() {
    let sent = CreateClientIdResponse {
        web_client_id: "client-id".to_string(),
        is_read_only: true,
        session_name: "session".to_string(),
        config: SetConfigPayload {
            font: "Monospace".to_string(),
            ..Default::default()
        },
    };
    let received: CreateClientIdResponse =
        serde_json::from_str(&serde_json::to_string(&sent).unwrap())
            .expect("the remote client parses what the server sends");

    assert_eq!(received.web_client_id, "client-id");
    assert!(received.is_read_only);
    assert_eq!(received.session_name, "session");
    assert_eq!(received.config.font, "Monospace");
}

#[test]
fn create_client_id_response_needs_only_the_web_client_id() {
    // What `remote_attach` relies on: it cares about the id and nothing else.
    let received: CreateClientIdResponse = serde_json::from_str(r#"{"web_client_id":"client-id"}"#)
        .expect("the browser-only fields are optional");

    assert_eq!(received.web_client_id, "client-id");
    assert!(!received.is_read_only);
}

#[test]
fn login_request_json_shape() {
    let request = LoginRequest {
        auth_token: "token".to_string(),
        remember_me: Some(false),
    };

    insta::assert_snapshot!(serde_json::to_string_pretty(&request).unwrap());
}

#[test]
fn create_client_id_response_json_shape() {
    let response = CreateClientIdResponse {
        web_client_id: "client-id".to_string(),
        is_read_only: false,
        session_name: "session".to_string(),
        config: SetConfigPayload {
            font: "Monospace".to_string(),
            theme: SetConfigPayloadTheme {
                background: Some("#000000".to_string()),
                foreground: Some("#ffffff".to_string()),
                bright_black: Some("#555555".to_string()),
                selection_background: Some("#222222".to_string()),
                cursor_accent: Some("#111111".to_string()),
                ..Default::default()
            },
            cursor_blink: true,
            cursor_inactive_style: Some("underline".to_string()),
            cursor_style: Some("block".to_string()),
            mac_option_is_meta: true,
            font_size: Some(14),
        },
    };

    insta::assert_snapshot!(serde_json::to_string_pretty(&response).unwrap());
}

#[test]
fn session_ws_route_pattern_extends_its_base() {
    assert!(endpoints::WS_TERMINAL_WITH_SESSION.starts_with(endpoints::WS_TERMINAL));
}

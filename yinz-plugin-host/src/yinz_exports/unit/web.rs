use super::*;
#[test]
fn response_validation_and_reply_correlation() {
    let mut env = crate::test_env::test_env();
    let (slot, mut receiver) = yinz_bus::web::ReplySlot::new();
    env.web_pending.lock().unwrap().insert(1, slot.clone());
    respond(&mut env, 2, "invalid".into()).unwrap();
    assert!(!slot.is_closed());
    respond(&mut env, 1, r#"{"status":200,"headers":{"content-type":"application/json","x-yinz-test":"yes"},"body":"{}"}"#.into()).unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        WebReply::Response(_)
    ));
    respond(&mut env, 1, "invalid duplicate".into()).unwrap();
    for value in [
        r#"{"status":199,"headers":{},"body":""}"#,
        r#"{"status":304,"headers":{"content-type":"text/plain"},"body":"x"}"#,
        r#"{"status":200,"headers":{},"body":"x"}"#,
        r#"{"status":200,"headers":{"set-cookie":"secret"},"body":""}"#,
        r#"{"status":200,"headers":{"content-type":"text/plain\r\nx: injected"},"body":"x"}"#,
    ] {
        assert!(validate_response(value).is_err(), "{value}");
    }
    for location in ["//host", "/\\host", "/%2fhost", "https://host", "/%0aevil"] {
        assert!(!safe_location(location));
    }
    assert!(safe_location("/login?next=ok"));
}
#[test]
fn matching_invalid_response_fails_once() {
    let mut env = crate::test_env::test_env();
    let (slot, mut receiver) = yinz_bus::web::ReplySlot::new();
    env.web_pending.lock().unwrap().insert(3, slot);
    respond(&mut env, 3, "invalid".into()).unwrap();
    assert!(matches!(receiver.try_recv().unwrap(), WebReply::Failed(_)));
}

#[test]
fn command_permissions_identity_and_instance_slots() {
    use yinz_types::data::{PermissionStatus, PermissionType, PluginCommand};
    let mut env = crate::test_env::test_env();
    let commands = [
        PluginCommand::HttpRespond {
            request_id: 1,
            response: "invalid".into(),
        },
        PluginCommand::HttpAssets {
            prefix: "/static/".into(),
            directory: "static".into(),
        },
        PluginCommand::HttpPublish {
            path: "/".into(),
            content_type: "text/html; charset=utf-8".into(),
            body: "".into(),
        },
        PluginCommand::AgentView {
            id: "x".into(),
            request: "{}".into(),
        },
    ];
    for command in &commands {
        assert_eq!(
            crate::yinz_exports::check_command_permission(&env, command),
            (PermissionStatus::Denied, Some(PermissionType::RunCommands))
        );
    }
    *env.permissions.lock().unwrap() = Some(std::collections::HashSet::from([
        PermissionType::RunCommands,
    ]));
    for command in &commands[1..] {
        let error = crate::yinz_exports::run_plugin_command(&mut env, command.clone()).unwrap_err();
        assert!(format!("{error:#}").contains(RESERVED), "{error:#}");
    }
    assert!(!is_web_plugin(&env));
    env.web.set_target(Some(env.plugin_id));
    assert!(is_web_plugin(&env));
    let mut peer = crate::test_env::test_env();
    let (slot, mut receiver) = yinz_bus::web::ReplySlot::new();
    env.web_pending.lock().unwrap().insert(1, slot.clone());
    respond(&mut peer, 1, "invalid".into()).unwrap();
    assert!(!slot.is_closed());
    env.close_web();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        WebReply::Unavailable { .. }
    ));
}
#[test]
fn response_boundaries_and_redirect_attacks() {
    for status in [200, 599] {
        let value = serde_json::json!({"status":status,"headers":{"content-type":"text/plain"},"body":"x".repeat(MAX_RESPONSE_BODY)});
        assert!(validate_response(&value.to_string()).is_ok());
    }
    let value = serde_json::json!({"status":200,"headers":{"content-type":"text/plain"},"body":"x".repeat(MAX_RESPONSE_BODY+1)});
    assert!(validate_response(&value.to_string()).is_err());
    for value in [
        serde_json::json!({"status":200.5,"headers":{},"body":""}),
        serde_json::json!({"status":600,"headers":{},"body":""}),
        serde_json::json!({"status":200,"headers":{"x-yinz-":"empty suffix"},"body":""}),
    ] {
        assert!(validate_response(&value.to_string()).is_err());
    }
    for location in [
        "/%09/host",
        "/%1f/host",
        "/%7f/host",
        "/%5Chost",
        "/%2Fhost",
    ] {
        assert!(!safe_location(location));
    }
    for status in [200, 302] {
        let value =
            serde_json::json!({"status":status,"headers":{"location":"/login?x=y"},"body":""});
        assert_eq!(validate_response(&value.to_string()).is_ok(), status == 302);
    }
}

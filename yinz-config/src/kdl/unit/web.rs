use super::*;
use std::net::SocketAddr;
use yinz_types::configuration::{Node, Patch};
use yinz_types::input::web::WebConfig;

fn parse(text: &str) -> Result<Config, ConfigError> {
    Config::from_kdl(text, None)
}

fn error_message(text: &str) -> String {
    match parse(text) {
        Err(ConfigError::KdlError(error)) => error.error_message,
        other => panic!("{text:?} should be a KdlError, got {other:?}"),
    }
}

const FULL: &str = r#"
web {
    enabled true
    bind "127.0.0.1"
    port 0
    plugin "file:/tmp/my-web.scm"
    allowed_hosts "box.tail1234.ts.net" "other.example"
    allowed_origins "https://box.tail1234.ts.net:7443"
    tailscale_users "me@github"
    tailscale_users "you@github"
    session_days 7
    https_port_hint 9443
}
"#;

#[test]
fn web_block_parses_every_field() {
    let web = parse(FULL).expect("parse").web;
    assert_eq!(
        web,
        WebConfig {
            enabled: Some(true),
            bind: Some("127.0.0.1".into()),
            port: Some(0),
            allow_non_loopback: None,
            plugin: Some("file:/tmp/my-web.scm".into()),
            allowed_hosts: vec!["box.tail1234.ts.net".into(), "other.example".into()],
            allowed_origins: vec!["https://box.tail1234.ts.net:7443".into()],
            tailscale_users: vec!["me@github".into(), "you@github".into()],
            session_days: Some(7),
            https_port_hint: Some(9443),
        }
    );
    assert!(web.enabled());
    assert_eq!(
        web.bind_addr(),
        Ok("127.0.0.1:0".parse::<SocketAddr>().unwrap())
    );
    assert_eq!(web.plugin_or_default(), "file:/tmp/my-web.scm");
}

#[test]
fn web_defaults_are_off_on_loopback_port_7419() {
    let web = WebConfig::default();
    assert!(!web.enabled());
    assert_eq!(
        web.bind_addr(),
        Ok("127.0.0.1:7419".parse::<SocketAddr>().unwrap())
    );
    assert_eq!(web.plugin_or_default(), "web");
    assert_eq!(web.session_days_or_default(), 30);
    assert_eq!(web.https_port_hint_or_default(), 7443);
    let empty = parse("web {\n}\n").expect("an empty block parses").web;
    assert_eq!(empty, WebConfig::default());
    assert_eq!(
        Config::from_default_assets().expect("defaults").web,
        WebConfig::default(),
        "the shipped config only shows the block in a comment"
    );
}

#[test]
fn web_block_round_trips_through_to_kdl() {
    let config = parse(FULL).expect("parse");
    let text = config.to_string(false).expect("serialize");
    let reparsed = parse(&text).expect("reparse");
    assert_eq!(reparsed.web, config.web);

    let default_text = Config::default().to_string(false).expect("serialize");
    assert!(
        !default_text.contains("web {"),
        "a default web config writes no block: {default_text}"
    );
}

#[test]
fn web_fragments_merge_field_by_field() {
    let base =
        parse("web {\n enabled true\n port 8000\n allowed_hosts \"a.example\"\n}\n").expect("base");
    let merged = Config::from_kdl("web {\n port 9000\n}\n", Some(base)).expect("merged");
    assert_eq!(merged.web.enabled, Some(true));
    assert_eq!(merged.web.port, Some(9000));
    assert_eq!(merged.web.allowed_hosts, vec!["a.example".to_owned()]);

    let replaced = WebConfig {
        allowed_hosts: vec!["a".into()],
        ..Default::default()
    }
    .merge(WebConfig {
        allowed_hosts: vec!["b".into()],
        enabled: Some(false),
        ..Default::default()
    });
    assert_eq!(replaced.allowed_hosts, vec!["b".to_owned()]);
    assert_eq!(replaced.enabled, Some(false));
}

#[test]
fn web_non_loopback_bind_needs_the_opt_in() {
    let message = error_message("web {\n enabled true\n bind \"0.0.0.0\"\n}\n");
    assert!(message.contains("allow_non_loopback"), "{message}");
    let web = parse("web {\n bind \"0.0.0.0\"\n allow_non_loopback true\n port 8080\n}\n")
        .expect("opted in")
        .web;
    assert_eq!(
        web.bind_addr(),
        Ok("0.0.0.0:8080".parse::<SocketAddr>().unwrap())
    );
    let v6 = parse("web {\n bind \"::1\"\n}\n")
        .expect("ipv6 loopback")
        .web;
    assert_eq!(
        v6.bind_addr(),
        Ok("[::1]:7419".parse::<SocketAddr>().unwrap())
    );
}

#[test]
fn web_rejects_bad_values() {
    for (text, expected) in [
        ("web {\n port 70000\n}\n", "not a port"),
        ("web {\n port -1\n}\n", "not a port"),
        ("web {\n port \"80\"\n}\n", "whole number"),
        ("web {\n bind \"localhost\"\n}\n", "not an IP address"),
        ("web {\n session_days 0\n}\n", "session_days"),
        ("web {\n session_days 91\n}\n", "session_days"),
        ("web {\n enabled \"yes\"\n}\n", "true or false"),
        ("web {\n enabled true false\n}\n", "exactly one value"),
        ("web {\n allowed_hosts 1\n}\n", "strings"),
        ("web {\n colour \"red\"\n}\n", "unknown web setting"),
        ("web true\n", "only child entries"),
    ] {
        let message = error_message(text);
        assert!(message.contains(expected), "{text:?}: {message}");
    }
    assert!(parse("web {\n session_days 90\n}\n").is_ok());
    assert!(parse("web {\n session_days 1\n}\n").is_ok());
}

fn patch(path: &[&str], value: Option<serde_json::Value>) -> Patch {
    Patch {
        reset: false,
        path: path.iter().map(|s| (*s).into()).collect(),
        node: value.map(|value| Node {
            name: path.last().unwrap().to_string(),
            arguments: vec![value],
            properties: Default::default(),
            children: vec![],
        }),
    }
}

#[test]
fn web_editor_applies_and_removes_entries() {
    let config = Config::default();
    let edited = crate::editor::apply(
        &config,
        &[
            patch(&["web", "enabled"], Some(serde_json::json!(true))),
            patch(&["web", "port"], Some(serde_json::json!(8123))),
        ],
    )
    .expect("apply");
    assert_eq!(edited.web.enabled, Some(true));
    assert_eq!(edited.web.port, Some(8123));
    assert_eq!(edited.keybinds, config.keybinds, "other sections untouched");

    let removed = crate::editor::apply(&edited, &[patch(&["web", "port"], None)]).expect("remove");
    assert_eq!(removed.web.port, None);
    assert_eq!(removed.web.enabled, Some(true));

    // `bind` is the keybinding node name to the editor's path keys, so a whole-block edit.
    let block = Patch {
        reset: false,
        path: vec!["web".into()],
        node: Some(Node {
            name: "web".into(),
            arguments: vec![],
            properties: Default::default(),
            children: vec![patch(&["bind"], Some(serde_json::json!("0.0.0.0")))
                .node
                .unwrap()],
        }),
    };
    let refused =
        crate::editor::apply(&config, &[block]).expect_err("the loopback rule applies to edits");
    assert!(refused.contains("allow_non_loopback"), "{refused}");
}

#[test]
fn web_example_configs_parse() {
    Config::from_kdl(include_str!("../../../../example/default.kdl"), None)
        .expect("example/default.kdl parses");
    let defaults = Config::from_default_assets().expect("default.kdl parses");
    assert!(
        defaults.plugins.aliases.contains_key("web"),
        "the shipped config names the web route plugin"
    );
    assert!(
        !defaults
            .background_plugins
            .iter()
            .any(|plugin| format!("{plugin:?}").contains("\"web\"")),
        "the web plugin is loaded on activation, never at session start"
    );
}

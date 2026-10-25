use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde_json::{json, Value};
use yinz_steel::Runtime;
use yinz_types::{internal_ids::PluginId, own_pane::own_pane_command};

use crate::fixture_source;

#[test]
fn registered_builtins_are_disk_entrypoints() {
    let names = yinz_runtime::consts::BUILTIN_PLUGIN_NAMES;
    let expected = names
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(expected.len(), names.len(), "builtin names must be unique");
    for tag in names {
        assert!(
            !tag.contains('.'),
            "a builtin must be an entrypoint, not a module or test"
        );
        assert!(fixture_source(tag).is_some(), "missing builtin {tag}");
    }
}

#[derive(Default)]
pub(super) struct Host {
    pub(super) commands: Vec<Value>,
    pane: Value,
    pub(super) replies: BTreeMap<String, Value>,
    pub(super) failures: BTreeMap<String, String>,
}

fn plugin(tag: &str, configuration: Value) -> (Runtime, Arc<Mutex<Host>>) {
    plugin_with_replies(tag, configuration, BTreeMap::new())
}

pub(super) fn plugin_with_replies(
    tag: &str,
    configuration: Value,
    replies: BTreeMap<String, Value>,
) -> (Runtime, Arc<Mutex<Host>>) {
    plugin_with_prelude(tag, configuration, replies, "")
}

pub(super) fn plugin_with_prelude(
    tag: &str,
    configuration: Value,
    replies: BTreeMap<String, Value>,
    prelude: &str,
) -> (Runtime, Arc<Mutex<Host>>) {
    plugin_with_source(
        tag,
        &fixture_source(tag).unwrap(),
        configuration,
        replies,
        prelude,
    )
}

pub(super) fn plugin_with_source(
    tag: &str,
    source: &str,
    configuration: Value,
    replies: BTreeMap<String, Value>,
    prelude: &str,
) -> (Runtime, Arc<Mutex<Host>>) {
    // The host decides on the configuration it passes, which holds string values only.
    let settings = configuration
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
        .collect();
    let headless = !yinz_steel::manifest::pane_declaration(source)
        .unwrap()
        .applies(&settings);
    let host = Arc::new(Mutex::new(Host {
        replies,
        ..Host::default()
    }));
    let captured = host.clone();
    let name = tag.to_owned();
    let mut runtime = Runtime::new(move |command: Value| {
        let typed = yinz_steel::binary::decode_command(command.clone())
            .unwrap_or_else(|error| panic!("invalid host command {command}: {error}"));
        if headless {
            let me = captured
                .lock()
                .unwrap()
                .replies
                .get("GetPluginIds")
                .and_then(|ids| ids["plugin_id"].as_u64())
                .unwrap_or(42);
            let me = PluginId::new(u32::try_from(me).unwrap());
            if let Some(refused) = own_pane_command(&typed, me) {
                panic!("{name} declares no pane, so the host refuses its '{refused}': {command}");
            }
        }
        let command = if command.pointer("/FsWrite/contents/$yinz-bytes").is_some()
            || command.pointer("/WebRequest/3/$yinz-bytes").is_some()
        {
            serde_json::to_value(&typed).unwrap()
        } else {
            command
        };
        if let yinz_types::data::PluginCommand::CommandPalette(
            yinz_types::command_palette::Command::Reply {
                result: Ok(reply), ..
            },
        ) = typed
        {
            reply.validate().unwrap();
        }
        let mut host = captured.lock().unwrap();
        host.commands.push(command.clone());
        let name = command
            .as_str()
            .or_else(|| {
                command
                    .as_object()
                    .and_then(|object| object.keys().next().map(String::as_str))
            })
            .unwrap();
        if let Some(error) = host.failures.get(name) {
            return Err(std::io::Error::other(error.clone()).into());
        }
        Ok(if let Some(reply) = host.replies.get(name) {
            reply.clone()
        } else if command.get("GetPaneInfo").is_some() {
            host.pane.clone()
        } else if command == "GetPluginIds" {
            json!({"plugin_id": 42, "client_id": 1, "max_session_name_bytes": 80})
        } else {
            Value::Null
        })
    })
    .unwrap();
    runtime.eval(prelude).unwrap();
    let mut root =
        yinz_steel::modules::SourceRoot::file(&crate::asset_dir().join(format!("{tag}.scm")), None)
            .unwrap();
    runtime.eval_with_source_root(source, &mut root).unwrap();
    runtime.load(configuration).unwrap();
    (runtime, host)
}

fn source() -> Value {
    json!({
        "request_id": 10, "pane_id": {"Terminal": 7},
        "items": [{"id": 1, "label": "Custom action"}],
        "selected_text": null, "link": null,
    })
}

fn key(name: Value, modifiers: &[&str]) -> Value {
    json!({"Key": {"bare_key": name, "key_modifiers": modifiers}})
}

#[test]
fn the_test_host_refuses_own_pane_commands_only_from_plugins_without_a_pane() {
    let load = |declaration: &str, configuration: Value| {
        let source = format!(
            "{declaration}\n(define (load configuration) (yinz-command (hash \"RenamePluginPane\" (list 42 \"x\"))) (yinz-command \"HideSelf\"))"
        );
        std::panic::catch_unwind(|| {
            plugin_with_source("repl", &source, configuration, BTreeMap::new(), "")
        })
        .map(|(_, host)| host.lock().unwrap().commands.len())
    };
    assert_eq!(load("(yinz-declare-pane)", json!({})).unwrap(), 2);
    let unless = r#"(yinz-declare-pane-unless "duplicate_focused" "true")"#;
    assert_eq!(load(unless, json!({})).unwrap(), 2);
    assert!(load(unless, json!({"duplicate_focused": "true"})).is_err());
    assert!(load("", json!({})).is_err());
}

#[test]
fn agent_policy_notifies_only_waiting_agents_and_uses_display_fallbacks() {
    let (mut runtime, host) = plugin("agent-policy", json!({}));
    runtime
        .update(json!({"Hook": {"point": "session_start", "payload": {"session_name": "test"}}}))
        .unwrap();
    assert_eq!(
        host.lock().unwrap().commands,
        [json!({"Subscribe": ["Hook"]})]
    );
    assert!(runtime.take_logs().unwrap().contains("session_start"));
    for (agent, expected) in [
        (
            json!({"kind": "codex", "detail": "needs an answer"}),
            "codex in pane 7: needs an answer",
        ),
        (
            json!({"kind": "", "detail": ""}),
            "agent in pane 7: is waiting for you",
        ),
    ] {
        runtime
            .update(json!({"Hook": {"point": "agent_waiting", "payload": {"pane_id": 7, "agent": agent}}}))
            .unwrap();
        assert_eq!(
            host.lock().unwrap().commands.last(),
            Some(&json!({"Notify": [expected, {"Terminal": 7}, null]}))
        );
    }
}

#[test]
fn opencode_registers_launch_restart_and_external_hook_contract() {
    let (_, host) = plugin("agent-opencode", json!({}));
    let host = host.lock().unwrap();
    assert_eq!(host.commands.len(), 1);
    let provider = &host.commands[0]["RegisterAgentProvider"];
    assert_eq!(provider["kind"], "opencode");
    assert_eq!(provider["executable"], "opencode");
    assert_eq!(
        provider["launch"]["prompt_args"],
        json!(["--prompt", "{value}"])
    );
    assert_eq!(
        provider["launch"]["resume_args"],
        json!(["--session", "{value}"])
    );
    assert_eq!(provider["launch"]["permissions"]["skip"], json!(["--auto"]));
    assert!(provider.get("input").is_none());
    assert_eq!(provider["restart"]["max_positionals"], 1);
    assert!(provider["restart"]["forbidden_commands"]
        .as_array()
        .unwrap()
        .contains(&json!("run")));
    let hook = &provider["hooks"][0];
    assert_eq!(hook["path"], "opencode/plugin/yinz.js");
    let content = hook["content"].as_str().unwrap();
    assert!(content.contains("JSON.stringify(toHookEvent(payload))"));
    assert!(content.contains("AskUserQuestion"));
    assert!(content.contains("__TOKEN_ENV__"));
}

#[test]
fn context_menu_adds_agent_actions_only_to_current_terminal_agent_records() {
    let (mut runtime, host) = plugin("context-menu", json!({}));
    for pane in [
        Value::Null,
        json!({"agent": null}),
        json!({"agent": {"kind": "codex"}}),
    ] {
        host.lock().unwrap().pane = pane.clone();
        for pane_id in [json!({"Terminal": 7}), json!({"Plugin": 7})] {
            let mut context = source();
            context["pane_id"] = pane_id.clone();
            runtime
                .update(json!({"ContextMenuRequested": context}))
                .unwrap();
            let mut items = vec![];
            if pane_id.get("Terminal").is_some() && !pane["agent"].is_null() {
                items.extend([
                    json!({"label": "Duplicate agent", "context": {"action": "duplicate_agent"}}),
                    json!({"label": "Fork agent", "context": {"action": "fork_agent"}}),
                    json!({"separator": true}),
                ]);
            }
            items.push(json!({"id": 1, "label": "Custom action"}));
            assert_eq!(
                host.lock().unwrap().commands.last(),
                Some(&json!({"SetContextMenu": {"request_id": 10, "items": items}}))
            );
        }
    }
}

#[test]
fn context_menu_agent_selection_rechecks_the_clicked_pane() {
    let (mut runtime, host) = plugin("context-menu", json!({}));
    for (action, flag) in [
        ("duplicate_agent", "duplicate_focused"),
        ("fork_agent", "fork_focused"),
    ] {
        host.lock().unwrap().pane = json!({"agent": {"kind": "codex"}});
        let event = json!({"ContextMenuItemSelected": {"item_context": {"action": action}, "context": source()}});
        runtime.update(event.clone()).unwrap();
        {
            let host = host.lock().unwrap();
            assert_eq!(
                host.commands[host.commands.len() - 2],
                json!({"GetPaneInfo": {"Terminal": 7}})
            );
            let launch = &host.commands.last().unwrap()["RunAction"][0]["LaunchPlugin"];
            assert_eq!(
                launch["plugin"]["RunPlugin"]["configuration"],
                json!({
                    "caller_terminal_pane_id": "7", flag: "true", "overlay": "true", "modal": "true",
                })
            );
        }
        for missing in [Value::Null, json!({"agent": null})] {
            host.lock().unwrap().pane = missing;
            runtime.update(event.clone()).unwrap();
            assert_eq!(
                host.lock().unwrap().commands.last(),
                Some(&json!({"GetPaneInfo": {"Terminal": 7}}))
            );
        }
    }
}

#[test]
fn context_menu_removes_unavailable_copy_and_redundant_separators() {
    let (mut runtime, host) = plugin("context-menu", json!({}));
    let mut context = source();
    context["items"] = json!([
        {"separator": true}, {"id": 1, "label": "Copy"}, {"separator": true},
        {"id": 2, "label": "Close"}, {"separator": true},
    ]);
    runtime
        .update(json!({"ContextMenuRequested": context}))
        .unwrap();
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"SetContextMenu": {
            "request_id": 10, "items": [{"id": 2, "label": "Close"}],
        }}))
    );
}

#[test]
fn context_menu_shortens_unicode_and_preserves_configured_item_context() {
    let (mut runtime, host) = plugin("context-menu", json!({}));
    let mut context = source();
    context["selected_text"] = json!(format!(" \n{}\tend ", "界".repeat(25)));
    context["items"] = json!([{"id": 9, "label": "Copy", "context": {"original": "kept"}}]);
    runtime
        .update(json!({"ContextMenuRequested": context}))
        .unwrap();
    assert_eq!(
        host.lock().unwrap().commands.last().unwrap()["SetContextMenu"]["items"],
        json!([
            {"id": 9, "label": format!("Copy \"{}…\"", "界".repeat(24)), "context": {"original": "kept"}},
        ])
    );
}

#[test]
fn context_menu_opener_probes_queue_links_and_prefer_xdg_open() {
    let (mut runtime, host) = plugin("context-menu", json!({}));
    let click = json!({"ContextMenuItemSelected": {"item_context": {"action": "open_link", "url": "https://example.test"}}});
    let initial = host.lock().unwrap().commands.len();
    runtime.update(click.clone()).unwrap();
    runtime
        .update(json!({"RunCommandResult": [1, [], [], {"probe": "xdg-open"}]}))
        .unwrap();
    assert_eq!(host.lock().unwrap().commands.len(), initial);
    runtime
        .update(json!({"RunCommandResult": [0, [], [], {"probe": "open"}]}))
        .unwrap();
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"RunCommand": [["open", "https://example.test"], {}, "", {}, 64, null]}))
    );
    runtime
        .update(json!({"RunCommandResult": [0, [], [], {"probe": "xdg-open"}]}))
        .unwrap();
    runtime.update(click).unwrap();
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"RunCommand": [["xdg-open", "https://example.test"], {}, "", {}, 64, null]}))
    );
}

#[test]
fn quit_confirmation_defaults_to_cancel_and_quit_is_one_tab_away() {
    for configuration in [
        json!({}),
        json!({"reason": "last_pane", "notice": "agent is working"}),
    ] {
        let (mut runtime, host) = plugin("quit-confirmation", configuration);
        runtime.update(key(json!("Enter"), &[])).unwrap();
        assert_eq!(
            host.lock().unwrap().commands.last(),
            Some(&json!("CloseSelf"))
        );
        assert!(!host.lock().unwrap().commands.contains(&json!("QuitYinz")));
        assert!(runtime.update(key(json!("Tab"), &[])).unwrap());
        runtime.update(key(json!("Enter"), &[])).unwrap();
        assert!(host
            .lock()
            .unwrap()
            .commands
            .ends_with(&[json!("CloseSelf"), json!("QuitYinz")]));
    }
}

#[test]
fn quit_confirmation_requires_unmodified_activation_and_supports_cancel_keys() {
    let (mut runtime, host) = plugin("quit-confirmation", json!({}));
    for modifiers in [&["Ctrl"][..], &["Shift"][..], &["Alt"][..]] {
        runtime.update(key(json!("Enter"), modifiers)).unwrap();
    }
    assert!(!host.lock().unwrap().commands.contains(&json!("CloseSelf")));
    for event in [key(json!("Esc"), &[]), key(json!({"Char": "c"}), &["Ctrl"])] {
        runtime.update(event).unwrap();
        assert_eq!(
            host.lock().unwrap().commands.last(),
            Some(&json!("CloseSelf"))
        );
    }
}

#[test]
fn quit_confirmation_mouse_requires_press_and_release_on_the_same_visible_button() {
    let (mut runtime, host) = plugin("quit-confirmation", json!({}));
    runtime
        .update(json!({"Mouse": {"LeftClick": [4, 12]}}))
        .unwrap();
    runtime
        .update(json!({"Mouse": {"Release": [4, 12]}}))
        .unwrap();
    assert!(!host.lock().unwrap().commands.contains(&json!("QuitYinz")));
    runtime.render(7, 60).unwrap();
    runtime
        .update(json!({"Mouse": {"LeftClick": [4, 12]}}))
        .unwrap();
    assert!(!host.lock().unwrap().commands.contains(&json!("QuitYinz")));
    runtime
        .update(json!({"Mouse": {"Release": [4, 2]}}))
        .unwrap();
    assert!(!host.lock().unwrap().commands.contains(&json!("QuitYinz")));
    runtime
        .update(json!({"Mouse": {"LeftClick": [4, 12]}}))
        .unwrap();
    runtime
        .update(json!({"Mouse": {"Release": [4, 12]}}))
        .unwrap();
    assert!(host
        .lock()
        .unwrap()
        .commands
        .ends_with(&[json!("CloseSelf"), json!("QuitYinz")]));
}

pub(super) fn rendered_text(output: &str) -> String {
    output
        .split("\u{1b}Py")
        .skip(1)
        .flat_map(|component| {
            component
                .split("\u{1b}\\")
                .next()
                .unwrap()
                .split(';')
                .skip(1)
                .filter(|field| !field.contains('/'))
                .map(|body| {
                    let text = body
                        .rsplit('$')
                        .next()
                        .unwrap()
                        .trim_start_matches(['x', 'z', 'd', '|']);
                    String::from_utf8(
                        text.split(',')
                            .filter(|byte| !byte.is_empty())
                            .map(|byte| byte.parse::<u8>().unwrap())
                            .collect(),
                    )
                    .unwrap()
                })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn quit_confirmation_shows_notices_and_last_pane_reason_and_fits_small_viewports() {
    let (mut runtime, host) = plugin(
        "quit-confirmation",
        json!({"reason": "last_pane", "notice": "agent is working\nan agent is busy"}),
    );
    assert_eq!(
        host.lock().unwrap().commands.last().unwrap()["ChangeOverlayCoordinates"]["placements"][0]
            [1]["height"],
        json!({"Fixed": 8})
    );
    let output = rendered_text(&runtime.render(8, 80).unwrap());
    assert!(output.contains("That was the last pane. Quit Yinz?"));
    assert!(output.contains("agent is working"));
    assert!(output.contains("an agent is busy"));
    for rows in 0..8 {
        for columns in 0..20 {
            runtime.render(rows, columns).unwrap();
        }
    }
}

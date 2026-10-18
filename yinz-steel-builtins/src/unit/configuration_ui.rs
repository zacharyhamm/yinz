use crate::tests::{plugin_with_replies, rendered_text, Host};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use yinz_config::config::Config;
use yinz_steel::Runtime;
use yinz_types::{
    data::{Event, InputMode, KeyWithModifier, ModeInfo},
    input::actions::Action,
};

struct Configuration {
    runtime: Runtime,
    host: Arc<Mutex<Host>>,
}
impl Configuration {
    fn new(setup: bool) -> Self {
        let (runtime, host) = plugin_with_replies(
            "configuration",
            json!({"is_setup_wizard":setup.to_string()}),
            BTreeMap::new(),
        );
        Self { runtime, host }
    }
    fn event(&mut self, event: Value) -> bool {
        serde_json::from_value::<Event>(event.clone()).unwrap();
        self.runtime.update(event).unwrap()
    }
    fn key(&mut self, bare: Value, modifiers: &[&str]) -> bool {
        self.event(json!({"Key":{"bare_key":bare,"key_modifiers":modifiers}}))
    }
    fn press(&mut self, key: &str) -> bool {
        self.key(json!(key), &[])
    }
    fn space(&mut self) {
        self.key(json!({"Char":" "}), &[]);
    }
    fn ctrl(&mut self, key: &str) -> bool {
        self.key(json!({"Char":key}), &["Ctrl"])
    }
    fn mode(&mut self, locked: bool, bindings: Value) {
        let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
        mode["base_mode"] = json!(if locked { "Locked" } else { "Normal" });
        mode["keybinds"] = bindings;
        self.event(json!({"ModeUpdate":mode}));
    }
    fn render(&mut self, rows: usize, cols: usize) -> String {
        rendered_text(&self.runtime.render(rows, cols).unwrap())
    }
    fn commands(&self) -> Vec<Value> {
        self.host.lock().unwrap().commands.clone()
    }
    fn last(&self, name: &str) -> Value {
        self.commands()
            .into_iter()
            .rev()
            .find_map(|c| c.get(name).cloned())
            .unwrap()
    }
}
fn key(character: &str, modifiers: &[&str]) -> Value {
    json!({"bare_key":{"Char":character},"key_modifiers":modifiers})
}
fn switch(mode: &str) -> Value {
    json!({"SwitchToMode":{"input_mode":mode}})
}
fn initial_bindings() -> Value {
    json!([
        ["Locked",[[key("g",&["Ctrl"]),[switch("Normal")]], [key("n",&["Alt"]),[{"NewPane":{"direction":null,"pane_name":null,"start_suppressed":false}}]]]],
        ["Normal",[[key("p",&["Ctrl"]),[switch("Pane")]], [key("n",&["Alt"]),[{"NewPane":{"direction":null,"pane_name":null,"start_suppressed":false}}]]]]
    ])
}

#[test]
fn configuration_load_and_wizard_preserve_pane_setup_contract() {
    let normal = Configuration::new(false);
    assert!(normal
        .commands()
        .contains(&json!({"RenamePluginPane":[42,"Configuration"]})));
    assert!(!normal.commands().iter().any(|c| c.get("Resize").is_some()));
    let mut setup = Configuration::new(true);
    assert!(setup
        .commands()
        .contains(&json!({"RenamePluginPane":[42,"First Run Setup Wizard (Step 1/1)"]})));
    assert_eq!(
        setup
            .commands()
            .iter()
            .filter(|c| c.get("Resize").is_some())
            .count(),
        3
    );
    let text = setup.render(30, 100);
    assert!(text.contains("How would you like to interact with Yinz?"));
    assert!(!text.contains("<TAB>"));
    assert!(!setup.press("Tab"));
    setup.mode(false, initial_bindings());
    assert_eq!(setup.runtime.value("preset-selected").unwrap(), 0);
    setup.press("Down");
    setup.press("Enter");
    assert!(setup.last("Reconfigure")[1].as_bool().unwrap());
    assert!(setup
        .commands()
        .ends_with(&[json!({"SwitchToMode":"Locked"}), json!("CloseSelf")]));
}

#[test]
fn every_preset_matches_templates_and_parses_with_agent_selection_and_notifications() {
    let mut plugin = Configuration::new(false);
    // Keep the original presets pinned after removing the new group bindings.
    for (primary, secondary, locked, expected) in [
        ("Ctrl", "Alt", false, 15960040746295113056u64),
        ("Ctrl", "Alt", true, 12161722900292792986),
        ("", "Alt", false, 18290454268979490468),
        ("Ctrl", "", false, 15107899114023689823),
        ("", "", false, 4563540240599315174),
        ("Ctrl", "Ctrl", false, 6344144133553793149),
        ("Ctrl Shift", "Alt Super", false, 981793447407902638),
    ] {
        let source = plugin
            .runtime
            .invoke(
                if locked {
                    "configuration-unlock-first-keybinds"
                } else {
                    "configuration-default-keybinds"
                },
                vec![json!(primary), json!(secondary)],
            )
            .unwrap();
        let source = source.as_str().unwrap();
        let legacy = source
            .split_inclusive('\n')
            .filter(|line| !line.contains("TabGroup"))
            .collect::<String>()
            .replace("GoToTabInGroup", "GoToTab");
        let hash = legacy.bytes().fold(14695981039346656037u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(1099511628211)
        });
        assert_eq!(
            hash, expected,
            "primary={primary:?}, secondary={secondary:?}, locked={locked}"
        );
        let config = Config::from_kdl(source, None).unwrap();
        let saved = Config::from_kdl(&config.to_string(false).unwrap(), None).unwrap();
        let key = |s: &str| -> KeyWithModifier { s.parse().unwrap() };
        for config in [&config, &saved] {
            for mode in config.keybinds.0.keys().copied() {
                let actions = config
                    .keybinds
                    .get_actions_for_key_in_mode(&mode, &key(&format!("{secondary} a")));
                if secondary.is_empty() || matches!(mode, InputMode::Locked | InputMode::Select) {
                    assert!(
                        !actions.is_some_and(
                            |actions| actions.contains(&Action::ActivateLastNotification)
                        )
                    );
                } else {
                    assert_eq!(
                        actions,
                        Some(&vec![Action::ActivateLastNotification]),
                        "{mode:?}"
                    );
                }
            }
            assert_eq!(
                config.keybinds.0[&InputMode::Tab][&key("g")][0],
                Action::NewTabGroup
            );
            assert_eq!(
                config.keybinds.0[&InputMode::Tab][&key("H")],
                vec![Action::GoToPreviousTabGroup]
            );
            assert_eq!(
                config.keybinds.0[&InputMode::Tab][&key("L")],
                vec![Action::GoToNextTabGroup]
            );
            assert!(matches!(
                config.keybinds.0[&InputMode::Tab][&key("1")][0],
                Action::GoToTabInGroup { .. }
            ));
            assert_eq!(
                config.keybinds.0[&InputMode::Session][&key("n")],
                vec![Action::SwitchToMode {
                    input_mode: InputMode::Notifications
                }]
            );
            assert_eq!(
                config.keybinds.0[&InputMode::Notifications][&key("Enter")],
                vec![Action::ActivateNotification]
            );
            assert!(config.keybinds.0[&InputMode::Agent].contains_key(&key("n")));
            assert!(config.keybinds.0[&InputMode::Select].contains_key(&key("Enter")));
        }
    }
}

#[test]
fn modifiers_are_editable_dirty_state_survives_mode_updates_and_reset_discards_edits() {
    let mut plugin = Configuration::new(false);
    plugin.mode(false, initial_bindings());
    plugin.press("Down");
    plugin.space();
    assert_eq!(plugin.runtime.value("primary").unwrap(), json!([]));
    plugin.press("Right");
    plugin.space();
    plugin.press("Down");
    plugin.space();
    assert_eq!(plugin.runtime.value("secondary").unwrap(), json!(["Ctrl"]));
    plugin.mode(false, initial_bindings());
    assert_eq!(plugin.runtime.value("primary").unwrap(), json!([]));
    assert_eq!(plugin.runtime.value("secondary").unwrap(), json!(["Ctrl"]));
    plugin.ctrl("c");
    assert_eq!(plugin.runtime.value("primary").unwrap(), json!(["Ctrl"]));
    assert_eq!(plugin.runtime.value("secondary").unwrap(), json!(["Alt"]));
    assert!(!plugin.commands().contains(&json!("CloseSelf")));
}

#[test]
fn applying_leaders_rebinds_all_modes_preserves_escape_enter_and_unrelated_actions() {
    let mut plugin = Configuration::new(false);
    let mut bindings = initial_bindings().as_array().unwrap().clone();
    bindings.push(json!(["Pane",[
        [{"bare_key":"Enter","key_modifiers":[]},[switch("Normal")]],
        [{"bare_key":"Esc","key_modifiers":[]},[switch("Normal")]],
        [key("x",&["Ctrl"]),[switch("Normal")]],
        [key("z",&["Alt"]),["TogglePaneFrames"]]
    ]]));
    plugin.mode(false, json!(bindings));
    plugin.press("Down");
    plugin.space();
    plugin.press("Down");
    plugin.press("Down");
    plugin.space();
    plugin.press("Enter");
    let plan = plugin.last("RebindKeys");
    assert_eq!(plan["write_config_to_disk"], false);
    let unbind = plan["keys_to_unbind"].as_array().unwrap();
    assert!(unbind.contains(&json!(["Pane", key("x", &["Ctrl"])])));
    assert!(!unbind
        .iter()
        .any(|binding| binding[1]["bare_key"] == "Enter" || binding[1]["bare_key"] == "Esc"));
    assert!(!unbind.contains(&json!(["Pane", key("z", &["Alt"])])));
    let rebind = plan["keys_to_rebind"].as_array().unwrap();
    for mode in [
        "Normal",
        "Pane",
        "Tab",
        "Resize",
        "Move",
        "Search",
        "Scroll",
        "Session",
        "Agent",
        "Select",
        "Notifications",
    ] {
        assert!(rebind.contains(&json!([mode, key("q", &["Super"]), ["Quit"]])));
        assert!(rebind.contains(&json!([
            mode,
            key("e", &["Super"]),
            [switch(if mode == "Agent" { "Normal" } else { "Agent" })]
        ])));
    }
    assert!(rebind.contains(&json!(["Locked", key("g", &["Super"]), [switch("Normal")]])));
    assert!(plugin
        .render(30, 100)
        .contains("Configuration applied to current session."));
}

#[test]
fn secondary_modifier_changes_move_notification_shortcut_and_preserve_exclusions() {
    for locked in [false, true] {
        for secondary in [&["Super"][..], &["Shift", "Super"][..], &[][..]] {
            let mut plugin = Configuration::new(false);
            let source = plugin
                .runtime
                .invoke(
                    if locked {
                        "configuration-unlock-first-keybinds"
                    } else {
                        "configuration-default-keybinds"
                    },
                    vec![json!("Ctrl"), json!("Alt")],
                )
                .unwrap();
            let config = Config::from_kdl(source.as_str().unwrap(), None).unwrap();
            let bindings: Vec<_> = config
                .keybinds
                .0
                .iter()
                .map(|(mode, bindings)| (mode, bindings.iter().collect::<Vec<_>>()))
                .collect();
            plugin.mode(locked, json!(bindings));
            plugin.press("Down");
            plugin.press("Right");
            plugin.press("Down");
            plugin.space(); // Remove Alt.
            if !secondary.is_empty() {
                plugin.press("Down");
                plugin.space(); // Add Super.
                if secondary.contains(&"Shift") {
                    plugin.press("Down");
                    plugin.space();
                }
            }
            assert_eq!(plugin.runtime.value("secondary").unwrap(), json!(secondary));
            plugin.press("Enter");
            let plan = plugin.last("RebindKeys");
            let unbind = plan["keys_to_unbind"].as_array().unwrap();
            let rebind = plan["keys_to_rebind"].as_array().unwrap();
            let notification_bindings: Vec<_> = rebind
                .iter()
                .filter(|binding| binding[2] == json!(["ActivateLastNotification"]))
                .collect();
            assert_eq!(
                notification_bindings.len(),
                if secondary.is_empty() { 0 } else { 15 }
            );
            for mode in config.keybinds.0.keys() {
                let excluded = matches!(mode, InputMode::Locked | InputMode::Select);
                assert_eq!(
                    unbind.contains(&json!([mode, key("a", &["Alt"])])),
                    !excluded,
                    "mode={mode:?}, secondary={secondary:?}, locked={locked}"
                );
                assert_eq!(
                    notification_bindings.contains(&&json!([
                        mode,
                        key("a", secondary),
                        ["ActivateLastNotification"]
                    ])),
                    !excluded && !secondary.is_empty(),
                    "mode={mode:?}, secondary={secondary:?}, locked={locked}"
                );
            }
        }
    }
}

#[test]
fn locked_preset_captures_unlock_key_and_updates_all_modes_without_changing_primary_actions() {
    let mut plugin = Configuration::new(false);
    plugin.mode(true, initial_bindings());
    assert!(plugin.render(30, 100).contains("Non-Colliding preset"));
    plugin.press("Down");
    plugin.space();
    assert!(plugin.render(30, 100).contains("[Enter new key]"));
    plugin.key(json!("Esc"), &["Alt"]);
    let unlock = json!({"bare_key":"Esc","key_modifiers":["Alt"]});
    assert_eq!(plugin.runtime.value("main-leader").unwrap(), unlock);
    plugin.mode(true, initial_bindings());
    assert_eq!(plugin.runtime.value("main-leader").unwrap(), unlock);
    plugin.ctrl("a");
    let plan = plugin.last("RebindKeys");
    assert_eq!(plan["write_config_to_disk"], true);
    let rebind = plan["keys_to_rebind"].as_array().unwrap();
    assert_eq!(rebind.len(), 61);
    for mode in [
        "Locked",
        "Normal",
        "Pane",
        "Tab",
        "Resize",
        "Move",
        "Search",
        "Scroll",
        "Session",
        "Agent",
        "Select",
        "Notifications",
    ] {
        assert!(rebind.contains(&json!([
            mode,
            unlock,
            [switch(if mode == "Locked" { "Normal" } else { "Locked" })]
        ])));
    }
    assert!(!rebind.iter().any(|binding| binding[2] == json!(["Quit"])));
}

#[test]
fn unlock_navigation_and_capture_escape_return_without_closing() {
    let mut plugin = Configuration::new(false);
    plugin.mode(true, initial_bindings());
    plugin.press("Left");
    assert_eq!(plugin.runtime.value("focus").unwrap(), "unlock");
    plugin.press("Right");
    plugin.press("Down");
    plugin.press("Down");
    plugin.press("Down");
    plugin.press("Down");
    assert_eq!(plugin.runtime.value("focus").unwrap(), "unlock");
    plugin.space();
    plugin.press("Esc");
    assert!(!plugin.commands().contains(&json!("CloseSelf")));
    plugin.press("Esc");
    assert_eq!(plugin.commands().last(), Some(&json!("CloseSelf")));
}

#[test]
fn preset_navigation_applies_session_or_saved_config_in_order_and_consumes_notifications() {
    let mut plugin = Configuration::new(false);
    plugin.press("Tab");
    assert!(plugin.render(30, 100).contains("Override keybindings"));
    plugin.press("Enter");
    assert!(!plugin
        .commands()
        .iter()
        .any(|c| c.get("Reconfigure").is_some()));
    plugin.press("Enter");
    assert_eq!(plugin.last("Reconfigure")[1], false);
    assert_eq!(
        plugin.commands().last(),
        Some(&json!({"SwitchToMode":"Normal"}))
    );
    plugin.press("Esc");
    assert!(!plugin.commands().contains(&json!("CloseSelf")));
    plugin.press("Down");
    plugin.press("Down");
    plugin.ctrl("a");
    assert_eq!(plugin.last("Reconfigure")[1], true);
    assert_eq!(
        plugin.commands().last(),
        Some(&json!({"SwitchToMode":"Locked"}))
    );
    plugin.press("Esc");
    plugin.press("Esc");
    assert_eq!(plugin.commands().last(), Some(&json!("CloseSelf")));
}

#[test]
fn write_failures_have_priority_and_first_key_only_dismisses_them() {
    let mut plugin = Configuration::new(false);
    plugin.event(json!({"FailedToWriteConfigToDisk":{"file_path":"/config.kdl","reason":"read-only filesystem"}}));
    assert!(plugin
        .render(30, 120)
        .contains("Failed to write configuration file: /config.kdl read-only filesystem"));
    plugin.press("Enter");
    assert!(!plugin
        .commands()
        .iter()
        .any(|c| c.get("RebindKeys").is_some()));
    assert!(!plugin.render(30, 120).contains("Failed to write"));
}

#[test]
fn warnings_and_all_screen_sizes_render_without_invalid_styles_or_offscreen_coordinates() {
    let mut plugin = Configuration::new(false);
    plugin.mode(false, initial_bindings());
    plugin.press("Down");
    plugin.press("Down");
    plugin.press("Down");
    plugin.space();
    assert!(plugin
        .render(30, 100)
        .contains("requires supporting terminal"));
    for setup in [false, true] {
        let mut plugin = Configuration::new(setup);
        for screen in 0..2 {
            if screen == 1 {
                plugin.press("Tab");
            }
            for rows in [0, 1, 5, 15, 30] {
                for cols in [0, 1, 20, 35, 62, 100] {
                    let output = plugin.runtime.render(rows, cols).unwrap();
                    if rows == 0 || cols == 0 {
                        assert!(output.is_empty());
                    }
                }
            }
        }
    }
}

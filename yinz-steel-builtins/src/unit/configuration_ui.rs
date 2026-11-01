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
    geometry::{Direction, Resize},
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
        .contains(&json!({"RenamePluginPane":[42,"First Run Setup Wizard (Step 1/2)"]})));
    assert_eq!(
        setup
            .commands()
            .iter()
            .filter(|c| c.get("Resize").is_some())
            .count(),
        0
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
    assert_eq!(setup.runtime.value("screen").unwrap(), "project");
    assert!(setup.render(30, 100).contains("Choose your first project"));
    assert!(setup.commands().contains(&json!({"SwitchToMode":"Locked"})));
    assert!(!setup.commands().contains(&json!("CloseSelf")));
    for rows in [0, 1, 5, 15, 30] {
        for cols in [0, 1, 20, 35, 100] {
            setup.render(rows, cols);
        }
    }
}

fn project_setup() -> Configuration {
    project_setup_at("/work/repo")
}

fn project_setup_at(cwd: &str) -> Configuration {
    let mut plugin = Configuration::new(true);
    plugin.host.lock().unwrap().replies.extend([
        (
            "GetPluginIds".into(),
            json!({"plugin_id":42,"client_id":1,"initial_cwd":cwd}),
        ),
        (
            "DataFetch".into(),
            json!({"columns":[],"rows":[],"truncated":false,"next_cursor":null}),
        ),
        ("OpenTerminalNearPlugin".into(), json!({"Terminal":7})),
    ]);
    plugin.press("Enter");
    plugin.event(json!({"HostFolderChanged":"/"}));
    plugin
}

fn submit_project(plugin: &mut Configuration) -> Value {
    plugin.press("Tab");
    plugin.press("Tab");
    plugin.press("Enter");
    plugin.last("RunCommand")[3].clone()
}

#[test]
fn configuration_project_picker_preserves_input_on_cancel_and_opens_selected_directory() {
    let mut plugin = project_setup();
    assert!(plugin.render(30, 100).contains("Ctrl-f"));
    plugin.ctrl("f");
    let pick = plugin.last("Pick")["Start"].clone();
    assert_eq!(
        pick["options"],
        json!({"kind":"directory","roots":["/work/repo"]})
    );
    plugin.event(json!({"PickResult":{"request_id":pick["request_id"],"result":{"Ok":null}}}));
    assert!(plugin.render(30, 100).contains("/work/repo"));
    plugin.ctrl("f");
    let pick = plugin.last("Pick")["Start"].clone();
    // A stale reply cannot overwrite the current request.
    assert!(!plugin.event(json!({"PickResult":{"request_id":"stale","result":{"Ok":"/wrong"}}})));
    plugin.event(
        json!({"PickResult":{"request_id":pick["request_id"],"result":{"Ok":"/chosen folder"}}}),
    );
    let context = submit_project(&mut plugin);
    let command = plugin.last("RunCommand");
    assert_eq!(
        command[0].as_array().unwrap().last().unwrap(),
        "/chosen folder"
    );
    assert!(!plugin.commands().contains(&json!("CloseSelf")));
    plugin.event(json!({"RunCommandResult":[0,b"/chosen folder\n".to_vec(),[],context]}));
    assert!(!plugin.last("DataApply")["mutations"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        plugin.last("OpenTerminalNearPlugin")["path"],
        "/host/chosen folder"
    );
    assert!(plugin.commands().ends_with(&[
        json!("CloseSelf"),
        json!({"FocusTerminalPane":[7,false,false]})
    ]));
}

#[test]
fn configuration_project_browse_and_submission_use_the_initial_working_directory() {
    let mut plugin = project_setup_at("/work");
    // Edit through real form events: replace the initial absolute path with a relative draft.
    plugin.press("Enter");
    plugin.ctrl("u");
    plugin.event(json!({"PastedText":"repo"}));
    plugin.ctrl("f");
    let pick = plugin.last("Pick")["Start"].clone();
    assert_eq!(
        pick["options"],
        json!({"kind":"directory","roots":["/work/repo"]})
    );
    plugin.event(json!({"PickResult":{"request_id":pick["request_id"],"result":{"Ok":null}}}));
    submit_project(&mut plugin);
    let command = plugin.last("RunCommand");
    let args = command[0].as_array().unwrap();
    assert_eq!(&args[args.len() - 2..], &[json!("/work"), json!("repo")]);
}

#[test]
fn configuration_project_validation_and_write_failures_leave_wizard_open() {
    let mut plugin = project_setup();
    let context = submit_project(&mut plugin);
    plugin.event(json!({"RunCommandResult":[1,[],b"not a directory".to_vec(),context]}));
    assert!(plugin
        .render(30, 100)
        .contains("Could not open that folder"));
    assert!(!plugin
        .commands()
        .iter()
        .any(|c| c.get("DataApply").is_some()));
    plugin
        .host
        .lock()
        .unwrap()
        .failures
        .insert("DataApply".into(), "data conflict: project changed".into());
    plugin.press("Enter");
    let context = plugin.last("RunCommand")[3].clone();
    plugin.event(json!({"RunCommandResult":[0,b"/work/repo\n".to_vec(),[],context]}));
    assert!(plugin.render(30, 100).contains("data conflict"));
    assert!(!plugin.commands().contains(&json!("CloseSelf")));
    assert!(!plugin
        .commands()
        .iter()
        .any(|c| c.get("OpenTerminalNearPlugin").is_some()));
}

#[test]
fn configuration_project_skip_cancels_picker_without_registering_a_project() {
    let mut plugin = project_setup();
    plugin.ctrl("f");
    let request = plugin.last("Pick")["Start"]["request_id"].clone();
    plugin.event(json!("BeforeClose"));
    assert_eq!(
        plugin.last("Pick"),
        json!({"Cancel":{"request_id":request,"timeout":false}})
    );
    assert!(!plugin
        .commands()
        .iter()
        .any(|c| c.get("DataApply").is_some()));
}

#[test]
fn configuration_project_probe_validates_directories_without_interpreting_path_text() {
    let mut plugin = project_setup();
    submit_project(&mut plugin);
    let args = plugin.last("RunCommand")[0].as_array().unwrap().clone();
    let probe = |path: &std::path::Path| {
        std::process::Command::new(args[0].as_str().unwrap())
            .args(args[1..args.len() - 2].iter().map(|s| s.as_str().unwrap()))
            .arg("/")
            .arg(path)
            .output()
            .unwrap()
    };
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("repo 'quoted' $(false) ");
    std::fs::create_dir(&folder).unwrap();
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&folder, &alias).unwrap();
    let output = probe(&alias);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", folder.canonicalize().unwrap().display())
    );
    let file = temp.path().join("file");
    std::fs::write(&file, "").unwrap();
    assert!(!probe(&file).status.success());
    assert!(!probe(&temp.path().join("missing")).status.success());
}

#[test]
fn configuration_project_name_collision_does_not_replace_an_existing_folder() {
    let mut plugin = project_setup();
    plugin.host.lock().unwrap().replies.insert(
        "DataFetch".into(),
        json!({
            "columns":["id","rev","name","root"],
            "rows":[[{"Int":1},{"Int":4},{"Text":"repo"},{"Text":"/other/repo"}]],
            "truncated":false,"next_cursor":null
        }),
    );
    let context = submit_project(&mut plugin);
    plugin.event(json!({"RunCommandResult":[0,b"/work/repo\n".to_vec(),[],context]}));
    assert!(plugin
        .render(30, 100)
        .contains("already uses this project name"));
    assert!(!plugin
        .commands()
        .iter()
        .any(|c| c.get("DataApply").is_some()));
}

#[test]
fn configuration_project_submission_waits_for_host_folder_preparation() {
    let mut plugin = Configuration::new(true);
    plugin.press("Enter");
    plugin.press("Tab");
    plugin.press("Tab");
    plugin.press("Enter");
    assert!(!plugin
        .commands()
        .iter()
        .any(|c| c.get("RunCommand").is_some()));
    assert!(plugin.render(30, 100).contains("Preparing local folders"));
    plugin.event(json!({"HostFolderChanged":"/"}));
    assert_eq!(plugin.last("RunCommand")[2], "/host");
    plugin.press("Esc");
    let context = plugin.last("RunCommand")[3].clone();
    plugin.event(json!({"RunCommandResult":[0,b"/work/repo\n".to_vec(),[],context]}));
    assert!(!plugin
        .commands()
        .iter()
        .any(|c| c.get("DataApply").is_some()));
}

#[test]
fn every_preset_parses_with_palette_agent_selection_and_notifications() {
    let mut plugin = Configuration::new(false);
    for (primary, secondary, locked) in [
        ("Ctrl", "Alt", false),
        ("Ctrl", "Alt", true),
        ("", "Alt", false),
        ("Ctrl", "", false),
        ("", "", false),
        ("Ctrl", "Ctrl", false),
        ("Ctrl Shift", "Alt Super", false),
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
        for plugin in ["session-manager", "layout-manager", "plugin-manager"] {
            assert!(!source.contains(plugin));
        }
        let config = Config::from_kdl(source, None).unwrap();
        let saved = Config::from_kdl(&config.to_string(false).unwrap(), None).unwrap();
        let key = |s: &str| -> KeyWithModifier { s.parse().unwrap() };
        for config in [&config, &saved] {
            for binding in ["w", "p", "l"] {
                assert_eq!(
                    config.keybinds.0[&InputMode::Session][&key(binding)],
                    vec![
                        Action::SwitchToMode {
                            input_mode: if locked {
                                InputMode::Locked
                            } else {
                                InputMode::Normal
                            }
                        },
                        Action::ToggleCommandPalette
                    ],
                    "primary={primary:?}, secondary={secondary:?}, locked={locked}"
                );
            }
            for (binding, action) in [
                ("{", Action::MovePaneToPreviousTabGroup),
                ("}", Action::MovePaneToNextTabGroup),
            ] {
                assert_eq!(
                    config.keybinds.0[&InputMode::Tab][&key(binding)],
                    vec![
                        action,
                        Action::SwitchToMode {
                            input_mode: InputMode::Normal
                        }
                    ]
                );
            }
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
            assert_eq!(
                config.keybinds.0[&InputMode::Agent][&key("a")],
                vec![
                    Action::SwitchToMode {
                        input_mode: config.options.default_mode.unwrap_or_default()
                    },
                    Action::OpenAgents
                ]
            );
            assert!(config.keybinds.0[&InputMode::Select].contains_key(&key("Enter")));
        }
    }
}

#[test]
fn group_shortcuts_follow_secondary_navigation_and_round_trip() {
    let mut plugin = Configuration::new(false);
    for (primary, secondary) in [
        ("Ctrl", "Alt"),
        ("", "Alt"),
        ("Ctrl", "Ctrl"),
        ("Ctrl Shift", "Alt Super"),
        ("Ctrl", "Alt Shift"),
        ("Ctrl", "Shift"),
        ("", "Alt Shift"),
        ("Alt Shift", "Alt Shift"),
        ("Ctrl", ""),
        ("", ""),
    ] {
        for locked in [false, true] {
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
            let config = Config::from_kdl(source.as_str().unwrap(), None).unwrap();
            let saved = Config::from_kdl(&config.to_string(false).unwrap(), None).unwrap();
            for config in [&config, &saved] {
                for (letter, direction, action) in [
                    ("h", Direction::Left, Action::GoToPreviousTabGroup),
                    ("l", Direction::Right, Action::GoToNextTabGroup),
                ] {
                    let shortcut: KeyWithModifier =
                        format!("{secondary} Shift {letter}").parse().unwrap();
                    for (mode, bindings) in &config.keybinds.0 {
                        let navigation_available = if locked {
                            matches!(mode, InputMode::Normal | InputMode::Locked)
                        } else {
                            !matches!(mode, InputMode::Locked | InputMode::Select)
                        };
                        let available = if secondary.is_empty() {
                            *mode == InputMode::Tab
                        } else {
                            navigation_available
                                && !secondary.split_whitespace().any(|m| m == "Shift")
                        };
                        if !secondary.is_empty() && navigation_available {
                            let navigation: KeyWithModifier =
                                format!("{secondary} {letter}").parse().unwrap();
                            let expected = if secondary == "Shift" && *mode == InputMode::Resize {
                                Action::Resize {
                                    resize: Resize::Decrease,
                                    direction: Some(direction),
                                }
                            } else {
                                Action::MoveFocusOrTab { direction }
                            };
                            assert_eq!(
                                bindings.get(&navigation).and_then(|actions| actions.first()),
                                Some(&expected),
                                "primary={primary:?}, secondary={secondary:?}, locked={locked}, mode={mode:?}",
                            );
                        }
                        let group_bindings: Vec<_> = bindings
                            .iter()
                            .filter(|(_, actions)| actions.contains(&action))
                            .collect();
                        let expected = vec![action.clone()];
                        assert_eq!(
                            group_bindings,
                            if available { vec![(&shortcut, &expected)] } else { vec![] },
                            "primary={primary:?}, secondary={secondary:?}, locked={locked}, mode={mode:?}",
                        );
                    }
                    if !secondary.is_empty() && secondary != "Shift" {
                        let old: KeyWithModifier = letter.to_uppercase().parse().unwrap();
                        assert!(!config.keybinds.0[&InputMode::Tab].contains_key(&old));
                    }
                }
            }
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

fn assert_wizard_bounds(output: &str, cols: usize, rows: usize) {
    if cols == 0 || rows == 0 {
        assert!(output.is_empty());
        return;
    }
    let mut occupied = std::collections::HashSet::new();
    for component in output.split("\x1bPy").skip(1) {
        let coordinates: Vec<_> = component.split(';').nth(1).unwrap().split('/').collect();
        let x: usize = coordinates[0].parse().unwrap();
        let y: usize = coordinates[1].parse().unwrap();
        let width: usize = coordinates[2].parse().unwrap_or(0);
        assert!(
            x < cols && x + width <= cols && y < rows,
            "{coordinates:?} in {cols}x{rows}"
        );
        assert!(occupied.insert(y), "overlap on row {y}");
    }
}

#[test]
fn setup_presets_wrap_scroll_and_preserve_selection_across_resizes() {
    let mut plugin = Configuration::new(true);
    for (cols, rows) in [
        (120, 40),
        (80, 24),
        (60, 20),
        (40, 12),
        (20, 8),
        (1, 1),
        (0, 8),
        (20, 0),
        (120, 40),
    ] {
        for (selected, title) in [(0, "1. Default"), (1, "2. Unlock First")] {
            if selected == 1 {
                plugin.press("Down");
            }
            let output = plugin.runtime.render(rows, cols).unwrap();
            assert_wizard_bounds(&output, cols, rows);
            if cols >= 20 && rows > 0 {
                assert!(rendered_text(&output).contains(title));
            }
            assert_eq!(plugin.runtime.value("preset-selected").unwrap(), selected);
            if selected == 1 {
                plugin.press("Up");
            }
        }
    }
    plugin.press("Down");
    plugin.press("Enter");
    assert!(plugin
        .commands()
        .contains(&json!({"SwitchToMode":"Locked"})));
}

#[test]
fn setup_project_resizes_preserve_drafts_validation_and_pending_picker() {
    let path = "/long/目录/with a project folder";
    let mut plugin = project_setup_at(path);
    plugin.press("Tab");
    plugin.event(json!({"PastedText":"draft project"}));
    plugin.press("Tab");
    plugin.press("Tab");
    plugin.ctrl("f");
    let pick = plugin.last("Pick")["Start"].clone();
    let picks_before = plugin
        .commands()
        .iter()
        .filter(|c| c.get("Pick").is_some())
        .count();
    for (cols, rows) in [
        (120, 40),
        (80, 24),
        (60, 20),
        (40, 12),
        (20, 8),
        (1, 1),
        (0, 8),
        (20, 0),
        (120, 40),
    ] {
        let output = plugin.runtime.render(rows, cols).unwrap();
        assert_wizard_bounds(&output, cols, rows);
    }
    assert_eq!(
        picks_before,
        plugin
            .commands()
            .iter()
            .filter(|c| c.get("Pick").is_some())
            .count()
    );
    plugin.event(json!({"PickResult":{"request_id":pick["request_id"],"result":{"Ok":null}}}));
    let text = plugin.render(40, 120);
    assert!(
        text.contains(path) && text.contains("draft project"),
        "{text}"
    );
    let context = submit_project(&mut plugin);
    plugin.event(json!({"RunCommandResult":[1,[],[],context]}));
    for (cols, rows) in [(20, 8), (40, 12), (120, 40)] {
        let output = plugin.runtime.render(rows, cols).unwrap();
        assert_wizard_bounds(&output, cols, rows);
        if cols == 120 {
            let text = rendered_text(&output);
            assert!(text.contains("Could not open that folder"));
            assert!(text.contains(path) && text.contains("draft project"));
        }
    }
}

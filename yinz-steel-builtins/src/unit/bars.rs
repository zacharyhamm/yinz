use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde_json::{json, Value};
use yinz_steel::Runtime;
use yinz_types::{
    data::{Event, ModeInfo, TabInfo},
    internal_ids::{ClientId, TabPosition},
    multiplayer::ColorSlot,
};

use crate::tests::{plugin_with_replies, rendered_text, Host};

fn tab(position: usize, name: &str, active: bool) -> TabInfo {
    TabInfo {
        position: TabPosition::new(position),
        name: name.to_owned(),
        active,
        ..Default::default()
    }
}
fn event(runtime: &mut Runtime, event: Event) -> bool {
    runtime
        .update(serde_json::to_value(event).unwrap())
        .unwrap()
}
fn wire(runtime: &mut Runtime, value: Value) -> bool {
    event(runtime, serde_json::from_value(value).unwrap())
}
fn plugin() -> (Runtime, Arc<Mutex<Host>>) {
    plugin_with_replies(
        "tab-bar",
        json!({}),
        BTreeMap::from([("GetPluginIds".to_owned(), json!({"hostname":"host"}))]),
    )
}

#[test]
fn tab_bar_starts_empty_then_handles_hints_frames_and_input() {
    let (mut runtime, host) = plugin();
    assert_eq!(runtime.render(1, 80).unwrap(), "");
    assert!(host
        .lock()
        .unwrap()
        .commands
        .contains(&json!({"SetSelectable":false})));
    assert!(event(
        &mut runtime,
        Event::TabUpdate(vec![tab(0, "main", true)])
    ));
    let hint = json!({"8":{"text":"hint-one","indices":[[0,1000],[],[],[],[2]]}});
    wire(&mut runtime, json!({"HintText":hint}));
    assert!(rendered_text(&runtime.render(1, 80).unwrap()).contains("hint-one"));
    wire(
        &mut runtime,
        json!({"HintText":{"8":{"text":"hint-two","indices":[]}}}),
    );
    wire(&mut runtime, json!({"Timer":5}));
    assert!(rendered_text(&runtime.render(1, 80).unwrap()).contains("hint-two"));
    wire(&mut runtime, json!({"Timer":5}));
    assert!(!rendered_text(&runtime.render(1, 80).unwrap()).contains("hint-two"));
    wire(&mut runtime, json!({"HintText":hint}));
    let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
    mode["pane_frame_style"] = json!("full");
    wire(&mut runtime, json!({"ModeUpdate":mode}));
    assert!(!rendered_text(&runtime.render(1, 80).unwrap()).contains("hint-one"));
    mode["pane_frame_style"] = json!("titles");
    wire(&mut runtime, json!({"ModeUpdate":mode}));
    assert!(rendered_text(&runtime.render(1, 80).unwrap()).contains("hint-one"));
    assert!(event(&mut runtime, Event::InputReceived));
    assert!(!rendered_text(&runtime.render(1, 80).unwrap()).contains("hint-one"));
    assert!(!event(&mut runtime, Event::InputReceived));
}

#[test]
fn shared_bar_defaults_key_normalization_and_selection_match_wire_types() {
    let (mut runtime, _) = plugin();
    assert_eq!(
        runtime.invoke("bar-default-mode", vec![]).unwrap(),
        serde_json::to_value(ModeInfo::default()).unwrap()
    );
    let capital = json!({"bare_key":{"Char":"A"},"key_modifiers":[]});
    let shifted = json!({"bare_key":{"Char":"a"},"key_modifiers":["Shift"]});
    assert_eq!(
        runtime
            .invoke("bar-equal?", vec![capital, shifted])
            .unwrap(),
        json!(true)
    );
    assert_eq!(
        runtime
            .invoke(
                "bar-key-label",
                vec![json!({"bare_key":"Left","key_modifiers":["Shift","Ctrl"]})]
            )
            .unwrap(),
        json!("Ctrl Shift ←")
    );
    assert_eq!(
        runtime
            .invoke("bar-key-separator", vec![json!(["←", "→"])])
            .unwrap(),
        json!("")
    );
    assert_eq!(
        runtime
            .invoke("bar-key-separator", vec![json!(["a", "b"])])
            .unwrap(),
        json!("|")
    );
    let keys = json!([
        {"bare_key":{"Char":"a"},"key_modifiers":["Alt"]},
        {"bare_key":"Right","key_modifiers":[]},
        {"bare_key":"Left","key_modifiers":[]}
    ]);
    assert_eq!(
        runtime.invoke("bar-own-key", vec![keys]).unwrap(),
        json!([{ "bare_key":"Left", "key_modifiers":[] }])
    );
    let binding = json!({"bare_key":{"Char":"a"},"key_modifiers":["Alt"]});
    let map = json!([
        [binding,[{"LaunchPlugin":{"plugin":{"Alias":{"name":"agents"}}}}]],
        [binding,[{"KeybindPipe":{"name":"agent_strip_reposition"}}]]
    ]);
    assert_eq!(
        runtime
            .invoke("bar-plugin-keys", vec![map.clone(), json!("agents")])
            .unwrap(),
        json!([binding])
    );
    assert_eq!(
        runtime
            .invoke("bar-pipe-keys", vec![map, json!("agent_strip_reposition")])
            .unwrap(),
        json!([binding])
    );
}

#[test]
fn only_the_first_tab_below_a_visible_group_row_has_a_flat_left_edge() {
    let (mut runtime, _) = plugin();
    for simple in [false, true] {
        let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
        mode["capabilities"]["arrow_fonts"] = json!(simple);
        for (role, color) in [
            ("text_unselected", 1),
            ("ribbon_unselected", 2),
            ("ribbon_selected", 3),
        ] {
            mode["style"]["colors"][role]["background"] = json!({"EightBit":color});
        }
        for active in [0, 1] {
            for multiplayer in [false, true] {
                let mut tabs = vec![tab(0, "main", active == 0), tab(1, "other", active == 1)];
                let mut hidden = tab(2, "hidden", false);
                hidden.group = Some(yinz_types::tab_bar::TabGroupInfo {
                    id: 1,
                    name: "Other".into(),
                });
                tabs.push(hidden);
                if multiplayer {
                    for tab in &mut tabs {
                        tab.other_focused_clients = vec![ClientId::new(1)];
                    }
                }
                for (groups_enabled, multiple_groups, rows) in [
                    (true, true, 1),
                    (true, true, 2),
                    (true, false, 1),
                    (true, false, 2),
                    (false, true, 1),
                    (false, true, 2),
                ] {
                    mode["tab_groups"] = json!(groups_enabled);
                    wire(&mut runtime, json!({"ModeUpdate":mode.clone()}));
                    let mut visible_tabs = tabs.clone();
                    if !multiple_groups {
                        visible_tabs.truncate(2);
                    }
                    event(&mut runtime, Event::TabUpdate(visible_tabs));
                    runtime.render(rows, 120).unwrap();
                    let group_row_visible = groups_enabled && multiple_groups && rows == 2;
                    assert_eq!(
                        runtime.value("group-row-visible").unwrap(),
                        json!(group_row_visible)
                    );
                    let parts = runtime.value("parts").unwrap();
                    for part in parts.as_array().unwrap() {
                        let text = part["part"].as_str().unwrap();
                        if let Some(position) = part["tab"].as_u64() {
                            let bg = if position == active { 3 } else { 2 };
                            if simple {
                                assert!(!text.contains(''), "{text:?}");
                            } else {
                                let fg = if position == 0 && group_row_visible {
                                    bg
                                } else {
                                    1
                                };
                                assert!(
                                    text.starts_with(&format!("\u{1b}[38;5;{fg};48;5;{bg}m")),
                                    "{text:?}"
                                );
                                assert!(
                                    text.ends_with(&format!("\u{1b}[38;5;{bg};48;5;1m\u{1b}[0m")),
                                    "{text:?}"
                                );
                            }
                            let title_len = tabs[position as usize].name.len();
                            assert_eq!(
                                part["len"],
                                json!(
                                    title_len
                                        + 2
                                        + if simple { 0 } else { 2 }
                                        + if multiplayer { 3 } else { 0 }
                                )
                            );
                        } else if !simple && text.contains(" + ") {
                            assert!(text.starts_with("\u{1b}[38;5;1;48;5;2m"), "{text:?}");
                        }
                    }
                    if group_row_visible && !simple {
                        let groups = runtime.value("group-parts").unwrap();
                        let group = groups
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|part| part["tab"] == 0)
                            .unwrap();
                        assert!(group["part"]
                            .as_str()
                            .unwrap()
                            .starts_with("\u{1b}[38;5;1;48;5;3m"));
                    }
                }
                for columns in [0, 1, 10, 20] {
                    runtime.render(1, columns).unwrap();
                }
            }
        }
    }
}

#[test]
fn tab_bar_mouse_targets_visible_tabs_button_and_breadcrumb() {
    let (mut runtime, host) = plugin();
    let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
    mode["capabilities"]["arrow_fonts"] = json!(false);
    mode["session_name"] = json!("child");
    mode["session_ancestry"] = json!(["parent"]);
    mode["host_fullscreen"] = json!(true);
    wire(&mut runtime, json!({"ModeUpdate":mode}));
    event(
        &mut runtime,
        Event::TabUpdate(vec![tab(0, "main", true), tab(1, "other", false)]),
    );
    runtime.render(1, 120).unwrap();
    let parts = runtime.value("parts").unwrap();
    let mut offset = 0;
    let mut other = None;
    for part in parts.as_array().unwrap() {
        if part["tab"] == 1 {
            other = Some(offset);
        }
        offset += part["len"].as_u64().unwrap();
    }
    let col = other.unwrap();
    assert!(wire(&mut runtime, json!({"Mouse":{"Hover":[0,col]}})));
    assert!(!wire(&mut runtime, json!({"Mouse":{"Hover":[0,col]}})));
    wire(&mut runtime, json!({"Mouse":{"LeftClick":[0,col]}}));
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"SwitchTabTo":2}))
    );
    let button = runtime.value("new-tab-range").unwrap()[0].clone();
    wire(&mut runtime, json!({"Mouse":{"LeftClick":[0,button]}}));
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"NewTab":{"name":null,"cwd":null}}))
    );
    let breadcrumb = runtime.value("breadcrumb-range").unwrap()[0].clone();
    wire(&mut runtime, json!({"Mouse":{"LeftClick":[0,breadcrumb]}}));
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!("FocusHostSession"))
    );
    wire(&mut runtime, json!({"Mouse":{"ScrollUp":1}}));
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"SwitchTabTo":2}))
    );
    wire(&mut runtime, json!({"Mouse":{"ScrollDown":1}}));
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"SwitchTabTo":1}))
    );
}

#[test]
fn tab_bar_displays_names_up_to_45_columns() {
    let (mut runtime, _) = plugin();
    let name = "a".repeat(45);
    event(&mut runtime, Event::TabUpdate(vec![tab(0, &name, true)]));
    let frame = runtime.render(1, 200).unwrap();
    assert!(frame.contains(&name));
    assert!(!frame.contains('…'));

    let longer_name = "a".repeat(46);
    event(
        &mut runtime,
        Event::TabUpdate(vec![tab(0, &longer_name, true)]),
    );
    let frame = runtime.render(1, 200).unwrap();
    assert!(frame.contains(&format!("{}…", "a".repeat(44))));
    assert!(!frame.contains(&name));
}

#[test]
fn tab_bar_fits_active_tab_unicode_flags_and_collapsed_neighbors() {
    let (mut runtime, _) = plugin();
    let mut tabs = (0..12)
        .map(|i| tab(i, &format!("long-tab-{i}-界界界界界界界界界"), i == 6))
        .collect::<Vec<_>>();
    tabs[6].is_fullscreen_active = true;
    tabs[6].has_bell_notification = true;
    tabs[6].other_focused_clients = serde_json::from_value(json!([1, 10, 11])).unwrap();
    assert!(event(&mut runtime, Event::TabUpdate(tabs.clone())));
    assert!(!event(&mut runtime, Event::TabUpdate(tabs.clone())));
    assert!(!event(&mut runtime, Event::TabUpdate(vec![])));
    let frame = runtime.render(1, 100).unwrap();
    assert!(frame.contains("(FULLSCREEN) [!]"));
    assert!(frame.contains('…'));
    let parts = runtime.value("parts").unwrap();
    assert!(parts
        .as_array()
        .unwrap()
        .iter()
        .any(|part| part["tab"] == 6));
    assert!(parts
        .as_array()
        .unwrap()
        .iter()
        .any(|part| part["part"].as_str().unwrap().contains("← +")));
    assert!(parts
        .as_array()
        .unwrap()
        .iter()
        .any(|part| part["part"].as_str().unwrap().contains("→")));
    for cols in [0, 1, 8, 20, 200] {
        runtime.render(1, cols).unwrap();
    }
}

#[test]
fn tab_bar_colors_other_clients_by_their_slot_or_by_id_from_an_older_host() {
    let (mut runtime, _) = plugin();
    let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
    for (player, rgb) in [("player_3", [3, 3, 3]), ("player_10", [10, 10, 10])] {
        mode["style"]["colors"]["multiplayer_user_colors"][player] = json!({ "Rgb": rgb });
    }
    wire(&mut runtime, json!({ "ModeUpdate": mode }));
    let cursor = |n: u8| format!("48;2;{n};{n};{n}m");
    let mut focused = tab(0, "main", true);
    focused.other_focused_clients = [40, 41, 42].into_iter().map(ClientId::new).collect();
    focused.other_focused_client_slots = vec![ColorSlot::new(3), None, ColorSlot::new(10)];
    event(&mut runtime, Event::TabUpdate(vec![focused.clone()]));
    let frame = runtime.render(1, 80).unwrap();
    assert!(
        frame.contains(&cursor(3)) && frame.contains(&cursor(10)),
        "{frame:?}"
    );

    focused.other_focused_clients = [3, 40].into_iter().map(ClientId::new).collect();
    focused.other_focused_client_slots = vec![];
    event(&mut runtime, Event::TabUpdate(vec![focused]));
    let frame = runtime.render(1, 80).unwrap();
    assert!(
        frame.contains(&cursor(3)),
        "an older host's id is the slot: {frame:?}"
    );
    assert!(!frame.contains(&cursor(10)), "{frame:?}");
}

#[test]
fn tab_bar_swap_and_scroll_indicators_keep_cached_keys_and_dimmed_state() {
    let (mut runtime, _) = plugin();
    let mut active = tab(0, "main", true);
    active.active_swap_layout_name = Some("stacked".to_owned());
    event(&mut runtime, Event::TabUpdate(vec![active.clone()]));
    wire(
        &mut runtime,
        json!({"InitialKeybinds":[["Normal",[
            [{"bare_key":{"Char":"["},"key_modifiers":["Alt"]},["PreviousSwapLayout"]],
            [{"bare_key":{"Char":"]"},"key_modifiers":["Alt"]},["NextSwapLayout"]]
        ]]]}),
    );
    event(&mut runtime, Event::ModeUpdate(ModeInfo::default()));
    let components = rendered_text(&runtime.render(1, 100).unwrap());
    assert!(components.contains("Alt <[]>"), "{components}");
    assert!(components.contains("STACKED"));
    assert!(!runtime.value("mi").unwrap()["keybinds"]
        .as_array()
        .unwrap()
        .is_empty());
    let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
    mode["session_dimmed"] = json!(true);
    wire(&mut runtime, json!({"ModeUpdate":mode}));
    let frame = runtime.render(1, 100).unwrap();
    assert!(frame.contains("\u{1b}[3;"));
    active.active_swap_layout_name = None;
    event(&mut runtime, Event::TabUpdate(vec![active]));
    wire(&mut runtime, json!({"ActivePaneScroll":[2,20]}));
    assert!(runtime.render(1, 100).unwrap().contains("[ SCROLL 2/20 ]"));
    wire(&mut runtime, json!({"ActivePaneScroll":null}));
    assert!(!runtime.render(1, 100).unwrap().contains("SCROLL"));
}

#[test]
fn single_group_keeps_session_and_alt_hints_on_the_tab_row() {
    let (mut runtime, host) = plugin();
    let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
    mode["session_name"] = json!("session");
    mode["session_ancestry"] = json!(["parent"]);
    mode["host_fullscreen"] = json!(true);
    wire(&mut runtime, json!({"ModeUpdate":mode}));
    wire(
        &mut runtime,
        json!({"InitialKeybinds":[["Normal",[
            [{"bare_key":{"Char":"["},"key_modifiers":["Alt"]},["PreviousSwapLayout"]],
            [{"bare_key":{"Char":"]"},"key_modifiers":["Alt"]},["NextSwapLayout"]]
        ]]]}),
    );
    let mut active = tab(0, "main", true);
    active.active_swap_layout_name = Some("stacked".into());
    let mut other = tab(1, "hidden", false);
    other.group = Some(yinz_types::tab_bar::TabGroupInfo {
        id: 1,
        name: "Other".into(),
    });
    for multiple in [false, true, false] {
        let mut tabs = vec![active.clone()];
        if multiple {
            tabs.push(other.clone());
        }
        event(&mut runtime, Event::TabUpdate(tabs));
        let frame = runtime.render(2, 120).unwrap();
        assert_eq!(frame.contains("\r\n"), multiple, "{frame}");
        let lines: Vec<_> = frame.split("\r\n").collect();
        let prefix = lines[0];
        for text in ["host", "parent ▸ ", "session"] {
            assert!(prefix.contains(text), "{prefix}");
        }
        for text in ["Alt <[]>", "STACKED"] {
            assert!(rendered_text(prefix).contains(text), "{prefix}");
        }
        assert_eq!(prefix.contains("Default"), multiple);
        assert_eq!(runtime.value("group-row-visible").unwrap(), json!(multiple));
        if multiple {
            assert!(lines[1].contains("main") && !lines[1].contains("host"));
            let col = runtime.value("new-group-range").unwrap()[0].clone();
            wire(&mut runtime, json!({"Mouse":{"Hover":[0,col]}}));
        } else {
            assert!(prefix.contains("main"));
            assert_eq!(runtime.value("new-group-range").unwrap(), json!(false));
            assert_eq!(runtime.value("hovered-group-button").unwrap(), json!(false));
            let col = runtime.value("breadcrumb-range").unwrap()[0].clone();
            wire(&mut runtime, json!({"Mouse":{"LeftClick":[0,col]}}));
            assert_eq!(
                host.lock().unwrap().commands.last(),
                Some(&json!("FocusHostSession"))
            );
        }
    }
}

#[test]
fn group_row_filters_tabs_and_mouse_targets_keep_global_addresses() {
    let (mut runtime, host) = plugin();
    let mut mode = ModeInfo::default();
    mode.capabilities.arrow_fonts = false;
    event(&mut runtime, Event::ModeUpdate(mode));
    let mut tabs = vec![
        tab(0, "hidden", false),
        tab(1, "first", true),
        tab(2, "second", false),
    ];
    for (index, tab) in tabs.iter_mut().enumerate() {
        tab.group = Some(yinz_types::tab_bar::TabGroupInfo {
            id: if index == 0 { 0 } else { 4 },
            name: if index == 0 { "Default" } else { "工程" }.into(),
        });
    }
    event(&mut runtime, Event::TabUpdate(tabs));
    let rendered = runtime.render(2, 120).unwrap();
    let lines: Vec<_> = rendered.lines().collect();
    assert!(
        lines[0].contains("Default") && lines[0].contains("工程"),
        "{rendered}"
    );
    assert!(
        lines[1].contains("first") && lines[1].contains("second"),
        "{rendered}"
    );
    assert!(!rendered.contains("hidden"));
    let parts = runtime.value("parts").unwrap();
    let mut col = 0;
    for part in parts.as_array().unwrap() {
        if part["tab"] == 1 {
            break;
        }
        col += part["len"].as_u64().unwrap();
    }
    wire(&mut runtime, json!({"Mouse":{"LeftClick":[1,col]}}));
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"SwitchTabTo":3}))
    );
    wire(&mut runtime, json!({"Mouse":{"Hover":[1,col]}}));
    assert_eq!(runtime.value("hovered-tab").unwrap(), json!(2));
    let group_parts = runtime.value("group-parts").unwrap();
    let group_col: u64 = group_parts
        .as_array()
        .unwrap()
        .iter()
        .take_while(|part| part["tab"] != 0)
        .map(|part| part["len"].as_u64().unwrap())
        .sum();
    wire(&mut runtime, json!({"Mouse":{"Hover":[0,group_col]}}));
    assert_eq!(runtime.value("hovered-tab").unwrap(), json!(false));
    assert_eq!(runtime.value("hovered-group").unwrap(), json!(0));
    wire(&mut runtime, json!({"Mouse":{"LeftClick":[0,group_col]}}));
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"RunAction":[{"SwitchTabGroup":{"id":0}},{}]}))
    );
    let single = runtime.render(1, 120).unwrap();
    assert!(single.contains("first") && !single.contains("Default"));
    for columns in [1, 2, 8, 20] {
        assert!(runtime.render(2, columns).is_ok());
    }
}

#[test]
fn group_plus_matches_tab_plus_styling_with_hover_arrows_and_dimming() {
    let (mut runtime, _) = plugin();
    let mut other = tab(2, "other", false);
    other.group = Some(yinz_types::tab_bar::TabGroupInfo {
        id: 1,
        name: "Other".into(),
    });
    event(
        &mut runtime,
        Event::TabUpdate(vec![tab(0, "main", true), tab(1, "second", false), other]),
    );
    for arrow_fonts in [false, true] {
        for dimmed in [false, true] {
            let mut mode = ModeInfo::default();
            mode.capabilities.arrow_fonts = arrow_fonts;
            mode.session_dimmed = Some(dimmed);
            event(&mut runtime, Event::ModeUpdate(mode));
            runtime.render(2, 80).unwrap();
            let button_part = |runtime: &mut Runtime, name: &str| {
                runtime
                    .value(name)
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|part| part["part"].as_str().unwrap().contains(" + "))
                    .unwrap()["part"]
                    .clone()
            };
            let plain = button_part(&mut runtime, "group-parts");
            assert_eq!(plain, button_part(&mut runtime, "parts"));
            let group_col = runtime.value("new-group-range").unwrap()[0].clone();
            wire(&mut runtime, json!({"Mouse":{"Hover":[0,group_col]}}));
            runtime.render(2, 80).unwrap();
            let hovered = button_part(&mut runtime, "group-parts");
            assert_eq!(plain == hovered, arrow_fonts || dimmed);
            let tab_col = runtime.value("new-tab-range").unwrap()[0].clone();
            wire(&mut runtime, json!({"Mouse":{"Hover":[1,tab_col]}}));
            runtime.render(2, 80).unwrap();
            assert_eq!(plain, button_part(&mut runtime, "group-parts"));
            assert_eq!(hovered, button_part(&mut runtime, "parts"));
            wire(&mut runtime, json!({"Mouse":{"Hover":[1,79]}}));
        }
    }
}

#[test]
fn group_items_match_tab_hover_with_arrows_and_dimming() {
    let (mut runtime, _) = plugin();
    // Skip the flat first edge while preserving alternating tile colors.
    let mut tabs = vec![
        tab(0, "first", false),
        tab(1, "second", false),
        tab(2, "inactive", false),
        tab(3, "selected", true),
        tab(4, "hidden", false),
    ];
    for (index, tab) in tabs.iter_mut().enumerate() {
        tab.group = Some(yinz_types::tab_bar::TabGroupInfo {
            id: if index == 4 { 0 } else { 1 },
            name: if index == 4 { "inactive" } else { "selected" }.into(),
        });
    }
    event(&mut runtime, Event::TabUpdate(tabs));
    let item = |runtime: &mut Runtime, name: &str, index: usize| {
        let parts = runtime.value(name).unwrap();
        let parts = parts.as_array().unwrap();
        let offset = parts.iter().position(|part| part["tab"] == index).unwrap();
        let col: u64 = parts[..offset]
            .iter()
            .map(|part| part["len"].as_u64().unwrap())
            .sum();
        (parts[offset]["part"].clone(), col)
    };
    for arrow_fonts in [false, true] {
        for dimmed in [false, true] {
            let mut mode = ModeInfo::default();
            mode.capabilities.arrow_fonts = arrow_fonts;
            mode.session_dimmed = Some(dimmed);
            event(&mut runtime, Event::ModeUpdate(mode));
            for index in [0, 1] {
                runtime.render(2, 120).unwrap();
                let (plain, group_col) = item(&mut runtime, "group-parts", index);
                let (tab_plain, tab_col) = item(&mut runtime, "parts", index + 2);
                assert_eq!(plain, tab_plain);
                wire(&mut runtime, json!({"Mouse":{"Hover":[0,group_col]}}));
                runtime.render(2, 120).unwrap();
                let (hovered, _) = item(&mut runtime, "group-parts", index);
                assert_eq!(plain == hovered, index == 1 || arrow_fonts || dimmed);
                wire(&mut runtime, json!({"Mouse":{"Hover":[1,tab_col]}}));
                runtime.render(2, 120).unwrap();
                assert_eq!(plain, item(&mut runtime, "group-parts", index).0);
                assert_eq!(hovered, item(&mut runtime, "parts", index + 2).0);
                wire(&mut runtime, json!({"Mouse":{"Hover":[0,119]}}));
                runtime.render(2, 120).unwrap();
                assert_eq!(plain, item(&mut runtime, "parts", index + 2).0);
            }
        }
    }
}

#[test]
fn disabled_groups_show_all_tabs_and_move_chrome_on_live_mode_updates() {
    let (mut runtime, host) = plugin();
    let mut tabs = vec![tab(0, "other", false), tab(1, "current", true)];
    for (id, tab) in tabs.iter_mut().enumerate() {
        tab.group = Some(yinz_types::tab_bar::TabGroupInfo {
            id: id as u32,
            name: format!("group-{id}"),
        });
    }
    tabs[1].active_swap_layout_name = Some("vertical".into());
    event(&mut runtime, Event::TabUpdate(tabs));
    let mut mode = ModeInfo {
        session_name: Some("session".into()),
        tab_groups: Some(true),
        ..Default::default()
    };
    event(&mut runtime, Event::ModeUpdate(mode.clone()));
    assert!(!runtime.render(2, 180).unwrap().contains("other"));
    mode.tab_groups = Some(false);
    event(&mut runtime, Event::ModeUpdate(mode.clone()));
    let output = runtime.render(2, 180).unwrap();
    assert!(!output.contains("\r\n"), "{output}");
    for label in ["host", "session", "other", "current"] {
        assert!(output.contains(label), "missing {label}: {output}");
    }
    assert!(rendered_text(&output).contains("VERTICAL"));
    assert!(!output.contains("group-"));
    assert_eq!(runtime.value("new-group-range").unwrap(), json!(false));
    let parts = runtime.value("parts").unwrap();
    let col: u64 = parts
        .as_array()
        .unwrap()
        .iter()
        .take_while(|part| part["tab"] != 0)
        .map(|part| part["len"].as_u64().unwrap())
        .sum();
    wire(&mut runtime, json!({"Mouse":{"LeftClick":[0,col]}}));
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"SwitchTabTo":1}))
    );
    for columns in [1, 2, 8, 20] {
        assert!(runtime.render(1, columns).is_ok());
    }
    mode.tab_groups = Some(true);
    event(&mut runtime, Event::ModeUpdate(mode));
    let output = runtime.render(2, 180).unwrap();
    assert!(output.contains("group-0") && output.contains("group-1"));
    assert!(!output.contains("other"));
}

#[test]
fn disabled_groups_hide_group_shortcuts_but_keep_numbered_tabs() {
    let (mut runtime, _) = plugin();
    let key = json!({"bare_key":{"Char":"g"},"key_modifiers":[]});
    let bindings = json!([[key,["NewTabGroup"]], [key,["MovePaneToNextTabGroup"]], [key,[{"GoToTabInGroup":{"index":2}}]]]);
    let mut mode = json!({"mode":"Tab", "tab_groups":false, "keybinds":[["Tab",bindings]]});
    let filtered = runtime
        .invoke("bar-mode-keymap", vec![mode.clone()])
        .unwrap();
    assert_eq!(filtered.as_array().unwrap(), &vec![bindings[2].clone()]);
    mode["tab_groups"] = json!(null);
    assert_eq!(
        runtime.invoke("bar-mode-keymap", vec![mode]).unwrap(),
        bindings
    );
}

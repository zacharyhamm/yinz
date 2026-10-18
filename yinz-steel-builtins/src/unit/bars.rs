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
    wire(&mut runtime, json!({"Mouse":{"Hover":[0,1]}}));
    assert_eq!(runtime.value("hovered-tab").unwrap(), json!(false));
    assert_eq!(runtime.value("hovered-group").unwrap(), json!(0));
    wire(&mut runtime, json!({"Mouse":{"LeftClick":[0,1]}}));
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

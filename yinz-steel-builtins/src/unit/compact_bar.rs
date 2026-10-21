use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde_json::{json, Value};
use yinz_steel::Runtime;
use yinz_types::{
    data::{Event, ModeInfo, PaneInfo, PipeMessage, TabInfo},
    internal_ids::TabPosition,
};

use crate::tests::{plugin_with_replies, Host};

struct Compact {
    runtime: Runtime,
    host: Arc<Mutex<Host>>,
}
impl Compact {
    fn new(config: Value) -> Self {
        let (runtime, host) = plugin_with_replies(
            "compact-bar",
            config,
            BTreeMap::from([(
                "GetPluginIds".to_owned(),
                json!({"plugin_id":7,"client_id":3,"hostname":"host"}),
            )]),
        );
        Self { runtime, host }
    }
    fn event(&mut self, value: Value) -> bool {
        let event: Event = serde_json::from_value(value).unwrap();
        self.runtime
            .update(serde_json::to_value(event).unwrap())
            .unwrap()
    }
    fn mode(&mut self, name: &str) {
        let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
        mode["mode"] = json!(name);
        self.event(json!({"ModeUpdate":mode}));
    }
    fn pipe(&mut self, name: &str, payload: Option<&str>, private: bool) {
        let message: PipeMessage = serde_json::from_value(json!({"source":"Keybind","name":name,"payload":payload,"args":{},"is_private":private})).unwrap();
        assert!(!self
            .runtime
            .pipe(serde_json::to_value(message).unwrap())
            .unwrap());
    }
    fn named(&self, name: &str) -> Vec<Value> {
        self.host
            .lock()
            .unwrap()
            .commands
            .iter()
            .filter(|command| command.get(name).is_some() || command.as_str() == Some(name))
            .cloned()
            .collect()
    }
    fn render(&mut self, rows: usize, cols: usize) -> String {
        let frame = self.runtime.render(rows, cols).unwrap();
        let plain = self
            .runtime
            .invoke("yinz-text-sanitize", vec![json!(frame)])
            .unwrap();
        let mut output = plain.as_str().unwrap().to_owned();
        // Tables contain several serialized cells; decode every cell, including ragged fallback rows.
        for component in frame.split("\u{1b}Py").skip(1) {
            for field in component
                .split("\u{1b}\\")
                .next()
                .unwrap()
                .split(';')
                .skip(1)
            {
                let text = field
                    .rsplit('$')
                    .next()
                    .unwrap()
                    .trim_start_matches(['x', 'z', 'd']);
                if text.contains(',')
                    && let Ok(bytes) = text
                        .split(',')
                        .map(str::parse::<u8>)
                        .collect::<Result<Vec<_>, _>>()
                {
                    output.push_str(&String::from_utf8(bytes).unwrap());
                    output.push('\n');
                }
            }
        }
        output
    }
    fn pane(&mut self, tab: usize, tooltip: bool) {
        let mut pane = serde_json::to_value(PaneInfo::default()).unwrap();
        pane["id"] = json!(7);
        pane["is_plugin"] = json!(true);
        pane["plugin_url"] = json!("yinz:compact-bar");
        pane["pane_content_x"] = json!(usize::from(tooltip));
        self.event(json!({"PaneUpdate":{"panes":{tab.to_string():[pane]}}}));
    }
}
fn tab(position: usize, name: &str, active: bool) -> TabInfo {
    TabInfo {
        position: TabPosition::new(position),
        name: name.to_owned(),
        active,
        display_area_rows: serde_json::from_value(json!(24)).unwrap(),
        ..Default::default()
    }
}
fn key(character: &str) -> Value {
    json!({"bare_key":{"Char":character},"key_modifiers":[]})
}

#[test]
fn compact_clipboard_precedence_input_clear_and_empty_tab_updates() {
    let mut bar = Compact::new(json!({}));
    assert_eq!(bar.runtime.render(1, 120).unwrap(), "");
    bar.event(json!({"TabUpdate":[tab(0,"main",true)]}));
    for (destination, text) in [
        ("System", "Text copied to system clipboard"),
        ("Primary", "Text copied to system primary selection"),
        ("Command", "Text piped to external command"),
    ] {
        assert!(bar.event(json!({"CopyToClipboard":destination})));
        assert!(!bar.event(json!({"CopyToClipboard":destination})));
        bar.event(json!("SystemClipboardFailure"));
        assert!(bar.render(1, 120).contains(text));
        assert!(bar.event(json!("InputReceived")));
        assert!(!bar.event(json!("InputReceived")));
        assert!(bar.render(1, 120).contains("main"));
    }
    bar.event(json!("SystemClipboardFailure"));
    assert!(bar
        .render(1, 120)
        .contains("Error using the system clipboard"));
    bar.event(json!("InputReceived"));
    assert!(!bar.event(json!({"TabUpdate":[tab(0,"ignored",false)]})));
    assert!(!bar.event(json!({"TabUpdate":[]})));
    assert!(bar.render(1, 120).contains("main"));
}

#[test]
fn compact_mouse_targets_rendered_tabs_and_nested_session_breadcrumb() {
    let mut bar = Compact::new(json!({}));
    bar.event(json!({"TabUpdate":[tab(0,"main",true),tab(1,"second",false)]}));
    assert!(bar.render(1, 120).contains("second"));
    let parts = bar.runtime.value("parts").unwrap();
    let mut start = 0;
    for part in parts.as_array().unwrap() {
        if part["tab"] == 0 || part["tab"] == 1 {
            bar.event(json!({"Mouse":{"LeftClick":[0,start]}}));
        }
        start += part["len"].as_u64().unwrap();
    }
    bar.event(json!({"Mouse":{"ScrollUp":0}}));
    bar.event(json!({"Mouse":{"ScrollDown":0}}));
    assert_eq!(
        bar.named("SwitchTabTo"),
        json!([{"SwitchTabTo":2},{"SwitchTabTo":2},{"SwitchTabTo":1}])
            .as_array()
            .unwrap()
            .clone()
    );
    let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
    mode["session_name"] = json!("child");
    mode["session_ancestry"] = json!(["parent"]);
    mode["host_fullscreen"] = json!(true);
    bar.event(json!({"ModeUpdate":mode}));
    assert!(bar.render(1, 120).contains("parent"));
    let start = bar.runtime.value("breadcrumb").unwrap()[0].clone();
    bar.event(json!({"Mouse":{"LeftClick":[0,start]}}));
    assert_eq!(
        bar.named("FocusHostSession"),
        vec![json!("FocusHostSession")]
    );
}

#[test]
fn compact_tooltip_toggle_is_scoped_to_private_client_and_active_tab() {
    let mut bar = Compact::new(json!({"tooltip":"Ctrl y"}));
    assert_eq!(bar.named("Reconfigure").len(), 1);
    bar.event(json!({"TabUpdate":[tab(0,"main",true)]}));
    bar.pane(0, false);
    bar.pipe("toggle_tooltip", Some("3"), false);
    bar.pipe("toggle_tooltip", Some("4"), true);
    bar.pane(1, false);
    bar.pipe("toggle_tooltip", Some("3"), true);
    assert!(bar.named("MessageToPlugin").is_empty());
    bar.pane(0, true);
    assert_eq!(
        bar.runtime.value("state").unwrap()["tooltipActive"],
        json!(true)
    );
    bar.pipe("toggle_tooltip", Some("3"), true);
    let message = bar.named("MessageToPlugin")[0]["MessageToPlugin"].clone();
    assert_eq!(message["message_name"], "toggle_persisted_tooltip");
    assert_eq!(message["plugin_config"]["is_tooltip"], "true");
    assert_eq!(message["message_args"], json!({"persist":""}));
    assert_eq!(
        message["floating_pane_coordinates"]["y"],
        json!({"Fixed":19})
    );
    bar.mode("Pane");
    assert_eq!(bar.named("MessageToPlugin").len(), 2);
    for mode in [
        "Locked",
        "EnterSearch",
        "RenameTab",
        "RenamePane",
        "Prompt",
        "Tmux",
        "Select",
        "Normal",
    ] {
        bar.mode(mode);
    }
    assert_eq!(bar.named("MessageToPlugin").len(), 2);
}

#[test]
fn compact_tooltip_retains_initial_keys_follows_tabs_and_closes_in_base_mode() {
    let mut bar = Compact::new(json!({"is_tooltip":"true"}));
    assert_eq!(
        bar.named("Subscribe"),
        vec![json!({"Subscribe":["ModeUpdate","TabUpdate","InitialKeybinds"]})]
    );
    bar.event(json!({"InitialKeybinds":[["Pane",[[key("f"),["ToggleFocusFullscreen"]]]]]}));
    bar.mode("Pane");
    assert!(bar.render(10, 80).contains("Toggle fullscreen"));
    assert_eq!(
        bar.named("RenamePluginPane"),
        vec![json!({"RenamePluginPane":[7,"Pane"]})]
    );
    bar.event(json!({"TabUpdate":[tab(0,"main",true)]}));
    bar.event(json!({"TabUpdate":[tab(0,"main",false),tab(1,"second",true)]}));
    assert_eq!(
        bar.named("BreakPanesToTabWithIndex").last(),
        Some(&json!({"BreakPanesToTabWithIndex":[[{"Plugin":7}],1,false]}))
    );
    bar.event(json!({"Mouse":{"ScrollUp":0}}));
    assert!(bar.named("SwitchTabTo").is_empty());
    assert!(!bar.event(json!("SystemClipboardFailure")));
    assert!(!bar.event(json!({"CopyToClipboard":"System"})));
    assert!(!bar.event(json!("InputReceived")));
    bar.mode("Normal");
    assert_eq!(bar.named("CloseSelf"), vec![json!("CloseSelf")]);
}

#[test]
fn compact_persisted_tooltip_survives_modes_then_closes_on_second_toggle() {
    let mut bar = Compact::new(json!({"is_tooltip":"true"}));
    bar.pipe("toggle_persisted_tooltip", None, false);
    assert_eq!(bar.runtime.value("persist").unwrap(), json!(false));
    bar.pipe("toggle_persisted_tooltip", None, true);
    bar.mode("Normal");
    bar.mode("RenamePane");
    assert!(bar.named("CloseSelf").is_empty());
    assert!(bar.render(10, 80).contains("Renaming pane..."));
    let frame = bar.runtime.render(10, 80).unwrap();
    assert!(frame.contains(";2;2;"), "{frame:?}");
    bar.pipe("toggle_persisted_tooltip", None, true);
    assert_eq!(bar.named("CloseSelf"), vec![json!("CloseSelf")]);
}

#[test]
fn compact_tooltip_groups_curated_directions_resize_and_lock_keys() {
    let mut bar = Compact::new(json!({"is_tooltip":"true"}));
    for (keys, brackets, locked, expected) in [
        (
            json!(["l", "k", "j", "h", "←", "←", "→", "+", "=", "-", "PgDn", "PgUp", "x"]),
            true,
            false,
            "<hjkl>/<←→>/<+->/<PgUp|PgDn>/<x>",
        ),
        (json!(["ESC", "ENTER", "x"]), false, true, "x"),
        (json!(["ESC", "ENTER"]), true, true, "<ESC>/<ENTER>"),
    ] {
        assert_eq!(
            bar.runtime
                .invoke(
                    "compact-grouped-keys",
                    vec![keys, json!(brackets), json!(locked)]
                )
                .unwrap(),
            json!(expected)
        );
    }
    let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
    mode["mode"] = json!("Pane");
    mode["keybinds"] = json!([["Pane",[
        [key("h"),[{"MoveFocus":{"direction":"Left"}}]],
        [key("l"),[{"MoveFocus":{"direction":"Right"}}]],
        [key("n"),[{"NewPane":{"direction":null,"pane_name":null,"start_suppressed":true}}]],
        [key("r"),[{"NewPane":{"direction":"Right","pane_name":null,"start_suppressed":false}}]],
        [key("d"),[{"NewPane":{"direction":"Down","pane_name":null,"start_suppressed":false}}]]
    ]]]);
    bar.event(json!({"ModeUpdate":mode.clone()}));
    assert_eq!(
        bar.runtime.invoke("compact-actions", vec![mode]).unwrap(),
        json!([
            ["<n>", "New pane"],
            ["<hl>", "Move focus"],
            ["<r>/<d>", "Split right/down"]
        ])
    );
    let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
    mode["mode"] = json!("Resize");
    mode["keybinds"] = json!([["Resize",[
        [key("+"),[{"Resize":{"resize":"Increase","direction":null}}]],
        [key("-"),[{"Resize":{"resize":"Decrease","direction":null}}]],
        [key("h"),[{"Resize":{"resize":"Increase","direction":"Left"}}]],
        [key("l"),[{"Resize":{"resize":"Increase","direction":"Right"}}]]
    ]]]);
    bar.event(json!({"ModeUpdate":mode.clone()}));
    assert_eq!(
        bar.runtime.invoke("compact-actions", vec![mode]).unwrap(),
        json!([
            ["<+->", "Increase or decrease size"],
            ["<hl>", "Increase size in direction"]
        ])
    );
}

#[test]
fn compact_session_tooltip_groups_palette_shortcuts_after_the_base_mode_switch() {
    let mut bar = Compact::new(json!({"is_tooltip":"true"}));
    for base in ["Normal", "Locked"] {
        let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
        mode["mode"] = json!("Session");
        mode["base_mode"] = json!(base);
        mode["keybinds"] = json!([["Session", [
            [key("w"),[{"SwitchToMode":{"input_mode":base}},"ToggleCommandPalette"]],
            [key("p"),[{"SwitchToMode":{"input_mode":base}},"ToggleCommandPalette"]],
            [key("l"),[{"SwitchToMode":{"input_mode":base}},"ToggleCommandPalette"]],
            [key("a"),["ToggleCommandPalette"]],
            [key("x"),[{"SwitchToMode":{"input_mode":base}},"Quit","ToggleCommandPalette"]]
        ]]]);
        assert_eq!(
            bar.runtime.invoke("compact-actions", vec![mode]).unwrap(),
            json!([["<l>/<w>/<p>", "Command palette"]])
        );
    }
}

#[test]
fn compact_tooltip_normal_mode_ribbons_and_all_mode_fallbacks_fit() {
    let mut bar = Compact::new(json!({"is_tooltip":"true"}));
    bar.pipe("toggle_persisted_tooltip", None, true);
    bar.event(json!({"InitialKeybinds":[["Normal",[[key("p"),[{"SwitchToMode":{"input_mode":"Pane"}}]],[key("q"),["Quit"]]]]]}));
    bar.mode("Normal");
    assert!(bar.render(3, 80).contains("Pane"));
    assert!(bar.render(3, 1).contains("..."));
    for mode in [
        "Locked",
        "Pane",
        "Tab",
        "Resize",
        "Move",
        "Scroll",
        "Search",
        "Select",
        "Session",
        "Notifications",
        "Agent",
        "EnterSearch",
        "Prompt",
        "RenamePane",
        "RenameTab",
        "Tmux",
    ] {
        bar.mode(mode);
        for rows in [0, 1, 10] {
            for cols in [0, 1, 80] {
                bar.render(rows, cols);
            }
        }
    }
}

#[test]
fn compact_tabs_render_flags_collapsed_neighbors_swap_and_nested_hints() {
    let mut bar = Compact::new(json!({"tooltip":"Ctrl y"}));
    let mut tabs = (0..12)
        .map(|i| tab(i, &format!("tab-{i}-界界界界界界界界界"), i == 6))
        .collect::<Vec<_>>();
    tabs[6].is_fullscreen_active = true;
    tabs[6].has_bell_notification = true;
    tabs[6].other_focused_clients = serde_json::from_value(json!([1, 10, 11])).unwrap();
    bar.event(json!({"TabUpdate":tabs}));
    let frame = bar.render(1, 100);
    assert!(frame.contains("(FULLSCREEN) [!]"), "{frame}");
    assert!(frame.contains('…'));
    assert!(frame.contains("← +"));
    let mut active = tab(0, "main", true);
    active.active_swap_layout_name = Some("stacked".to_owned());
    bar.event(json!({"TabUpdate":[active]}));
    let frame = bar.render(1, 120);
    assert!(frame.contains("Tooltip"));
    assert!(frame.contains("STACKED"));
    let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
    mode["session_dimmed"] = json!(true);
    bar.event(json!({"ModeUpdate":mode}));
    let frame = bar.render(1, 120);
    assert!(frame.contains("Ascend:"));
    assert!(frame.contains("<unbound>"));
    assert!(!frame.contains("Tooltip"));
    for cols in [0, 1, 8, 20, 200] {
        bar.render(1, cols);
    }
}

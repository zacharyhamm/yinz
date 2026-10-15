use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde_json::{json, Value};
use yinz_steel::Runtime;
use yinz_types::data::{Event, ModeInfo, PaneInfo, TabInfo};

use crate::tests::{plugin_with_prelude, rendered_text, Host};

struct Status {
    runtime: Runtime,
    host: Arc<Mutex<Host>>,
}
impl Status {
    fn new(classic: bool) -> Self {
        let (runtime, host) = plugin_with_prelude("status-bar",json!({"classic":classic.to_string()}),
            BTreeMap::from([("GetLocalUtcOffset".to_owned(),json!(0)),("GetYinzVersion".to_owned(),json!("test"))]),
            "(define test-clock (vector 0)) (define (yinz-wall-clock-ms) (vector-ref test-clock 0))");
        Self { runtime, host }
    }
    fn event(&mut self, value: Value) -> bool {
        let event: Event = serde_json::from_value(value).unwrap();
        self.runtime
            .update(serde_json::to_value(event).unwrap())
            .unwrap()
    }
    fn render(&mut self, rows: usize, cols: usize) -> String {
        let output = self.runtime.render(rows, cols).unwrap();
        let components = rendered_text(&output);
        let plain = self
            .runtime
            .invoke("yinz-text-sanitize", vec![json!(output)])
            .unwrap();
        format!("{}{components}", plain.as_str().unwrap())
    }
    fn advance(&mut self, now: usize) {
        self.runtime
            .eval(&format!("(vector-set! test-clock 0 {now})"))
            .unwrap();
        self.event(json!({"Timer":0}));
    }
    fn named(&self, name: &str) -> Vec<Value> {
        self.host
            .lock()
            .unwrap()
            .commands
            .iter()
            .filter(|c| c.get(name).is_some())
            .cloned()
            .collect()
    }
    fn tab(&mut self) {
        self.event(json!({"TabUpdate":[TabInfo {active:true,name:"main".to_owned(),selectable_tiled_panes_count:1,..Default::default()}]}));
    }
    fn mode(&mut self, mode: Value) {
        self.event(json!({"ModeUpdate":mode}));
    }
}

#[test]
fn status_clock_and_tip_draw_in_single_and_classic_rows() {
    let mut status = Status::new(false);
    assert!(status.render(1, 80).contains("00:00"));
    status.tab();
    assert!(status.render(2, 200).contains("open new pane"));
    assert_eq!(status.named("SetTimeout"), vec![json!({"SetTimeout":60.0})]);
    let mut classic = Status::new(true);
    classic.tab();
    assert!(classic.render(2, 200).contains("open new pane"));
    for rows in [0, 1, 2] {
        for cols in [0, 1, 20, 80, 200] {
            classic.render(rows, cols);
        }
    }
}

#[test]
fn status_deduplicates_alt_pointer_and_late_timer_events() {
    let mut status = Status::new(false);
    status.render(1, 80);
    for _ in 0..20 {
        status.event(json!({"ModifiersHeld":{"modifiers":["Alt"],"is_live":false}}));
        status.event(json!({"Mouse":{"Hover":[0,0]}}));
        status.event(json!({"Mouse":{"Hover":[0,65535]}}));
    }
    assert_eq!(
        status.named("SetTimeout"),
        vec![json!({"SetTimeout":60.0}), json!({"SetTimeout":3.0})]
    );
    status.advance(3000);
    assert_eq!(
        status.named("SetTimeout").last(),
        Some(&json!({"SetTimeout":57.0}))
    );
    status.advance(4000);
    assert_eq!(status.named("SetTimeout").len(), 3);
    status.advance(60000);
    assert_eq!(status.named("SetTimeout").len(), 4);
    assert!(status.render(1, 80).contains("00:01"));
}

#[test]
fn status_refreshes_local_offset_hourly_and_falls_back_on_host_failure() {
    let mut status = Status::new(false);
    status
        .host
        .lock()
        .unwrap()
        .replies
        .insert("GetLocalUtcOffset".to_owned(), json!(19800));
    status.advance(3600000);
    let frame = status.render(1, 80);
    assert!(
        frame.contains("06:30"),
        "{frame}; offset {:?}; clock {:?}; calls {:?}",
        status.runtime.value("utc-offset"),
        status.runtime.value("state").unwrap()["clock"],
        status.named("GetLocalUtcOffset")
    );
    status.advance(7199999);
    assert_eq!(status.named("GetLocalUtcOffset").len(), 2);
    status
        .host
        .lock()
        .unwrap()
        .replies
        .insert("GetLocalUtcOffset".to_owned(), json!(-18000));
    status.advance(7200000);
    assert!(status.render(1, 80).contains("21:00"));
    assert_eq!(status.named("GetLocalUtcOffset").len(), 3);
    status.host.lock().unwrap().failures.insert(
        "GetLocalUtcOffset".to_owned(),
        "offset unavailable".to_owned(),
    );
    status.advance(10800000);
    assert!(status.render(1, 80).contains("03:00"));
}

#[test]
fn status_clipboard_and_live_alt_release_restore_clock_and_mouse_actions() {
    let mut status = Status::new(false);
    let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
    mode["keybinds"] = json!([["Normal",[
        [{"bare_key":{"Char":"n"},"key_modifiers":["Alt"]},[{"NewPane":{"direction":null,"pane_name":null,"start_suppressed":false}}]],
        [{"bare_key":{"Char":"p"},"key_modifiers":["Ctrl"]},[{"SwitchToMode":{"input_mode":"Pane"}}]],
        [{"bare_key":{"Char":"f"},"key_modifiers":["Alt"]},["ToggleFloatingPanes"]]
    ]]]);
    status.mode(mode);
    status.event(json!({"ModifiersHeld":{"modifiers":["Alt"],"is_live":true}}));
    assert!(status.render(1, 200).contains("New Pane"));
    let col = status.runtime.value("new-pane-range").unwrap()[0].clone();
    status.event(json!({"Mouse":{"LeftClick":[0,col]}}));
    assert!(status
        .host
        .lock()
        .unwrap()
        .commands
        .contains(&json!("NewPane")));
    status.event(json!({"Mouse":{"Hover":[0,65535]}}));
    status.event(json!({"ModifiersHeld":{"modifiers":[],"is_live":true}}));
    assert!(status.render(1, 200).contains("00:00"));
    status.event(json!({"CopyToClipboard":"System"}));
    assert!(status
        .render(1, 200)
        .contains("Text copied to system clipboard"));
    status.event(json!("SystemClipboardFailure"));
    assert!(status
        .render(1, 200)
        .contains("Text copied to system clipboard"));
    status.event(json!("InputReceived"));
    assert!(status.render(1, 200).contains("00:00"));
    status.event(json!("SystemClipboardFailure"));
    assert!(status
        .render(1, 200)
        .contains("Error using the system clipboard"));
}

#[test]
fn status_mode_hints_prefer_tab_arrows_and_match_panel_message_name() {
    let mut status = Status::new(true);
    status.tab();
    let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
    let done =
        json!([{"bare_key":"Enter","key_modifiers":[]},[{"SwitchToMode":{"input_mode":"Normal"}}]]);
    mode["mode"] = json!("Tab");
    let mut bindings = vec![];
    for (bare, action) in [
        (json!("Left"), "GoToPreviousTab"),
        (json!("Up"), "GoToPreviousTab"),
        (json!({"Char":"h"}), "GoToPreviousTab"),
        (json!("Right"), "GoToNextTab"),
        (json!("Down"), "GoToNextTab"),
        (json!({"Char":"l"}), "GoToNextTab"),
    ] {
        bindings.push(json!([{"bare_key":bare,"key_modifiers":[]},[action,{"SwitchToMode":{"input_mode":"Normal"}}]]));
    }
    bindings.push(done.clone());
    mode["keybinds"] = json!([["Tab", bindings]]);
    status.mode(mode.clone());
    let frame = status.render(2, 200);
    assert!(frame.contains("<←→> Change focus"), "{frame}");
    let binding = |character: &str, name: &str| {
        json!([
            {"bare_key":{"Char":character},"key_modifiers":[]},[
                {"KeybindPipe":{"name":name,"payload":null,"args":null,"plugin":null,"plugin_id":null,"configuration":null,
                    "launch_new":false,"skip_cache":false,"floating":false,"in_place":null,"cwd":null,"pane_title":null}},
                {"SwitchToMode":{"input_mode":"Normal"}}
            ]
        ])
    };
    mode["mode"] = json!("Agent");
    mode["keybinds"] = json!([["Agent", [binding("m", "something_else"), done]]]);
    status.mode(mode.clone());
    assert!(!status.render(2, 200).contains("Move panel"));
    mode["keybinds"][0][1]
        .as_array_mut()
        .unwrap()
        .push(binding("o", "agent_strip_reposition"));
    status.mode(mode);
    let frame = status.render(2, 200);
    assert!(frame.contains("<o> Move panel"), "{frame}");
}

#[test]
fn status_counts_agents_and_uses_bounded_animation_ticks() {
    let mut status = Status::new(false);
    status.render(1, 200);
    let panes = [
        ("Busy", false),
        ("Waiting", false),
        ("Waiting", true),
        ("Idle", false),
        ("Exited", false),
    ]
    .into_iter()
    .map(|(state, person)| {
        let mut pane = serde_json::to_value(PaneInfo::default()).unwrap();
        pane["agent"] = json!({"kind":"codex","state":state,"wait_needs_a_person":person});
        pane
    })
    .collect::<Vec<_>>();
    let event = json!({"PaneUpdate":{"panes":{"0":panes}}});
    assert!(status.event(event.clone()));
    assert!(!status.event(event));
    assert_eq!(
        status.runtime.value("state").unwrap()["counts"],
        json!({"busy":2,"waiting":1,"idle":1})
    );
    assert_eq!(
        status.named("SetTimeout").last(),
        Some(&json!({"SetTimeout":0.125}))
    );
    let frame = status.render(1, 200);
    assert!(frame.contains("1w"));
    assert!(frame.contains("2|"));
    assert!(frame.contains("1i"));
    status.advance(125);
    assert!(status.render(1, 200).contains("2/"));
    for mode_name in [
        "Normal",
        "Locked",
        "Pane",
        "Tab",
        "Resize",
        "Move",
        "Scroll",
        "Select",
        "EnterSearch",
        "Search",
        "Session",
        "Notifications",
        "Agent",
        "Tmux",
        "RenamePane",
        "RenameTab",
        "Prompt",
    ] {
        let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
        mode["mode"] = json!(mode_name);
        status.mode(mode);
        for rows in [1, 2] {
            for cols in [0, 1, 20, 80, 200] {
                status.render(rows, cols);
            }
        }
    }
}

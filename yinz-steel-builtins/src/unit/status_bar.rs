use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde_json::{json, Value};
use yinz_steel::Runtime;
use yinz_types::data::{Event, ModeInfo, TabInfo};

use crate::tests::{plugin_with_prelude, rendered_text, Host};

struct Status {
    runtime: Runtime,
    host: Arc<Mutex<Host>>,
}
impl Status {
    fn new(classic: bool) -> Self {
        let (runtime, host) = plugin_with_prelude(
            "status-bar",
            json!({"classic":classic.to_string()}),
            BTreeMap::from([
                ("GetLocalUtcOffset".to_owned(), json!(0)),
                ("GetYinzVersion".to_owned(), json!("test")),
                (
                    "GetSessionList".to_owned(),
                    json!({"live_sessions":[{"is_current_session":true,"plugins":{"42":{"location":"yinz:agent-service"}}}]}),
                ),
            ]),
            "(define test-clock (vector 0)) (define (yinz-now-ms) (vector-ref test-clock 0))
             (define (yinz-wall-clock-ms) (vector-ref test-clock 0))",
        );
        Self { runtime, host }
    }
    fn counts(&mut self, busy: u64, waiting: u64, idle: u64) -> bool {
        let seq = self.runtime.value("service-seq").unwrap().as_u64().unwrap() + 1;
        self.count_message(42, seq, json!({"total":busy+waiting+idle,"busy":busy,"waiting":waiting,"idle":idle,"held":0,"exited":0,"unknown":0}))
    }
    fn count_message(&mut self, sender: u32, seq: u64, payload: Value) -> bool {
        self.runtime.pipe(json!({"source":{"Plugin":sender},"name":"agent_counts","payload":payload.to_string(),"args":{"seq":seq.to_string()},"is_private":true})).unwrap()
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
    fn set_time(&mut self, now: usize) {
        self.runtime
            .eval(&format!("(vector-set! test-clock 0 {now})"))
            .unwrap();
    }
    fn advance(&mut self, now: usize) -> bool {
        self.set_time(now);
        self.event(json!({"Timer":0}))
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
    assert!(status.advance(3000));
    assert_eq!(status.named("SetTimeout").len(), 2);
    status.advance(60000);
    assert_eq!(status.named("SetTimeout").len(), 3);
    assert!(status.render(1, 80).contains("00:01"));
    status.advance(120000);
    assert_eq!(status.named("SetTimeout").len(), 4);
    assert!(status.render(1, 80).contains("00:02"));
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
fn group_navigation_hint_follows_pane_navigation_and_distinct_group_ids() {
    use yinz_types::{
        data::InputMode, input::actions::Action, keys::KeyWithModifier, tab_bar::TabGroupInfo,
    };

    let config = yinz_config::config::Config::from_kdl(
        include_str!("../../../yinz-config/assets/config/default.kdl"),
        None,
    )
    .unwrap();
    let mut mode = ModeInfo {
        keybinds: config.keybinds.to_keybinds_vec(),
        ..Default::default()
    };
    let mut tabs = vec![
        TabInfo {
            active: true,
            selectable_tiled_panes_count: 1,
            ..Default::default()
        },
        TabInfo::default(),
    ];
    let mut status = Status::new(false);
    status.mode(json!(mode));
    status.event(json!({"ModifiersHeld":{"modifiers":["Alt"],"is_live":true}}));
    for (ids, visible) in [([0, 0], false), ([0, 2], true), ([2, 2], false)] {
        for (tab, id) in tabs.iter_mut().zip(ids) {
            tab.group = Some(TabGroupInfo {
                id,
                name: "same".into(),
            });
        }
        status.event(json!({"TabUpdate":tabs}));
        let frame = status.render(1, 300);
        assert_eq!(frame.contains("Switch group"), visible, "{frame}");
        if visible {
            assert!(frame.contains("Shift h|Shift l"), "{frame}");
        }
    }
    tabs[1].group.as_mut().unwrap().id = 3;
    status.event(json!({"TabUpdate":tabs}));
    for input_mode in [InputMode::Locked, InputMode::Select, InputMode::Tab] {
        mode.mode = input_mode;
        status.mode(json!(mode));
        let frame = status.render(1, 300);
        assert!(
            !frame.contains("Switch group") && !frame.contains("Groups"),
            "{frame}"
        );
    }

    mode.mode = InputMode::Normal;
    let normal = &mut mode
        .keybinds
        .iter_mut()
        .find(|(mode, _)| *mode == InputMode::Normal)
        .unwrap()
        .1;
    for (shortcut, action) in [
        ("Ctrl Shift j", Action::GoToPreviousTabGroup),
        ("Ctrl Shift k", Action::GoToNextTabGroup),
    ] {
        normal.retain(|(_, actions)| !actions.contains(&action));
        normal.push((shortcut.parse::<KeyWithModifier>().unwrap(), vec![action]));
    }
    status.mode(json!(mode));
    let frame = status.render(1, 300);
    assert!(
        frame.contains("Switch group") && frame.contains("Ctrl-Shift <j|k>"),
        "{frame}"
    );
    let normal = &mut mode
        .keybinds
        .iter_mut()
        .find(|(mode, _)| *mode == InputMode::Normal)
        .unwrap()
        .1;
    normal.retain(|(_, actions)| {
        !actions.iter().any(|action| {
            matches!(
                action,
                Action::MoveFocusOrTab { .. } | Action::MoveFocus { .. }
            )
        })
    });
    status.mode(json!(mode));
    assert!(!status.render(1, 300).contains("Switch group"));
}

#[test]
fn classic_and_multi_row_tab_hints_preserve_group_navigation() {
    use yinz_types::{data::InputMode, input::actions::Action, keys::KeyWithModifier};

    let mut mode = ModeInfo {
        mode: InputMode::Tab,
        keybinds: vec![(
            InputMode::Tab,
            vec![
                (
                    "Ctrl Shift j".parse::<KeyWithModifier>().unwrap(),
                    vec![Action::GoToPreviousTabGroup],
                ),
                (
                    "Ctrl Shift k".parse::<KeyWithModifier>().unwrap(),
                    vec![Action::GoToNextTabGroup],
                ),
            ],
        )],
        ..Default::default()
    };
    for (classic, rows, visible) in [
        (true, 1, true),
        (true, 2, true),
        (false, 2, true),
        (false, 1, false),
    ] {
        let mut status = Status::new(classic);
        status.tab();
        status.mode(json!(mode));
        let frame = status.render(rows, 300);
        assert_eq!(frame.contains("Switch group"), visible, "{frame}");
        if visible {
            assert!(frame.contains("Ctrl-Shift + <j|k> Switch group"), "{frame}");
        }
        let bindings = std::mem::take(&mut mode.keybinds[0].1);
        status.mode(json!(mode));
        let frame = status.render(rows, 300);
        assert!(
            !frame.contains("Switch group") && !frame.contains("Groups"),
            "{frame}"
        );
        mode.keybinds[0].1 = bindings;
    }
}

#[test]
fn status_mode_hints_prefer_tab_arrows_and_open_agents() {
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
    mode["mode"] = json!("Agent");
    mode["keybinds"] = json!([["Agent", [
        [{"bare_key":{"Char":"a"},"key_modifiers":[]}, [
            {"SwitchToMode":{"input_mode":"Normal"}}, "OpenAgents"
        ]], done
    ]]]);
    status.mode(mode);
    let frame = status.render(2, 200);
    assert!(frame.contains("<a> Agents"), "{frame}");
    assert!(!frame.contains("Move panel"), "{frame}");
}

#[test]
fn session_mode_hints_name_the_palette_after_returning_to_the_base_mode() {
    let mut status = Status::new(true);
    status.tab();
    for base in ["Normal", "Locked"] {
        let mut mode = serde_json::to_value(ModeInfo::default()).unwrap();
        mode["mode"] = json!("Session");
        mode["base_mode"] = json!(base);
        mode["keybinds"] = json!([["Session", [
            [{"bare_key":{"Char":"w"},"key_modifiers":[]},[
                {"SwitchToMode":{"input_mode":base}}, "ToggleCommandPalette"
            ]]
        ]]]);
        status.mode(mode);
        let frame = status.render(2, 200);
        assert!(frame.contains("<w> Command palette"), "{frame}");
        assert!(!frame.contains("Manager"), "{frame}");
    }
}

#[test]
fn status_counts_agents_without_animation_timers() {
    let mut status = Status::new(false);
    status.render(1, 200);
    assert!(status.counts(2, 1, 1));
    assert!(!status.counts(2, 1, 1));
    assert_eq!(
        status.runtime.value("state").unwrap()["counts"],
        json!({"busy":2,"waiting":1,"idle":1})
    );
    assert_eq!(status.named("SetTimeout"), vec![json!({"SetTimeout":60.0})]);
    let frame = status.render(1, 200);
    assert!(frame.contains("1w"));
    assert!(frame.contains("2b"));
    assert!(frame.contains("1i"));
    for now in [125, 500, 999] {
        status.set_time(now);
        assert_eq!(status.render(1, 200), frame);
    }
    assert_eq!(status.named("SetTimeout"), vec![json!({"SetTimeout":60.0})]);
    assert!(!status.advance(1000));
    assert!(status.advance(60000));
    assert!(status.render(1, 200).contains("00:01"));
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

#[test]
fn status_agent_transitions_are_static_across_fonts_widths_and_layouts() {
    for (classic, rows) in [(false, 1), (true, 1), (false, 2), (true, 2)] {
        for simple in [false, true] {
            let mut status = Status::new(classic);
            status.tab();
            let mut mode = ModeInfo::default();
            mode.capabilities.arrow_fonts = simple;
            status.mode(json!(mode));
            for (state, glyph) in [
                ("Busy", "●1"),
                ("Waiting", "◆1"),
                ("Idle", "·1"),
                ("Exited", ""),
            ] {
                let counts = (
                    u64::from(state == "Busy"),
                    u64::from(state == "Waiting"),
                    u64::from(state == "Idle"),
                );
                assert!(status.counts(counts.0, counts.1, counts.2));
                assert!(!status.counts(counts.0, counts.1, counts.2));
                if !classic && rows == 1 {
                    let expected = if simple {
                        match state {
                            "Busy" => "1b",
                            "Waiting" => "1w",
                            "Idle" => "1i",
                            _ => "",
                        }
                    } else {
                        glyph
                    };
                    assert!(status.render(rows, 200).contains(expected));
                }
                for cols in [0, 1, 20, 80, 200] {
                    status.set_time(0);
                    let frame = status.runtime.render(rows, cols).unwrap();
                    for now in [125, 499, 500, 999] {
                        status.set_time(now);
                        assert_eq!(status.runtime.render(rows, cols).unwrap(), frame);
                    }
                }
                assert_eq!(status.named("SetTimeout"), vec![json!({"SetTimeout":60.0})]);
            }
            assert_eq!(
                status.runtime.value("state").unwrap()["counts"],
                json!({"busy":0,"waiting":0,"idle":0})
            );
        }
    }
}

#[test]
fn status_counts_reject_invalid_messages_and_clear_on_service_changes() {
    let mut status = Status::new(false);
    assert_eq!(status.named("MessageToPlugin").len(), 1);
    assert!(status.counts(2, 1, 3));
    let valid = json!({"total":1,"busy":1,"waiting":0,"idle":0,"held":0,"exited":0,"unknown":0});
    assert!(!status.count_message(99, 2, valid.clone()));
    assert!(!status.count_message(42, 1, valid.clone()));
    for args in [
        json!({}),
        json!({"seq":"no"}),
        json!({"seq":"-1"}),
        json!({"seq":"1.5"}),
    ] {
        assert!(!status.runtime.pipe(json!({"source":{"Plugin":42},"name":"agent_counts","payload":valid.to_string(),"args":args,"is_private":true})).unwrap());
    }
    assert!(!status.runtime.pipe(json!({"source":{"Plugin":42},"name":"agent_counts","payload":"{","args":{"seq":"2"},"is_private":true})).unwrap());
    assert!(!status.runtime.pipe(json!({"source":{"Plugin":99},"name":"agent_hello","payload":null,"args":{},"is_private":false})).unwrap());
    for invalid in [json!({}), json!([]), json!(null), json!("bad")] {
        assert!(!status.count_message(42, 2, invalid));
    }
    for field in [
        "total", "busy", "waiting", "idle", "held", "exited", "unknown",
    ] {
        for value in [json!(-1), json!(1.5), json!("1"), json!(null)] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(!status.count_message(42, 2, invalid));
        }
    }
    assert_eq!(
        status.runtime.value("state").unwrap()["counts"],
        json!({"busy":2,"waiting":1,"idle":3})
    );
    assert!(status.count_message(42, 2, valid));
    assert!(!status.event(json!({"PaneUpdate":{"panes":{}}})));
    assert!(status.runtime.pipe(json!({"source":{"Plugin":42},"name":"agent_hello","payload":null,"args":{},"is_private":false})).unwrap());
    assert_eq!(status.named("MessageToPlugin").len(), 2);
    assert_eq!(
        status.runtime.value("state").unwrap()["counts"],
        json!({"busy":0,"waiting":0,"idle":0})
    );
    assert!(status.counts(1, 0, 0));
    assert_eq!(
        status
            .runtime
            .invoke("discover-service!", vec![json!([])])
            .unwrap(),
        true
    );
    assert!(!status.counts(2, 0, 0));
    assert_eq!(status.runtime.invoke("discover-service!", vec![json!([{"is_current_session":true,"plugins":{"43":{"location":"yinz:agent-service"}}}])]).unwrap(), false);
    assert_eq!(status.named("MessageToPlugin").len(), 3);
    assert!(status.count_message(
        43,
        1,
        json!({"total":1,"busy":1,"waiting":0,"idle":0,"held":0,"exited":0,"unknown":0})
    ));
}

use super::host;
use crate::plugin_tests::runtime_parity::SteelReference;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{atomic::AtomicU64, Arc},
};

fn plugin(snapshot: Value) -> SteelReference {
    let clock = Arc::new(AtomicU64::new(90_000));
    let (_, responder) = host(&[], clock.clone());
    let mut plugin = SteelReference::with_clock(
        "agents",
        BTreeMap::from([("view".into(), "agents".into())]),
        Path::new("/work"),
        responder,
        Some(clock),
    );
    plugin.runtime.invoke("a-reset!", vec![]).unwrap();
    plugin
        .runtime
        .invoke("a-set!", vec![json!("snapshot"), snapshot])
        .unwrap();
    plugin
}

fn row(pane: u32, symbol: char, waiting: bool) -> Value {
    json!({"Agent": {"pane_id": pane, "kind": "claude", "text": format!("{pane} CLA {symbol}"), "waiting": waiting}})
}

/// A snapshot in which every listed pane reports a tool call, so each agent has an activity line.
fn busy(panes: impl Iterator<Item = u32>) -> Value {
    let hooks: serde_json::Map<String, Value> = panes
        .map(|pane| {
            (
                pane.to_string(),
                json!({"phase": "Busy", "detail": "running tests"}),
            )
        })
        .collect();
    json!({ "hooks": hooks })
}

fn info(plugin: &mut SteelReference, row: Value) -> Value {
    plugin.runtime.invoke("a-agent-info", vec![row]).unwrap()
}

#[test]
fn agents_activity_uses_current_reported_action_and_sanitizes_text() {
    let mut plugin = plugin(json!({
        "panes": [{"pane_id": 7, "title": "  Fix\n the\t 日本語 issue\u{0007}  "}],
        "hooks": {"7": {"phase": "Busy", "detail": " Bash\n cargo\t test\u{001b} ", "waiting_on": "stale question"}},
        "summaries": {"7": {"text": "Old summary question", "awaiting": true, "hash": 12}}
    }));
    assert_eq!(
        info(&mut plugin, row(7, '●', false)),
        json!({"status": "working", "activity": "Bash cargo test", "task": "Fix the 日本語 issue",
            "tracking": "hooks"})
    );
    plugin
        .runtime
        .eval(
            r#"(a-set! "snapshot" (hash-insert (a-get "snapshot") "hooks"
                (hash "7" (hash "phase" "Waiting" "waiting_on" "Approve\n this?" "detail" "stale tool"))))"#,
        )
        .unwrap();
    let waiting = info(&mut plugin, row(7, '!', true));
    assert_eq!(waiting["status"], "needs input");
    assert_eq!(waiting["activity"], "Approve this?");
    assert_eq!(waiting["tracking"], "hooks");
}

#[test]
fn agents_activity_shows_summary_question_only_for_canonical_waiting_rows() {
    let mut plugin = plugin(json!({
        "panes": [{"pane_id": 7, "title": "Current task"}],
        "activity": {"7": [12, 5]},
        "summaries": {"7": {"text": " Choose\n a\t deployment target. ", "awaiting": true, "hash": 12}}
    }));
    assert_eq!(
        info(&mut plugin, row(7, '!', true)),
        json!({"status": "needs input", "activity": "Choose a deployment target.",
            "task": "Current task", "tracking": "screen summary"})
    );
    for symbol in ['●', '○'] {
        let result = info(&mut plugin, row(7, symbol, false));
        assert_eq!(result["activity"], "Current task");
        assert_eq!(result["tracking"], "screen activity");
    }
}

#[test]
fn agents_activity_does_not_turn_quiet_or_stale_hooks_into_completed_work() {
    let mut plugin = plugin(json!({
        "panes": [{"pane_id": 7, "title": "Current task"}],
        "hooks": {"7": {"phase": "Waiting", "waiting_on": "Old question", "detail": "Old tool"}},
        "activity": {"7": [12, 100]}
    }));
    for (symbol, status) in [
        ('●', "working"),
        ('○', "idle"),
        ('■', "held"),
        ('×', "exited"),
    ] {
        let result = info(&mut plugin, row(7, symbol, false));
        assert_eq!(result["status"], status);
        assert_eq!(result["activity"], "Current task");
        assert_eq!(
            result["tracking"],
            if matches!(symbol, '■' | '×') {
                "pane status"
            } else {
                "screen activity"
            }
        );
    }
    let missing = info(&mut plugin, row(99, '?', false));
    assert_eq!(
        missing,
        json!({"status": "unknown", "activity": "", "task": "",
        "tracking": "no activity yet"})
    );
}

#[test]
fn agents_normal_list_has_no_heading_but_waiting_keeps_its_heading() {
    let mut plugin = plugin(json!({}));
    set(
        &mut plugin,
        "rows",
        json!([row(7, '●', false), row(8, '!', true)]),
    );
    plugin.render(24, 90);
    assert_eq!(state(&mut plugin, "last_list_rows"), 23);
    assert_eq!(key(&mut plugin, "w"), true);
    plugin.render(24, 90);
    assert_eq!(state(&mut plugin, "last_list_rows"), 22);
}

fn set(plugin: &mut SteelReference, key: &str, value: Value) {
    plugin
        .runtime
        .invoke("a-set!", vec![json!(key), value])
        .unwrap();
}

fn state(plugin: &mut SteelReference, key: &str) -> Value {
    plugin.runtime.invoke("a-get", vec![json!(key)]).unwrap()
}

fn key(plugin: &mut SteelReference, char: &str) -> Value {
    let selection = plugin.runtime.invoke("a-selection-key", vec![]).unwrap();
    let changed = plugin
        .runtime
        .invoke(
            "a-on-key!",
            vec![json!({
                "bare_key": {"Char": char}, "key_modifiers": []
            })],
        )
        .unwrap();
    // The plugin's update callback follows selection after dispatching a key.
    plugin
        .runtime
        .invoke("a-follow-selection!", vec![selection])
        .unwrap();
    changed
}

#[test]
fn agents_centered_rows_never_open_on_an_orphaned_activity_line() {
    let mut plugin = plugin(busy(1..=20));
    let mut rows = vec![json!({"Project": "project"}), json!({"Branch": "  topic"})];
    rows.extend((1..=20).map(|pane| row(pane, '●', false)));
    set(&mut plugin, "rows", json!(rows));
    for selected in 2..22 {
        set(&mut plugin, "selected", json!(selected));
        set(&mut plugin, "list_scroll", json!(false));
        plugin.render(24, 90);
        let lines = state(&mut plugin, "list_lines");
        let lines = lines.as_array().unwrap();
        let start = state(&mut plugin, "last_window_start").as_u64().unwrap() as usize;
        let height = state(&mut plugin, "last_list_window").as_u64().unwrap() as usize;
        assert_eq!(lines[start][1], false, "selected {selected}");
        let window = &lines[start..lines.len().min(start + height)];
        assert!(
            window.contains(&json!([selected, false])),
            "selected {selected}"
        );
        assert!(
            window.contains(&json!([selected, true])),
            "selected {selected}"
        );
    }
}

#[test]
fn agents_next_waiting_wraps_and_leaves_terminal_focus_unchanged() {
    let mut plugin = plugin(json!({}));
    set(
        &mut plugin,
        "rows",
        json!([
            {"Project": "project"}, row(7, '●', false), row(8, '!', true),
            {"Branch": "  another"}, row(9, '○', false), row(10, '!', true)
        ]),
    );
    set(&mut plugin, "selected", json!(1));
    set(&mut plugin, "focused", json!(7));
    set(&mut plugin, "focus", json!("history"));
    set(&mut plugin, "list_scroll", json!(3));
    plugin.take_commands();
    for selected in [2, 5, 2] {
        assert_eq!(key(&mut plugin, "n"), true);
        assert_eq!(state(&mut plugin, "selected"), selected);
        assert_eq!(state(&mut plugin, "focus"), "list");
        assert_eq!(state(&mut plugin, "list_scroll"), false);
        assert_eq!(state(&mut plugin, "focused"), 7);
    }
    assert!(plugin.take_commands().is_empty());
}

#[test]
fn agents_next_waiting_restores_all_rows_from_waiting_and_search_filters() {
    let mut plugin = plugin(json!({}));
    let rows = json!([
        {"Project": "project"}, row(7, '●', false), row(8, '!', true), row(9, '!', true)
    ]);
    set(&mut plugin, "rows", rows.clone());
    set(&mut plugin, "selected", json!(2));
    assert_eq!(key(&mut plugin, "w"), true);
    assert_ne!(state(&mut plugin, "waiting_base"), false);
    assert_eq!(key(&mut plugin, "n"), true);
    assert_eq!(state(&mut plugin, "waiting_base"), false);
    assert_eq!(state(&mut plugin, "rows"), rows);
    assert_eq!(state(&mut plugin, "selected"), 3);

    set(
        &mut plugin,
        "search_base",
        json!({"rows": rows, "summaries": {}, "focused": null}),
    );
    set(
        &mut plugin,
        "rows",
        json!([{"Run": {"run_id": "search-hit"}}]),
    );
    set(&mut plugin, "selected", json!(0));
    set(&mut plugin, "search_query", json!("previous task"));
    set(&mut plugin, "applied_search_query", json!("previous task"));
    plugin.take_commands();
    assert_eq!(key(&mut plugin, "n"), true);
    assert_eq!(state(&mut plugin, "search_base"), false);
    assert_eq!(state(&mut plugin, "search_query"), "");
    assert_eq!(state(&mut plugin, "applied_search_query"), false);
    assert_eq!(state(&mut plugin, "rows"), rows);
    assert_eq!(state(&mut plugin, "selected"), 2);
    assert!(plugin.take_commands().is_empty());
}

#[test]
fn agents_next_waiting_is_a_noop_without_waiting_agents() {
    let mut plugin = plugin(json!({}));
    let rows = json!([row(7, '●', false), row(8, '○', false)]);
    set(&mut plugin, "rows", rows.clone());
    set(&mut plugin, "selected", json!(1));
    set(&mut plugin, "focus", json!("history"));
    set(&mut plugin, "list_scroll", json!(1));
    plugin.take_commands();
    assert_eq!(key(&mut plugin, "n"), false);
    assert_eq!(state(&mut plugin, "selected"), 1);
    assert_eq!(state(&mut plugin, "rows"), rows);
    assert_eq!(state(&mut plugin, "focus"), "history");
    assert_eq!(state(&mut plugin, "list_scroll"), 1);
    assert!(plugin.take_commands().is_empty());
}

#[test]
fn agents_activity_keeps_words_apart_across_bare_carriage_returns() {
    let mut plugin = plugin(json!({"panes": [{"pane_id": 7, "title": "Fix\rthe\r\nissue"}]}));
    assert_eq!(
        info(&mut plugin, row(7, '○', false))["task"],
        "Fix the issue"
    );
}

fn mouse(plugin: &mut SteelReference, kind: &str, point: Value) {
    plugin
        .runtime
        .invoke("a-on-mouse!", vec![json!({kind: point})])
        .unwrap();
}

#[test]
fn agents_bottom_window_stays_put_when_a_click_or_wheel_pins_it() {
    let mut plugin = plugin(busy(1..=20));
    let mut rows = vec![json!({"Project": "project"}), json!({"Branch": "  topic"})];
    rows.extend((1..=20).map(|pane| row(pane, '●', false)));
    set(&mut plugin, "rows", json!(rows));
    set(&mut plugin, "selected", json!(21));
    set(&mut plugin, "focus", json!("history"));
    // 21 list rows leave a 19-line window between the scroll cues. The last full window would
    // open on the eleventh agent's activity line, so it starts one line later, on the twelfth.
    plugin.render(22, 90);
    assert_eq!(state(&mut plugin, "last_list_rows"), 21);
    assert_eq!(state(&mut plugin, "last_list_window"), 19);
    assert_eq!(state(&mut plugin, "last_window_start"), 23);
    assert_eq!(state(&mut plugin, "last_list_max_scroll"), 23);
    // The last agent's line: one merged heading line precedes the agents, and the top cue
    // row precedes the window.
    let point = json!([39 - 23 + 1, 5]);
    plugin.take_commands();
    mouse(&mut plugin, "LeftClick", point.clone());
    mouse(&mut plugin, "Release", point.clone());
    plugin.render(22, 90);
    assert_eq!(state(&mut plugin, "last_window_start"), 23);
    assert_eq!(state(&mut plugin, "selected"), 21);
    mouse(&mut plugin, "LeftClick", point.clone());
    mouse(&mut plugin, "Release", point);
    assert!(plugin
        .take_commands()
        .iter()
        .any(|command| command.get("FocusTerminalPane") == Some(&json!([20, false, false]))));

    set(&mut plugin, "list_scroll", json!(false));
    plugin.render(22, 90);
    mouse(&mut plugin, "ScrollDown", json!(1));
    assert_eq!(state(&mut plugin, "list_scroll"), false);
    mouse(&mut plugin, "ScrollUp", json!(1));
    assert_eq!(state(&mut plugin, "list_scroll"), 22);
}

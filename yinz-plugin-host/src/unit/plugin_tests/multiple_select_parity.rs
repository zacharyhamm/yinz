use super::runtime_parity::{default_reply, Pair};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use yinz_types::{
    data::{Event, ModeInfo, PaneInfo, PaneManifest, TabInfo},
    input::actions::Action,
    internal_ids::{ClientId, PluginId, TabPosition, TerminalId},
    pane_id::PaneId,
    position::{ColumnCount, RowCount},
    units::FractionalSeconds,
};

fn pair() -> (Pair, Arc<AtomicU64>) {
    let clock = Arc::new(AtomicU64::new(1_000));
    (
        Pair::with_host(
            "multiple-select",
            BTreeMap::new(),
            Arc::new(default_reply),
            Some(clock.clone()),
        ),
        clock,
    )
}
fn panes(count: usize, other_client: bool, own_tab: usize, tabs: usize) -> Event {
    let mut panes = vec![PaneInfo {
        id: PaneId::Plugin(PluginId::new(1)),
        ..Default::default()
    }];
    for index in 0..count {
        let mut groups = BTreeMap::from([(ClientId::new(1), count - index - 1)]);
        if other_client {
            groups.insert(ClientId::new(2), index);
        }
        panes.push(PaneInfo {
            id: PaneId::Terminal(TerminalId::new(index as u32 + 10)),
            index_in_pane_group: groups,
            ..Default::default()
        });
    }
    if other_client {
        panes.push(PaneInfo {
            id: PaneId::Plugin(PluginId::new(8)),
            index_in_pane_group: BTreeMap::from([(ClientId::new(2), count)]),
            ..Default::default()
        });
    }
    let mut manifest = PaneManifest::default();
    for tab in 0..tabs {
        manifest.panes.insert(TabPosition::new(tab), vec![]);
    }
    manifest.panes.insert(TabPosition::new(own_tab), panes);
    Event::PaneUpdate(manifest)
}
fn press(pair: &mut Pair, key: &str) -> Vec<serde_json::Value> {
    pair.event(Event::InterceptedKeyPress(key.parse().unwrap()))
}

#[test]
fn multiple_select_actual_wasm_and_steel_match_group_actions_and_closing() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "multiple_select_actual_wasm_and_steel_match_group_actions_and_closing",
    );
    for key in ["b", "s", "f", "e", "c", "Esc"] {
        let (mut pair, _) = pair();
        pair.event(panes(3, true, 1, 3));
        pair.render(16, 90);
        let commands = press(&mut pair, key);
        assert!(
            commands.contains(&json!("CloseSelf")),
            "{key}: {commands:?}"
        );
        assert!(pair.event(panes(2, false, 1, 3)).is_empty());
        assert!(press(&mut pair, "b").is_empty());
    }
    for (tab, tabs) in [(0, 1), (0, 3), (1, 3), (2, 3)] {
        for key in ["l", "r"] {
            let (mut pair, _) = pair();
            pair.event(panes(2, true, tab, tabs));
            assert!(press(&mut pair, key).contains(&json!("CloseSelf")));
        }
    }
    let (mut pair, _) = pair();
    press(&mut pair, "l");
    press(&mut pair, "r");
    pair.event(panes(0, false, 0, 1));
    assert!(press(&mut pair, "Tab").is_empty());
}

#[test]
fn multiple_select_actual_wasm_and_steel_match_viewports_bindings_and_coordinates() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "multiple_select_actual_wasm_and_steel_match_viewports_bindings_and_coordinates",
    );
    let (mut pair, _) = pair();
    pair.viewports();
    pair.event(panes(0, true, 0, 1));
    pair.viewports();
    pair.event(panes(1, true, 0, 1));
    pair.viewports();
    let mut mode = ModeInfo::default();
    mode.keybinds = vec![(
        mode.mode,
        vec![
            (
                "Ctrl Alt x".parse().unwrap(),
                vec![Action::TogglePaneInGroup, Action::Quit],
            ),
            (
                "Ctrl Alt y".parse().unwrap(),
                vec![Action::ToggleGroupMarking],
            ),
        ],
    )];
    for following in [false, true] {
        mode.currently_marking_pane_group = Some(following);
        pair.event(Event::ModeUpdate(mode.clone()));
        pair.viewports();
        pair.event(Event::ModeUpdate(mode.clone()));
    }
    mode.keybinds[0].1[0].0 = "Ctrl Left".parse().unwrap();
    mode.keybinds[0].1[1].0 = "Alt 雪".parse().unwrap();
    pair.event(Event::ModeUpdate(mode));
    pair.viewports();
    for key in ["Ctrl b", "Alt s", "Shift Tab", "x"] {
        press(&mut pair, key);
    }
    pair.key("b"); // Only intercepted keys perform actions.
    for (rows, cols) in [(60, 240), (10, 30)] {
        pair.event(Event::TabUpdate(vec![TabInfo {
            active: true,
            display_area_rows: RowCount::new(rows),
            display_area_columns: ColumnCount::new(cols),
            ..Default::default()
        }]));
        press(&mut pair, "Tab");
        press(&mut pair, "Tab");
    }
}

#[test]
fn multiple_select_actual_wasm_and_steel_match_highlight_deadlines() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "multiple_select_actual_wasm_and_steel_match_highlight_deadlines",
    );
    let (mut pair, clock) = pair();
    pair.event(Event::Timer(FractionalSeconds::new(0.4)));
    pair.event(panes(1, true, 0, 1));
    let commands = pair.event(panes(2, true, 0, 1));
    assert!(commands.contains(&json!({"SetTimeout": 0.4})));
    clock.store(1_399, Ordering::SeqCst);
    pair.event(Event::Timer(FractionalSeconds::new(0.4)));
    let commands = pair.event(panes(3, true, 0, 1));
    assert!(!commands
        .iter()
        .any(|command| command.get("SetTimeout").is_some()));
    clock.store(1_400, Ordering::SeqCst);
    pair.event(Event::Timer(FractionalSeconds::new(0.4)));
    let commands = pair.event(panes(2, true, 0, 1));
    assert!(commands.contains(&json!({"SetTimeout": 0.4})));
    clock.store(1_800, Ordering::SeqCst);
    pair.event(Event::Timer(FractionalSeconds::new(0.4)));
    pair.event(panes(0, true, 0, 1));
    pair.render(18, 90);
}

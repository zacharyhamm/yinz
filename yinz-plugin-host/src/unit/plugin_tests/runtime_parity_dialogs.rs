use super::runtime_parity::{default_reply, Responder, SteelReference, WasmReference};
use prost::Message;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};
use yinz_plugin_api::{
    plugin_command::ProtobufOpenTerminalFloatingResponse, plugin_ids::ProtobufPluginIds,
};
use yinz_terminal::panes::{grid::Grid, terminal_character::TerminalCharacter};
use yinz_types::{
    data::{Event, ModeInfo, Mouse, PluginCommand},
    keys::{BareKey, KeyModifier, KeyWithModifier},
};

fn cells(text: &str, rows: usize, columns: usize) -> Vec<Vec<TerminalCharacter>> {
    let mut grid = Grid::new(
        rows.max(1),
        columns.max(1),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        ModeInfo::default().style,
        false,
        true,
        true,
        true,
        false,
    );
    vte::Parser::new().advance(&mut grid, text.replace('\n', "\n\r").as_bytes());
    grid.as_character_lines()
}
fn compare_frame(
    wasm: &mut WasmReference,
    steel: &mut SteelReference,
    rows: usize,
    columns: usize,
) {
    let expected = wasm.render(rows, columns);
    let actual = steel.render(rows, columns);
    let left = cells(&expected, rows, columns);
    let right = cells(&actual, rows, columns);
    if left != right {
        let first = left
            .iter()
            .zip(&right)
            .enumerate()
            .find_map(|(row, (a, b))| {
                a.iter()
                    .zip(b)
                    .enumerate()
                    .find(|(_, (a, b))| a != b)
                    .map(|(column, (a, b))| (row, column, a, b))
            });
        panic!("{rows}x{columns} differs at {first:?}\nWasm: {expected:?}\nSteel: {actual:?}");
    }
    assert_eq!(steel.take_commands(), wasm.take_commands());
}
fn compare_event(wasm: &mut WasmReference, steel: &mut SteelReference, event: Event) {
    assert_eq!(
        steel.update(&event),
        wasm.update(&event),
        "redraw decision for {event:?}"
    );
    assert_eq!(
        steel.take_commands(),
        wasm.take_commands(),
        "commands for {event:?}"
    );
}
fn configuration(notice: Option<&str>, last_pane: bool) -> BTreeMap<String, String> {
    let mut configuration = BTreeMap::new();
    if let Some(notice) = notice {
        configuration.insert("notice".to_owned(), notice.to_owned());
    }
    if last_pane {
        configuration.insert("reason".to_owned(), "last_pane".to_owned());
    }
    configuration
}

#[test]
fn quit_confirmation_actual_wasm_steel_dialog_cells_and_viewports() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "quit_confirmation_actual_wasm_steel_dialog_cells_and_viewports",
    );
    let root = tempfile::tempdir().unwrap();
    for last_pane in [false, true] {
        for notice in [
            None,
            Some(""),
            Some("agent is working\nanother agent is busy"),
            Some("日本語\tworking\u{1b}[31m!"),
        ] {
            let config = configuration(notice, last_pane);
            let mut wasm = WasmReference::new(
                "quit-confirmation",
                config.clone(),
                root.path(),
                Arc::new(default_reply),
            );
            let mut steel = SteelReference::new(
                "quit-confirmation",
                config,
                root.path(),
                Arc::new(default_reply),
            );
            assert_eq!(steel.take_commands(), wasm.take_commands());
            for rows in 0..=10 {
                for columns in [0, 1, 2, 4, 5, 8, 10, 14, 15, 16, 20, 40, 80] {
                    compare_frame(&mut wasm, &mut steel, rows, columns);
                }
            }
        }
    }
}

#[test]
fn quit_confirmation_actual_wasm_steel_keyboard_decisions_and_commands() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "quit_confirmation_actual_wasm_steel_keyboard_decisions_and_commands",
    );
    let root = tempfile::tempdir().unwrap();
    let mut wasm = WasmReference::new(
        "quit-confirmation",
        configuration(Some("busy"), true),
        root.path(),
        Arc::new(default_reply),
    );
    let mut steel = SteelReference::new(
        "quit-confirmation",
        configuration(Some("busy"), true),
        root.path(),
        Arc::new(default_reply),
    );
    assert_eq!(steel.take_commands(), wasm.take_commands());
    for modifiers in [
        vec![],
        vec![KeyModifier::Shift],
        vec![KeyModifier::Ctrl],
        vec![KeyModifier::Alt],
        vec![KeyModifier::Ctrl, KeyModifier::Shift],
    ] {
        for bare in [
            BareKey::Enter,
            BareKey::Tab,
            BareKey::Left,
            BareKey::Right,
            BareKey::Up,
            BareKey::Down,
            BareKey::Esc,
            BareKey::Char(' '),
            BareKey::Char('c'),
            BareKey::Char('x'),
        ] {
            let mut key = KeyWithModifier::new(bare);
            key.key_modifiers = modifiers.iter().copied().collect();
            compare_event(&mut wasm, &mut steel, Event::Key(key));
            compare_frame(&mut wasm, &mut steel, 8, 40);
        }
    }
}

#[test]
fn quit_confirmation_actual_wasm_steel_mouse_hit_testing_and_drag_release() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "quit_confirmation_actual_wasm_steel_mouse_hit_testing_and_drag_release",
    );
    let root = tempfile::tempdir().unwrap();
    let mut wasm = WasmReference::new(
        "quit-confirmation",
        BTreeMap::new(),
        root.path(),
        Arc::new(default_reply),
    );
    let mut steel = SteelReference::new(
        "quit-confirmation",
        BTreeMap::new(),
        root.path(),
        Arc::new(default_reply),
    );
    assert_eq!(steel.take_commands(), wasm.take_commands());
    for columns in [1, 8, 14, 20, 40] {
        compare_frame(&mut wasm, &mut steel, 7, columns);
        for row in 0..7 {
            for column in 0..columns {
                for event in [
                    Mouse::LeftClick(row, column),
                    Mouse::Hold(row, column),
                    Mouse::Release(row, column),
                    Mouse::Hover(row, column),
                    Mouse::RightClick(row, column),
                ] {
                    compare_event(&mut wasm, &mut steel, Event::Mouse(event));
                    compare_frame(&mut wasm, &mut steel, 7, columns);
                }
            }
        }
    }
}

#[test]
fn shared_test_actual_wasm_steel_acting_clients_render_sweeps_and_actions() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "shared_test_actual_wasm_steel_acting_clients_render_sweeps_and_actions",
    );
    let root = tempfile::tempdir().unwrap();
    let client = Arc::new(AtomicU32::new(1));
    let acting = client.clone();
    let responder: Responder = Arc::new(move |command| match command {
        PluginCommand::GetPluginIds => Some(
            ProtobufPluginIds {
                plugin_id: 42,
                client_id: acting.load(Ordering::SeqCst),
                ..Default::default()
            }
            .encode_to_vec(),
        ),
        PluginCommand::OpenTerminalFloating(..) => {
            Some(ProtobufOpenTerminalFloatingResponse::default().encode_to_vec())
        },
        _ => default_reply(command),
    });
    let mut wasm = WasmReference::new(
        "shared-test",
        BTreeMap::new(),
        root.path(),
        responder.clone(),
    );
    let mut steel = SteelReference::new("shared-test", BTreeMap::new(), root.path(), responder);
    steel.grant_permissions();
    let expected = wasm.take_commands();
    let actual = steel.take_commands();
    // The Scheme permission waiter retains a private event subscription until the host grants it.
    let without_permission_bookkeeping = |commands: Vec<Value>| {
        commands
            .into_iter()
            .filter(|command| {
                command != &json!({"Subscribe":["PermissionRequestResult"]})
                    && command != &json!({"Unsubscribe":["PermissionRequestResult"]})
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(without_permission_bookkeeping(actual), expected);
    for id in [1, 10, 1, 10, 2, 10, 2, 1] {
        client.store(id, Ordering::SeqCst);
        compare_frame(&mut wasm, &mut steel, 10, 120);
        for bare in [
            BareKey::Char('x'),
            BareKey::Char('m'),
            BareKey::Char('f'),
            BareKey::Esc,
        ] {
            compare_event(
                &mut wasm,
                &mut steel,
                Event::Key(KeyWithModifier::new(bare)),
            );
            compare_frame(&mut wasm, &mut steel, 10, 120);
        }
    }
}

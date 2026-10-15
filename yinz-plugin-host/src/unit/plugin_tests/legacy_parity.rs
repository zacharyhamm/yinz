use super::runtime_parity::Pair;
use serde_json::json;
use std::collections::BTreeMap;
use yinz_types::data::{Event, FileMetadata, Mouse};
fn directory() -> Event {
    Event::FileSystemUpdate(vec![
        (
            "/host/docs".into(),
            Some(FileMetadata {
                is_dir: true,
                ..Default::default()
            }),
        ),
        (
            "/host/alpha.rs".into(),
            Some(FileMetadata {
                is_file: true,
                len: 13000,
                ..Default::default()
            }),
        ),
        (
            "/host/zebra.txt".into(),
            Some(FileMetadata {
                is_file: true,
                len: 1024,
                ..Default::default()
            }),
        ),
        (
            "/host/.env".into(),
            Some(FileMetadata {
                is_file: true,
                len: 14,
                ..Default::default()
            }),
        ),
        (
            "/host/symlink".into(),
            Some(FileMetadata {
                is_symlink: true,
                ..Default::default()
            }),
        ),
    ])
}

#[test]
fn actual_wasm_and_steel_strider_navigation_search_and_file_selection() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "actual_wasm_and_steel_strider_navigation_search_and_file_selection",
    );
    let mut pair = Pair::new("strider", BTreeMap::new());
    pair.event(directory());
    pair.render(20, 100);
    pair.key("Down");
    pair.render(20, 100);
    pair.key("Right");
    pair.event(Event::HostFolderChanged("/work/docs".into()));
    pair.key("Left");
    pair.event(directory());
    pair.key("a");
    pair.render(20, 100);
    pair.key("r");
    pair.key("s");
    pair.render(20, 100);
    let commands = pair.key("Enter");
    assert!(commands
        .iter()
        .any(|command| command.get("OpenFile").is_some()));
}

#[test]
fn actual_wasm_and_steel_strider_filepicker_and_standalone_open() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "actual_wasm_and_steel_strider_filepicker_and_standalone_open",
    );
    for source in [json!({"Plugin":8}), json!({"Cli":"pipe-8"})] {
        let mut pair = Pair::new("strider", BTreeMap::new());
        pair.pipe(serde_json::from_value(json!({"source":source,"name":"filepicker","payload":null,"args":{"request":"pick-42"},"is_private":true})).unwrap());
        pair.event(directory());
        pair.key("z");
        pair.key("Enter");
    }
    let mut pair = Pair::new(
        "strider",
        BTreeMap::from([("close_on_selection".into(), "true".into())]),
    );
    pair.key("Enter");
    pair.event(directory());
    pair.key("a");
    pair.key("Enter");
}

#[test]
fn actual_wasm_and_steel_configuration_leader_edits_and_unlock_capture() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "actual_wasm_and_steel_configuration_leader_edits_and_unlock_capture",
    );
    let mut pair = Pair::new("configuration", BTreeMap::new());
    pair.mode(false);
    pair.viewports();
    pair.key("Down");
    pair.key("Space");
    pair.key("Down");
    pair.key("Down");
    pair.key("Space");
    pair.viewports();
    pair.key("Enter");
    pair.viewports();
    pair.key("Esc");
    pair.mode(true);
    pair.viewports();
    pair.key("Down");
    pair.key("Space");
    pair.viewports();
    pair.key("Alt Esc");
    pair.key("Ctrl a");
    pair.viewports();
}

#[test]
fn actual_wasm_and_steel_configuration_presets_and_setup_wizard() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "actual_wasm_and_steel_configuration_presets_and_setup_wizard",
    );
    let mut pair = Pair::new("configuration", BTreeMap::new());
    pair.mode(false);
    pair.key("Tab");
    pair.viewports();
    pair.key("Down");
    pair.key("Enter");
    pair.viewports();
    pair.key("Esc");
    pair.key("Down");
    pair.key("Down");
    pair.key("Ctrl a");
    let mut wizard = Pair::new(
        "configuration",
        BTreeMap::from([("is_setup_wizard".into(), "true".into())]),
    );
    wizard.mode(false);
    wizard.viewports();
    wizard.key("Down");
    wizard.key("Enter");
}

#[test]
fn actual_wasm_and_steel_strider_hidden_files_live_search_and_scrolling() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "actual_wasm_and_steel_strider_hidden_files_live_search_and_scrolling",
    );
    let mut pair = Pair::new(
        "strider",
        BTreeMap::from([("show_hidden_files".into(), "true".into())]),
    );
    pair.event(directory());
    pair.viewports();
    pair.key("Ctrl e");
    pair.event(directory());
    pair.viewports();
    pair.key("t");
    pair.key("x");
    pair.key("t");
    pair.event(Event::FileSystemUpdate(vec![(
        "/host/new.txt".into(),
        Some(FileMetadata {
            is_file: true,
            len: 14,
            ..Default::default()
        }),
    )]));
    pair.render(20, 80);
    pair.key("Backspace");
    pair.render(20, 80);
    pair.key("Ctrl c");
    pair.key("Ctrl c");
    pair.key("/");
    pair.event(Event::FileSystemUpdate(
        (0..20)
            .map(|n| {
                (
                    format!("/host/file{n:02}.txt").into(),
                    Some(FileMetadata {
                        is_file: true,
                        len: n,
                        ..Default::default()
                    }),
                )
            })
            .collect(),
    ));
    pair.render(10, 80);
    for _ in 0..12 {
        pair.event(Event::Mouse(Mouse::ScrollDown(1)));
    }
    pair.render(10, 80);
    pair.event(Event::Mouse(Mouse::Hover(4, 0)));
    pair.render(10, 80);
    pair.event(Event::Mouse(Mouse::LeftClick(4, 0)));
    pair.render(10, 80);
}

#[test]
fn actual_wasm_and_steel_configuration_notifications_and_custom_bindings() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "actual_wasm_and_steel_configuration_notifications_and_custom_bindings",
    );
    let mut pair = Pair::new("configuration", BTreeMap::new());
    pair.mode(false);
    let mut custom = pair.mode.clone();
    custom.keybinds.iter_mut().for_each(|(_, bindings)| {
        bindings.iter_mut().for_each(|(key, _)| {
            if !key.key_modifiers.is_empty() {
                key.key_modifiers = "Super Shift g"
                    .parse::<yinz_types::data::KeyWithModifier>()
                    .unwrap()
                    .key_modifiers;
            }
        })
    });
    pair.event(Event::ModeUpdate(custom.clone()));
    pair.viewports();
    custom.keybinds.clear();
    pair.event(Event::ModeUpdate(custom));
    pair.viewports();
    pair.event(Event::FailedToWriteConfigToDisk {
        file_path: Some("/tmp/config.kdl".into()),
        reason: Some("read only".into()),
    });
    pair.viewports();
    pair.key("Down");
    pair.viewports();
    pair.key("Enter");
    pair.commands();
}

#[test]
fn strider_rejects_legacy_clicks_outside_the_file_list() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "strider_rejects_legacy_clicks_outside_the_file_list",
    );
    let mut pair = Pair::new("strider", BTreeMap::new());
    pair.event(directory());
    pair.render(20, 80);
    // The original maps header clicks to the first entry and opens it on a second click.
    // Rejecting those coordinates prevents opening an unrelated path.
    for _ in 0..2 {
        assert!(pair.wasm.update(&Event::Mouse(Mouse::LeftClick(0, 0))));
        assert!(!pair.steel.update(&Event::Mouse(Mouse::LeftClick(0, 0))));
    }
    assert!(pair
        .wasm
        .take_commands()
        .iter()
        .any(|command| command.get("ChangeHostFolder").is_some()));
    assert!(pair.steel.take_commands().is_empty());
}

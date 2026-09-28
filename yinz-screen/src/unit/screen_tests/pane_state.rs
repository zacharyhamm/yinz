//! Tests for the pane and tab state the screen reports, and for the sizes it derives.
//!
//! Salvaged from the since-removed global-pane suites, which used one only as scaffolding: none
//! of what is asserted here was ever about it.

use super::*;
use yinz_types::internal_ids::PluginId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabPosition;

/// `CwdChanged` announces a move once. The manifest says where every pane is
/// every time, which is what puts right a plugin that was not listening yet.
#[test]
fn a_pane_manifest_carries_the_pane_cwd() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, TabId::new(0));
    screen.terminal_cwds.insert(
        crate::TerminalId::new(1),
        std::path::PathBuf::from("/work/hocus"),
    );

    let manifest = screen.generate_and_report_pane_state().expect("TEST");

    let pane = manifest
        .panes
        .values()
        .flatten()
        .find(|pane| pane.id == PaneId::Terminal(crate::TerminalId::new(1)))
        .expect("the terminal pane is in the manifest");
    assert_eq!(pane.cwd.as_deref(), Some("/work/hocus"));
}

/// A pane that named a VM and no worktree of its own gets the one the provider named,
/// so every reader of the manifest can say which checkout its guest folder is a copy of.
/// The pane's own `invoked_with` is what it *asked* for, and a pane joining a VM
/// somebody else provisioned asked for a VM by name and nothing else.
#[test]
fn a_pane_manifest_carries_the_worktree_a_firecracker_vm_mirrors() {
    use yinz_types::command::PaneTarget;
    use yinz_types::input::command::RunCommand;
    use yinz_types::input::layout::Run;

    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    let joined = RunCommand {
        command: std::path::PathBuf::from("claude"),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: None,
        },
        ..Default::default()
    };
    screen
        .new_tab(
            TabId::new(0),
            (vec![], vec![]),
            None,
            Some(crate::ClientId::new(1)),
        )
        .unwrap();
    screen
        .apply_layout(
            TiledPaneLayout {
                run: Some(Run::Command(joined)),
                ..TiledPaneLayout::default()
            },
            vec![],
            vec![(crate::TerminalId::new(1), None)],
            vec![],
            HashMap::new(),
            TabId::new(0),
            true,
            crate::ClientId::new(1),
            None,
        )
        .expect("TEST");
    screen
        .set_pane_host(
            crate::TerminalId::new(1),
            Some("firecracker:review".to_owned()),
            Some(std::path::PathBuf::from("/work/hocus")),
        )
        .unwrap();

    let manifest = screen.generate_and_report_pane_state().expect("TEST");
    let pane = manifest
        .panes
        .values()
        .flatten()
        .find(|pane| pane.id == PaneId::Terminal(crate::TerminalId::new(1)))
        .expect("the terminal pane is in the manifest");
    assert_eq!(
        pane.target.firecracker_worktree(),
        Some(std::path::Path::new("/work/hocus"))
    );
    assert_eq!(pane.host.as_deref(), Some("firecracker:review"));
}

/// A plugin is not a process in a folder, and a plugin id shares its number
/// space with a terminal id — so an unguarded lookup would hand it one.
#[test]
fn a_plugin_pane_has_no_cwd() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, TabId::new(0));
    screen.terminal_cwds.insert(
        crate::TerminalId::new(1),
        std::path::PathBuf::from("/work/hocus"),
    );

    let manifest = screen.generate_and_report_pane_state().expect("TEST");

    assert!(
        manifest
            .panes
            .values()
            .flatten()
            .filter(|pane| pane.is_plugin())
            .all(|pane| pane.cwd.is_none()),
        "no plugin pane was given a folder"
    );
}

#[test]
fn recorder_pane_metadata() {
    use {yinz_types::data::PaneInfo, yinz_types::data::PaneManifest, yinz_types::data::TabInfo};
    // Create one tab with one terminal pane and one plugin pane
    let mut pane_manifest = PaneManifest::default();
    let terminal_pane = PaneInfo {
        id: yinz_types::pane_id::PaneId::from_legacy(5, false),
        title: "my terminal".to_string(),
        pane_content_rows: yinz_types::position::RowCount::new(24),
        pane_content_columns: yinz_types::position::ColumnCount::new(80),
        ..Default::default()
    };
    let plugin_pane = PaneInfo {
        id: yinz_types::pane_id::PaneId::from_legacy(6, true),
        title: "plugin".to_string(),
        pane_content_rows: yinz_types::position::RowCount::new(24),
        pane_content_columns: yinz_types::position::ColumnCount::new(80),
        ..Default::default()
    };
    pane_manifest.panes.insert(
        TabPosition::new(0),
        vec![terminal_pane.clone(), plugin_pane],
    );
    let tab_info = TabInfo {
        position: TabPosition::new(0),
        name: "my tab".to_string(),
        ..Default::default()
    };
    let metadata = crate::screen::Screen::convert_pane_metadata(
        &pane_manifest,
        std::slice::from_ref(&tab_info),
    );
    assert_eq!(metadata.len(), 1, "should filter plugin panes");
    assert_eq!(
        metadata[0].terminal_id,
        yinz_types::internal_ids::TerminalId::new(5)
    );
    assert_eq!(metadata[0].tab_id, TabId::new(0));
    assert_eq!(metadata[0].tab_position, TabPosition::new(0));
    assert_eq!(metadata[0].tab_name, "my tab");
    assert_eq!(metadata[0].title, "my terminal");
    assert_eq!(metadata[0].cols, 80);
    assert_eq!(metadata[0].rows, 24);
}

#[test]
fn plugin_bytes_with_no_pane_are_cached_without_pipe_acknowledgements() {
    let size = Size { cols: 60, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    let unhandled = screen
        .handle_plugin_render_assets(vec![PluginRenderAsset::new(
            PluginId::new(99),
            crate::ClientId::new(1),
            b"ORPHAN_BYTES".to_vec(),
        )])
        .unwrap();
    assert_eq!(unhandled.len(), 1);
    assert_eq!(unhandled[0].bytes, b"ORPHAN_BYTES");
}

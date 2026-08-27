//! Tests for the pane and tab state the screen reports, and for the sizes it derives.
//!
//! Salvaged from the since-removed global-pane suites, which used one only as scaffolding: none
//! of what is asserted here was ever about it.

use super::*;

/// `CwdChanged` announces a move once. The manifest says where every pane is
/// every time, which is what puts right a plugin that was not listening yet.
#[test]
fn a_pane_manifest_carries_the_pane_cwd() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, 0);
    screen
        .terminal_cwds
        .insert(1, std::path::PathBuf::from("/work/hocus"));

    let manifest = screen.generate_and_report_pane_state().expect("TEST");

    let pane = manifest
        .panes
        .values()
        .flatten()
        .find(|pane| pane.id == 1 && !pane.is_plugin)
        .expect("the terminal pane is in the manifest");
    assert_eq!(pane.cwd.as_deref(), Some("/work/hocus"));
}

/// A plugin is not a process in a folder, and a plugin id shares its number
/// space with a terminal id — so an unguarded lookup would hand it one.
#[test]
fn a_plugin_pane_has_no_cwd() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, 0);
    screen
        .terminal_cwds
        .insert(1, std::path::PathBuf::from("/work/hocus"));

    let manifest = screen.generate_and_report_pane_state().expect("TEST");

    assert!(
        manifest
            .panes
            .values()
            .flatten()
            .filter(|pane| pane.is_plugin)
            .all(|pane| pane.cwd.is_none()),
        "no plugin pane was given a folder"
    );
}

#[test]
fn recorder_pane_metadata() {
    use zellij_utils::data::{PaneInfo, PaneManifest, TabInfo};
    // Create one tab with one terminal pane and one plugin pane
    let mut pane_manifest = PaneManifest::default();
    let terminal_pane = PaneInfo {
        id: 5,
        is_plugin: false,
        title: "my terminal".to_string(),
        pane_content_rows: 24,
        pane_content_columns: 80,
        ..Default::default()
    };
    let plugin_pane = PaneInfo {
        id: 6,
        is_plugin: true,
        title: "plugin".to_string(),
        pane_content_rows: 24,
        pane_content_columns: 80,
        ..Default::default()
    };
    pane_manifest
        .panes
        .insert(0, vec![terminal_pane.clone(), plugin_pane]);
    let tab_info = TabInfo {
        position: 0,
        name: "my tab".to_string(),
        ..Default::default()
    };
    let metadata = crate::screen::Screen::convert_pane_metadata(
        &pane_manifest,
        std::slice::from_ref(&tab_info),
    );
    assert_eq!(metadata.len(), 1, "should filter plugin panes");
    assert_eq!(metadata[0].terminal_id, 5);
    assert_eq!(metadata[0].tab_id, 0);
    assert_eq!(metadata[0].tab_position, 0);
    assert_eq!(metadata[0].tab_name, "my tab");
    assert_eq!(metadata[0].title, "my terminal");
    assert_eq!(metadata[0].cols, 80);
    assert_eq!(metadata[0].rows, 24);
}

#[test]
fn plugin_bytes_with_no_pane_still_unblock_cli_pipes() {
    // regression: unblocking used to be gated on finding a pane to draw the bytes into, so a
    // plugin that rendered without one (a background plugin, or one whose pane was closed
    // mid-render) never unblocked and anything piping to it hung forever
    let size = Size { cols: 60, rows: 10 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(450));

    // no pane exists for plugin 99, and none is ever created for it
    mock_screen
        .to_screen
        .send(ScreenInstruction::PluginBytes(vec![
            PluginRenderAsset::new(99, 1, b"ORPHAN_BYTES".to_vec()),
        ]))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));

    let unblocked_plugin_ids: Vec<_> = plugin_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::UnblockCliPipes(assets) => Some(
                assets
                    .into_iter()
                    .map(|asset| asset.plugin_id)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect();
    mock_screen.teardown(vec![server_thread, screen_thread]);

    assert!(
        unblocked_plugin_ids.contains(&99),
        "a plugin that rendered without a pane must still unblock, got {unblocked_plugin_ids:?}"
    );
}

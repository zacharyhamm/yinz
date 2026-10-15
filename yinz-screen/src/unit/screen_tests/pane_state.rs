//! Tests for the pane and tab state the screen reports, and for the sizes it derives.
//!
//! Salvaged from the since-removed global-pane suites, which used one only as scaffolding: none
//! of what is asserted here was ever about it.

use super::*;
use yinz_types::internal_ids::PluginId;
use yinz_types::internal_ids::TabId;

/// `CwdChanged` announces a move once. The manifest says where every pane is
/// every time, which is what puts right a plugin that was not listening yet.
#[test]
fn a_pane_manifest_carries_the_pane_cwd() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, TabId::new(0));
    screen.terminal_cwds.insert(
        crate::TerminalId::new(1),
        std::path::PathBuf::from("/work/agent"),
    );

    let manifest = screen.generate_and_report_pane_state().expect("TEST");

    let pane = manifest
        .panes
        .values()
        .flatten()
        .find(|pane| pane.id == PaneId::Terminal(crate::TerminalId::new(1)))
        .expect("the terminal pane is in the manifest");
    assert_eq!(pane.cwd.as_deref(), Some("/work/agent"));
}

/// A plugin is not a process in a folder, and a plugin id shares its number
/// space with a terminal id — so an unguarded lookup would hand it one.
#[test]
fn a_plugin_pane_has_no_cwd() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, TabId::new(0));
    screen.terminal_cwds.insert(
        crate::TerminalId::new(1),
        std::path::PathBuf::from("/work/agent"),
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
fn plugin_bytes_with_no_pane_are_cached_without_pipe_acknowledgements() {
    let size = Size { cols: 60, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
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

#[test]
fn disconnect_discards_plugin_output_waiting_for_a_pane_before_client_id_reuse() {
    let client = ClientId::new(1);
    let other = ClientId::new(2);
    let plugin = PluginId::new(99);
    let mut pending = super::super::thread_main::PendingEvents::default();
    pending.waiting_for_pane.insert(
        PaneId::Plugin(plugin),
        vec![
            ScreenInstruction::PluginBytes(vec![
                PluginRenderAsset::new(plugin, client, b"departed".to_vec()),
                PluginRenderAsset::new(plugin, other, b"remaining".to_vec()),
            ]),
            ScreenInstruction::ShowPluginCursor(plugin, client, Some((1, 1))),
            ScreenInstruction::ShowPluginCursor(plugin, other, Some((2, 2))),
        ],
    );
    pending.remove_client_plugin_output(client);
    let remaining = &pending.waiting_for_pane[&PaneId::Plugin(plugin)];
    assert_eq!(remaining.len(), 2);
    assert!(
        matches!(&remaining[0], ScreenInstruction::PluginBytes(assets)
        if assets.len() == 1 && assets[0].client_id == other && assets[0].bytes == b"remaining")
    );
    assert!(matches!(&remaining[1], ScreenInstruction::ShowPluginCursor(_, id, _) if *id == other));
    pending.remove_client_plugin_output(other);
    assert!(pending.waiting_for_pane.is_empty());
}

#[test]
fn reloaded_plugin_location_updates_existing_panes_and_preserves_alias_configuration() {
    for placement in [
        NewPanePlacement::default(),
        NewPanePlacement::Floating(None),
        NewPanePlacement::Overlay(OverlayPlacement::default()),
    ] {
        for suppressed in [false, true] {
            for alias in [false, true] {
                let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
                let (plugin_sender, _plugin_receiver) = channels::unbounded();
                screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_sender));
                new_tab(&mut screen, 1, TabId::new(0));
                let mut original = RunPlugin {
                    location: RunPluginLocation::File("/plugins/v1.scm".into()),
                    declared_path: Some("/plugins/current.scm".into()),
                    initial_cwd: Some("/work".into()),
                    ..Default::default()
                };
                original.configuration.insert("greeting", "configured");
                let run = if alias {
                    RunPluginOrAlias::Alias(PluginAlias {
                        name: "current".into(),
                        configuration: Some(original.configuration.clone()),
                        initial_cwd: Some("/alias-cwd".into()),
                        run_plugin: Some(original.clone()),
                    })
                } else {
                    RunPluginOrAlias::RunPlugin(original.clone())
                };
                let plugin_id = PluginId::new(2);
                let pane_id = PaneId::Plugin(plugin_id);
                let tab = screen.get_active_tab_mut(ClientId::new(1)).unwrap();
                tab.new_pane(
                    pane_id,
                    None,
                    Some(Run::Plugin(run.clone())),
                    false,
                    true,
                    placement.clone(),
                    Some(ClientId::new(1)),
                    None,
                )
                .unwrap();
                if suppressed {
                    tab.suppress_pane(pane_id, None);
                }
                let location = RunPluginLocation::File("/plugins/v2.scm".into());
                screen.update_plugin_location(plugin_id, location.clone());
                let tab = screen.get_active_tab(ClientId::new(1)).unwrap();
                let stored = tab.get_pane_with_id(pane_id).unwrap().invoked_with();
                let mut expected = run;
                match &mut expected {
                    RunPluginOrAlias::RunPlugin(plugin) => plugin.location = location.clone(),
                    RunPluginOrAlias::Alias(alias) => {
                        alias.run_plugin.as_mut().unwrap().location = location.clone()
                    },
                }
                assert_eq!(stored, &Some(Run::Plugin(expected.clone())));
                let Some(Run::Plugin(stored_plugin)) = stored else {
                    panic!("missing stored plugin");
                };
                assert_eq!(stored_plugin.get_run_plugin(), expected.get_run_plugin());
                assert_eq!(stored_plugin.get_initial_cwd(), expected.get_initial_cwd());
                assert_eq!(tab.find_plugin(&expected), Some(pane_id));
                if !alias {
                    // Sharing a declared symlink is no substitute for matching canonical identity.
                    assert!(!RunPluginOrAlias::RunPlugin(original).is_equivalent_to_run(stored));
                }
            }
        }
    }
}

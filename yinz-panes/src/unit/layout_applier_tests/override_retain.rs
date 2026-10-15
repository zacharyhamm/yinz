use super::*;

#[test]
fn test_override_tiled_retain_terminal_panes_partial_match() {
    // Test that when retain_existing_terminal_panes is true, terminal panes that don't match
    // the new layout are retained instead of being closed.
    // Setup: Apply initial layout with 3 panes running different commands
    let initial_kdl = r#"
        layout {
            pane command="htop"
            pane command="vim"
            pane command="tail" {
                args "-f" "/var/log/syslog"
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];

    let size = Size {
        cols: 120,
        rows: 40,
    };
    let (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        mut tiled_panes,
        mut floating_panes,
        draw_pane_frames,
        mut focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        pty_receiver,
        plugin_receiver,
    ) = create_layout_applier_fixtures_with_receivers(size);

    let mut applier = LayoutApplier::new(
        &viewport,
        &senders,
        &sixel_image_store,
        &Rc::new(RefCell::new(KittyImageStore::default())),
        &link_handler,
        &terminal_emulator_colors,
        &terminal_emulator_color_codes,
        &character_cell_size,
        &connected_clients,
        &style,
        &display_area,
        &mut tiled_panes,
        &mut floating_panes,
        draw_pane_frames,
        &mut focus_pane_id,
        os_api.as_ref(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );

    applier
        .apply_layout(
            initial_tiled,
            initial_floating,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId::new(1),
        )
        .unwrap();

    // Override: New layout with only vim and htop (tail is not in the new layout)
    let override_kdl = r#"
        layout {
            pane command="vim"
            pane command="htop"
        }
    "#;

    let (override_tiled, _) = parse_kdl_layout(override_kdl);

    // Override with retain_existing_terminal_panes = true
    applier
        .override_tiled_panes_layout_for_existing_panes(
            &override_tiled,
            vec![],
            &mut HashMap::new(),
            true, // retain_existing_terminal_panes
            false,
            crate::ClientId::new(1),
        )
        .unwrap();

    // With retain_existing_terminal_panes = true:
    // - NO terminal panes should be closed (not even tail)
    // - All 3 original terminals (Terminal(1), Terminal(2), Terminal(3)) should still exist
    // - vim and htop panes should match the new layout positions
    // - tail pane should be retained and added after the matched panes

    // Verify NO close messages were sent
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(
        closed_panes.len(),
        0,
        "No terminal panes should be closed when retain_existing_terminal_panes is true"
    );

    // No plugins should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    // All 3 original terminals should still exist
    assert_eq!(
        tiled_panes.visible_panes_count(),
        3,
        "All 3 terminal panes should be retained"
    );

    // we're not asserting a snapshot here because adding panes uses unstable sorting and so the
    // test would be flaky
}

#[test]
fn test_override_tiled_retain_terminal_panes_no_matches() {
    // Test that when retain_existing_terminal_panes is true and NO panes match,
    // all original terminals are retained AND new terminals are created.
    // Setup: Apply initial layout with 3 panes
    let initial_kdl = r#"
        layout {
            pane command="htop"
            pane command="vim"
            pane command="tail" {
                args "-f" "/var/log/syslog"
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];

    let size = Size {
        cols: 120,
        rows: 40,
    };
    let (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        mut tiled_panes,
        mut floating_panes,
        draw_pane_frames,
        mut focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        pty_receiver,
        plugin_receiver,
    ) = create_layout_applier_fixtures_with_receivers(size);

    let mut applier = LayoutApplier::new(
        &viewport,
        &senders,
        &sixel_image_store,
        &Rc::new(RefCell::new(KittyImageStore::default())),
        &link_handler,
        &terminal_emulator_colors,
        &terminal_emulator_color_codes,
        &character_cell_size,
        &connected_clients,
        &style,
        &display_area,
        &mut tiled_panes,
        &mut floating_panes,
        draw_pane_frames,
        &mut focus_pane_id,
        os_api.as_ref(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );

    applier
        .apply_layout(
            initial_tiled,
            initial_floating,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId::new(1),
        )
        .unwrap();

    // Override: New layout with completely different commands (no matches)
    let override_kdl = r#"
        layout {
            pane command="cargo"
            pane command="npm"
            pane command="python"
        }
    "#;

    let (override_tiled, _) = parse_kdl_layout(override_kdl);
    let new_terminal_ids = vec![
        (TerminalId::new(4), None),
        (TerminalId::new(5), None),
        (TerminalId::new(6), None),
    ];

    // Override with retain_existing_terminal_panes = true
    applier
        .override_tiled_panes_layout_for_existing_panes(
            &override_tiled,
            new_terminal_ids,
            &mut HashMap::new(),
            true, // retain_existing_terminal_panes
            false,
            crate::ClientId::new(1),
        )
        .unwrap();

    // With retain_existing_terminal_panes = true and no matches:
    // - NO terminal panes should be closed
    // - All 3 original terminals (Terminal(1), Terminal(2), Terminal(3)) should still exist
    // - 3 NEW terminals (Terminal(4), Terminal(5), Terminal(6)) should be created
    // - Total: 6 terminal panes

    // Verify NO close messages were sent
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(
        closed_panes.len(),
        0,
        "No terminal panes should be closed when retain_existing_terminal_panes is true"
    );

    // No plugins should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    // Should have 6 total panes (3 original + 3 new)
    assert_eq!(
        tiled_panes.visible_panes_count(),
        6,
        "Should have 6 terminal panes (3 original + 3 new)"
    );

    // we're not asserting a snapshot here because adding panes uses unstable sorting and so the
    // test would be flaky
}

#[test]
fn test_override_floating_retain_terminal_panes_partial_match() {
    // Test that when retain_existing_terminal_panes is true, floating terminal panes
    // that don't match the new layout are retained instead of being closed.
    // Setup: 1 tiled pane + 2 floating panes running htop and vim
    let initial_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 10
                    y 10
                    width 40
                    height 20
                    command "htop"
                }
                pane {
                    x 20
                    y 20
                    width 50
                    height 25
                    command "vim"
                }
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![(TerminalId::new(1), None)];
    let floating_terminal_ids = vec![(TerminalId::new(2), None), (TerminalId::new(3), None)];

    let size = Size {
        cols: 120,
        rows: 40,
    };
    let (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        mut tiled_panes,
        mut floating_panes,
        draw_pane_frames,
        mut focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        pty_receiver,
        plugin_receiver,
    ) = create_layout_applier_fixtures_with_receivers(size);

    let mut applier = LayoutApplier::new(
        &viewport,
        &senders,
        &sixel_image_store,
        &Rc::new(RefCell::new(KittyImageStore::default())),
        &link_handler,
        &terminal_emulator_colors,
        &terminal_emulator_color_codes,
        &character_cell_size,
        &connected_clients,
        &style,
        &display_area,
        &mut tiled_panes,
        &mut floating_panes,
        draw_pane_frames,
        &mut focus_pane_id,
        os_api.as_ref(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );

    applier
        .apply_layout(
            initial_tiled,
            initial_floating,
            terminal_ids,
            floating_terminal_ids,
            HashMap::new(),
            crate::ClientId::new(1),
        )
        .unwrap();

    // Override: Layout with only htop (vim is not in the new layout)
    let override_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 50
                    y 30
                    width 45
                    height 22
                    command "htop"
                }
            }
        }
    "#;

    let (_, override_floating) = parse_kdl_layout(override_kdl);

    // Override with retain_existing_terminal_panes = true
    applier
        .override_floating_panes_layout_for_existing_panes(
            &override_floating,
            vec![],
            &mut HashMap::new(),
            true, // retain_existing_terminal_panes
            false,
        )
        .unwrap();

    // With retain_existing_terminal_panes = true:
    // - NO terminal panes should be closed (not even vim)
    // - Both original floating terminals (Terminal(2), Terminal(3)) should still exist
    // - htop pane should match the new layout position
    // - vim pane should be retained as a floating pane

    // Verify NO close messages were sent
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(
        closed_panes.len(),
        0,
        "No terminal panes should be closed when retain_existing_terminal_panes is true"
    );

    // No plugins should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    // Both floating panes should still exist
    assert_eq!(
        floating_panes.visible_panes_count(),
        2,
        "Both floating terminal panes should be retained"
    );

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_floating_retain_terminal_panes_no_matches() {
    // Test that when retain_existing_terminal_panes is true and NO floating panes match,
    // all original floating terminals are retained AND new floating terminals are created.
    // Setup: 1 tiled pane + 2 floating panes running htop and vim
    let initial_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 10
                    y 10
                    width 40
                    height 20
                    command "htop"
                }
                pane {
                    x 20
                    y 20
                    width 50
                    height 25
                    command "vim"
                }
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![(TerminalId::new(1), None)];
    let floating_terminal_ids = vec![(TerminalId::new(2), None), (TerminalId::new(3), None)];

    let size = Size {
        cols: 120,
        rows: 40,
    };
    let (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        mut tiled_panes,
        mut floating_panes,
        draw_pane_frames,
        mut focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        pty_receiver,
        plugin_receiver,
    ) = create_layout_applier_fixtures_with_receivers(size);

    let mut applier = LayoutApplier::new(
        &viewport,
        &senders,
        &sixel_image_store,
        &Rc::new(RefCell::new(KittyImageStore::default())),
        &link_handler,
        &terminal_emulator_colors,
        &terminal_emulator_color_codes,
        &character_cell_size,
        &connected_clients,
        &style,
        &display_area,
        &mut tiled_panes,
        &mut floating_panes,
        draw_pane_frames,
        &mut focus_pane_id,
        os_api.as_ref(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );

    applier
        .apply_layout(
            initial_tiled,
            initial_floating,
            terminal_ids,
            floating_terminal_ids,
            HashMap::new(),
            crate::ClientId::new(1),
        )
        .unwrap();

    // Override: Layout with 2 different floating panes (top and emacs)
    let override_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 30
                    y 30
                    width 40
                    height 20
                    command "top"
                }
                pane {
                    x 40
                    y 40
                    width 50
                    height 25
                    command "emacs"
                }
            }
        }
    "#;

    let (_, override_floating) = parse_kdl_layout(override_kdl);
    let new_floating_terminal_ids = vec![(TerminalId::new(4), None), (TerminalId::new(5), None)];

    // Override with retain_existing_terminal_panes = true
    applier
        .override_floating_panes_layout_for_existing_panes(
            &override_floating,
            new_floating_terminal_ids,
            &mut HashMap::new(),
            true, // retain_existing_terminal_panes
            false,
        )
        .unwrap();

    // With retain_existing_terminal_panes = true and no matches:
    // - NO terminal panes should be closed
    // - Both original floating terminals (Terminal(2), Terminal(3)) should still exist
    // - 2 NEW floating terminals (Terminal(4), Terminal(5)) should be created
    // - Total: 4 floating panes

    // Verify NO close messages were sent
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(
        closed_panes.len(),
        0,
        "No terminal panes should be closed when retain_existing_terminal_panes is true"
    );

    // No plugins should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    // Should have 4 floating panes (2 original + 2 new)
    assert_eq!(
        floating_panes.visible_panes_count(),
        4,
        "Should have 4 floating panes (2 original + 2 new)"
    );

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_mixed_retain_terminal_panes_both_tiled_and_floating() {
    // Test that when retain_existing_terminal_panes is true, both tiled and floating
    // terminal panes that don't match the new layout are retained.
    // Setup: Apply initial layout with 3 tiled panes + 1 floating pane
    let initial_kdl = r#"
        layout {
            pane command="htop"
            pane command="vim"
            pane command="tail" {
                args "-f" "/var/log/syslog"
            }
            floating_panes {
                pane {
                    x 10
                    y 10
                    width 40
                    height 20
                    command "watch"
                    args "df" "-h"
                }
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let floating_terminal_ids = vec![(TerminalId::new(4), None)];

    let size = Size {
        cols: 120,
        rows: 40,
    };
    let (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        mut tiled_panes,
        mut floating_panes,
        draw_pane_frames,
        mut focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        pty_receiver,
        plugin_receiver,
    ) = create_layout_applier_fixtures_with_receivers(size);

    let mut applier = LayoutApplier::new(
        &viewport,
        &senders,
        &sixel_image_store,
        &Rc::new(RefCell::new(KittyImageStore::default())),
        &link_handler,
        &terminal_emulator_colors,
        &terminal_emulator_color_codes,
        &character_cell_size,
        &connected_clients,
        &style,
        &display_area,
        &mut tiled_panes,
        &mut floating_panes,
        draw_pane_frames,
        &mut focus_pane_id,
        os_api.as_ref(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );

    applier
        .apply_layout(
            initial_tiled,
            initial_floating,
            terminal_ids,
            floating_terminal_ids,
            HashMap::new(),
            crate::ClientId::new(1),
        )
        .unwrap();

    // Override: 2 tiled (top, htop) + 1 floating (different watch command)
    let override_kdl = r#"
        layout {
            pane command="top"
            pane command="htop"
            floating_panes {
                pane {
                    x 20
                    y 20
                    width 50
                    height 25
                    command "watch"
                    args "free" "-h"
                }
            }
        }
    "#;

    let (override_tiled, override_floating) = parse_kdl_layout(override_kdl);
    let new_terminal_ids = vec![(TerminalId::new(5), None)];
    let new_floating_terminal_ids = vec![(TerminalId::new(6), None)];

    let retain_existing_terminal_panes = true;
    let retain_existing_plugin_panes = false;
    applier
        .override_layout(
            override_tiled,
            override_floating,
            new_terminal_ids,
            new_floating_terminal_ids,
            HashMap::new(),
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            crate::ClientId::new(1),
        )
        .unwrap();

    // With retain_existing_terminal_panes = true:
    // - NO terminal panes should be closed (neither tiled nor floating)
    // - Original tiled terminals: Terminal(1), Terminal(2), Terminal(3) retained
    //   (htop matches, so it's reused; vim and tail are retained)
    // - Original floating terminal: Terminal(4) retained
    // - New tiled terminal: Terminal(5) created (for top, htop matches)
    // - New floating terminal: Terminal(6) created (different watch command)

    // Verify NO close messages were sent
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(
        closed_panes.len(),
        0,
        "No terminal panes should be closed when retain_existing_terminal_panes is true"
    );

    // No plugins should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    // Check tiled pane count (3 original + 1 new = 4)
    assert_eq!(
        tiled_panes.visible_panes_count(),
        4,
        "Should have 4 tiled panes (3 original + 1 new)"
    );

    // Check floating pane count (1 original + 1 new = 2)
    assert_eq!(
        floating_panes.visible_panes_count(),
        2,
        "Should have 2 floating panes (1 original + 1 new)"
    );

    // we're not asserting a snapshot here because adding panes uses unstable sorting and so the
    // test would be flaky
}

#[test]
fn test_override_retain_terminal_but_close_plugin_panes() {
    // Test that when retain_existing_terminal_panes is true, the flag ONLY affects
    // terminal panes and plugin panes are still closed as normal.
    // Setup: Initial layout with 2 terminal panes + 1 plugin pane
    let initial_kdl = r#"
        layout {
            pane command="htop"
            pane command="vim"
            pane {
                plugin location="yinz:tab-bar"
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![(TerminalId::new(1), None), (TerminalId::new(2), None)];

    let mut initial_plugin_ids = HashMap::new();
    let tab_bar_plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:tab-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    initial_plugin_ids.insert(
        tab_bar_plugin.clone(),
        vec![yinz_types::internal_ids::PluginId::new(100)],
    );

    let size = Size {
        cols: 120,
        rows: 40,
    };
    let (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        mut tiled_panes,
        mut floating_panes,
        draw_pane_frames,
        mut focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        pty_receiver,
        plugin_receiver,
    ) = create_layout_applier_fixtures_with_receivers(size);

    let mut applier = LayoutApplier::new(
        &viewport,
        &senders,
        &sixel_image_store,
        &Rc::new(RefCell::new(KittyImageStore::default())),
        &link_handler,
        &terminal_emulator_colors,
        &terminal_emulator_color_codes,
        &character_cell_size,
        &connected_clients,
        &style,
        &display_area,
        &mut tiled_panes,
        &mut floating_panes,
        draw_pane_frames,
        &mut focus_pane_id,
        os_api.as_ref(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );

    applier
        .apply_layout(
            initial_tiled,
            initial_floating,
            terminal_ids,
            vec![],
            initial_plugin_ids,
            crate::ClientId::new(1),
        )
        .unwrap();

    // Override: New layout with only htop (vim and tab-bar plugin not in new layout)
    let override_kdl = r#"
        layout {
            pane command="htop"
        }
    "#;

    let (override_tiled, _) = parse_kdl_layout(override_kdl);

    // Override with retain_existing_terminal_panes = true
    applier
        .override_tiled_panes_layout_for_existing_panes(
            &override_tiled,
            vec![],
            &mut HashMap::new(),
            true, // retain_existing_terminal_panes
            false,
            crate::ClientId::new(1),
        )
        .unwrap();

    // With retain_existing_terminal_panes = true:
    // - Terminal panes NOT closed: Terminal(1) matched (htop), Terminal(2) retained (vim)
    // - Plugin pane IS closed: Plugin(100) unloaded (tab-bar)
    // - Both terminals should exist, but plugin should be gone

    // No terminal panes should be closed
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(
        closed_panes.len(),
        0,
        "No terminal panes should be closed when retain_existing_terminal_panes is true"
    );

    // Plugin pane should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert_eq!(
        unloaded_plugins.len(),
        1,
        "Plugin pane should be unloaded even with retain_existing_terminal_panes"
    );
    assert!(
        unloaded_plugins.contains(&yinz_types::internal_ids::PluginId::new(100)),
        "Tab-bar plugin (100) should be unloaded"
    );

    // Both terminals should exist
    assert_eq!(
        tiled_panes.visible_panes_count(),
        2,
        "Both terminal panes should be retained"
    );

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_tiled_retain_plugin_panes_partial_match() {
    // Verify that when retain_existing_plugin_panes = true, plugin panes that don't match
    // the new layout are retained instead of being closed
    let initial_kdl = r#"
        layout {
            pane
            pane {
                plugin location="yinz:tab-bar"
            }
            pane {
                plugin location="yinz:status-bar"
            }
            pane {
                plugin location="file:///path/to/custom.scm"
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);

    let terminal_ids = vec![(TerminalId::new(1), None)];

    let mut initial_plugin_ids = HashMap::new();
    let tab_bar = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:tab-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    let status_bar = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:status-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    let custom = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "file:///path/to/custom.scm",
        &None,
        None,
        None,
    )
    .unwrap();
    initial_plugin_ids.insert(tab_bar, vec![yinz_types::internal_ids::PluginId::new(100)]);
    initial_plugin_ids.insert(
        status_bar,
        vec![yinz_types::internal_ids::PluginId::new(101)],
    );
    initial_plugin_ids.insert(custom, vec![yinz_types::internal_ids::PluginId::new(102)]);

    let size = Size {
        cols: 120,
        rows: 40,
    };
    let (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        mut tiled_panes,
        mut floating_panes,
        draw_pane_frames,
        mut focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        pty_receiver,
        plugin_receiver,
    ) = create_layout_applier_fixtures_with_receivers(size);

    let mut applier = LayoutApplier::new(
        &viewport,
        &senders,
        &sixel_image_store,
        &Rc::new(RefCell::new(KittyImageStore::default())),
        &link_handler,
        &terminal_emulator_colors,
        &terminal_emulator_color_codes,
        &character_cell_size,
        &connected_clients,
        &style,
        &display_area,
        &mut tiled_panes,
        &mut floating_panes,
        draw_pane_frames,
        &mut focus_pane_id,
        os_api.as_ref(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );

    applier
        .apply_layout(
            initial_tiled,
            initial_floating,
            terminal_ids,
            vec![],
            initial_plugin_ids,
            crate::ClientId::new(1),
        )
        .unwrap();

    let override_kdl = r#"
        layout {
            pane
            pane {
                plugin location="yinz:tab-bar"
            }
            pane {
                plugin location="yinz:status-bar"
            }
        }
    "#;

    let (override_tiled, _) = parse_kdl_layout(override_kdl);

    applier
        .override_tiled_panes_layout_for_existing_panes(
            &override_tiled,
            vec![],
            &mut HashMap::new(),
            false,
            true,
            crate::ClientId::new(1),
        )
        .unwrap();

    // Verify NO plugin panes were unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert_eq!(
        unloaded_plugins.len(),
        0,
        "No plugin panes should be unloaded when retain_existing_plugin_panes is true"
    );

    // No terminals should be closed
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert!(closed_panes.is_empty());

    // All 4 panes should exist (1 terminal + 3 plugins)
    assert_eq!(
        tiled_panes.visible_panes_count(),
        4,
        "All plugin panes should be retained"
    );
}

#[test]
fn test_override_tiled_retain_plugin_panes_no_matches() {
    // When NO plugins match the new layout, all original plugins are retained AND new plugins are created
    let initial_kdl = r#"
        layout {
            pane
            pane {
                plugin location="yinz:tab-bar"
            }
            pane {
                plugin location="yinz:status-bar"
            }
            pane {
                plugin location="yinz:compact-bar"
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);

    let terminal_ids = vec![(TerminalId::new(1), None)];

    let mut initial_plugin_ids = HashMap::new();
    let tab_bar = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:tab-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    let status_bar = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:status-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    let compact_bar = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:compact-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    initial_plugin_ids.insert(tab_bar, vec![yinz_types::internal_ids::PluginId::new(100)]);
    initial_plugin_ids.insert(
        status_bar,
        vec![yinz_types::internal_ids::PluginId::new(101)],
    );
    initial_plugin_ids.insert(
        compact_bar,
        vec![yinz_types::internal_ids::PluginId::new(102)],
    );

    let size = Size {
        cols: 120,
        rows: 40,
    };
    let (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        mut tiled_panes,
        mut floating_panes,
        draw_pane_frames,
        mut focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        _pty_receiver,
        plugin_receiver,
    ) = create_layout_applier_fixtures_with_receivers(size);

    let mut applier = LayoutApplier::new(
        &viewport,
        &senders,
        &sixel_image_store,
        &Rc::new(RefCell::new(KittyImageStore::default())),
        &link_handler,
        &terminal_emulator_colors,
        &terminal_emulator_color_codes,
        &character_cell_size,
        &connected_clients,
        &style,
        &display_area,
        &mut tiled_panes,
        &mut floating_panes,
        draw_pane_frames,
        &mut focus_pane_id,
        os_api.as_ref(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );

    applier
        .apply_layout(
            initial_tiled,
            initial_floating,
            terminal_ids,
            vec![],
            initial_plugin_ids,
            crate::ClientId::new(1),
        )
        .unwrap();

    let override_kdl = r#"
        layout {
            pane
            pane {
                plugin location="file:///path/to/plugin1.scm"
            }
            pane {
                plugin location="file:///path/to/plugin2.scm"
            }
            pane {
                plugin location="file:///path/to/plugin3.scm"
            }
        }
    "#;

    let (override_tiled, _) = parse_kdl_layout(override_kdl);

    let mut override_plugin_ids = HashMap::new();
    let plugin1 = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "file:///path/to/plugin1.scm",
        &None,
        None,
        None,
    )
    .unwrap();
    let plugin2 = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "file:///path/to/plugin2.scm",
        &None,
        None,
        None,
    )
    .unwrap();
    let plugin3 = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "file:///path/to/plugin3.scm",
        &None,
        None,
        None,
    )
    .unwrap();
    override_plugin_ids.insert(plugin1, vec![yinz_types::internal_ids::PluginId::new(103)]);
    override_plugin_ids.insert(plugin2, vec![yinz_types::internal_ids::PluginId::new(104)]);
    override_plugin_ids.insert(plugin3, vec![yinz_types::internal_ids::PluginId::new(105)]);

    applier
        .override_tiled_panes_layout_for_existing_panes(
            &override_tiled,
            vec![],
            &mut override_plugin_ids,
            false,
            true,
            crate::ClientId::new(1),
        )
        .unwrap();

    // Verify NO plugin panes were unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert_eq!(
        unloaded_plugins.len(),
        0,
        "No plugin panes should be unloaded when retain_existing_plugin_panes is true"
    );

    // Total: 1 terminal + 6 plugins (3 original + 3 new)
    assert_eq!(
        tiled_panes.visible_panes_count(),
        7,
        "All original plugins retained and new plugins created"
    );
}

#[test]
fn test_override_floating_retain_plugin_panes_partial_match() {
    // Floating plugin panes that don't match the new layout are retained
    let initial_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 10
                    y 10
                    width 40
                    height 20
                    plugin location="yinz:tab-bar"
                }
                pane {
                    x 20
                    y 20
                    width 50
                    height 25
                    plugin location="yinz:status-bar"
                }
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);

    let terminal_ids = vec![(TerminalId::new(1), None)];

    let mut initial_plugin_ids = HashMap::new();
    let tab_bar = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:tab-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    let status_bar = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:status-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    initial_plugin_ids.insert(tab_bar, vec![yinz_types::internal_ids::PluginId::new(100)]);
    initial_plugin_ids.insert(
        status_bar,
        vec![yinz_types::internal_ids::PluginId::new(101)],
    );

    let size = Size {
        cols: 120,
        rows: 40,
    };
    let (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        mut tiled_panes,
        mut floating_panes,
        draw_pane_frames,
        mut focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        pty_receiver,
        plugin_receiver,
    ) = create_layout_applier_fixtures_with_receivers(size);

    let mut applier = LayoutApplier::new(
        &viewport,
        &senders,
        &sixel_image_store,
        &Rc::new(RefCell::new(KittyImageStore::default())),
        &link_handler,
        &terminal_emulator_colors,
        &terminal_emulator_color_codes,
        &character_cell_size,
        &connected_clients,
        &style,
        &display_area,
        &mut tiled_panes,
        &mut floating_panes,
        draw_pane_frames,
        &mut focus_pane_id,
        os_api.as_ref(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );

    applier
        .apply_layout(
            initial_tiled,
            initial_floating,
            terminal_ids,
            vec![],
            initial_plugin_ids,
            crate::ClientId::new(1),
        )
        .unwrap();

    let override_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 15
                    y 15
                    width 45
                    height 22
                    plugin location="yinz:tab-bar"
                }
            }
        }
    "#;

    let (_, override_floating) = parse_kdl_layout(override_kdl);

    applier
        .override_floating_panes_layout_for_existing_panes(
            &override_floating,
            vec![],
            &mut HashMap::new(),
            false,
            true,
        )
        .unwrap();

    // Verify NO plugin panes were unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert_eq!(
        unloaded_plugins.len(),
        0,
        "No floating plugin panes should be unloaded when retain_existing_plugin_panes is true"
    );

    // No terminals should be closed
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert!(closed_panes.is_empty());

    // Both floating plugins should still exist
    assert_eq!(
        floating_panes.visible_panes_count(),
        2,
        "Both floating plugin panes should be retained"
    );

    // Verify tiled pane exists
    assert_eq!(tiled_panes.visible_panes_count(), 1);
}

#[test]
fn test_override_floating_retain_plugin_panes_no_matches() {
    // All original floating plugins are retained AND new floating plugins are created when there are no matches
    let initial_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 10
                    y 10
                    width 40
                    height 20
                    plugin location="yinz:tab-bar"
                }
                pane {
                    x 20
                    y 20
                    width 50
                    height 25
                    plugin location="yinz:status-bar"
                }
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);

    let terminal_ids = vec![(TerminalId::new(1), None)];

    let mut initial_plugin_ids = HashMap::new();
    let tab_bar = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:tab-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    let status_bar = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:status-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    initial_plugin_ids.insert(tab_bar, vec![yinz_types::internal_ids::PluginId::new(100)]);
    initial_plugin_ids.insert(
        status_bar,
        vec![yinz_types::internal_ids::PluginId::new(101)],
    );

    let size = Size {
        cols: 120,
        rows: 40,
    };
    let (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        mut tiled_panes,
        mut floating_panes,
        draw_pane_frames,
        mut focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        _pty_receiver,
        plugin_receiver,
    ) = create_layout_applier_fixtures_with_receivers(size);

    let mut applier = LayoutApplier::new(
        &viewport,
        &senders,
        &sixel_image_store,
        &Rc::new(RefCell::new(KittyImageStore::default())),
        &link_handler,
        &terminal_emulator_colors,
        &terminal_emulator_color_codes,
        &character_cell_size,
        &connected_clients,
        &style,
        &display_area,
        &mut tiled_panes,
        &mut floating_panes,
        draw_pane_frames,
        &mut focus_pane_id,
        os_api.as_ref(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );

    applier
        .apply_layout(
            initial_tiled,
            initial_floating,
            terminal_ids,
            vec![],
            initial_plugin_ids,
            crate::ClientId::new(1),
        )
        .unwrap();

    let override_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 15
                    y 15
                    width 45
                    height 22
                    plugin location="file:///path/to/plugin1.scm"
                }
                pane {
                    x 25
                    y 25
                    width 55
                    height 27
                    plugin location="file:///path/to/plugin2.scm"
                }
            }
        }
    "#;

    let (_, override_floating) = parse_kdl_layout(override_kdl);

    let mut override_plugin_ids = HashMap::new();
    let plugin1 = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "file:///path/to/plugin1.scm",
        &None,
        None,
        None,
    )
    .unwrap();
    let plugin2 = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "file:///path/to/plugin2.scm",
        &None,
        None,
        None,
    )
    .unwrap();
    override_plugin_ids.insert(plugin1, vec![yinz_types::internal_ids::PluginId::new(102)]);
    override_plugin_ids.insert(plugin2, vec![yinz_types::internal_ids::PluginId::new(103)]);

    applier
        .override_floating_panes_layout_for_existing_panes(
            &override_floating,
            vec![],
            &mut override_plugin_ids,
            false,
            true,
        )
        .unwrap();

    // Verify NO plugin panes were unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert_eq!(
        unloaded_plugins.len(),
        0,
        "No floating plugin panes should be unloaded when retain_existing_plugin_panes is true"
    );

    // Total: 4 floating plugins (2 original + 2 new)
    assert_eq!(
        floating_panes.visible_panes_count(),
        4,
        "All original floating plugins retained and new plugins created"
    );

    // 1 tiled terminal pane
    assert_eq!(tiled_panes.visible_panes_count(), 1);
}

#[test]
fn test_override_mixed_retain_plugin_panes_both_tiled_and_floating() {
    // Both tiled and floating plugin panes are retained when the flag is true
    let initial_kdl = r#"
        layout {
            pane command="htop"
            pane command="vim"
            pane {
                plugin location="yinz:tab-bar"
            }
            floating_panes {
                pane {
                    x 10
                    y 10
                    width 40
                    height 20
                    plugin location="yinz:status-bar"
                }
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);

    let terminal_ids = vec![(TerminalId::new(1), None), (TerminalId::new(2), None)];

    let mut initial_plugin_ids = HashMap::new();
    let tab_bar = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:tab-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    let status_bar = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:status-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    initial_plugin_ids.insert(tab_bar, vec![yinz_types::internal_ids::PluginId::new(100)]);
    initial_plugin_ids.insert(
        status_bar,
        vec![yinz_types::internal_ids::PluginId::new(101)],
    );

    let size = Size {
        cols: 120,
        rows: 40,
    };
    let (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        mut tiled_panes,
        mut floating_panes,
        draw_pane_frames,
        mut focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        pty_receiver,
        plugin_receiver,
    ) = create_layout_applier_fixtures_with_receivers(size);

    let mut applier = LayoutApplier::new(
        &viewport,
        &senders,
        &sixel_image_store,
        &Rc::new(RefCell::new(KittyImageStore::default())),
        &link_handler,
        &terminal_emulator_colors,
        &terminal_emulator_color_codes,
        &character_cell_size,
        &connected_clients,
        &style,
        &display_area,
        &mut tiled_panes,
        &mut floating_panes,
        draw_pane_frames,
        &mut focus_pane_id,
        os_api.as_ref(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );

    applier
        .apply_layout(
            initial_tiled,
            initial_floating,
            terminal_ids,
            vec![],
            initial_plugin_ids,
            crate::ClientId::new(1),
        )
        .unwrap();

    let override_kdl = r#"
        layout {
            pane command="htop"
            pane {
                plugin location="yinz:tab-bar"
            }
            pane {
                plugin location="yinz:compact-bar"
            }
            floating_panes {
                pane {
                    x 20
                    y 20
                    width 50
                    height 25
                    plugin location="file:///path/to/custom.scm"
                }
            }
        }
    "#;

    let (override_tiled, override_floating) = parse_kdl_layout(override_kdl);

    let mut override_plugin_ids = HashMap::new();
    let compact_bar = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:compact-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    let custom = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "file:///path/to/custom.scm",
        &None,
        None,
        None,
    )
    .unwrap();
    override_plugin_ids.insert(
        compact_bar,
        vec![yinz_types::internal_ids::PluginId::new(102)],
    );
    override_plugin_ids.insert(custom, vec![yinz_types::internal_ids::PluginId::new(103)]);

    let retain_existing_terminal_panes = false;
    let retain_existing_plugin_panes = true;
    applier
        .override_layout(
            override_tiled,
            override_floating,
            vec![],
            vec![],
            override_plugin_ids,
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            crate::ClientId::new(1),
        )
        .unwrap();

    // Verify NO plugin panes were unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert_eq!(
        unloaded_plugins.len(),
        0,
        "No plugin panes should be unloaded when retain_existing_plugin_panes is true"
    );

    // vim terminal should be closed (doesn't match layout)
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 1, "vim terminal should be closed");
    assert!(
        closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(2))),
        "Terminal(2) (vim) should be closed"
    );

    // Verify plugins exist (exact counts may vary based on where retained panes land)
    assert!(
        tiled_panes.visible_panes_count() >= 2,
        "At least htop and matched plugins"
    );
    assert!(
        floating_panes.visible_panes_count() >= 1,
        "At least one floating plugin"
    );
}

#[test]
fn test_override_retain_plugin_but_close_terminal_panes() {
    // Verify that retain_existing_plugin_panes = true ONLY affects plugin panes; terminals are still closed normally
    let initial_kdl = r#"
        layout {
            pane command="htop"
            pane command="vim"
            pane {
                plugin location="yinz:tab-bar"
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);

    let terminal_ids = vec![(TerminalId::new(1), None), (TerminalId::new(2), None)];

    let mut initial_plugin_ids = HashMap::new();
    let tab_bar = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:tab-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    initial_plugin_ids.insert(tab_bar, vec![yinz_types::internal_ids::PluginId::new(100)]);

    let size = Size {
        cols: 120,
        rows: 40,
    };
    let (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        mut tiled_panes,
        mut floating_panes,
        draw_pane_frames,
        mut focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        pty_receiver,
        plugin_receiver,
    ) = create_layout_applier_fixtures_with_receivers(size);

    let mut applier = LayoutApplier::new(
        &viewport,
        &senders,
        &sixel_image_store,
        &Rc::new(RefCell::new(KittyImageStore::default())),
        &link_handler,
        &terminal_emulator_colors,
        &terminal_emulator_color_codes,
        &character_cell_size,
        &connected_clients,
        &style,
        &display_area,
        &mut tiled_panes,
        &mut floating_panes,
        draw_pane_frames,
        &mut focus_pane_id,
        os_api.as_ref(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );

    applier
        .apply_layout(
            initial_tiled,
            initial_floating,
            terminal_ids,
            vec![],
            initial_plugin_ids,
            crate::ClientId::new(1),
        )
        .unwrap();

    let override_kdl = r#"
        layout {
            pane command="htop"
        }
    "#;

    let (override_tiled, _) = parse_kdl_layout(override_kdl);

    applier
        .override_tiled_panes_layout_for_existing_panes(
            &override_tiled,
            vec![],
            &mut HashMap::new(),
            false,
            true,
            crate::ClientId::new(1),
        )
        .unwrap();

    // vim terminal should be closed
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(
        closed_panes.len(),
        1,
        "Terminal pane should be closed even with retain_existing_plugin_panes"
    );
    assert!(
        closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(2))),
        "vim (Terminal(2)) should be closed"
    );

    // Plugin should NOT be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert_eq!(
        unloaded_plugins.len(),
        0,
        "No plugin panes should be unloaded"
    );

    // Final panes: htop + tab-bar = 2
    assert_eq!(
        tiled_panes.visible_panes_count(),
        2,
        "htop and tab-bar should remain"
    );

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

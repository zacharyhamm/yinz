use super::*;

#[test]
fn test_override_layout_basic_with_both_tiled_and_floating() {
    // Setup: Apply initial layout with 2 tiled panes + 1 floating pane
    let initial_kdl = r#"
        layout {
            pane command="htop"
            pane command="vim"
            floating_panes {
                pane {
                    x 10
                    y 10
                    width 40
                    height 20
                    command "tail"
                    args "-f" "/var/log/syslog"
                }
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![(TerminalId::new(1), None), (TerminalId::new(2), None)];
    let floating_terminal_ids = vec![(TerminalId::new(3), None)];

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

    // Now override with different layout (2 tiled + 1 floating)
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
                    args "df" "-h"
                }
            }
        }
    "#;

    let (override_tiled, override_floating) = parse_kdl_layout(override_kdl);
    let new_terminal_ids = vec![(TerminalId::new(4), None)];
    let new_floating_terminal_ids = vec![(TerminalId::new(5), None)];

    let retain_existing_terminal_panes = false;
    let retain_existing_plugin_panes = false;
    let should_show_floating = applier
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

    // Should show floating panes
    assert!(should_show_floating);

    // Verify close messages were sent for vim (Terminal(2)) and tail (Terminal(3))
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 2);
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(2)))); // vim
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(3)))); // tail

    // No plugins should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_layout_hide_floating_panes_true() {
    // Setup: Initial layout with floating panes
    let initial_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 10
                    y 10
                    width 40
                    height 20
                }
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![(TerminalId::new(1), None)];
    let floating_terminal_ids = vec![(TerminalId::new(2), None)];

    let size = Size {
        cols: 100,
        rows: 50,
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

    // Override with layout that has hide_floating_panes true
    let override_kdl = r#"
        layout {
            hide_floating_panes true
            pane
            pane
            floating_panes {
                pane {
                    x 15
                    y 15
                    width 45
                    height 22
                }
            }
        }
    "#;

    let (override_tiled, override_floating) = parse_kdl_layout(override_kdl);
    let new_terminal_ids = vec![(TerminalId::new(3), None)];
    let new_floating_terminal_ids = vec![(TerminalId::new(4), None)];

    let retain_existing_terminal_panes = false;
    let retain_existing_plugin_panes = false;
    let should_show_floating = applier
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

    // Should NOT show floating panes because of hide_floating_panes
    assert!(!should_show_floating);

    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 0);

    // No plugins should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_layout_show_floating_panes() {
    // Setup: Initial layout
    let initial_kdl = r#"
        layout {
            pane
            pane
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![(TerminalId::new(1), None), (TerminalId::new(2), None)];

    let size = Size {
        cols: 100,
        rows: 50,
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

    // Override with layout containing floating panes
    let override_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 20
                    y 10
                    width 50
                    height 30
                }
            }
        }
    "#;

    let (override_tiled, override_floating) = parse_kdl_layout(override_kdl);
    let new_floating_terminal_ids = vec![(TerminalId::new(3), None)];

    let retain_existing_terminal_panes = false;
    let retain_existing_plugin_panes = false;
    let should_show_floating = applier
        .override_layout(
            override_tiled,
            override_floating,
            vec![],
            new_floating_terminal_ids,
            HashMap::new(),
            retain_existing_terminal_panes,
            retain_existing_plugin_panes,
            crate::ClientId::new(1),
        )
        .unwrap();

    // Should show floating panes
    assert!(should_show_floating);

    // Verify close message was sent for one tiled pane (Terminal(2))
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 1);
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(2))));

    // No plugins should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

// ============================================================================
// Suite 2: override_tiled_panes_layout_for_existing_panes Tests
// ============================================================================

#[test]
fn test_override_tiled_exact_match_preservation_commands() {
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

    // Override: New layout with only htop and vim in different positions
    let override_kdl = r#"
        layout {
            pane command="vim"
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
            false,
            crate::ClientId::new(1),
        )
        .unwrap();

    // htop and vim panes should be preserved (same PaneIds: Terminal(1) and Terminal(2))
    // tail pane should be closed
    // Panes should be repositioned to new layout positions

    // Verify close message was sent for tail pane (Terminal(3))
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 1);
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(3)))); // tail

    // No plugins should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_tiled_exact_match_preservation_plugins() {
    // Setup: Initial layout with 2 terminal panes + 1 plugin pane
    let initial_kdl = r#"
        layout {
            pane
            pane
            pane {
                plugin location="zellij:tab-bar"
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![(TerminalId::new(1), None), (TerminalId::new(2), None)];

    let mut initial_plugin_ids = HashMap::new();
    let tab_bar_plugin = zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
        "zellij:tab-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    initial_plugin_ids.insert(
        tab_bar_plugin.clone(),
        vec![zellij_types::internal_ids::PluginId::new(100)],
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

    // Override: New layout with only the plugin pane
    let override_kdl = r#"
        layout {
            pane {
                plugin location="zellij:tab-bar"
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
            false,
            crate::ClientId::new(1),
        )
        .unwrap();

    // Plugin pane should be preserved
    // Terminal panes should be closed
    // Total pane count is 1

    // Verify close messages were sent for both terminal panes (Terminal(1) and Terminal(2))
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 2);
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(1))));
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(2))));

    // No plugins should be unloaded (plugin is preserved)
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_tiled_all_panes_closed_no_matches() {
    // Setup: 3 panes running htop, vim, tail
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

    // Override: New layout with 3 completely different commands
    let override_kdl = r#"
        layout {
            pane command="cargo" {
                args "watch"
            }
            pane command="npm" {
                args "start"
            }
            pane command="python" {
                args "-m" "http.server"
            }
        }
    "#;

    let (override_tiled, _) = parse_kdl_layout(override_kdl);
    let new_terminal_ids = vec![
        (TerminalId::new(4), None),
        (TerminalId::new(5), None),
        (TerminalId::new(6), None),
    ];

    applier
        .override_tiled_panes_layout_for_existing_panes(
            &override_tiled,
            new_terminal_ids,
            &mut HashMap::new(),
            false,
            false,
            crate::ClientId::new(1),
        )
        .unwrap();

    // All original pane IDs gone (1, 2, 3 should not be present)
    // 3 new panes with new IDs (4, 5, 6)
    // Total pane count is 3

    // Verify close messages were sent for all original panes (Terminal(1), Terminal(2), Terminal(3))
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 3);
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(1)))); // htop
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(2)))); // vim
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(3)))); // tail

    // No plugins should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_tiled_mixed_some_matches_some_new() {
    // Setup: 2 panes - one running htop, one generic shell (no command)
    let initial_kdl = r#"
        layout {
            pane command="htop"
            pane
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![(TerminalId::new(1), None), (TerminalId::new(2), None)];

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

    // Override: Layout with htop, vim, and generic shell
    let override_kdl = r#"
        layout {
            pane command="htop"
            pane command="vim"
            pane
        }
    "#;

    let (override_tiled, _) = parse_kdl_layout(override_kdl);
    let new_terminal_ids = vec![(TerminalId::new(3), None), (TerminalId::new(4), None)];

    applier
        .override_tiled_panes_layout_for_existing_panes(
            &override_tiled,
            new_terminal_ids,
            &mut HashMap::new(),
            false,
            false,
            crate::ClientId::new(1),
        )
        .unwrap();

    // htop preserved with same ID (Terminal(1))
    // Original shell pane closed (generic shells are NOT exact matches)
    // 2 new panes created (vim and new shell)
    // Total pane count is 3

    // Verify close message was not sent for original shell pane (Terminal(2))
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 0);

    // No plugins should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_tiled_new_panes_for_unmatched_positions() {
    // Setup: 1 pane running htop
    let initial_kdl = r#"
        layout {
            pane command="htop"
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![(TerminalId::new(1), None)];

    let size = Size {
        cols: 150,
        rows: 50,
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
    ) = create_layout_applier_fixtures(size);

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

    // Override: Layout with 4 positions
    let override_kdl = r#"
        layout {
            pane command="htop"
            pane command="vim"
            pane
            pane
        }
    "#;

    let (override_tiled, _) = parse_kdl_layout(override_kdl);
    let new_terminal_ids = vec![
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
        (TerminalId::new(4), None),
    ];

    applier
        .override_tiled_panes_layout_for_existing_panes(
            &override_tiled,
            new_terminal_ids,
            &mut HashMap::new(),
            false,
            false,
            crate::ClientId::new(1),
        )
        .unwrap();

    // 1 original htop pane preserved (Terminal(1))
    // 3 new panes created (Terminal(2), Terminal(3), Terminal(4))
    // Total 4 panes
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_tiled_focus_on_new_pane() {
    // Setup: 2 panes, first one focused
    let initial_kdl = r#"
        layout {
            pane focus=true
            pane
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![(TerminalId::new(1), None), (TerminalId::new(2), None)];

    let size = Size {
        cols: 100,
        rows: 50,
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
    ) = create_layout_applier_fixtures(size);

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

    // Override: Layout with 3 panes where second pane has focus=true
    let override_kdl = r#"
        layout {
            pane
            pane focus=true
            pane
        }
    "#;

    let (override_tiled, _) = parse_kdl_layout(override_kdl);
    let new_terminal_ids = vec![(TerminalId::new(3), None)];

    applier
        .override_tiled_panes_layout_for_existing_panes(
            &override_tiled,
            new_terminal_ids,
            &mut HashMap::new(),
            false,
            false,
            crate::ClientId::new(1),
        )
        .unwrap();

    // focus_pane_id should point to the newly created middle pane (Terminal(3))
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_tiled_focus_when_focused_pane_closed() {
    // Setup: 3 panes, middle one focused running vim
    let initial_kdl = r#"
        layout {
            pane command="htop"
            pane command="vim" focus=true
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

    // Override: Layout with 2 panes running htop and cargo (not vim)
    let override_kdl = r#"
        layout {
            pane command="htop"
            pane command="cargo" {
                args "check"
            }
        }
    "#;

    let (override_tiled, _) = parse_kdl_layout(override_kdl);
    let new_terminal_ids = vec![(TerminalId::new(4), None)];

    applier
        .override_tiled_panes_layout_for_existing_panes(
            &override_tiled,
            new_terminal_ids,
            &mut HashMap::new(),
            false,
            false,
            crate::ClientId::new(1),
        )
        .unwrap();

    // Focused pane (vim) no longer exists
    // Focus should be moved to one of the remaining panes

    // Verify close messages were sent for vim (Terminal(2)) and tail (Terminal(3))
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 2);
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(2)))); // vim (focused)
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(3)))); // tail

    // No plugins should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_tiled_empty_layout_closes_all() {
    // Setup: 3 panes running various commands
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

    // Override: Empty layout
    let override_kdl = r#"
        layout {
        }
    "#;

    let (override_tiled, _) = parse_kdl_layout(override_kdl);

    applier
        .override_tiled_panes_layout_for_existing_panes(
            &override_tiled,
            vec![],
            &mut HashMap::new(),
            false,
            false,
            crate::ClientId::new(1),
        )
        .unwrap();

    // No panes in snapshot
    // Pane count is 0

    // Verify close messages were sent for all panes (Terminal(1), Terminal(2), Terminal(3))
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 3);
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(1)))); // htop
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(2)))); // vim
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(3)))); // tail

    // No plugins should be unloaded
    let unloaded_plugins = collect_unload_plugin_messages(&plugin_receiver);
    assert!(unloaded_plugins.is_empty());

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

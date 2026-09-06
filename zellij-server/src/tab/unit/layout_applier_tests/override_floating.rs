use super::*;

// ============================================================================
// Suite 3: override_floating_panes_layout_for_existing_panes Tests
// ============================================================================

#[test]
fn test_override_floating_exact_match_preservation() {
    // Setup: 2 floating panes running htop and vim
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
        &os_api,
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

    // Override: Layout with htop at different x/y position
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

    applier
        .override_floating_panes_layout_for_existing_panes(
            &override_floating,
            vec![],
            &mut HashMap::new(),
            false,
            false,
        )
        .unwrap();

    // htop pane preserved (Terminal(2)), repositioned
    // vim pane closed (Terminal(3))
    // Total floating pane count is 1

    // Verify close message was sent for vim pane (Terminal(3))
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 1);
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(3)))); // vim

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
fn test_override_floating_all_closed_no_matches() {
    // Setup: 2 floating panes with specific commands
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
        &os_api,
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

    // Override: Layout with different commands
    let override_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 15
                    y 15
                    width 45
                    height 22
                    command "top"
                }
                pane {
                    x 25
                    y 25
                    width 55
                    height 27
                    command "emacs"
                }
            }
        }
    "#;

    let (_, override_floating) = parse_kdl_layout(override_kdl);
    let new_floating_terminal_ids = vec![(TerminalId::new(4), None), (TerminalId::new(5), None)];

    applier
        .override_floating_panes_layout_for_existing_panes(
            &override_floating,
            new_floating_terminal_ids,
            &mut HashMap::new(),
            false,
            false,
        )
        .unwrap();

    // Both original panes closed (IDs 2, 3 gone)
    // New panes created with new IDs (4, 5)
    // Pane count matches new layout (2 floating panes)

    // Verify close messages were sent for both floating panes (Terminal(2), Terminal(3))
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 2);
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(2)))); // htop
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(3)))); // vim

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
fn test_override_floating_new_panes_created() {
    // Setup: 1 floating pane running htop
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
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![(TerminalId::new(1), None)];
    let floating_terminal_ids = vec![(TerminalId::new(2), None)];

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
        &os_api,
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

    // Override: Layout with 3 floating panes: htop, vim, and generic shell
    let override_kdl = r#"
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
                pane {
                    x 30
                    y 30
                    width 45
                    height 22
                }
            }
        }
    "#;

    let (_, override_floating) = parse_kdl_layout(override_kdl);
    let new_floating_terminal_ids = vec![(TerminalId::new(3), None), (TerminalId::new(4), None)];

    applier
        .override_floating_panes_layout_for_existing_panes(
            &override_floating,
            new_floating_terminal_ids,
            &mut HashMap::new(),
            false,
            false,
        )
        .unwrap();

    // Original htop preserved (Terminal(2))
    // 2 new floating panes created (Terminal(3), Terminal(4))
    // Total 3 floating panes
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_floating_focus_handling() {
    // Setup: 2 floating panes, one focused
    let initial_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 10
                    y 10
                    width 40
                    height 20
                    focus true
                }
                pane {
                    x 20
                    y 20
                    width 50
                    height 25
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
        &os_api,
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

    // Override: Layout with 1 new pane that has focus=true
    let override_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 30
                    y 30
                    width 60
                    height 30
                    focus true
                }
            }
        }
    "#;

    let (_, override_floating) = parse_kdl_layout(override_kdl);
    let new_floating_terminal_ids = vec![(TerminalId::new(4), None)];

    applier
        .override_floating_panes_layout_for_existing_panes(
            &override_floating,
            new_floating_terminal_ids,
            &mut HashMap::new(),
            false,
            false,
        )
        .unwrap();

    // Focus should be set on newly created pane (Terminal(4))

    // Verify close messages were sent for Terminal(3)
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 1);
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(3))));

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
fn test_override_floating_position_and_size_update() {
    // Setup: 1 floating pane running htop at specific position
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
            }
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![(TerminalId::new(1), None)];
    let floating_terminal_ids = vec![(TerminalId::new(2), None)];

    let size = Size {
        cols: 120,
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
        &os_api,
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

    // Override: Layout with htop at different position and size
    let override_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 50
                    y 30
                    width 60
                    height 30
                    command "htop"
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
            false,
        )
        .unwrap();

    // Same pane ID preserved (Terminal(2))
    // Geometry updated: x=50, y=30, cols=60, rows=30
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_floating_return_value_has_panes() {
    // Setup: Empty floating panes
    let initial_kdl = r#"
        layout {
            pane
        }
    "#;

    let (initial_tiled, initial_floating) = parse_kdl_layout(initial_kdl);
    let terminal_ids = vec![(TerminalId::new(1), None)];

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
        &os_api,
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

    // Override: Layout with 1 floating pane
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

    let (_, override_floating) = parse_kdl_layout(override_kdl);
    let new_floating_terminal_ids = vec![(TerminalId::new(2), None)];

    let has_floating_panes = applier
        .override_floating_panes_layout_for_existing_panes(
            &override_floating,
            new_floating_terminal_ids,
            &mut HashMap::new(),
            false,
            false,
        )
        .unwrap();

    // Function should return true because layout has floating panes
    assert!(has_floating_panes);

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_override_floating_return_value_no_panes() {
    // Setup: 1 floating pane
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
        &os_api,
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

    // Override: Empty floating layout (no floating_panes block)
    let override_kdl = r#"
        layout {
            pane
        }
    "#;

    let (_, override_floating) = parse_kdl_layout(override_kdl);

    let has_floating_panes = applier
        .override_floating_panes_layout_for_existing_panes(
            &override_floating,
            vec![],
            &mut HashMap::new(),
            false,
            false,
        )
        .unwrap();

    // Function should return false because layout has no floating panes
    assert!(!has_floating_panes);

    // Verify close message was sent for floating pane (Terminal(2))
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
// Suite 4: Integration Tests
// ============================================================================

#[test]
fn test_override_full_tiled_and_floating_together() {
    // Setup: Initial layout with 3 tiled panes + 2 floating panes
    let initial_kdl = r#"
        layout {
            pane command="htop"
            pane command="vim"
            pane
            floating_panes {
                pane {
                    x 10
                    y 10
                    width 40
                    height 20
                    command "cargo"
                    args "watch"
                }
                pane {
                    x 20
                    y 20
                    width 50
                    height 25
                    command "tail"
                    args "-f" "/var/log/syslog"
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
    let floating_terminal_ids = vec![(TerminalId::new(4), None), (TerminalId::new(5), None)];

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
        &os_api,
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

    // Override: Layout with 2 tiled (htop, npm start) + 1 floating (cargo watch)
    let override_kdl = r#"
        layout {
            pane command="htop"
            pane command="npm" {
                args "start"
            }
            floating_panes {
                pane {
                    x 30
                    y 30
                    width 60
                    height 30
                    command "cargo"
                    args "watch"
                }
            }
        }
    "#;

    let (override_tiled, override_floating) = parse_kdl_layout(override_kdl);
    let new_terminal_ids = vec![(TerminalId::new(6), None)];

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

    applier
        .override_floating_panes_layout_for_existing_panes(
            &override_floating,
            vec![],
            &mut HashMap::new(),
            false,
            false,
        )
        .unwrap();

    // Tiled: htop preserved (Terminal(1)), vim and shell closed, npm start created (Terminal(6))
    // Floating: cargo watch preserved (Terminal(4)), tail closed
    // Total: 2 tiled + 1 floating

    // Verify close messages were sent for vim (Terminal(2)), shell (Terminal(3)), and tail (Terminal(5))
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert_eq!(closed_panes.len(), 3);
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(2)))); // vim
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(3)))); // shell
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(5)))); // tail

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
fn test_override_viewport_adjustment_with_borderless() {
    // Setup: Initial layout with borderless panes
    let initial_kdl = r#"
        layout {
            pane borderless=true
            pane
            pane borderless=true
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
        &os_api,
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

    // Override: Layout with different borderless configuration
    let override_kdl = r#"
        layout {
            pane
            pane borderless=true
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

    // Viewport dimensions should be correctly adjusted for borderless panes

    // Verify close message was sent for at least the extra pane (Terminal(3))
    // Generic panes without commands don't match exactly, so all 3 may be closed
    let closed_panes = collect_close_pane_messages(&pty_receiver);
    assert!(!closed_panes.is_empty());
    assert!(closed_panes.contains(&PaneId::Terminal(crate::TerminalId::new(3))));

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

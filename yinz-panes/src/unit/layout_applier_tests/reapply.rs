use super::*;

#[test]
fn test_reapply_layout_exact_match() {
    // First apply initial layout
    let initial_kdl = r#"
        layout {
            pane command="htop"
            pane command="vim"
            pane
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(initial_kdl);
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId::new(1),
        )
        .unwrap();

    // Now reapply with commands in different positions
    let new_kdl = r#"
        layout {
            pane
            pane command="htop"
            pane command="vim"
        }
    "#;

    let (new_layout, _) = parse_kdl_layout(new_kdl);

    applier
        .apply_tiled_panes_layout_to_existing_panes(&new_layout)
        .unwrap();

    let snapshot = take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    );

    // Snapshot will show panes matched by command and repositioned
    assert_snapshot!(snapshot);
}

#[test]
fn test_reapply_layout_logical_position_match() {
    // Apply initial layout - 3 panes in horizontal split
    let initial_kdl = r#"
        layout {
            pane
            pane
            pane
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(initial_kdl);
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId::new(1),
        )
        .unwrap();

    // Reapply DIFFERENT layout - still 3 panes but with different split
    // This tests logical position matching (position 0, 1, 2) without exact command match
    let new_kdl = r#"
        layout {
            pane split_direction="Vertical" {
                pane size="50%"
                pane size="50%"
            }
            pane
        }
    "#;

    let (new_layout, _) = parse_kdl_layout(new_kdl);

    applier
        .apply_tiled_panes_layout_to_existing_panes(&new_layout)
        .unwrap();

    // Panes should be repositioned according to new layout structure
    // while being matched by their logical positions (0, 1, 2)
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_reapply_layout_with_more_positions() {
    // Apply initial layout with 2 panes
    let initial_kdl = r#"
        layout {
            pane
            pane
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(initial_kdl);
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId::new(1),
        )
        .unwrap();

    // Reapply with 4 positions (but we only have 2 panes)
    let new_kdl = r#"
        layout {
            pane
            pane
            pane
            pane
        }
    "#;

    let (new_layout, _) = parse_kdl_layout(new_kdl);

    applier
        .apply_tiled_panes_layout_to_existing_panes(&new_layout)
        .unwrap();

    // Should show 2 panes filling first 2 positions, remaining positions empty
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_reapply_floating_pane_layout() {
    // Apply initial layout
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

    let (tiled_layout, floating_layout) = parse_kdl_layout(initial_kdl);
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            floating_terminal_ids,
            HashMap::new(),
            crate::ClientId::new(1),
        )
        .unwrap();

    // Reapply with different position
    let new_kdl = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 20
                    y 20
                    width 50
                    height 25
                }
            }
        }
    "#;

    let (_, new_floating_layout) = parse_kdl_layout(new_kdl);

    applier
        .apply_floating_panes_layout_to_existing_panes(&new_floating_layout)
        .unwrap();

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_apply_complex_nested_layout() {
    let kdl_layout = r#"
        layout {
            pane split_direction="Vertical" {
                pane size="25%"
                pane size="50%" split_direction="Horizontal" {
                    pane size="60%"
                    pane size="40%" split_direction="Vertical" {
                        pane
                        pane
                    }
                }
                pane size="25%"
            }
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
        (TerminalId::new(4), None),
        (TerminalId::new(5), None),
    ];

    let size = Size {
        cols: 200,
        rows: 60,
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId::new(1),
        )
        .unwrap();

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_apply_layout_with_stacked_panes() {
    let kdl_layout = r#"
        layout {
            pane split_direction="Vertical" {
                pane size="70%" stacked=true {
                    pane name="editor-1"
                    pane name="editor-2"
                    pane name="editor-3"
                }
                pane size="30%"
            }
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
        (TerminalId::new(4), None),
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId::new(1),
        )
        .unwrap();

    // Snapshot should show:
    // - 4 panes total (3 in stack + 1 regular)
    // - Stack panes should have stacked field set with same stack_id
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_apply_layout_with_multiple_stacks() {
    let kdl_layout = r#"
        layout {
            pane split_direction="Vertical" {
                pane size="50%" stacked=true {
                    pane name="left-1"
                    pane name="left-2"
                }
                pane size="50%" stacked=true {
                    pane name="right-1"
                    pane name="right-2"
                    pane name="right-3"
                }
            }
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
        (TerminalId::new(4), None),
        (TerminalId::new(5), None),
    ];

    let size = Size {
        cols: 150,
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId::new(1),
        )
        .unwrap();

    // Snapshot should show:
    // - 5 panes in 2 different stacks (different stack_ids)
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_apply_layout_with_plugin_panes() {
    let kdl_layout = r#"
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

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![(TerminalId::new(1), None)];

    // Create plugin IDs - need to match the RunPluginOrAlias from the layout

    let mut new_plugin_ids = HashMap::new();

    // Create plugin aliases that match the layout
    let tab_bar_plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:tab-bar",
        &None,
        None,
        None,
    )
    .unwrap();
    let status_bar_plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:status-bar",
        &None,
        None,
        None,
    )
    .unwrap();

    new_plugin_ids.insert(
        tab_bar_plugin,
        vec![yinz_types::internal_ids::PluginId::new(100)],
    );
    new_plugin_ids.insert(
        status_bar_plugin,
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            new_plugin_ids,
            crate::ClientId::new(1),
        )
        .unwrap();

    // Snapshot should show:
    // - 1 terminal pane
    // - 2 plugin panes with correct plugin locations
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_apply_layout_with_mixed_plugin_and_terminal_panes() {
    let kdl_layout = r#"
        layout {
            pane split_direction="Vertical" {
                pane size="20%" {
                    plugin location="file:///path/to/filebrowser.scm"
                }
                pane size="60%" split_direction="Horizontal" {
                    pane command="vim"
                    pane command="cargo" {
                        args "watch" "-x" "test"
                    }
                }
                pane size="20%" {
                    plugin location="yinz:compact-bar"
                }
            }
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![(TerminalId::new(1), None), (TerminalId::new(2), None)];

    let mut new_plugin_ids = HashMap::new();

    let filebrowser_plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "file:///path/to/filebrowser.scm",
        &None,
        None,
        None,
    )
    .unwrap();
    let compact_bar_plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:compact-bar",
        &None,
        None,
        None,
    )
    .unwrap();

    new_plugin_ids.insert(
        filebrowser_plugin,
        vec![yinz_types::internal_ids::PluginId::new(102)],
    );
    new_plugin_ids.insert(
        compact_bar_plugin,
        vec![yinz_types::internal_ids::PluginId::new(103)],
    );

    let size = Size {
        cols: 200,
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            new_plugin_ids,
            crate::ClientId::new(1),
        )
        .unwrap();

    // Snapshot should show:
    // - 2 terminal panes with commands
    // - 2 plugin panes with different locations
    // - Correct size distribution (20%, 60%, 20%)
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_apply_layout_with_missing_plugin_ids() {
    let kdl_layout = r#"
        layout {
            pane
            pane {
                plugin location="yinz:tab-bar"
            }
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![(TerminalId::new(1), None)];
    // Don't provide plugin IDs - empty HashMap
    let new_plugin_ids = HashMap::new();

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

    let result = applier.apply_layout(
        tiled_layout,
        floating_layout,
        terminal_ids,
        vec![],
        new_plugin_ids,
        crate::ClientId::new(1),
    );

    // This should return an error - missing plugin ID
    assert!(result.is_err());
}

#[test]
fn test_apply_layout_with_excess_terminal_ids() {
    let kdl_layout = r#"
        layout {
            pane
            pane
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    // Provide more terminal IDs than needed
    let terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
        (TerminalId::new(4), None),
    ];

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

    let result = applier.apply_layout(
        tiled_layout,
        floating_layout,
        terminal_ids,
        vec![],
        HashMap::new(),
        crate::ClientId::new(1),
    );

    assert!(result.is_ok());

    // Snapshot should show only 2 panes created
    // Excess IDs should be closed by the applier
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

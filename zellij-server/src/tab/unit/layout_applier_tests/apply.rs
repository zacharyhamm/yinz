use super::*;

#[test]
fn test_apply_empty_layout() {
    let kdl_layout = r#"
        layout {
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![];

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
        None, // blocking_terminal
    );

    let result = applier.apply_layout(
        tiled_layout,
        floating_layout,
        terminal_ids,
        vec![],             // new_floating_terminal_ids
        HashMap::new(),     // new_plugin_ids
        crate::ClientId(1), // client_id
    );

    assert!(result.is_ok());

    let snapshot = take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    );

    assert_snapshot!(snapshot);
}

#[test]
fn test_apply_simple_two_pane_layout() {
    let kdl_layout = r#"
        layout {
            pane
            pane
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![(TerminalId(1), None), (TerminalId(2), None)];

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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId(1),
        )
        .unwrap();

    let snapshot = take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    );

    assert_snapshot!(snapshot);
}

#[test]
fn test_apply_three_pane_layout() {
    let kdl_layout = r#"
        layout {
            pane
            pane
            pane
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![
        (TerminalId(1), None),
        (TerminalId(2), None),
        (TerminalId(3), None),
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId(1),
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
fn test_apply_horizontal_split_with_sizes() {
    let kdl_layout = r#"
        layout {
            pane split_direction="Horizontal" {
                pane size="30%"
                pane size="70%"
            }
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![(TerminalId(1), None), (TerminalId(2), None)];

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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId(1),
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
fn test_apply_vertical_split_with_sizes() {
    let kdl_layout = r#"
        layout {
            pane split_direction="Vertical" {
                pane size="60%"
                pane size="40%"
            }
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![(TerminalId(1), None), (TerminalId(2), None)];

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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId(1),
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
fn test_apply_nested_layout() {
    let kdl_layout = r#"
        layout {
            pane split_direction="Vertical" {
                pane size="60%"
                pane size="40%" split_direction="Horizontal" {
                    pane
                    pane
                }
            }
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![
        (TerminalId(1), None),
        (TerminalId(2), None),
        (TerminalId(3), None),
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId(1),
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
fn test_apply_layout_with_focus() {
    let kdl_layout = r#"
        layout {
            pane
            pane focus=true
            pane
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![
        (TerminalId(1), None),
        (TerminalId(2), None),
        (TerminalId(3), None),
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId(1),
        )
        .unwrap();

    // Snapshot should show FOCUS: Some(Terminal(2))
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_apply_layout_with_commands() {
    let kdl_layout = r#"
        layout {
            pane command="htop"
            pane command="tail" {
                args "-f" "/var/log/syslog"
            }
            pane
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![
        (TerminalId(1), None),
        (TerminalId(2), None),
        (TerminalId(3), None),
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId(1),
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
fn test_apply_layout_with_named_panes() {
    let kdl_layout = r#"
        layout {
            pane name="editor"
            pane name="terminal"
            pane name="logs"
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![
        (TerminalId(1), None),
        (TerminalId(2), None),
        (TerminalId(3), None),
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId(1),
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
fn test_apply_layout_with_borderless_panes() {
    let kdl_layout = r#"
        layout {
            pane borderless=true
            pane
            pane borderless=true
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![
        (TerminalId(1), None),
        (TerminalId(2), None),
        (TerminalId(3), None),
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
            tiled_layout,
            floating_layout,
            terminal_ids,
            vec![],
            HashMap::new(),
            crate::ClientId(1),
        )
        .unwrap();

    // Snapshot should show viewport adjusted for borderless panes
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_apply_layout_with_floating_panes() {
    let kdl_layout = r#"
        layout {
            pane
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

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![(TerminalId(1), None), (TerminalId(2), None)];
    let floating_terminal_ids = vec![(TerminalId(3), None)];

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

    let should_show_floating = applier
        .apply_layout(
            tiled_layout,
            floating_layout,
            terminal_ids,
            floating_terminal_ids,
            HashMap::new(),
            crate::ClientId(1),
        )
        .unwrap();

    assert!(should_show_floating);

    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

#[test]
fn test_apply_layout_with_floating_pane_with_command() {
    let kdl_layout = r#"
        layout {
            pane
            floating_panes {
                pane {
                    x 10
                    y 10
                    width 50
                    height 25
                    command "htop"
                }
            }
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![(TerminalId(1), None)];
    let floating_terminal_ids = vec![(TerminalId(2), None)];

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
            tiled_layout,
            floating_layout,
            terminal_ids,
            floating_terminal_ids,
            HashMap::new(),
            crate::ClientId(1),
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
fn test_apply_layout_with_mixed_tiled_and_floating_panes() {
    let kdl_layout = r#"
        layout {
            pane split_direction="Vertical" {
                pane size="60%" command="vim"
                pane size="40%" split_direction="Horizontal" {
                    pane name="terminal" focus=true
                    pane command="tail" {
                        args "-f" "/var/log/syslog"
                    }
                }
            }
            floating_panes {
                pane {
                    x 5
                    y 5
                    width 40
                    height 15
                    command "htop"
                    name "monitor"
                }
                pane {
                    x "50%"
                    y 10
                    width 45
                    height 20
                    name "notes"
                }
            }
        }
    "#;

    let (tiled_layout, floating_layout) = parse_kdl_layout(kdl_layout);
    let terminal_ids = vec![
        (TerminalId(1), None),
        (TerminalId(2), None),
        (TerminalId(3), None),
    ];
    let floating_terminal_ids = vec![(TerminalId(4), None), (TerminalId(5), None)];

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
        &os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );

    let should_show_floating = applier
        .apply_layout(
            tiled_layout,
            floating_layout,
            terminal_ids,
            floating_terminal_ids,
            HashMap::new(),
            crate::ClientId(1),
        )
        .unwrap();

    assert!(should_show_floating);

    // Snapshot should show:
    // - 3 tiled panes with correct geometries, commands, and names
    // - 2 floating panes with correct positions and properties
    // - Focus on terminal pane (Terminal(2))
    assert_snapshot!(take_pane_state_snapshot(
        &tiled_panes,
        &floating_panes,
        &focus_pane_id,
        &viewport,
        &display_area,
    ));
}

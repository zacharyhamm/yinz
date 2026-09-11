use super::*;

#[test]
fn layout_with_plugins_and_commands_swaped_properly() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            pane size=1 borderless=true {
                plugin location="zellij:tab-bar"
            }
            pane split_direction="vertical" {
                pane command="command1"
                pane
                pane command="command2"
            }
            pane size=2 borderless=true {
                plugin location="zellij:status-bar"
            }
        }
    "#;
    // this swap layout changes both the split direction of the two command panes and the location
    // of the plugins - we want to make sure that they are all placed properly and not switched
    // around
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane size=2 borderless=true {
                        plugin location="zellij:status-bar"
                    }
                    pane command="command2"
                    pane command="command1"
                    pane
                    pane size=1 borderless=true {
                        plugin location="zellij:tab-bar"
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let mut command_1 = RunCommand::default();
    command_1.command = PathBuf::from("command1");
    command_1.hold_on_close = true;
    let mut command_2 = RunCommand::default();
    command_2.command = PathBuf::from("command2");
    command_2.hold_on_close = true;
    let new_terminal_ids = vec![
        (TerminalId::new(1), Some(command_1)),
        (TerminalId::new(2), None),
        (TerminalId::new(3), Some(command_2)),
    ];
    let new_floating_terminal_ids = vec![];
    let mut new_plugin_ids = HashMap::new();
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:tab-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(1)],
    );
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:status-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(2)],
    );

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(1),
        crate::ClientId::new(1),
        "I am a tab bar".as_bytes().to_vec(),
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(2),
        crate::ClientId::new(1),
        "I am a\n\rstatus bar".as_bytes().to_vec(),
    );
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn base_layout_is_included_in_swap_layouts() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            pane size=1 borderless=true {
                plugin location="zellij:tab-bar"
            }
            pane split_direction="vertical" {
                pane command="command1"
                pane
                pane command="command2"
            }
            pane size=2 borderless=true {
                plugin location="zellij:status-bar"
            }
        }
    "#;
    // this swap layout changes both the split direction of the two command panes and the location
    // of the plugins - we want to make sure that they are all placed properly and not switched
    // around
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane size=2 borderless=true {
                        plugin location="zellij:status-bar"
                    }
                    pane command="command2"
                    pane command="command1"
                    pane
                    pane size=1 borderless=true {
                        plugin location="zellij:tab-bar"
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let mut command_1 = RunCommand::default();
    command_1.command = PathBuf::from("command1");
    let mut command_2 = RunCommand::default();
    command_2.command = PathBuf::from("command2");
    let new_terminal_ids = vec![
        (TerminalId::new(1), Some(command_1)),
        (TerminalId::new(2), None),
        (TerminalId::new(3), Some(command_2)),
    ];
    let new_floating_terminal_ids = vec![];
    let mut new_plugin_ids = HashMap::new();
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:tab-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(1)],
    );
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:status-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(2)],
    );

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(1),
        crate::ClientId::new(1),
        "I am a tab bar".as_bytes().to_vec(),
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(2),
        crate::ClientId::new(1),
        "I am a\n\rstatus bar".as_bytes().to_vec(),
    );
    tab.next_swap_layout().unwrap();
    tab.previous_swap_layout().unwrap(); // move back to the base layout
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn swap_layouts_including_command_panes_absent_from_existing_layout() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            pane size=1 borderless=true {
                plugin location="zellij:tab-bar"
            }
            pane split_direction="vertical" {
                pane
                pane
                pane
            }
            pane size=2 borderless=true {
                plugin location="zellij:status-bar"
            }
        }
    "#;
    // this swap layout changes both the split direction of the two command panes and the location
    // of the plugins - we want to make sure that they are all placed properly and not switched
    // around
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane size=2 borderless=true {
                        plugin location="zellij:status-bar"
                    }
                    pane command="command2"
                    pane command="command1"
                    pane
                    pane size=1 borderless=true {
                        plugin location="zellij:tab-bar"
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_floating_terminal_ids = vec![];
    let mut new_plugin_ids = HashMap::new();
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:tab-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(1)],
    );
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:status-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(2)],
    );

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(1),
        crate::ClientId::new(1),
        "I am a tab bar".as_bytes().to_vec(),
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(2),
        crate::ClientId::new(1),
        "I am a\n\rstatus bar".as_bytes().to_vec(),
    );
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn swap_layouts_not_including_command_panes_present_in_existing_layout() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            pane size=1 borderless=true {
                plugin location="zellij:tab-bar"
            }
            pane split_direction="vertical" {
                pane command="command1"
                pane
                pane command="command2"
            }
            pane size=2 borderless=true {
                plugin location="zellij:status-bar"
            }
        }
    "#;
    // this swap layout changes both the split direction of the two command panes and the location
    // of the plugins - we want to make sure that they are all placed properly and not switched
    // around
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane size=2 borderless=true {
                        plugin location="zellij:status-bar"
                    }
                    pane
                    pane
                    pane
                    pane size=1 borderless=true {
                        plugin location="zellij:tab-bar"
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let mut command_1 = RunCommand::default();
    command_1.command = PathBuf::from("command1");
    let mut command_2 = RunCommand::default();
    command_2.command = PathBuf::from("command2");
    let new_terminal_ids = vec![
        (TerminalId::new(1), Some(command_1)),
        (TerminalId::new(2), None),
        (TerminalId::new(3), Some(command_2)),
    ];
    let new_floating_terminal_ids = vec![];
    let mut new_plugin_ids = HashMap::new();
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:tab-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(1)],
    );
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:status-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(2)],
    );

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(1),
        crate::ClientId::new(1),
        "I am a tab bar".as_bytes().to_vec(),
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(2),
        crate::ClientId::new(1),
        "I am a\n\rstatus bar".as_bytes().to_vec(),
    );
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn swap_layouts_including_plugin_panes_absent_from_existing_layout() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            pane size=2 borderless=true
            pane split_direction="vertical" {
                pane
                pane
                pane
            }
            pane size=1 borderless=true
        }
    "#;
    // this swap layout changes both the split direction of the two command panes and the location
    // of the plugins - we want to make sure that they are all placed properly and not switched
    // around
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane size=2 borderless=true {
                        plugin location="zellij:status-bar"
                    }
                    pane
                    pane
                    pane
                    pane size=1 borderless=true {
                        plugin location="zellij:tab-bar"
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_floating_terminal_ids = vec![];
    let new_plugin_ids = HashMap::new();

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn swap_layouts_not_including_plugin_panes_present_in_existing_layout() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            pane size=1 borderless=true {
                plugin location="zellij:tab-bar"
            }
            pane split_direction="vertical" {
                pane command="command1"
                pane
                pane command="command2"
            }
            pane size=2 borderless=true {
                plugin location="zellij:status-bar"
            }
        }
    "#;
    // this swap layout changes both the split direction of the two command panes and the location
    // of the plugins - we want to make sure that they are all placed properly and not switched
    // around
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane size=2 borderless=true {
                        plugin location="zellij:status-bar"
                    }
                    pane
                    pane
                    pane
                    pane size=1 borderless=true {
                        plugin location="zellij:tab-bar"
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let mut command_1 = RunCommand::default();
    command_1.command = PathBuf::from("command1");
    let mut command_2 = RunCommand::default();
    command_2.command = PathBuf::from("command2");
    let new_terminal_ids = vec![
        (TerminalId::new(1), Some(command_1)),
        (TerminalId::new(2), None),
        (TerminalId::new(3), Some(command_2)),
    ];
    let new_floating_terminal_ids = vec![];
    let mut new_plugin_ids = HashMap::new();
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:tab-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(1)],
    );
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:status-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(2)],
    );

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(1),
        crate::ClientId::new(1),
        "I am a tab bar".as_bytes().to_vec(),
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(2),
        crate::ClientId::new(1),
        "I am a\n\rstatus bar".as_bytes().to_vec(),
    );
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn new_pane_in_auto_layout() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout
    "#;
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab max_panes=5 {
                    pane split_direction="vertical" {
                        pane
                        pane { children; }
                    }
                }
                tab max_panes=8 {
                    pane split_direction="vertical" {
                        pane { children; }
                        pane { pane; pane; pane; pane; }
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_floating_terminal_ids = vec![];
    let new_plugin_ids = HashMap::new();

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );

    let mut expected_cursor_coordinates = vec![
        (62, 1),
        (62, 11),
        (62, 15),
        (62, 16),
        (62, 16),
        (62, 16),
        (62, 16),
    ];
    for i in 0..7 {
        let new_pane_id = i + 2;
        tab.new_pane(
            PaneId::Terminal(crate::TerminalId::new(new_pane_id)),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(client_id),
            None,
        )
        .unwrap();
        tab.render(&mut output, None).unwrap();

        let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
            output.serialize().unwrap().get(&client_id).unwrap(),
            size.rows,
            size.cols,
            Palette::default(),
        );
        let (expected_x, expected_y) = expected_cursor_coordinates.remove(0);
        assert_eq!(
            cursor_coordinates,
            Some((expected_x, expected_y)),
            "cursor coordinates moved to the new pane",
        );
        assert_snapshot!(snapshot);
    }
}

#[test]
fn new_pane_in_stacked_resizes() {
    let size = Size {
        cols: 200,
        rows: 40,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_floating_terminal_ids = vec![];
    let new_plugin_ids = HashMap::new();

    let swap_tiled_layouts = vec![];
    let swap_floating_layouts = vec![];
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );

    let mut expected_cursor_coordinates = vec![(101, 1), (101, 21), (151, 21), (151, 22)];
    for i in 0..4 {
        let new_pane_id = i + 2;
        tab.new_pane(
            PaneId::Terminal(crate::TerminalId::new(new_pane_id)),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(client_id),
            None,
        )
        .unwrap();
        tab.render(&mut output, None).unwrap();

        let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
            output.serialize().unwrap().get(&client_id).unwrap(),
            size.rows,
            size.cols,
            Palette::default(),
        );
        let (expected_x, expected_y) = expected_cursor_coordinates.remove(0);
        assert_eq!(
            cursor_coordinates,
            Some((expected_x, expected_y)),
            "cursor coordinates moved to the new pane",
        );
        assert_snapshot!(snapshot);
    }
}

#[test]
fn when_swapping_tiled_layouts_in_a_damaged_state_layout_and_pane_focus_are_unchanged() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            pane
            pane
            pane
        }
    "#;
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane
                        pane
                        pane
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_floating_terminal_ids = vec![];
    let new_plugin_ids = HashMap::new();

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.move_focus_down(client_id);
    tab.resize(
        client_id,
        ResizeStrategy::new(Resize::Increase, Some(Direction::Down)),
    )
    .unwrap();
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();

    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((1, 8)),
        "cursor coordinates moved to the new pane",
    );

    assert_snapshot!(snapshot);
}

#[test]
fn when_swapping_tiled_layouts_in_an_undamaged_state_pane_focuses_on_focused_node() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            pane
            pane
            pane
        }
    "#;
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane
                        pane
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_floating_terminal_ids = vec![];
    let new_plugin_ids = HashMap::new();

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.move_focus_down(client_id);
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();

    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((1, 1)),
        "cursor coordinates moved to the new pane",
    );

    assert_snapshot!(snapshot);
}

#[test]
fn when_swapping_tiled_layouts_in_an_undamaged_state_with_no_focus_node_pane_focuses_on_deepest_node(
) {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            pane
            pane
            pane
        }
    "#;
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane
                        pane
                        pane
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_floating_terminal_ids = vec![];
    let new_plugin_ids = HashMap::new();

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.move_focus_down(client_id);
    let _ = tab.move_focus_down(client_id);
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();

    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((82, 1)),
        "cursor coordinates moved to the new pane",
    );

    assert_snapshot!(snapshot);
}

#[test]
fn when_closing_a_pane_in_auto_layout_the_focus_goes_to_last_focused_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            pane
            pane
            pane
        }
    "#;
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane
                        pane
                        pane
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_floating_terminal_ids = vec![];
    let new_plugin_ids = HashMap::new();

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.move_focus_down(client_id);
    let _ = tab.move_focus_down(client_id);
    tab.close_pane(PaneId::Terminal(crate::TerminalId::new(3)), false, None);
    tab.render(&mut output, None).unwrap();

    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((62, 1)),
        "cursor coordinates moved to the new pane",
    );
    assert_snapshot!(snapshot);
}

#[test]
fn floating_layout_with_plugins_and_commands_swaped_properly() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            floating_panes {
                pane x=0 y=0 {
                    plugin location="zellij:tab-bar"
                }
                pane x=0 y=10 command="command1"
                pane
                pane x=50 y=10 command="command2"
                pane x=50 y=0 {
                    plugin location="zellij:status-bar"
                }
            }
        }
    "#;
    // this swap layout swaps between the location of the plugins and the commands
    let swap_layouts = r#"
        layout {
            swap_floating_layout {
                floating_panes {
                    pane x=0 y=0 {
                        plugin location="zellij:status-bar"
                    }
                    pane x=0 y=10 command="command2"
                    pane
                    pane x=50 y=10 command="command1"
                    pane x=50 y=0 {
                        plugin location="zellij:tab-bar"
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let mut command_1 = RunCommand::default();
    command_1.command = PathBuf::from("command1");
    let mut command_2 = RunCommand::default();
    command_2.command = PathBuf::from("command2");
    let new_floating_terminal_ids = vec![
        (TerminalId::new(1), Some(command_1)),
        (TerminalId::new(2), None),
        (TerminalId::new(3), Some(command_2)),
    ];
    let new_terminal_ids = vec![(TerminalId::new(4), None)];
    let mut new_plugin_ids = HashMap::new();
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:tab-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(1)],
    );
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:status-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(2)],
    );

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(1),
        crate::ClientId::new(1),
        "I am a tab bar".as_bytes().to_vec(),
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(2),
        crate::ClientId::new(1),
        "I am a\n\rstatus bar".as_bytes().to_vec(),
    );
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn base_floating_layout_is_included_in_swap_layouts() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            floating_panes {
                pane x=0 y=0 {
                    plugin location="zellij:tab-bar"
                }
                pane x=0 y=10 command="command1"
                pane
                pane x=50 y=10 command="command2"
                pane x=50 y=0 {
                    plugin location="zellij:status-bar"
                }
            }
        }
    "#;
    // this swap layout swaps between the location of the plugins and the commands
    let swap_layouts = r#"
        layout {
            swap_floating_layout {
                floating_panes {
                    pane x=0 y=0 {
                        plugin location="zellij:status-bar"
                    }
                    pane x=0 y=10 command="command2"
                    pane
                    pane x=50 y=10 command="command1"
                    pane x=50 y=0 {
                        plugin location="zellij:tab-bar"
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let mut command_1 = RunCommand::default();
    command_1.command = PathBuf::from("command1");
    let mut command_2 = RunCommand::default();
    command_2.command = PathBuf::from("command2");
    let new_floating_terminal_ids = vec![
        (TerminalId::new(1), Some(command_1)),
        (TerminalId::new(2), None),
        (TerminalId::new(3), Some(command_2)),
    ];
    let new_terminal_ids = vec![(TerminalId::new(4), None)];
    let mut new_plugin_ids = HashMap::new();
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:tab-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(1)],
    );
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:status-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(2)],
    );

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(1),
        crate::ClientId::new(1),
        "I am a tab bar".as_bytes().to_vec(),
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(2),
        crate::ClientId::new(1),
        "I am a\n\rstatus bar".as_bytes().to_vec(),
    );
    tab.next_swap_layout().unwrap();
    tab.previous_swap_layout().unwrap(); // move back to the base layout
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn swap_floating_layouts_including_command_panes_absent_from_existing_layout() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            floating_panes {
                pane {
                    plugin location="zellij:tab-bar"
                }
                pane
                pane
                pane
                pane {
                    plugin location="zellij:status-bar"
                }
            }
        }
    "#;
    // this swap layout changes both the split direction of the two command panes and the location
    // of the plugins - we want to make sure that they are all placed properly and not switched
    // around
    let swap_layouts = r#"
        layout {
            swap_floating_layout {
                floating_panes {
                    pane {
                        plugin location="zellij:status-bar"
                    }
                    pane x=0 y=0 command="command1"
                    pane x=10 y=10 command="command2"
                    pane
                    pane {
                        plugin location="zellij:tab-bar"
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_floating_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_terminal_ids = vec![(TerminalId::new(4), None)];
    let mut new_plugin_ids = HashMap::new();
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:tab-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(1)],
    );
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:status-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(2)],
    );

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(1),
        crate::ClientId::new(1),
        "I am a tab bar".as_bytes().to_vec(),
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(2),
        crate::ClientId::new(1),
        "I am a\n\rstatus bar".as_bytes().to_vec(),
    );
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn swap_floating_layouts_not_including_command_panes_present_in_existing_layout() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            floating_panes {
                pane {
                    plugin location="zellij:tab-bar"
                }
                pane command="command1"
                pane
                pane command="command2"
                pane {
                    plugin location="zellij:status-bar"
                }
            }
        }
    "#;
    // this swap layout changes both the split direction of the two command panes and the location
    // of the plugins - we want to make sure that they are all placed properly and not switched
    // around
    let swap_layouts = r#"
        layout {
            swap_floating_layout {
                floating_panes {
                    pane {
                        plugin location="zellij:status-bar"
                    }
                    pane
                    pane
                    pane
                    pane {
                        plugin location="zellij:tab-bar"
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let mut command_1 = RunCommand::default();
    command_1.command = PathBuf::from("command1");
    let mut command_2 = RunCommand::default();
    command_2.command = PathBuf::from("command2");
    let new_floating_terminal_ids = vec![
        (TerminalId::new(1), Some(command_1)),
        (TerminalId::new(2), None),
        (TerminalId::new(3), Some(command_2)),
    ];
    let new_terminal_ids = vec![(TerminalId::new(4), None)];
    let mut new_plugin_ids = HashMap::new();
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:tab-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(1)],
    );
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:status-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(2)],
    );

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(1),
        crate::ClientId::new(1),
        "I am a tab bar".as_bytes().to_vec(),
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(2),
        crate::ClientId::new(1),
        "I am a\n\rstatus bar".as_bytes().to_vec(),
    );
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn swap_floating_layouts_including_plugin_panes_absent_from_existing_layout() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            floating_panes {
                pane
                pane
                pane
            }
        }
    "#;
    let swap_layouts = r#"
        layout {
            swap_floating_layout {
                floating_panes {
                    pane {
                        plugin location="zellij:status-bar"
                    }
                    pane
                    pane {
                        plugin location="zellij:tab-bar"
                    }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_floating_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_terminal_ids = vec![(TerminalId::new(4), None)];
    let new_plugin_ids = HashMap::new();

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn swap_floating_layouts_not_including_plugin_panes_present_in_existing_layout() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            floating_panes {
                pane {
                    plugin location="zellij:tab-bar"
                }
                pane
                pane {
                    plugin location="zellij:status-bar"
                }
            }
        }
    "#;
    // this swap layout changes both the split direction of the two command panes and the location
    // of the plugins - we want to make sure that they are all placed properly and not switched
    // around
    let swap_layouts = r#"
        layout {
            swap_floating_layout {
                floating_panes {
                    pane
                    pane
                    pane
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let mut command_1 = RunCommand::default();
    command_1.command = PathBuf::from("command1");
    let mut command_2 = RunCommand::default();
    command_2.command = PathBuf::from("command2");
    let new_floating_terminal_ids = vec![
        (TerminalId::new(1), Some(command_1)),
        (TerminalId::new(2), None),
        (TerminalId::new(3), Some(command_2)),
    ];
    let new_terminal_ids = vec![(TerminalId::new(4), None)];
    let mut new_plugin_ids = HashMap::new();
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:tab-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(1)],
    );
    new_plugin_ids.insert(
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "zellij:status-bar",
            &None,
            None,
            None,
        )
        .unwrap(),
        vec![zellij_types::internal_ids::PluginId::new(2)],
    );

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(1),
        crate::ClientId::new(1),
        "I am a tab bar".as_bytes().to_vec(),
    );
    let _ = tab.handle_plugin_bytes(
        crate::PluginId::new(2),
        crate::ClientId::new(1),
        "I am a\n\rstatus bar".as_bytes().to_vec(),
    );
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn new_floating_pane_in_auto_layout() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout
    "#;
    let swap_layouts = r#"
        layout {
            swap_floating_layout name="spread" {
                floating_panes max_panes=1 {
                    pane {y "50%"; x "50%"; }
                }
                floating_panes max_panes=2 {
                    pane { x "1%"; y "25%"; width "45%"; }
                    pane { x "50%"; y "25%"; width "45%"; }
                }
                floating_panes max_panes=3 {
                    pane { x "1%"; y "1%"; width "45%"; }
                    pane { x "50%"; y "1%"; width "45%"; }
                    pane { y "55%"; width "45%"; height "45%"; }
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_terminal_ids = vec![(TerminalId::new(1), None)];
    let new_floating_terminal_ids = vec![];
    let new_plugin_ids = HashMap::new();

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );

    let mut expected_cursor_coordinates = vec![(61, 11), (61, 6), (34, 12)];
    for i in 0..3 {
        let new_pane_id = i + 2;
        tab.new_pane(
            PaneId::Terminal(crate::TerminalId::new(new_pane_id)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(None),
            Some(client_id),
            None,
        )
        .unwrap();
        tab.render(&mut output, None).unwrap();

        let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
            output.serialize().unwrap().get(&client_id).unwrap(),
            size.rows,
            size.cols,
            Palette::default(),
        );
        let (expected_x, expected_y) = expected_cursor_coordinates.remove(0);
        assert_eq!(
            cursor_coordinates,
            Some((expected_x, expected_y)),
            "cursor coordinates moved to the new pane",
        );
        assert_snapshot!(snapshot);
    }
}

#[test]
fn when_swapping_floating_layouts_in_a_damaged_state_layout_and_pane_focus_are_unchanged() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            floating_panes {
                pane x=0 y=0
                pane
                pane
            }
        }
    "#;
    let swap_layouts = r#"
        layout {
            swap_floating_layout {
                floating_panes {
                    pane
                    pane
                    pane
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_floating_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_terminal_ids = vec![(TerminalId::new(4), None)];
    let new_plugin_ids = HashMap::new();

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    tab.resize(
        client_id,
        ResizeStrategy::new(Resize::Increase, Some(Direction::Down)),
    )
    .unwrap();
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();

    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((33, 8)),
        "cursor coordinates moved to the new pane",
    );

    assert_snapshot!(snapshot);
}

#[test]
fn when_swapping_floating_layouts_in_an_undamaged_state_pane_focuses_on_focused_node() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            floating_panes {
                pane x=0 y=0
                pane
                pane
            }
        }
    "#;
    let swap_layouts = r#"
        layout {
            swap_floating_layout {
                floating_panes {
                    pane
                    pane
                    pane
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_floating_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_terminal_ids = vec![(TerminalId::new(4), None)];
    let new_plugin_ids = HashMap::new();

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();

    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((35, 10)),
        "cursor coordinates moved to the new pane",
    );

    assert_snapshot!(snapshot);
}

#[test]
fn when_swapping_floating_layouts_in_an_undamaged_state_with_no_focus_node_pane_focuses_on_deepest_node(
) {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            floating_panes {
                pane
                pane
                pane
            }
        }
    "#;
    let swap_layouts = r#"
        layout {
            swap_floating_layout {
                floating_panes {
                    pane
                    pane
                    pane
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_floating_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_terminal_ids = vec![(TerminalId::new(4), None)];
    let new_plugin_ids = HashMap::new();

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    tab.next_swap_layout().unwrap();
    tab.render(&mut output, None).unwrap();

    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((35, 10)),
        "cursor coordinates moved to the new pane",
    );

    assert_snapshot!(snapshot);
}

#[test]
fn when_closing_a_floating_pane_in_auto_layout_the_focus_goes_to_last_focused_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            floating_panes {
                pane
                pane
                pane
            }
        }
    "#;
    let swap_layouts = r#"
        layout {
            swap_floating_layout {
                floating_panes {
                    pane
                    pane
                    pane
                }
            }
        }
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_floating_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_terminal_ids = vec![(TerminalId::new(4), None)];
    let new_plugin_ids = HashMap::new();

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let _ = tab.move_focus_up(client_id);
    let _ = tab.move_focus_up(client_id);
    tab.close_pane(PaneId::Terminal(crate::TerminalId::new(1)), false, None);
    tab.render(&mut output, None).unwrap();

    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((31, 6)),
        "cursor coordinates moved to the new pane",
    );
    assert_snapshot!(snapshot);
}

#[test]
fn when_resizing_whole_tab_with_auto_layout_and_floating_panes_the_layout_is_maintained() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let base_layout = r#"
        layout {
            floating_panes {
                pane
                pane
                pane
            }
        }
    "#;
    let swap_layouts = r#"
        layout
    "#;
    let (base_layout, base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();

    let new_floating_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
    ];
    let new_terminal_ids = vec![(TerminalId::new(4), None)];
    let new_plugin_ids = HashMap::new();

    let swap_layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = swap_layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = swap_layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        Some((
            base_layout,
            base_floating_layout,
            new_terminal_ids,
            new_floating_terminal_ids,
            new_plugin_ids,
        )),
        true,
        stacked_resize,
    );
    let new_size = Size {
        cols: 150,
        rows: 30,
    };
    let _ = tab.resize_whole_tab(new_size);
    tab.render(&mut output, None).unwrap();

    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        new_size.rows,
        new_size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((43, 13)),
        "cursor coordinates moved to the new pane",
    );
    assert_snapshot!(snapshot);
}

#[test]
fn when_applying_a_truncated_swap_layout_child_attributes_are_not_ignored() {
    // here we want to make sure that the nested borderless is preserved on resize (when the layout
    // is reapplied, and thus is truncated to just one pane rather than a logical container pane
    // and an actual pane as it is described here)
    let layout = r#"
        layout {
            pane {
                pane borderless=true
            }
        }
    "#;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab_with_layout(size, ModeInfo::default(), layout);
    let mut output = Output::default();
    let new_size = Size {
        cols: 122,
        rows: 20,
    };
    let _ = tab.resize_whole_tab(new_size);
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        new_size.rows,
        new_size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn can_define_expanded_pane_in_stack() {
    let layout = r#"
        layout {
            pane split_direction="Vertical" {
                pane
                pane stacked=true {
                    pane
                    pane expanded=true
                    pane
                    pane
                }
            }
        }
    "#;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab_with_layout(size, ModeInfo::default(), layout);
    let mut output = Output::default();
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

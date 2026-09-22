use super::*;

#[test]
fn can_swap_tiled_layout_at_runtime() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab max_panes=2 split_direction="vertical" {
                    pane
                    pane
                }
            }
            swap_tiled_layout {
                tab max_panes=2 {
                    pane
                    pane
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
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
fn can_swap_floating_layout_at_runtime() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_floating_layout {
                floating_panes max_panes=2 {
                    pane
                    pane
                }
            }
            swap_floating_layout {
                floating_panes max_panes=2 {
                    pane {
                        x "0%"
                    }
                    pane {
                        x "100%"
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
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
fn swapping_layouts_after_resize_snaps_to_current_layout() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane
                    pane
                }
            }
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane
                        pane
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.next_swap_layout().unwrap();
    tab.resize(client_id, ResizeStrategy::new(Resize::Increase, None))
        .unwrap();
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
fn swap_tiled_layout_with_stacked_children() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
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
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn swap_tiled_layout_with_only_stacked_children() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane stacked=true { children; }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
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
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn swap_tiled_layout_with_stacked_children_and_no_pane_frames() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        false,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
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
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn move_focus_up_with_stacked_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.move_focus_right(client_id);
    let _ = tab.move_focus_up(client_id);
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
fn move_focus_down_with_stacked_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.move_focus_right(client_id);
    let _ = tab.move_focus_up(client_id);
    let _ = tab.move_focus_down(client_id);
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
fn move_focus_right_into_stacked_panes() {
    // here we make sure that when we focus right into a stack,
    // we will always focus on the "main" pane of the stack
    // and not on one of its folds
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    for i in 0..12 {
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
    }
    let _ = tab.move_focus_left(client_id);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(16)),
        None,
        client_id,
        None,
        None,
    )
    .unwrap();

    let _ = tab.move_focus_up(client_id);
    let _ = tab.move_focus_right(client_id);
    tab.render(&mut output, None).unwrap();

    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((62, 12)),
        "cursor coordinates moved to the main pane of the stack",
    );

    assert_snapshot!(snapshot);
}

#[test]
fn move_focus_left_into_stacked_panes() {
    // here we make sure that when we focus left into a stack,
    // we will always focus on the "main" pane of the stack
    // and not on one of its folds
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane stacked=true { children; }
                        pane focus=true
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    for i in 0..13 {
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
    }
    let _ = tab.move_focus_right(client_id);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(1)),
        None,
        client_id,
        None,
        None,
    )
    .unwrap();

    let _ = tab.move_focus_up(client_id);
    let _ = tab.move_focus_left(client_id);
    tab.render(&mut output, None).unwrap();

    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((1, 12)),
        "cursor coordinates moved to the main pane of the stack",
    );

    assert_snapshot!(snapshot);
}

#[test]
fn move_focus_up_into_stacked_panes() {
    // here we make sure that when we focus up into a stack,
    // the main pane will become the lowest pane and the sizes
    // in the stack will be adjusted accordingly
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                    pane
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    for i in 0..4 {
        let new_pane_id = i + 3;
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
    }
    let _ = tab.move_focus_right(client_id);
    let _ = tab.move_focus_up(client_id);
    let _ = tab.move_focus_left(client_id);
    let _ = tab.move_focus_down(client_id);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(7)),
        None,
        client_id,
        None,
        None,
    )
    .unwrap();

    let _ = tab.move_focus_up(client_id);
    tab.render(&mut output, None).unwrap();

    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((62, 9)),
        "cursor coordinates moved to the main pane of the stack",
    );
    assert_snapshot!(snapshot);
}

#[test]
fn move_focus_down_into_stacked_panes() {
    // here we make sure that when we focus down into a stack,
    // the main pane will become the highest pane and the sizes
    // in the stack will be adjusted accordingly
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane
                    pane split_direction="vertical" {
                        pane
                        pane stacked=true { children; }
                    }
                    pane
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    for i in 0..4 {
        let new_pane_id = i + 3;
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
    }
    let _ = tab.move_focus_left(client_id);
    let _ = tab.move_focus_up(client_id);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(7)),
        None,
        client_id,
        None,
        None,
    )
    .unwrap();

    let _ = tab.move_focus_down(client_id);
    tab.render(&mut output, None).unwrap();

    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((62, 8)),
        "cursor coordinates moved to the main pane of the stack",
    );

    assert_snapshot!(snapshot);
}

#[test]
fn focus_last_stacked_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.move_focus_right(client_id).unwrap();
    tab.move_focus_up(client_id).unwrap();
    tab.move_focus_up(client_id).unwrap();
    tab.focus_last_pane(client_id);
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
fn close_main_stacked_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.close_pane(new_pane_id_2, false, None);
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
fn close_main_stacked_pane_in_mid_stack() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.move_focus_right(client_id);
    let _ = tab.move_focus_up(client_id);
    let _ = tab.move_focus_up(client_id);
    tab.close_pane(new_pane_id_3, false, None);
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
fn close_one_liner_stacked_pane_below_main_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.move_focus_left(client_id);
    let _ = tab.move_focus_right(client_id);
    let _ = tab.move_focus_up(client_id);
    let _ = tab.move_focus_up(client_id);
    tab.close_pane(new_pane_id_2, false, None);
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
fn close_one_liner_stacked_pane_above_main_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.move_focus_right(client_id);
    let _ = tab.move_focus_up(client_id);
    let _ = tab.move_focus_up(client_id);
    tab.close_pane(new_pane_id_2, false, None);
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
fn can_increase_size_of_main_pane_in_stack_horizontally() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.move_focus_right(client_id);
    tab.resize(
        client_id,
        ResizeStrategy::new(Resize::Increase, Some(Direction::Left)),
    )
    .unwrap();
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
fn can_increase_size_of_main_pane_in_stack_vertically() {
    let size = Size {
        cols: 121,
        rows: 40,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                    pane
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.move_focus_right(client_id);
    tab.resize(
        client_id,
        ResizeStrategy::new(Resize::Increase, Some(Direction::Down)),
    )
    .unwrap();
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
fn can_increase_size_of_main_pane_in_stack_non_directionally() {
    let size = Size {
        cols: 121,
        rows: 40,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                    pane
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = false;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.move_focus_right(client_id);
    tab.resize(client_id, ResizeStrategy::new(Resize::Increase, None))
        .unwrap();
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
fn can_increase_size_into_pane_stack_horizontally() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.resize(
        client_id,
        ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
    )
    .unwrap();
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
fn can_increase_size_into_pane_stack_vertically() {
    let size = Size {
        cols: 121,
        rows: 40,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                    pane
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.move_focus_right(client_id);
    let _ = tab.move_focus_down(client_id);
    tab.resize(
        client_id,
        ResizeStrategy::new(Resize::Increase, Some(Direction::Up)),
    )
    .unwrap();
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
fn can_increase_size_into_pane_stack_non_directionally() {
    // note - this is not the default behavior, by default stacked_resize is enabled (set to true)
    let size = Size {
        cols: 121,
        rows: 40,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                    pane
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = false;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.resize(client_id, ResizeStrategy::new(Resize::Increase, None))
        .unwrap();
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
fn decreasing_size_of_whole_tab_treats_stacked_panes_properly() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.resize_whole_tab(Size {
        cols: 100,
        rows: 10,
    });
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
fn increasing_size_of_whole_tab_treats_stacked_panes_properly() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.resize_whole_tab(Size {
        cols: 100,
        rows: 10,
    });
    let _ = tab.resize_whole_tab(Size {
        cols: 121,
        rows: 20,
    });
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
fn cannot_decrease_stack_size_beyond_minimum_height() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                    pane
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.move_focus_down(client_id);
    for _ in 0..6 {
        tab.resize(
            client_id,
            ResizeStrategy::new(Resize::Increase, Some(Direction::Up)),
        )
        .unwrap();
    }
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
fn focus_stacked_pane_over_flexible_pane_with_the_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                    pane
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(1, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(1, 71)),
        client_id,
    )
    .unwrap();
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
fn focus_stacked_pane_under_flexible_pane_with_the_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                    pane
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(1, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(9, 71)),
        client_id,
    )
    .unwrap();
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
fn close_stacked_pane_with_previously_focused_other_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane
                        pane stacked=true { children; }
                    }
                    pane
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(2, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(2, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(1, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(1, 71)),
        client_id,
    )
    .unwrap();
    tab.close_pane(PaneId::Terminal(crate::TerminalId::new(4)), false, None);
    tab.render(&mut output, None).unwrap();
    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((62, 2)),
        "cursor coordinates moved to the main pane of the stack",
    );

    assert_snapshot!(snapshot);
}

#[test]
fn close_pane_near_stacked_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane
                        pane stacked=true { children; }
                    }
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.close_pane(PaneId::Terminal(crate::TerminalId::new(6)), false, None);
    tab.render(&mut output, None).unwrap();
    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((62, 4)),
        "cursor coordinates moved to the main pane of the stack",
    );

    assert_snapshot!(snapshot);
}

#[test]
fn focus_next_pane_expands_stacked_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                    pane
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.move_focus_left(client_id);
    tab.focus_next_pane(client_id);
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
fn stacked_panes_can_become_fullscreen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut output = Output::default();
    let swap_layouts = r#"
        layout {
            swap_tiled_layout {
                tab {
                    pane split_direction="vertical" {
                        pane focus=true
                        pane stacked=true { children; }
                    }
                    pane
                }
            }
        }
    "#;
    let layout = zellij_config::kdl::layout::from_kdl(
        swap_layouts,
        Some("file_name.kdl".into()),
        None,
        None,
    )
    .unwrap();
    let swap_tiled_layouts = layout.swap_tiled_layouts.clone();
    let swap_floating_layouts = layout.swap_floating_layouts.clone();
    let stacked_resize = true;
    let mut tab = create_new_tab_with_swap_layouts(
        size,
        ModeInfo::default(),
        (swap_tiled_layouts, swap_floating_layouts),
        None,
        true,
        stacked_resize,
    );
    let new_pane_id_1 = PaneId::Terminal(crate::TerminalId::new(2));
    let new_pane_id_2 = PaneId::Terminal(crate::TerminalId::new(3));
    let new_pane_id_3 = PaneId::Terminal(crate::TerminalId::new(4));
    let new_pane_id_4 = PaneId::Terminal(crate::TerminalId::new(5));
    let new_pane_id_5 = PaneId::Terminal(crate::TerminalId::new(6));

    tab.new_pane(
        new_pane_id_1,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_2,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_3,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_4,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id_5,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    let _ = tab.move_focus_up(client_id);
    tab.toggle_active_pane_fullscreen(client_id);
    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

fn sorted_kitty_visible_pane_ids(tab: &Tab) -> Vec<PaneId> {
    let mut pane_ids: Vec<PaneId> = tab.kitty_visible_pane_ids().into_iter().collect();
    pane_ids.sort();
    pane_ids
}

#[test]
fn kitty_visible_panes_exclude_collapsed_stack_members() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    for i in 2..4 {
        tab.new_pane(
            PaneId::Terminal(crate::TerminalId::new(i)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: None,
                borderless: None,
            },
            Some(client_id),
            None,
        )
        .unwrap();
    }
    let visible_before_focus_change = sorted_kitty_visible_pane_ids(&tab);
    tab.move_focus_up(client_id).unwrap();
    let visible_after_focus_change = sorted_kitty_visible_pane_ids(&tab);
    assert_eq!(
        visible_before_focus_change,
        vec![PaneId::Terminal(crate::TerminalId::new(3))],
        "only the expanded stack member is kitty visible"
    );
    assert_eq!(
        visible_after_focus_change,
        vec![PaneId::Terminal(crate::TerminalId::new(2))],
        "kitty visibility follows the expanded stack member"
    );
}

fn a_stack_beside_a_plain_pane_layout() -> (TiledPaneLayout, Vec<(TerminalId, Option<RunCommand>)>)
{
    let base_layout = r#"
        layout {
            pane split_direction="vertical" {
                pane stacked=true {
                    pane focus=true
                    pane
                    pane
                }
                pane
            }
        }
    "#;
    let (base_layout, _base_floating_layout) =
        zellij_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();
    let new_terminal_ids = (1..=4).map(|id| (TerminalId::new(id), None)).collect();
    (base_layout, new_terminal_ids)
}

/// A tiled pane resized by id in stack-list mode dissolves the lists first, as a keyboard
/// resize does: the classic solver has to see every pane. The by-id path used to dissolve
/// only when the resized pane was itself a list member.
#[test]
fn resizing_a_tiled_pane_by_id_in_stack_list_mode_dissolves_the_lists_as_the_keyboard_does() {
    let size = Size {
        cols: 150,
        rows: 40,
    };
    let mut tab = create_new_tab_with_stacked_pane_list(
        size,
        ModeInfo::default(),
        true,
        Some(a_stack_beside_a_plain_pane_layout()),
    );
    tab.render(&mut Output::default(), None).unwrap();
    assert!(
        tab.has_stack_lists(),
        "the state under test: a stack list is up"
    );
    let plain_pane_id = tab
        .get_all_pane_ids()
        .into_iter()
        .find(|id| !tab.pane_is_stack_list_member(id))
        .expect("the layout has a pane outside the stack");

    tab.resize_pane_with_id(ResizeStrategy::new(Resize::Increase, None), plain_pane_id)
        .unwrap();

    assert!(
        !tab.has_stack_lists(),
        "the lists were dissolved for the classic resize"
    );
}

#[test]
fn resizing_a_suppressed_or_unknown_pane_preserves_stack_lists_and_geometry() {
    for target in [TerminalId::new(5), TerminalId::new(99)] {
        let mut tab = create_new_tab_with_stacked_pane_list(
            Size {
                cols: 150,
                rows: 40,
            },
            ModeInfo::default(),
            true,
            Some(a_stack_beside_a_plain_pane_layout()),
        );
        let suppressed_pane = PaneId::Terminal(TerminalId::new(5));
        tab.new_pane(
            suppressed_pane,
            None,
            None,
            true,
            false,
            NewPanePlacement::default(),
            Some(ClientId::new(1)),
            None,
        )
        .unwrap();
        tab.render(&mut Output::default(), None).unwrap();
        assert!(tab.has_stack_lists());
        assert!(tab.suppressed_panes.contains_key(&suppressed_pane));
        let membership = tab.stack_list_of_member.clone();
        let geometry: Vec<_> = (1..=5)
            .map(|id| {
                let id = PaneId::Terminal(TerminalId::new(id));
                (id, tab.get_pane_with_id(id).unwrap().current_geom())
            })
            .collect();

        tab.resize_pane_with_id(
            ResizeStrategy::new(Resize::Increase, None),
            PaneId::Terminal(target),
        )
        .unwrap();

        assert!(tab.has_stack_lists());
        assert_eq!(tab.stack_list_of_member, membership);
        assert!(tab.suppressed_panes.contains_key(&suppressed_pane));
        for (id, geom) in geometry {
            assert_eq!(tab.get_pane_with_id(id).unwrap().current_geom(), geom);
        }
    }
}

#[test]
fn resizing_a_hidden_stack_list_member_dissolves_the_lists() {
    let mut tab = create_new_tab_with_stacked_pane_list(
        Size {
            cols: 150,
            rows: 40,
        },
        ModeInfo::default(),
        true,
        Some(a_stack_beside_a_plain_pane_layout()),
    );
    tab.render(&mut Output::default(), None).unwrap();
    let hidden_member = tab
        .get_all_pane_ids()
        .into_iter()
        .find(|id| tab.pane_is_hidden_stack_list_member(id))
        .expect("the layout has a hidden stack-list member");

    tab.resize_pane_with_id(ResizeStrategy::new(Resize::Increase, None), hidden_member)
        .unwrap();

    assert!(!tab.has_stack_lists());
    assert!(tab.tiled_panes.panes_contain(&hidden_member));
    assert!(!tab.suppressed_panes.contains_key(&hidden_member));
}

#[test]
fn a_pane_stacked_onto_an_existing_stack_is_inset_and_its_pty_resized() {
    // `add_pane_to_stack` put the pane straight into the map, skipping the frame inset
    // `add_pane_with_existing_geom` applies and the pty resize a relayout would send, so
    // a shell stacked through `Screen::stack_panes` kept a pty two columns too wide until
    // the next resize.
    let (tx, rx) = channels::unbounded();
    let mut tab = create_new_tab_with_mock_pty_writer(
        Size { cols: 80, rows: 30 },
        ModeInfo::default(),
        SenderWithContext::new(tx),
    );
    let client_id = crate::ClientId::new(1);
    let root = PaneId::Terminal(crate::TerminalId::new(1));
    let member = PaneId::Terminal(crate::TerminalId::new(2));
    let loose = PaneId::Terminal(crate::TerminalId::new(3));
    // The loose pane first and unfocused: a tiled pane opened while a stack member is
    // focused would join that stack.
    tab.new_tiled_pane(loose, None, None, false, false, Some(client_id), None, None)
        .unwrap();
    tab.new_stacked_pane(
        member,
        None,
        None,
        false,
        true,
        Some(root),
        Some(client_id),
        None,
        None,
    )
    .unwrap();
    assert!(
        !tab.tiled_panes
            .get_pane(loose)
            .unwrap()
            .position_and_size()
            .is_stacked(),
        "precondition: the third pane sits beside the stack"
    );
    let mut pane = tab.extract_pane(loose, false).expect("the loose pane");
    // As it would arrive from a tab that draws no frames.
    pane.set_content_offset(zellij_types::pane_size::Offset::default());
    let _ = rx.try_iter().count(); // whatever creation and extraction sent

    tab.stack_panes(root, vec![pane]);

    let stacked = tab
        .tiled_panes
        .get_pane(loose)
        .expect("the pane joined the stack");
    assert!(stacked.position_and_size().is_stacked());
    assert_eq!(
        stacked.get_content_columns(),
        stacked.cols() - 2,
        "full frames inset the content by a column on each side"
    );
    let resized: Vec<PaneId> = rx
        .try_iter()
        .filter_map(|(msg, _)| match msg {
            PtyWriteInstruction::ResizePty(id, ..) => Some(PaneId::Terminal(id)),
            _ => None,
        })
        .collect();
    assert!(resized.contains(&loose), "{resized:?}");
}

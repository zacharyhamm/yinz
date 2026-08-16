use super::*;

#[test]
fn can_swap_tiled_layout_at_runtime() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
            PaneId::Terminal(new_pane_id),
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
    tab.horizontal_split(PaneId::Terminal(16), None, client_id, None, None)
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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
            PaneId::Terminal(new_pane_id),
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
    tab.horizontal_split(PaneId::Terminal(1), None, client_id, None, None)
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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
            PaneId::Terminal(new_pane_id),
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
    tab.vertical_split(PaneId::Terminal(7), None, client_id, None, None)
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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
            PaneId::Terminal(new_pane_id),
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
    tab.vertical_split(PaneId::Terminal(7), None, client_id, None, None)
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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    tab.close_pane(PaneId::Terminal(4), false, None);
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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    tab.close_pane(PaneId::Terminal(6), false, None);
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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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
    let client_id = 1;
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
    let layout = Layout::from_kdl(swap_layouts, Some("file_name.kdl".into()), None, None).unwrap();
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
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);

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

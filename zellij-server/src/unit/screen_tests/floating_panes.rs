use super::*;

#[test]
fn open_new_floating_pane_with_custom_coordinates() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(crate::ClientId(1)).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(crate::TerminalId(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Percent(10)),
                y: Some(PercentOrFixed::Fixed(5)),
                width: Some(PercentOrFixed::Percent(1)),
                height: Some(PercentOrFixed::Fixed(2)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(crate::ClientId(1)),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(crate::ClientId(1)).unwrap();
    assert_eq!(active_pane.x(), 12, "x coordinates set properly");
    assert_eq!(active_pane.y(), 5, "y coordinates set properly");
    assert_eq!(active_pane.rows(), 2, "rows set properly");
    assert_eq!(active_pane.cols(), 1, "columns set properly");
}

#[test]
fn open_new_floating_pane_with_custom_coordinates_exceeding_viewport() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(crate::ClientId(1)).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(crate::TerminalId(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(122)),
                y: Some(PercentOrFixed::Fixed(21)),
                width: Some(PercentOrFixed::Fixed(10)),
                height: Some(PercentOrFixed::Fixed(10)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(crate::ClientId(1)),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(crate::ClientId(1)).unwrap();
    assert_eq!(active_pane.x(), 111, "x coordinates set properly");
    assert_eq!(active_pane.y(), 10, "y coordinates set properly");
    assert_eq!(active_pane.rows(), 10, "rows set properly");
    assert_eq!(active_pane.cols(), 10, "columns set properly");
}

#[test]
fn floating_pane_auto_centers_horizontally_with_only_width() {
    let size = Size {
        cols: 120,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(crate::ClientId(1)).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(crate::TerminalId(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: None,
                y: Some(PercentOrFixed::Fixed(5)),
                width: Some(PercentOrFixed::Fixed(60)),
                height: Some(PercentOrFixed::Fixed(10)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(crate::ClientId(1)),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(crate::ClientId(1)).unwrap();
    assert_eq!(active_pane.x(), 30, "x centered: (120-60)/2 = 30");
    assert_eq!(active_pane.y(), 5, "y explicitly set");
    assert_eq!(active_pane.cols(), 60, "width set");
    assert_eq!(active_pane.rows(), 10, "height set");
}

#[test]
fn floating_pane_auto_centers_vertically_with_only_height() {
    let size = Size {
        cols: 120,
        rows: 40,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(crate::ClientId(1)).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(crate::TerminalId(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(10)),
                y: None,
                width: Some(PercentOrFixed::Fixed(50)),
                height: Some(PercentOrFixed::Fixed(20)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(crate::ClientId(1)),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(crate::ClientId(1)).unwrap();
    assert_eq!(active_pane.x(), 10, "x explicitly set");
    assert_eq!(active_pane.y(), 10, "y centered: (40-20)/2 = 10");
    assert_eq!(active_pane.cols(), 50, "width set");
    assert_eq!(active_pane.rows(), 20, "height set");
}

#[test]
fn floating_pane_auto_centers_both_axes_with_only_size() {
    let size = Size {
        cols: 120,
        rows: 40,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(crate::ClientId(1)).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(crate::TerminalId(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: None,
                y: None,
                width: Some(PercentOrFixed::Fixed(80)),
                height: Some(PercentOrFixed::Fixed(30)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(crate::ClientId(1)),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(crate::ClientId(1)).unwrap();
    assert_eq!(active_pane.x(), 20, "x centered: (120-80)/2 = 20");
    assert_eq!(active_pane.y(), 5, "y centered: (40-30)/2 = 5");
    assert_eq!(active_pane.cols(), 80, "width set");
    assert_eq!(active_pane.rows(), 30, "height set");
}

#[test]
fn floating_pane_respects_explicit_coordinates_with_size() {
    let size = Size {
        cols: 120,
        rows: 40,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(crate::ClientId(1)).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(crate::TerminalId(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(15)),
                y: Some(PercentOrFixed::Fixed(8)),
                width: Some(PercentOrFixed::Fixed(80)),
                height: Some(PercentOrFixed::Fixed(30)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(crate::ClientId(1)),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(crate::ClientId(1)).unwrap();
    assert_eq!(active_pane.x(), 15, "x explicitly set, not centered");
    assert_eq!(active_pane.y(), 8, "y explicitly set, not centered");
    assert_eq!(active_pane.cols(), 80, "width set");
    assert_eq!(active_pane.rows(), 30, "height set");
}

#[test]
fn floating_pane_centers_with_percentage_width() {
    let size = Size {
        cols: 120,
        rows: 40,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(crate::ClientId(1)).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(crate::TerminalId(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: None,
                y: Some(PercentOrFixed::Fixed(5)),
                width: Some(PercentOrFixed::Percent(50)),
                height: Some(PercentOrFixed::Fixed(20)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(crate::ClientId(1)),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(crate::ClientId(1)).unwrap();
    let expected_width = ((50.0_f64 / 100.0) * 120.0).floor() as usize;
    let expected_x = (120 - expected_width) / 2;
    assert_eq!(active_pane.cols(), expected_width, "width is 50% of 120");
    assert_eq!(
        active_pane.x(),
        expected_x,
        "x centered based on calculated width"
    );
    assert_eq!(active_pane.y(), 5, "y explicitly set");
}

#[test]
fn floating_pane_centers_large_pane_safely() {
    let size = Size {
        cols: 100,
        rows: 30,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(crate::ClientId(1)).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(crate::TerminalId(2)),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: None,
                y: None,
                width: Some(PercentOrFixed::Fixed(150)),
                height: Some(PercentOrFixed::Fixed(50)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(crate::ClientId(1)),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(crate::ClientId(1)).unwrap();
    assert_eq!(
        active_pane.x(),
        0,
        "x is 0 when pane larger than viewport (saturating_sub)"
    );
    assert_eq!(
        active_pane.y(),
        0,
        "y is 0 when pane larger than viewport (saturating_sub)"
    );
    assert!(active_pane.cols() <= 100, "width clamped to viewport");
    assert!(active_pane.rows() <= 30, "height clamped to viewport");
}

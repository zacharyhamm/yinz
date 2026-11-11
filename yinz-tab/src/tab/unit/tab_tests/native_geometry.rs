use super::*;
use yinz_types::structured_render::{PaneRect, PANE_FLOATING, PANE_FOCUSED, PANE_SELECTABLE};

fn bounds(rect: &PaneRect) -> (usize, usize, usize, usize) {
    (
        rect.x as usize,
        rect.y as usize,
        rect.cols as usize,
        rect.rows as usize,
    )
}

fn pane_bounds(tab: &Tab, id: PaneId) -> (usize, usize, usize, usize) {
    let pane = tab.get_pane_with_id(id).unwrap();
    (pane.x(), pane.y(), pane.cols(), pane.rows())
}

#[test]
fn native_geometry_excludes_panes_hidden_by_tiled_fullscreen() {
    let client = ClientId::new(1);
    let mut tab = create_tab_with_four_panes();
    let first = PaneId::Terminal(TerminalId::new(1));
    tab.tiled_panes.focus_pane(first, client);
    tab.toggle_active_pane_fullscreen(client);

    let geometry = tab.native_geometry(client);
    assert_eq!(geometry.panes.len(), 1);
    assert_eq!(bounds(&geometry.panes[0]), pane_bounds(&tab, first));
    assert_eq!(geometry.panes[0].flags & PANE_FLOATING, 0);
    assert_ne!(geometry.panes[0].flags & PANE_FOCUSED, 0);
    assert_ne!(geometry.panes[0].flags & PANE_SELECTABLE, 0);
}

#[test]
fn native_geometry_tracks_raised_floating_panes_in_paint_order() {
    let client = ClientId::new(1);
    let mut tab = create_tab_with_two_floating_panes();
    let tiled = PaneId::Terminal(TerminalId::new(1));
    let older = PaneId::Terminal(TerminalId::new(2));
    let newer = PaneId::Terminal(TerminalId::new(3));
    tab.floating_panes.toggle_show_panes(true);
    // Ensure the overlapping rectangles identify their panes independently of flags.
    let mut geom = floating_pane_geom(&tab, older);
    geom.x = 1;
    tab.floating_panes.get_mut(&older).unwrap().set_geom(geom);
    let mut geom = floating_pane_geom(&tab, newer);
    geom.x = 2;
    tab.floating_panes.get_mut(&newer).unwrap().set_geom(geom);

    for (focused, order) in [
        (older, [tiled, newer, older]),
        (newer, [tiled, older, newer]),
    ] {
        tab.floating_panes.focus_pane(focused, client);
        let geometry = tab.native_geometry(client);
        assert_eq!(
            geometry.panes.iter().map(bounds).collect::<Vec<_>>(),
            order.map(|id| pane_bounds(&tab, id))
        );
        assert_eq!(geometry.panes[0].flags & PANE_FLOATING, 0);
        for rect in &geometry.panes[1..] {
            assert_ne!(rect.flags & PANE_FLOATING, 0);
        }
        assert_ne!(geometry.panes[2].flags & PANE_FOCUSED, 0);
        assert_eq!(geometry.panes[1].flags & PANE_FOCUSED, 0);
    }
}

#[test]
fn native_geometry_retains_only_pinned_floats_when_hidden() {
    let client = ClientId::new(1);
    let mut tab = create_tab_with_two_floating_panes();
    let tiled = PaneId::Terminal(TerminalId::new(1));
    let pinned = PaneId::Terminal(TerminalId::new(2));
    tab.floating_panes.toggle_show_panes(false);
    let geometry = tab.native_geometry(client);
    assert_eq!(geometry.panes.len(), 1);
    assert_eq!(bounds(&geometry.panes[0]), pane_bounds(&tab, tiled));

    tab.floating_panes
        .get_mut(&pinned)
        .unwrap()
        .set_pinned(true);
    let geometry = tab.native_geometry(client);
    assert_eq!(geometry.panes.len(), 2);
    assert_eq!(bounds(&geometry.panes[0]), pane_bounds(&tab, tiled));
    assert_eq!(bounds(&geometry.panes[1]), pane_bounds(&tab, pinned));
    assert_ne!(geometry.panes[1].flags & PANE_FLOATING, 0);
}

#[test]
fn native_geometry_excludes_floats_covered_by_floating_fullscreen() {
    let client = ClientId::new(1);
    let mut tab = create_tab_with_two_floating_panes();
    let fullscreen = PaneId::Terminal(TerminalId::new(2));
    tab.floating_panes.toggle_show_panes(true);
    tab.floating_panes.focus_pane(fullscreen, client);
    tab.toggle_active_pane_fullscreen(client);

    let geometry = tab.native_geometry(client);
    let floats: Vec<_> = geometry
        .panes
        .iter()
        .filter(|rect| rect.flags & PANE_FLOATING != 0)
        .collect();
    assert_eq!(floats.len(), 1);
    assert_eq!(bounds(floats[0]), pane_bounds(&tab, fullscreen));
    assert_ne!(floats[0].flags & PANE_FOCUSED, 0);
    assert_eq!(
        bounds(geometry.panes.last().unwrap()),
        pane_bounds(&tab, fullscreen)
    );
}

fn add_geometry_overlay(tab: &mut Tab, id: PaneId, client: ClientId) {
    let coordinates = yinz_types::geometry::FloatingPaneCoordinates::default()
        .with_x_fixed(10)
        .with_y_fixed(2)
        .with_width_fixed(100)
        .with_height_fixed(12);
    tab.new_pane(
        id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(coordinates))),
        Some(client),
        None,
    )
    .unwrap();
}

#[test]
fn native_geometry_places_an_overlay_spanning_tiled_splits_after_floating_panes() {
    let client = ClientId::new(1);
    let mut tab = create_tab_with_four_panes();
    let floating = PaneId::Terminal(TerminalId::new(5));
    tab.new_floating_pane(floating, None, None, false, true, None, None)
        .unwrap();
    tab.floating_panes.toggle_show_panes(true);
    let baseline = tab.native_geometry(client).panes;
    let overlay = PaneId::Terminal(TerminalId::new(6));
    add_geometry_overlay(&mut tab, overlay, client);

    let geometry = tab.native_geometry(client);
    assert_eq!(geometry.panes.len(), baseline.len() + 1);
    let rect = geometry.panes.last().unwrap();
    assert_eq!(bounds(rect), pane_bounds(&tab, overlay));
    assert_eq!(bounds(rect), (10, 2, 100, 12));
    assert_ne!(rect.flags & PANE_FLOATING, 0);
    assert_ne!(rect.flags & PANE_FOCUSED, 0);
    assert_eq!(
        bounds(&geometry.panes[geometry.panes.len() - 2]),
        pane_bounds(&tab, floating)
    );
    // The overlay crosses a vertical tiled split rather than fitting inside one tile.
    assert!(tab.tiled_panes.rendered_pane_ids().iter().any(|id| {
        let (x, _, cols, _) = pane_bounds(&tab, *id);
        x + cols > 10 && x + cols < 110
    }));

    assert!(tab.overlay_panes.hide_pane(overlay));
    let hidden = tab.native_geometry(client);
    assert_eq!(
        hidden.panes.iter().map(bounds).collect::<Vec<_>>(),
        baseline.iter().map(bounds).collect::<Vec<_>>()
    );
}

#[test]
fn native_geometry_keeps_overlays_above_tiled_fullscreen() {
    for covers_ui in [false, true] {
        let client = ClientId::new(1);
        let mut tab = create_tab_with_four_panes();
        let tiled = PaneId::Terminal(TerminalId::new(1));
        tab.tiled_panes.focus_pane(tiled, client);
        if covers_ui {
            tab.toggle_active_pane_no_ui_fullscreen(client);
        } else {
            tab.toggle_active_pane_fullscreen(client);
        }
        let overlay = PaneId::Terminal(TerminalId::new(5));
        add_geometry_overlay(&mut tab, overlay, client);

        let geometry = tab.native_geometry(client);
        assert_eq!(geometry.panes.len(), 2);
        assert_eq!(bounds(&geometry.panes[0]), pane_bounds(&tab, tiled));
        assert_eq!(bounds(&geometry.panes[1]), pane_bounds(&tab, overlay));
        assert_ne!(geometry.panes[1].flags & PANE_FLOATING, 0);
    }
}

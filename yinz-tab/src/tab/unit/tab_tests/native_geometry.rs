use super::*;
use yinz_types::structured_render::{
    PaneRect, PANE_FLOATING, PANE_FOCUSED, PANE_SELECTABLE, PANE_UNDER_FLOATING,
};

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

/// A three-pane stack beside a plain pane: one stack member is expanded, two are collapsed.
fn tab_with_a_stack_beside_a_pane() -> Tab {
    let stack = TiledPaneLayout {
        children_are_stacked: true,
        children: vec![
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
        ],
        ..Default::default()
    };
    let layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), stack],
        ..Default::default()
    };
    create_new_tab_with_layout(
        Size {
            cols: 121,
            rows: 20,
        },
        layout,
    )
}

fn collapsed_in_stack(tab: &Tab) -> Vec<PaneId> {
    tab.tiled_panes
        .panes
        .iter()
        .filter(|(_, pane)| pane.current_geom().is_collapsed_in_stack())
        .map(|(id, _)| *id)
        .collect()
}

#[test]
fn native_geometry_includes_collapsed_stack_title_rows() {
    let client = ClientId::new(1);
    let tab = tab_with_a_stack_beside_a_pane();
    let collapsed = collapsed_in_stack(&tab);
    assert_eq!(collapsed.len(), 2, "the stack should collapse two members");

    let geometry = tab.native_geometry(client);
    assert_eq!(geometry.panes.len(), 4);
    let ids: Vec<PaneId> = tab.tiled_panes.panes.keys().copied().collect();
    assert_eq!(
        geometry.panes.iter().map(bounds).collect::<Vec<_>>(),
        ids.iter()
            .map(|id| pane_bounds(&tab, *id))
            .collect::<Vec<_>>(),
        "every tiled pane, collapsed or not, has a rect in id order"
    );
    let focused = tab.get_active_pane_id(client).unwrap();
    for (rect, id) in geometry.panes.iter().zip(&ids) {
        assert_eq!(rect.flags & PANE_FLOATING, 0);
        assert_eq!(rect.flags & PANE_UNDER_FLOATING, 0);
        assert_ne!(rect.flags & PANE_SELECTABLE, 0);
        assert_eq!(rect.flags & PANE_FOCUSED != 0, *id == focused);
        if collapsed.contains(id) {
            assert_eq!(rect.rows, 1, "a collapsed member shows only its title row");
            let pane = tab.get_pane_with_id(*id).unwrap();
            assert_eq!(
                rect.rows as usize - rect.top as usize - rect.bottom as usize,
                pane.get_content_rows(),
                "the rect's content rows match the pane's"
            );
        }
    }
}

#[test]
fn native_geometry_shows_one_rect_for_a_stacked_fullscreen_pane() {
    let client = ClientId::new(1);
    let mut tab = tab_with_a_stack_beside_a_pane();
    let expanded = tab
        .tiled_panes
        .panes
        .iter()
        .find(|(_, pane)| pane.current_geom().is_expanded_in_stack())
        .map(|(id, _)| *id)
        .unwrap();
    tab.tiled_panes.focus_pane(expanded, client);
    let before = tab.native_geometry(client).panes;
    tab.toggle_active_pane_fullscreen(client);

    let geometry = tab.native_geometry(client);
    assert_eq!(geometry.panes.len(), 1);
    assert_eq!(bounds(&geometry.panes[0]), pane_bounds(&tab, expanded));
    assert_ne!(geometry.panes[0].flags & PANE_FOCUSED, 0);

    tab.toggle_active_pane_fullscreen(client);
    let after = tab.native_geometry(client).panes;
    assert_eq!(
        after.iter().map(bounds).collect::<Vec<_>>(),
        before.iter().map(bounds).collect::<Vec<_>>(),
        "leaving fullscreen restores every rect, collapsed titles included"
    );
}

#[test]
fn native_geometry_marks_tiled_panes_under_a_shown_floating_layer() {
    let client = ClientId::new(1);
    let mut tab = create_tab_with_two_floating_panes();
    let pinned = PaneId::Terminal(TerminalId::new(2));
    let under = |tab: &Tab| -> Vec<bool> {
        tab.native_geometry(client)
            .panes
            .iter()
            .map(|rect| rect.flags & PANE_UNDER_FLOATING != 0)
            .collect()
    };

    tab.floating_panes.toggle_show_panes(true);
    assert_eq!(
        under(&tab),
        [true, false, false],
        "only tiled panes are under"
    );

    tab.floating_panes.toggle_show_panes(false);
    assert_eq!(under(&tab), [false]);

    // Pinned floats stay up while the layer is hidden, and tiled panes stay active.
    tab.floating_panes
        .get_mut(&pinned)
        .unwrap()
        .set_pinned(true);
    assert_eq!(under(&tab), [false, false]);

    tab.floating_panes.toggle_show_panes(true);
    assert_eq!(under(&tab), [true, false, false]);
}

#[test]
fn native_geometry_marks_no_tiled_pane_under_an_empty_floating_layer() {
    let client = ClientId::new(1);
    let mut tab = create_tab_with_four_panes();
    tab.floating_panes.toggle_show_panes(true);
    assert!(tab.are_floating_panes_visible());
    assert!(tab
        .native_geometry(client)
        .panes
        .iter()
        .all(|rect| rect.flags & PANE_UNDER_FLOATING == 0));
}

#[test]
fn native_geometry_overlay_panes_stay_floating_without_covering_tiled_panes() {
    let client = ClientId::new(1);
    let mut tab = create_tab_with_four_panes();
    let overlay = PaneId::Terminal(TerminalId::new(5));
    add_geometry_overlay(&mut tab, overlay, client);

    let geometry = tab.native_geometry(client);
    let (last, tiled) = geometry.panes.split_last().unwrap();
    assert_eq!(bounds(last), pane_bounds(&tab, overlay));
    assert_ne!(last.flags & PANE_FLOATING, 0);
    assert_eq!(last.flags & PANE_UNDER_FLOATING, 0);
    assert!(tiled
        .iter()
        .all(|rect| rect.flags & (PANE_UNDER_FLOATING | PANE_FLOATING) == 0));
}

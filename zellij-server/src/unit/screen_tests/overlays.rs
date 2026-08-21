use super::*;
use std::collections::BTreeMap;
use zellij_utils::data::{OverlayKind, OverlayPlacement, Resize, ResizeStrategy};

#[test]
fn modal_plugin_configuration_requests_overlay() {
    let mut modal = RunPlugin::from_url("zellij:quit-confirmation").expect("test plugin");
    modal.configuration.insert("modal", "true");
    let modal = RunPluginOrAlias::RunPlugin(modal);
    assert!(modal.requests_overlay());
    assert!(modal.requests_modal());

    let plain = RunPlugin::from_url("zellij:quit-confirmation").expect("test plugin");
    let plain = RunPluginOrAlias::RunPlugin(plain);
    assert!(!plain.requests_overlay());
    assert!(!plain.requests_modal());
}

#[test]
fn resolved_modal_plugin_alias_requests_overlay() {
    let mut modal = RunPlugin::from_url("zellij:quit-confirmation").expect("test plugin");
    modal.configuration.insert("modal", "true");
    let alias = PluginAlias {
        name: "quit".to_owned(),
        run_plugin: Some(modal),
        ..Default::default()
    };

    let alias = RunPluginOrAlias::Alias(alias);
    assert!(alias.requests_overlay());
    assert!(alias.requests_modal());
}

#[test]
fn alt_f_opens_a_floating_pane_when_only_an_overlay_exists() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    let (pty_sender, pty_receiver) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));
    new_tab(&mut screen, 1, 0);

    let overlay = PaneId::Terminal(2);
    let tab = screen.get_active_tab_mut(1).unwrap();
    tab.new_pane(
        overlay,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
            FloatingPaneCoordinates {
                ..Default::default()
            },
        ))),
        Some(1),
        None,
    )
    .unwrap();
    tab.toggle_floating_panes(Some(1), None, None).unwrap();

    assert!(matches!(
        pty_receiver.try_recv().map(|(instruction, _)| instruction),
        Ok(PtyInstruction::SpawnTerminal(
            _,
            _,
            NewPanePlacement::Floating(None),
            _,
            ClientTabIndexOrPaneId::ClientId(1),
            _,
            _,
            _
        ))
    ));
    assert!(tab.pane_is_overlay(overlay));
    assert_eq!(tab.get_active_pane_id(1), Some(overlay));
}

#[test]
fn alt_f_hides_regular_floating_panes_while_an_overlay_exists() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    let (pty_sender, pty_receiver) = channels::unbounded();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_sender));
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);
    open_floating_pane(&mut screen, 0, 3, "OVERLAY", true);

    let tab = screen.get_active_tab_mut(1).unwrap();
    assert!(tab.are_floating_panes_visible());
    tab.toggle_floating_panes(Some(1), None, None).unwrap();

    assert!(!tab.are_floating_panes_visible());
    assert!(tab.pane_is_overlay(PaneId::Terminal(3)));
    assert!(pty_receiver.try_recv().is_err());
}

#[test]
fn popup_overlay_uses_the_whole_surface_and_temporarily_takes_global_focus() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size {
        cols: 120,
        rows: 20,
    };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 40);
    screen.render_to_clients().expect("TEST");
    screen
        .focus_global_pane(PaneId::Terminal(99), 1)
        .expect("TEST");

    let popup = PaneId::Terminal(2);
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .new_pane(
            popup,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
                FloatingPaneCoordinates {
                    x: Some(PercentOrFixed::Percent(20)),
                    width: Some(PercentOrFixed::Percent(60)),
                    height: Some(PercentOrFixed::Fixed(4)),
                    ..Default::default()
                },
            ))),
            Some(1),
            None,
        )
        .expect("TEST");
    screen.activate_overlay(popup);

    let geom = screen.tabs[&0]
        .get_pane_with_id(popup)
        .unwrap()
        .position_and_size();
    assert_eq!((geom.x, geom.cols.as_usize()), (24, 72));
    assert_eq!(screen.active_pane_id(1), Some(popup));

    let restore = screen.take_global_focus_after_overlay_close(popup);
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .close_pane(popup, false, None);
    screen.restore_global_focus(restore);
    assert_eq!(
        screen.global_panes.focused_pane_id(1),
        Some(PaneId::Terminal(99))
    );
}

/// A pane's z-index has to be its place in the occlusion stack, which is not the same as its place
/// among the panes a given render pass happens to draw. Taking the index from the pass rather than
/// from the stack clips a pane against its own geometry, dropping every one of its chunks - the
/// popup renders as nothing at all and whatever sits behind it shows through.
#[test]
fn an_overlay_popup_still_renders_when_another_floating_pane_is_open() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 20);
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    // the tabs learn where the global pane strips are when they first render, and an overlay's
    // coordinates are resolved against a surface that includes them
    screen.render_to_clients().expect("TEST");
    while server_receiver.try_recv().is_ok() {}

    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);
    open_floating_pane(&mut screen, 0, 3, "POPUP", true);

    let mut grid = grid_for_render_snapshot(size);
    let snapshot = render_frame_into(&mut screen, &server_receiver, 1, &mut grid);
    assert!(
        snapshot.contains("POPUP"),
        "the overlay popup must render above the other floating pane, got:\n{snapshot}"
    );
}

/// The global panes render after every tab has had its turn, against whichever floating panes
/// stack the last tab left on the output. With more than one tab that stack belongs to a tab
/// nobody in this render group is looking at, so the global pane paints over the overlay in front
/// of them - and the overlay, not being dirty, never paints itself back.
#[test]
fn a_global_pane_does_not_paint_over_an_overlay_in_another_client_s_tab() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    // a second client, viewing the second tab: that is what makes some other tab render last
    screen.add_client(2, false).expect("TEST");
    screen.switch_active_tab(1, None, true, 2).expect("TEST");
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 20);
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    // as above: the strips have to be in place before the overlay is positioned
    screen.render_to_clients().expect("TEST");
    while server_receiver.try_recv().is_ok() {}

    open_floating_pane(&mut screen, 0, 3, "POPUP", true);

    // the host terminal keeps what the first frame drew, so both frames go into the same grid -
    // the popup only paints when it changes, the global pane repaints every frame
    let mut grid = grid_for_render_snapshot(size);
    let first = render_frame_into(&mut screen, &server_receiver, 1, &mut grid);
    assert!(
        first.contains("POPUP"),
        "the overlay popup must render on the first frame, got:\n{first}"
    );
    let second = render_frame_into(&mut screen, &server_receiver, 1, &mut grid);
    assert!(
        second.contains("POPUP"),
        "the global pane must not paint over the overlay on a later frame, got:\n{second}"
    );
}

/// A popup bound with `move_to_focused_tab` follows the client from tab to tab. The tab it leaves
/// forgets the coordinates it was placed with, so unless they travel with it the popup lands in
/// the new tab as an ordinary floating pane - sized against the tab's viewport, and painted under
/// the global panes rather than over them.
#[test]
fn an_overlay_popup_stays_an_overlay_when_it_follows_the_client_to_another_tab() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 20);
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    screen.render_to_clients().expect("TEST");
    while server_receiver.try_recv().is_ok() {}

    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/fake/popup", &None, None, None).unwrap();
    let plugin_sender = screen.bus.senders.to_plugin.clone().unwrap_or_else(|| {
        let (plugin_tx, _plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
        SenderWithContext::new(plugin_tx)
    });
    let popup = PaneFactory::plugin(
        7,
        Default::default(),
        plugin_sender,
        "popup".to_owned(),
        "popup".to_owned(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.connected_clients.borrow().keys().copied().collect(),
        screen.style,
        Some(Run::Plugin(run_plugin.clone())),
        screen.debug,
        screen.arrow_fonts,
        screen.styled_underlines,
    );
    let popup_id = PaneId::Plugin(7);
    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(2)),
        y: Some(PercentOrFixed::Fixed(2)),
        width: Some(PercentOrFixed::Percent(60)),
        height: Some(PercentOrFixed::Fixed(4)),
        ..Default::default()
    };
    let first_tab = screen.tabs.get_mut(&0).expect("TEST");
    first_tab.show_floating_panes();
    first_tab
        .add_overlay_pane(
            popup,
            popup_id,
            OverlayKind::Dismissible,
            coordinates,
            true,
            Some(1),
        )
        .expect("TEST");
    assert!(first_tab.pane_is_overlay(popup_id));
    let placed_at = first_tab
        .get_pane_with_id(popup_id)
        .expect("TEST")
        .position_and_size();

    // the client moves to the other tab and asks for the popup again
    screen.switch_active_tab(1, None, true, 1).expect("TEST");
    assert!(screen
        .focus_plugin_pane(&run_plugin, false, true, false, 1, &mut None)
        .expect("TEST"));

    let second_tab = screen.tabs.get(&1).expect("TEST");
    assert!(
        second_tab.pane_is_overlay(popup_id),
        "the popup must still be an overlay in the tab it followed the client into"
    );
    let moved_to = second_tab
        .get_pane_with_id(popup_id)
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        (moved_to.x, moved_to.cols.as_usize()),
        (placed_at.x, placed_at.cols.as_usize()),
        "an overlay is sized against the whole surface in either tab"
    );
}

/// The tab is laid out over the area the global panes leave it, so its viewport starts where the
/// global pane ends. A floating pane dragged with the mouse used to be clamped to that viewport and
/// stopped dead at the global pane's edge, no matter how far the pointer kept going.
#[test]
fn a_floating_pane_can_be_dragged_over_a_global_pane() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 20);
    // the tabs learn where the global pane strips are when they first render
    screen.render_to_clients().expect("TEST");
    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);

    let before = floating_geom(&screen, 2);
    drag_floating_pane_left_over_the_global_pane(&mut screen, 2);

    let after = floating_geom(&screen, 2);
    assert_eq!(
        after.x,
        before.x - 25,
        "the pane follows the pointer past the tab's edge (it started at x={}, the tab starts at \
         x={})",
        before.x,
        screen.tabs[&0].get_display_area_origin().0,
    );
}

/// A floating pane over a global pane is outside the viewport of every tab, so nothing but its own
/// tab paints those cells - and it has to keep painting them above the global pane, which renders
/// after every tab has had its turn.
#[test]
fn a_floating_pane_dragged_over_a_global_pane_renders_above_it() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 20);
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    screen.render_to_clients().expect("TEST");
    while server_receiver.try_recv().is_ok() {}

    open_floating_pane(&mut screen, 0, 2, "DRAGGED", false);
    drag_floating_pane_left_over_the_global_pane(&mut screen, 2);

    let geom = floating_geom(&screen, 2);
    assert!(
        geom.x < screen.tabs[&0].get_display_area_origin().0,
        "the pane has to actually be over the global pane for this test to mean anything"
    );

    // the host terminal keeps what earlier frames drew, so they all go into the same grid - the
    // pane only paints when it changes, the global pane repaints every frame
    let mut grid = grid_for_render_snapshot(size);
    let first = render_frame_into(&mut screen, &server_receiver, 1, &mut grid);
    assert!(
        first.contains("DRAGGED"),
        "the dragged pane must render over the global pane, got:\n{first}"
    );
    // make the global pane repaint on its own, which is what would paint over the floating pane
    screen.global_panes.set_force_render();
    let second = render_frame_into(&mut screen, &server_receiver, 1, &mut grid);
    assert!(
        second.contains("DRAGGED"),
        "a global pane repaint must not paint over the pane in front of it, got:\n{second}"
    );
}

/// Every resize re-fits the floating panes into the tab's viewport, restoring each one to the
/// position it wants. A pane parked over a global pane wants a position outside that viewport, and
/// would be herded back inside on the next resize.
#[test]
fn a_floating_pane_dragged_over_a_global_pane_stays_there_across_a_resize() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 20);
    screen.render_to_clients().expect("TEST");
    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);
    drag_floating_pane_left_over_the_global_pane(&mut screen, 2);
    let dragged_to = floating_geom(&screen, 2);

    screen
        .resize_to_screen(Size { cols: 90, rows: 14 })
        .expect("TEST");

    let after = floating_geom(&screen, 2);
    assert_eq!(
        (after.x, after.y),
        (dragged_to.x, dragged_to.y),
        "the pane stays where it was dragged instead of being fitted back into the tab"
    );
}

/// The global panes get first refusal on every mouse event, by position. A floating pane drawn over
/// one is what the user sees there, so it is what the click has to reach - otherwise the pane
/// cannot even be focused, let alone dragged away again.
#[test]
fn a_click_on_a_floating_pane_over_a_global_pane_focuses_the_floating_pane() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 20);
    screen.render_to_clients().expect("TEST");
    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);
    drag_floating_pane_left_over_the_global_pane(&mut screen, 2);

    screen
        .focus_global_pane(PaneId::Terminal(99), 1)
        .expect("TEST");

    // a cell of the dragged pane that lies over the global pane
    let geom = floating_geom(&screen, 2);
    assert!(geom.x < screen.tabs[&0].get_display_area_origin().0);
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(geom.y as i32 + 1, geom.x as u16 + 1)),
        1,
    );

    assert_eq!(
        screen.global_panes.focused_pane_id(1),
        None,
        "the click must not land on the global pane behind the floating one"
    );
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(2)));
}

/// A global pane's border is draggable to resize it. A floating pane sitting over that border hides
/// it, so pressing there belongs to the floating pane rather than starting a resize behind it.
#[test]
fn a_press_on_a_floating_pane_covering_the_global_pane_border_does_not_resize_it() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 20);
    screen.render_to_clients().expect("TEST");
    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);
    drag_floating_pane_left_over_the_global_pane(&mut screen, 2);

    let global_pane_before = screen
        .global_panes
        .pane(PaneId::Terminal(99))
        .expect("TEST")
        .position_and_size();
    let geom = floating_geom(&screen, 2);
    let border_column = global_pane_before.x + global_pane_before.cols.as_usize();
    assert!(
        geom.x < border_column && geom.x + geom.cols.as_usize() > border_column,
        "the floating pane has to cover the border for this test to mean anything"
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(geom.y as i32 + 1, border_column as u16)),
        1,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(Position::new(geom.y as i32 + 1, 40)),
        1,
    );

    let global_pane_after = screen
        .global_panes
        .pane(PaneId::Terminal(99))
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        global_pane_after.cols.as_usize(),
        global_pane_before.cols.as_usize(),
        "the global pane must not have been resized by a press on the pane in front of it"
    );
}

/// A selection started in a global pane holds that client's mouse until it is released. A floating
/// pane the pointer passes under on the way must not steal it mid-drag.
#[test]
fn a_global_pane_selection_keeps_the_mouse_when_it_wanders_under_a_floating_pane() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 20);
    screen.render_to_clients().expect("TEST");
    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);
    drag_floating_pane_left_over_the_global_pane(&mut screen, 2);

    let geom = floating_geom(&screen, 2);
    // start the selection in the global pane, well clear of the floating pane
    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(1, 1)), 1);
    assert!(
        screen.global_panes.is_selecting_with_mouse(1),
        "the press must have started a selection in the global pane"
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(Position::new(geom.y as i32 + 1, geom.x as u16 + 1)),
        1,
    );
    assert!(
        screen.global_panes.is_selecting_with_mouse(1),
        "the global pane keeps the mouse for the duration of its selection"
    );
    assert!(
        !screen.tabs[&0].floating_pane_is_being_moved_with_mouse(),
        "and the floating pane must not have been picked up by it"
    );
}

/// The keyboard move and the mouse drag have to agree on where a floating pane may go, or a pane
/// nudged with the keyboard walks back out of the area it can be dragged into.
#[test]
fn a_floating_pane_can_be_moved_over_a_global_pane_with_the_keyboard() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 20);
    screen.render_to_clients().expect("TEST");
    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);

    let tab_starts_at = screen.tabs[&0].get_display_area_origin().0;
    let tab = screen.tabs.get_mut(&0).expect("TEST");
    for _ in 0..30 {
        tab.move_active_pane_left(1);
    }

    let geom = floating_geom(&screen, 2);
    assert!(
        geom.x < tab_starts_at,
        "the pane must cross the tab's edge at x={tab_starts_at}, it stopped at x={}",
        geom.x
    );
}

/// The bypass has to be narrow: a global pane keeps the mouse everywhere no floating pane is drawn,
/// whether or not the tab has floating panes open somewhere else.
#[test]
fn a_global_pane_still_takes_the_mouse_where_no_floating_pane_covers_it() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 20);
    screen.render_to_clients().expect("TEST");
    // a floating pane, but one that stays inside its tab
    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(1, 1)), 1);

    assert_eq!(
        screen.global_panes.focused_pane_id(1),
        Some(PaneId::Terminal(99)),
        "a press on the global pane still focuses it"
    );
}

fn floating_geom(screen: &Screen, pane_id: u32) -> PaneGeom {
    screen.tabs[&0]
        .get_pane_with_id(PaneId::Terminal(pane_id))
        .expect("TEST")
        .position_and_size()
}

/// Press the pane's top frame and drag it 25 columns left, which takes it over the global pane on
/// the left edge, then release.
fn drag_floating_pane_left_over_the_global_pane(screen: &mut Screen, pane_id: u32) {
    let geom = floating_geom(screen, pane_id);
    let row = geom.y as i32;
    let from = geom.x as u16 + 2;
    let to = from - 25;
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(row, from)),
        1,
    );
    screen.handle_mouse_event(MouseEvent::new_left_motion_event(Position::new(row, to)), 1);
    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(Position::new(row, to)),
        1,
    );
}

/// Where the overlays in these tests sit on the session surface. Just geometry: what makes a pane
/// an overlay is its placement, which is decided when it is born.
fn overlay_coordinates() -> FloatingPaneCoordinates {
    FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(2)),
        y: Some(PercentOrFixed::Fixed(2)),
        width: Some(PercentOrFixed::Percent(60)),
        height: Some(PercentOrFixed::Fixed(4)),
        ..Default::default()
    }
}

/// Opening an ordinary floating pane shows the floating layer, and the whole of zellij reads that
/// flag as "the user has floating panes up": the status bar lights its `<f>` ribbon, the next new
/// pane joins the floating layer, splits and focus moves get swallowed. An overlay is not on that
/// layer, so opening one must leave it exactly as it was.
#[test]
fn opening_an_overlay_does_not_raise_the_floating_layer() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "POPUP", true);

    let tab = screen.get_active_tab_mut(1).unwrap();
    assert!(tab.pane_is_overlay(PaneId::Terminal(2)));
    assert!(
        !tab.are_floating_panes_visible(),
        "an overlay is not the floating layer"
    );
    assert_eq!(
        tab.get_active_pane_id(1),
        Some(PaneId::Terminal(2)),
        "the overlay has the keyboard"
    );
}

/// The layer only goes down because nothing but the overlay was on it. A real floating pane
/// behind the overlay is still the user's floating pane, and still visible.
#[test]
fn the_floating_layer_stays_up_when_a_real_floating_pane_is_behind_the_new_overlay() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);
    open_floating_pane(&mut screen, 0, 3, "POPUP", true);

    let tab = screen.get_active_tab_mut(1).unwrap();
    assert!(tab.pane_is_overlay(PaneId::Terminal(3)));
    assert!(
        tab.are_floating_panes_visible(),
        "the floating pane behind the overlay is still up"
    );
}

/// Overlay status is permanent for the pane's lifetime, and coordinates cannot express it either
/// way. A move must not turn an overlay back into an ordinary floating pane on a hidden layer.
#[test]
fn changing_an_overlays_coordinates_does_not_remove_its_overlay_status() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "POPUP", true);

    let tab = screen.get_active_tab_mut(1).unwrap();
    assert!(!tab.are_floating_panes_visible());

    tab.change_floating_pane_coordinates(
        &PaneId::Terminal(2),
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(4)),
            y: Some(PercentOrFixed::Fixed(4)),
            ..Default::default()
        },
    )
    .expect("TEST");

    assert!(tab.pane_is_overlay(PaneId::Terminal(2)));
    assert!(
        !tab.are_floating_panes_visible(),
        "moving an overlay must not reveal the floating layer"
    );
    assert_eq!(tab.get_active_pane_id(1), Some(PaneId::Terminal(2)));
}

/// A plugin moving a hidden floating pane must not pop the layer open under the user.
#[test]
fn moving_an_unfocused_hidden_floating_pane_does_not_show_the_floating_layer() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);

    let tab = screen.get_active_tab_mut(1).unwrap();
    tab.hide_floating_panes();
    assert!(!tab.are_floating_panes_visible());

    tab.change_floating_pane_coordinates(
        &PaneId::Terminal(2),
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(6)),
            y: Some(PercentOrFixed::Fixed(6)),
            ..Default::default()
        },
    )
    .expect("TEST");

    assert!(
        !tab.are_floating_panes_visible(),
        "moving a hidden floating pane must not reveal the floating layer"
    );
}

/// A modal is the client's active pane while the floating layer is down. Reading that as "this
/// client is working on the floating layer" sends the next new pane there - and reveals every
/// hidden floating pane on the way.
#[test]
fn a_new_pane_opened_over_a_modal_is_tiled() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "MODAL", true);

    let tab = screen.get_active_tab_mut(1).unwrap();
    assert_eq!(tab.get_active_pane_id(1), Some(PaneId::Terminal(2)));

    tab.new_pane(
        PaneId::Terminal(3),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(1),
        None,
    )
    .expect("TEST");

    assert!(
        !tab.are_floating_panes_visible(),
        "opening a pane over a modal must not reveal the floating layer"
    );
    assert!(
        !tab.pane_id_is_floating(&PaneId::Terminal(3)),
        "the new pane belongs on the tiled surface"
    );
    assert_eq!(
        tab.get_active_pane_id(1),
        Some(PaneId::Terminal(3)),
        "the modal gives the keyboard up to the pane the user asked for - a popup that closes \
         when it loses focus depends on this"
    );
}

/// A modal plugin overlay, focused, over the active tab - what `hocus-new-agent` is.
fn add_modal_plugin_overlay(screen: &mut Screen, plugin_id: u32) -> PaneId {
    let mut configuration = BTreeMap::new();
    configuration.insert("modal".to_owned(), "true".to_owned());
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/fake/modal", &Some(configuration), None, None)
            .unwrap();
    let plugin_sender = screen.bus.senders.to_plugin.clone().unwrap_or_else(|| {
        let (plugin_tx, _plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
        SenderWithContext::new(plugin_tx)
    });
    let modal = PaneFactory::plugin(
        plugin_id,
        Default::default(),
        plugin_sender,
        "modal".to_owned(),
        "modal".to_owned(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.connected_clients.borrow().keys().copied().collect(),
        screen.style,
        Some(Run::Plugin(run_plugin)),
        screen.debug,
        screen.arrow_fonts,
        screen.styled_underlines,
    );
    let modal_id = PaneId::Plugin(plugin_id);
    let tab = screen.get_active_tab_mut(1).unwrap();
    tab.add_floating_pane(modal, modal_id, Some(overlay_coordinates()), true, Some(1))
        .expect("TEST");
    modal_id
}

/// A modal overlay (e.g. hocus-new-agent) only closes on Esc/submit/`Event::Visible(false)` - it
/// has no way to learn it lost focus, unlike a dismissible overlay that watches `PaneUpdate`.
/// Stealing its focus the way `a_new_pane_opened_over_a_modal_is_tiled` does for a plain overlay
/// would leave it stuck on screen while input goes to the pane underneath, so it must keep focus.
#[test]
fn a_new_pane_opened_over_a_modal_plugin_overlay_does_not_steal_its_focus() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);

    let modal_id = add_modal_plugin_overlay(&mut screen, 7);
    let tab = screen.get_active_tab_mut(1).unwrap();
    assert!(tab.pane_is_overlay(modal_id));
    assert_eq!(tab.get_active_pane_id(1), Some(modal_id));

    tab.new_pane(
        PaneId::Terminal(3),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(1),
        None,
    )
    .expect("TEST");

    assert!(
        !tab.are_floating_panes_visible(),
        "opening a pane over a modal must not reveal the floating layer"
    );
    assert!(
        !tab.pane_id_is_floating(&PaneId::Terminal(3)),
        "the new pane belongs on the tiled surface"
    );
    assert_eq!(
        tab.get_active_pane_id(1),
        Some(modal_id),
        "a modal overlay has no way to learn it lost focus, so it must keep it"
    );
}

/// `Tab::pane_infos` has listed overlays since they got their own collection; the singular
/// lookup behind the `GetPaneInfo` plugin API has to answer for the same panes, or a plugin that
/// opens an overlay and asks about the ID it was handed back gets nothing.
#[test]
fn an_overlay_answers_the_singular_pane_info_lookup() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);

    let modal_id = add_modal_plugin_overlay(&mut screen, 7);
    let tab = screen.get_active_tab_mut(1).unwrap();
    assert!(tab.pane_is_overlay(modal_id));

    let pane_info = tab
        .get_pane_info(modal_id)
        .expect("an overlay is a pane the singular API knows about");
    assert_eq!(pane_info.id, 7);
    assert!(pane_info.is_plugin);
    assert!(
        pane_info.is_floating,
        "every consumer reads `is_floating` as `not tiled`, which an overlay is"
    );
    assert!(!pane_info.is_fullscreen, "an overlay cannot go fullscreen");
    assert!(!pane_info.is_suppressed);

    assert!(
        tab.pane_infos()
            .iter()
            .any(|info| info.is_plugin && info.id == 7),
        "and it agrees with the plural API it sits beside"
    );
}

/// The other half of the test above: keeping focus is not the end of the story, or the pane the
/// user asked for would stay unreachable. `hocus-new-agent` submits, focuses the agent pane it
/// just opened, and only then closes itself - and this is that hand-off. Focusing the tiled pane
/// while the overlay is still up moves nothing a client can see (`get_active_pane_id` prefers the
/// overlay), it moves the tiled focus *underneath*, which is what the close then lands on. Doing
/// it in the other order - close, then focus - would have nothing left to focus from.
#[test]
fn a_modal_overlay_hands_focus_to_the_pane_it_opened_when_it_closes() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);

    let modal_id = add_modal_plugin_overlay(&mut screen, 7);
    let tab = screen.get_active_tab_mut(1).unwrap();
    tab.new_pane(
        PaneId::Terminal(3),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(1),
        None,
    )
    .expect("TEST");
    assert_eq!(tab.get_active_pane_id(1), Some(modal_id));

    tab.focus_pane_with_id(PaneId::Terminal(3), false, false, 1)
        .expect("TEST");
    tab.close_pane(modal_id, false, None);

    assert_eq!(
        tab.get_active_pane_id(1),
        Some(PaneId::Terminal(3)),
        "the modal hands the keyboard to the pane it opened, not back to where the user came from"
    );
    assert!(
        !tab.are_floating_panes_visible(),
        "the floating layer stays down - only the modal was ever up"
    );
}

/// The overlay did not lose focus - the layer under it went away. Telling its terminal otherwise
/// is a lie, and one an application that tracks focus reporting will act on.
///
/// `FakeInputOutput::write_to_tty_stdin` is `unimplemented!()`, so the focus-out this used to
/// send would panic the test rather than merely being wrong. That is the assertion.
#[test]
fn an_overlay_terminal_is_not_told_it_lost_focus_when_the_layer_goes_down() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "POPUP", true);

    let tab = screen.get_active_tab_mut(1).unwrap();
    // DECSET 1004: the pane asks to be told when it gains and loses focus
    tab.handle_pty_bytes(2, b"\x1b[?1004h".to_vec())
        .expect("TEST");

    tab.change_overlay_coordinates(&PaneId::Terminal(2), overlay_coordinates())
        .expect("TEST");

    assert!(!tab.are_floating_panes_visible());
    assert_eq!(
        tab.get_active_pane_id(1),
        Some(PaneId::Terminal(2)),
        "the overlay is still the active pane"
    );
}

/// Open a popup the way every popup plugin in this fork arrives: born onto the session surface,
/// never on the tab's floating layer.
fn open_popup(screen: &mut Screen, pane_id: u32, contents: &str) {
    open_floating_pane(screen, 0, pane_id, contents, true);
}

/// What the frame actually shows, which is the only thing the user can see. `are_floating_panes_visible`
/// is the flag; this is the consequence.
fn rendered_frame(screen: &mut Screen, size: Size) -> String {
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    let mut grid = grid_for_render_snapshot(size);
    render_frame_into(screen, &server_receiver, 1, &mut grid)
}

/// The reported bug. A modal is not a floating pane, so putting one on screen must not put the
/// user's hidden floating panes on screen with it.
///
/// This used to turn on the whole floating layer: the modal was opened as an ordinary floating
/// pane (which reveals the layer), and the corrective that ran when it became an overlay only
/// fired when nothing else was on the layer. One hidden floating pane was enough to defeat it.
#[test]
fn a_modal_opened_onto_a_hidden_layer_does_not_reveal_hidden_floating_panes() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "HIDDENFLOATING", false);
    screen.get_active_tab_mut(1).unwrap().hide_floating_panes();
    assert!(!screen
        .get_active_tab_mut(1)
        .unwrap()
        .are_floating_panes_visible());

    open_popup(&mut screen, 3, "MODAL");

    let tab = screen.get_active_tab_mut(1).unwrap();
    assert!(tab.pane_is_overlay(PaneId::Terminal(3)));
    assert!(
        !tab.are_floating_panes_visible(),
        "showing a modal must not show the floating layer"
    );

    let frame = rendered_frame(&mut screen, size);
    assert!(
        frame.contains("MODAL"),
        "the modal itself is on screen, got:\n{frame}"
    );
    assert!(
        !frame.contains("HIDDENFLOATING"),
        "the pane the user hid is still hidden, got:\n{frame}"
    );
}

/// The same, with more than one pane behind it - "if any exist" was the shape of the bug, and one
/// pane is the case the old corrective happened to get right.
#[test]
fn a_modal_opened_onto_a_hidden_layer_with_several_floating_panes_behind_it() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "FIRSTHIDDEN", false);
    open_floating_pane(&mut screen, 0, 3, "SECONDHIDDEN", false);
    screen.get_active_tab_mut(1).unwrap().hide_floating_panes();

    open_popup(&mut screen, 4, "MODAL");

    assert!(!screen
        .get_active_tab_mut(1)
        .unwrap()
        .are_floating_panes_visible());
    let frame = rendered_frame(&mut screen, size);
    for hidden in ["FIRSTHIDDEN", "SECONDHIDDEN"] {
        assert!(
            !frame.contains(hidden),
            "{hidden} was hidden and must stay hidden, got:\n{frame}"
        );
    }
}

/// The other direction of "the layer is whatever the user left it as": a layer the user had up
/// stays up, and the floating pane on it stays on screen next to the modal.
#[test]
fn a_modal_opened_onto_a_visible_layer_leaves_it_up() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "VISIBLEFLOATING", false);

    open_popup(&mut screen, 3, "MODAL");

    assert!(
        screen
            .get_active_tab_mut(1)
            .unwrap()
            .are_floating_panes_visible(),
        "the floating pane the user had up is still the user's floating pane"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        frame.contains("VISIBLEFLOATING"),
        "the visible floating pane is still visible, got:\n{frame}"
    );
}

/// Closing the modal gives back the tab it found, not one with the floating layer switched on.
#[test]
fn closing_a_modal_leaves_the_floating_layer_as_it_found_it() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "HIDDENFLOATING", false);
    screen.get_active_tab_mut(1).unwrap().hide_floating_panes();

    open_popup(&mut screen, 3, "MODAL");
    let tab = screen.get_active_tab_mut(1).unwrap();
    tab.close_pane(PaneId::Terminal(3), false, None);

    assert!(
        !tab.are_floating_panes_visible(),
        "the layer was down before the modal and is down after it"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        !frame.contains("HIDDENFLOATING"),
        "the hidden floating pane is still hidden once the modal is gone, got:\n{frame}"
    );
}

/// `create_non_mirrored_screen` leaves `to_plugin` unset, and a tab cannot build a `PluginPane`
/// without it. The receiver comes back so the channel outlives the screen.
fn screen_with_plugin_sender(
    size: Size,
) -> (
    Screen,
    channels::Receiver<(PluginInstruction, ErrorContext)>,
) {
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let mut screen = create_non_mirrored_screen(size);
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    (screen, plugin_rx)
}

/// A plugin launched with `modal true` and nothing else - no coordinates, the way a keybinding
/// written before the flags existed reaches the tab.
fn modal_plugin_run() -> Run {
    let mut configuration = BTreeMap::new();
    configuration.insert("modal".to_owned(), "true".to_owned());
    Run::Plugin(
        RunPluginOrAlias::from_url("file:/path/to/fake/modal", &Some(configuration), None, None)
            .unwrap(),
    )
}

/// A modal plugin opened with no coordinates at all. `ScreenInstruction::AddPlugin` synthesizes
/// overlay coordinates from the plugin's configuration only when nobody passed any, so a caller
/// that passes its own (`pane_groups`) or a path that passes none reaches the tab with the
/// `modal` flag stripped. The pane itself still knows what it is, and must be placed as an
/// overlay on that basis - born one, never an ordinary floating pane even for a frame.
#[test]
fn a_modal_plugin_opened_without_overlay_coordinates_is_still_born_an_overlay() {
    let size = Size { cols: 80, rows: 12 };
    let (mut screen, _plugin_rx) = screen_with_plugin_sender(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "HIDDENFLOATING", false);
    screen.get_active_tab_mut(1).unwrap().hide_floating_panes();

    let tab = screen.get_active_tab_mut(1).unwrap();
    tab.new_pane(
        PaneId::Plugin(7),
        Some("modal".to_owned()),
        Some(modal_plugin_run()),
        false,
        true,
        NewPanePlacement::Floating(None),
        Some(1),
        None,
    )
    .expect("TEST");

    assert!(
        tab.pane_is_overlay(PaneId::Plugin(7)),
        "the plugin's own configuration says it is an overlay"
    );
    assert!(
        !tab.are_floating_panes_visible(),
        "and an overlay never raises the floating layer, not even for a frame"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        !frame.contains("HIDDENFLOATING"),
        "the pane the user hid is still hidden, got:\n{frame}"
    );
}

/// A modal that was parked in the suppressed panes and is asked for again comes back as a modal.
/// This path passed no coordinates and unconditionally raised the layer, so it both lost the
/// overlay placement and revealed everything the user had hidden.
#[test]
fn refocusing_a_suppressed_modal_does_not_reveal_the_floating_layer() {
    let size = Size { cols: 80, rows: 12 };
    let (mut screen, _plugin_rx) = screen_with_plugin_sender(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "HIDDENFLOATING", false);
    screen.get_active_tab_mut(1).unwrap().hide_floating_panes();

    let tab = screen.get_active_tab_mut(1).unwrap();
    let start_suppressed = true;
    tab.new_pane(
        PaneId::Plugin(7),
        Some("modal".to_owned()),
        Some(modal_plugin_run()),
        start_suppressed,
        true,
        NewPanePlacement::Floating(None),
        Some(1),
        None,
    )
    .expect("TEST");

    let should_float = true;
    let should_be_in_place = false;
    tab.focus_pane_with_id(PaneId::Plugin(7), should_float, should_be_in_place, 1)
        .expect("TEST");

    assert!(
        tab.pane_is_overlay(PaneId::Plugin(7)),
        "a modal taken back out of the suppressed panes is still a modal"
    );
    assert!(
        !tab.are_floating_panes_visible(),
        "and still does not raise the floating layer"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        !frame.contains("HIDDENFLOATING"),
        "the pane the user hid is still hidden, got:\n{frame}"
    );
}

// ---------------------------------------------------------------------------
// Characterization tests added ahead of the modals/overlays refactor.
//
// These pin behavior that the move to a dedicated overlay collection must
// preserve. They assert what the code does *today*, so that the refactor is
// provably behavior-preserving; where today's behavior is itself wrong, that is
// called out in the test's doc comment and fixed in a later commit rather than
// baked in here.
// ---------------------------------------------------------------------------

/// An overlay is positioned against the whole session surface and re-resolves whenever that
/// surface changes, so a resize leaves it proportional to the new surface.
///
/// It used to keep its old width. `FloatingPanes::reapply_overlay_coordinates` ran from inside
/// `floating_panes.resize()`, and the shared `display_area` cell it reads is only updated by
/// `tiled_panes.resize()` - which runs after. The overlay was re-resolved against the surface the
/// terminal had *before* the resize. `OverlayPanes::reapply_coordinates` is called from
/// `Tab::resize_whole_tab_locally` once the surface is known.
#[test]
fn an_overlay_is_re_resolved_against_the_surface_on_resize() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "OVERLAY", true);

    let before = screen.tabs[&0]
        .get_pane_with_id(PaneId::Terminal(2))
        .expect("TEST")
        .position_and_size();
    // `open_floating_pane`'s overlay coordinates ask for 60% of the surface
    assert_eq!(before.cols.as_usize(), 48);

    screen
        .resize_to_screen(Size {
            cols: 120,
            rows: 20,
        })
        .expect("TEST");

    let after = screen.tabs[&0]
        .get_pane_with_id(PaneId::Terminal(2))
        .expect("TEST")
        .position_and_size();
    assert_eq!(after.cols.as_usize(), 72, "60% of the new 120 columns");
    assert!(
        screen.tabs[&0].pane_is_overlay(PaneId::Terminal(2)),
        "and it is still an overlay afterwards"
    );
}

/// An overlay renders and owns the mouse while the floating layer is down - that is the whole
/// point of it. `visible_pane_id_at` answers from the occlusion stack rather than from the pane
/// map, so the overlay is hit-tested even though no floating pane is visible.
#[test]
fn an_overlay_owns_the_mouse_where_it_is_painted_while_the_layer_is_hidden() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_popup(&mut screen, 3, "OVERLAY");

    let tab = screen.get_active_tab_mut(1).unwrap();
    assert!(!tab.are_floating_panes_visible());

    // `overlay_coordinates()` puts the overlay at (2, 2), 60% wide and 4 rows tall
    assert_eq!(
        tab.visible_floating_pane_at(&Position::new(3, 4)),
        Some(PaneId::Terminal(3)),
        "the overlay is hit-tested inside its own geometry with the layer down"
    );
    assert_eq!(
        tab.visible_floating_pane_at(&Position::new(15, 70)),
        None,
        "and owns nothing outside it"
    );

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(3, 4)), 1);

    assert_eq!(
        screen.active_pane_id(1),
        Some(PaneId::Terminal(3)),
        "clicking it focuses it"
    );
    assert!(
        !screen.tabs[&0].are_floating_panes_visible(),
        "and does not raise the floating layer"
    );
}

/// While an overlay exists it masks whatever else a client is focused on: `get_active_pane_id`
/// prefers the overlay over the tiled pane for as long as the overlay is in the occlusion stack.
/// That is deliberate for a modal, which must never silently lose the keyboard - and it is what
/// happens for every overlay today.
///
/// The tiled focus underneath is still recorded, which is what lets
/// `a_modal_overlay_hands_focus_to_the_pane_it_opened_when_it_closes` work: it surfaces again the
/// moment the overlay goes away. This pins both halves, for both clients of a non-mirrored
/// session.
#[test]
fn an_overlay_masks_each_client_s_tiled_focus_until_it_closes() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    screen.add_client(2, false).expect("TEST");

    screen
        .get_active_tab_mut(1)
        .unwrap()
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .expect("TEST");
    open_popup(&mut screen, 3, "OVERLAY");

    let tab = screen.get_active_tab_mut(1).unwrap();
    tab.focus_pane_with_id(PaneId::Terminal(2), false, false, 2)
        .expect("TEST");

    assert_eq!(
        tab.get_active_pane_id(1),
        Some(PaneId::Terminal(3)),
        "client 1 is on the overlay"
    );
    assert_eq!(
        tab.get_active_pane_id(2),
        Some(PaneId::Terminal(3)),
        "and so is client 2 - the overlay masks the tiled pane it just focused"
    );
    assert!(
        !tab.are_floating_panes_visible(),
        "and neither client's focus raises the floating layer"
    );

    tab.close_pane(PaneId::Terminal(3), false, None);

    assert_eq!(
        tab.get_active_pane_id(2),
        Some(PaneId::Terminal(2)),
        "once the overlay is gone, the tiled focus recorded underneath surfaces again"
    );
}

// ---------------------------------------------------------------------------
// Render snapshots.
//
// Every other rendering claim in this file is a `.contains()` on
// `rendered_frame`, which says a pane is somewhere on screen but nothing about
// what is drawn over what. Occlusion is exactly what a z-order refactor breaks,
// so these pin whole frames.
// ---------------------------------------------------------------------------

/// The frame that exercises the overlay coordinate space, the occlusion stack and the global-pane
/// surface expansion at once: an overlay drawn over a *hidden* floating layer, with a global pane
/// docked to the left. The hidden pane must not appear, the overlay must sit on the surface
/// (crossing over the global pane rather than being bounded by the tab viewport), and the global
/// pane must not paint over it.
#[test]
fn frame_of_an_overlay_over_a_hidden_layer_beside_a_global_pane() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 20);
    // allocates the global pane strips onto the tab, which is what expands the overlay surface
    screen.render_to_clients().expect("TEST");
    open_floating_pane(&mut screen, 0, 2, "HIDDENFLOATING", false);
    screen.get_active_tab_mut(1).unwrap().hide_floating_panes();
    open_popup(&mut screen, 3, "OVERLAY");

    let frame = rendered_frame(&mut screen, size);
    assert!(
        !frame.contains("HIDDENFLOATING"),
        "the hidden floating pane must stay hidden, got:\n{frame}"
    );
    assert_snapshot!(frame);
}

/// Relative z-order between the three things that can be on top of the tiled panes while the
/// floating layer is up: a plain floating pane, a pinned floating pane, and an overlay.
///
/// The overlay paints above both, pinned included. It used to be the other way round: overlays
/// shared the floating layer's `z_indices`, and `make_sure_pinned_panes_are_on_top` had already
/// lifted the pinned pane to the end of that list. Now `Tab::occlusion_stack` appends the overlay
/// layers after every floating layer, so an overlay outranks anything on the layer by
/// construction - which is what a modal should do, being the one pane the user has to answer.
///
/// The frame also shows the one place overlay-ness reaches the pane frame: the two plain floating
/// panes carry a `PIN` toggle and the overlay does not.
#[test]
fn frame_of_an_overlay_with_a_pinned_and_a_plain_floating_pane() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "PLAINFLOATING", false);
    open_floating_pane(&mut screen, 0, 3, "PINNEDFLOATING", false);
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .change_floating_pane_coordinates(
            &PaneId::Terminal(3),
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(4)),
                y: Some(PercentOrFixed::Fixed(3)),
                width: Some(PercentOrFixed::Fixed(30)),
                height: Some(PercentOrFixed::Fixed(5)),
                pinned: Some(true),
                ..Default::default()
            },
        )
        .expect("TEST");
    open_popup(&mut screen, 4, "OVERLAY");

    assert_snapshot!(rendered_frame(&mut screen, size));
}

/// A modal over a tiled pane fullscreened across the whole UI. This combination had no test
/// before Phase 4 because the overlay was simply not drawn - which left the modal focused and
/// invisible.
///
/// It is also the sharpest test of the flat-list discipline: `Tab::occlusion_stack` and
/// `render_floating_panes_after_global_panes` index into one list, and a fullscreen tiled pane is
/// the case where the floating half of that list is not drawn while the overlay half is. A pane
/// handed an index that is not its own is clipped against its own geometry and erases itself, so
/// an empty box here is the failure this pins.
#[test]
fn frame_of_an_overlay_over_a_ui_covering_fullscreen_tiled_pane() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .handle_pty_bytes(1, b"TILED".to_vec())
        .expect("TEST");
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .toggle_active_pane_no_ui_fullscreen(1);
    assert!(screen.get_active_tab_mut(1).unwrap().fullscreen_covers_ui());

    open_popup(&mut screen, 2, "OVERLAY");

    let frame = rendered_frame(&mut screen, size);
    assert!(
        frame.contains("OVERLAY"),
        "the overlay must be drawn over the fullscreen pane, got:\n{frame}"
    );
    assert_snapshot!(frame);
}

// ---------------------------------------------------------------------------
// Structural invariants.
//
// The characterization tests above each pin one path. This drives a random
// sequence of operations instead and asserts `Tab::assert_invariants` after
// every step, so a path nobody thought to write a test for still has to hold
// the line. The two invariants that only become true once overlays have their
// own collection are listed in that helper and switched on with the refactor.
// ---------------------------------------------------------------------------

mod invariant_props {
    use super::*;
    use hegel::generators;

    /// One thing a user (or a plugin) can do to a tab. Deliberately includes the operations that
    /// the overlay special-casing threads through - opening onto a hidden layer, toggling the
    /// layer underneath an overlay - because those are where the collections have historically
    /// fallen out of step.
    #[derive(Debug, Clone)]
    enum Op {
        NewTiledPane,
        NewFloatingPane,
        NewOverlay,
        NewModalPlugin,
        ClosePane,
        ShowFloatingLayer,
        HideFloatingLayer,
        FocusFirstTiledPane,
        Resize(u16),
    }

    #[hegel::composite]
    fn op(tc: &hegel::TestCase) -> Op {
        match tc.draw(generators::integers::<u8>().max_value(8)) {
            0 => Op::NewTiledPane,
            1 => Op::NewFloatingPane,
            2 => Op::NewOverlay,
            3 => Op::NewModalPlugin,
            4 => Op::ClosePane,
            5 => Op::ShowFloatingLayer,
            6 => Op::HideFloatingLayer,
            7 => Op::FocusFirstTiledPane,
            _ => Op::Resize(tc.draw(generators::integers::<u16>().min_value(40).max_value(160))),
        }
    }

    #[hegel::test]
    fn a_tab_holds_its_invariants_under_any_sequence_of_pane_operations(tc: hegel::TestCase) {
        let op_count = tc.draw(generators::integers::<usize>().max_value(12));
        let ops: Vec<Op> = (0..op_count).map(|_| tc.draw(op())).collect();

        let size = Size { cols: 80, rows: 20 };
        let (mut screen, _plugin_rx) = screen_with_plugin_sender(size);
        new_tab(&mut screen, 1, 0);
        screen.get_active_tab_mut(1).unwrap().assert_invariants();

        // pane ids have to be unique across the whole run, including the plugin ones
        let mut next_id = 2u32;
        // the ids we opened, so `ClosePane` picks a real one
        let mut opened: Vec<PaneId> = vec![];

        for (index, op) in ops.iter().enumerate() {
            // Invariant 3, the transition half: opening or focusing an overlay must never change
            // whether the floating layer is up. That reading - "floating panes are visible" -
            // lights the status bar's <f>, sends the next new pane to the layer, and puts every
            // hidden floating pane back on screen under a modal.
            //
            // Nothing is exempt. The one operation that used to be - a floating pane promoting
            // itself to an overlay, which had to put back the reveal its own birth caused - does
            // not exist any more: a pane is born an overlay or is never one.
            let layer_was_visible = screen
                .get_active_tab_mut(1)
                .unwrap()
                .are_floating_panes_visible();
            let op_may_move_the_layer = matches!(
                op,
                Op::NewTiledPane
                    | Op::NewFloatingPane
                    | Op::ClosePane
                    | Op::ShowFloatingLayer
                    | Op::HideFloatingLayer
                    | Op::FocusFirstTiledPane
                    | Op::Resize(_)
            );
            match op {
                Op::NewTiledPane | Op::NewFloatingPane | Op::NewOverlay => {
                    let pane_id = PaneId::Terminal(next_id);
                    next_id += 1;
                    let placement = match op {
                        Op::NewTiledPane => NewPanePlacement::default(),
                        Op::NewFloatingPane => NewPanePlacement::Floating(None),
                        _ => NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
                            overlay_coordinates(),
                        ))),
                    };
                    screen
                        .get_active_tab_mut(1)
                        .unwrap()
                        .new_pane(pane_id, None, None, false, true, placement, Some(1), None)
                        .expect("TEST");
                    opened.push(pane_id);
                },
                Op::NewModalPlugin => {
                    let pane_id = PaneId::Plugin(next_id);
                    next_id += 1;
                    screen
                        .get_active_tab_mut(1)
                        .unwrap()
                        .new_pane(
                            pane_id,
                            Some("modal".to_owned()),
                            Some(modal_plugin_run()),
                            false,
                            true,
                            NewPanePlacement::Floating(None),
                            Some(1),
                            None,
                        )
                        .expect("TEST");
                    opened.push(pane_id);
                },
                Op::ClosePane => {
                    if !opened.is_empty() {
                        let pane_id = opened.remove(0);
                        screen
                            .get_active_tab_mut(1)
                            .unwrap()
                            .close_pane(pane_id, false, None);
                    }
                },
                Op::ShowFloatingLayer => {
                    screen.get_active_tab_mut(1).unwrap().show_floating_panes();
                },
                Op::HideFloatingLayer => {
                    screen.get_active_tab_mut(1).unwrap().hide_floating_panes();
                },
                Op::FocusFirstTiledPane => {
                    let _ = screen.get_active_tab_mut(1).unwrap().focus_pane_with_id(
                        PaneId::Terminal(1),
                        false,
                        false,
                        1,
                    );
                },
                Op::Resize(cols) => {
                    screen
                        .resize_to_screen(Size {
                            cols: *cols as usize,
                            rows: 20,
                        })
                        .expect("TEST");
                },
            }

            let tab = screen
                .get_active_tab_mut(1)
                .unwrap_or_else(|_| panic!("the tab is gone after {op:?} (step {index})"));
            if !op_may_move_the_layer {
                assert_eq!(
                    tab.are_floating_panes_visible(),
                    layer_was_visible,
                    "{op:?} (step {index}) changed whether the floating layer is up"
                );
            }
            tab.assert_invariants();
        }
    }
}

// ---------------------------------------------------------------------------
// The new placement.
// ---------------------------------------------------------------------------

/// Modal-ness used to be a string in a plugin's own configuration map, which meant a terminal
/// pane could not be one: there was no configuration to read. `NewPanePlacement::Overlay` carries
/// the kind with the placement, so any pane can be placed as a modal.
#[test]
fn a_terminal_pane_can_be_placed_as_a_modal() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);

    let modal = PaneId::Terminal(2);
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .new_pane(
            modal,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::modal(Some(overlay_coordinates()))),
            Some(1),
            None,
        )
        .expect("TEST");

    let tab = screen.get_active_tab_mut(1).unwrap();
    assert!(
        tab.pane_is_overlay(modal),
        "a pane placed as a modal is an overlay"
    );
    assert!(
        !tab.are_floating_panes_visible(),
        "and does not raise the floating layer"
    );
    assert_eq!(tab.get_active_pane_id(1), Some(modal), "and holds focus");
    tab.assert_invariants();
}

/// The same placement without coordinates. Nothing is dropped on the way through: the placement
/// itself says the pane belongs on the surface, so there is no coordinates field for a caller to
/// rebuild without the flag.
#[test]
fn an_overlay_placement_without_coordinates_is_still_an_overlay() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);

    let overlay = PaneId::Terminal(2);
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .new_pane(
            overlay,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(1),
            None,
        )
        .expect("TEST");

    let tab = screen.get_active_tab_mut(1).unwrap();
    assert!(tab.pane_is_overlay(overlay));
    assert!(!tab.are_floating_panes_visible());
    tab.assert_invariants();
}

/// `--modal` on the CLI is `--overlay` plus "do not take focus from it". Both reach the tab as an
/// overlay placement rather than as a coordinates flag a caller could drop.
#[test]
fn the_cli_modal_and_overlay_flags_both_produce_an_overlay_placement() {
    use zellij_utils::data::{NewPanePlacement, OverlayKind};

    let modal =
        NewPanePlacement::floating_or_overlay(None, None, None, None, None, None, None, Some(true));
    assert!(matches!(
        &modal,
        NewPanePlacement::Overlay(placement) if placement.kind == OverlayKind::Modal
    ));

    let overlay =
        NewPanePlacement::floating_or_overlay(None, None, None, None, None, None, Some(true), None);
    assert!(matches!(
        &overlay,
        NewPanePlacement::Overlay(placement) if placement.kind == OverlayKind::Dismissible
    ));

    // --modal wins over --overlay, being the stricter of the two
    let both = NewPanePlacement::floating_or_overlay(
        None,
        None,
        None,
        None,
        None,
        None,
        Some(true),
        Some(true),
    );
    assert!(matches!(
        &both,
        NewPanePlacement::Overlay(placement) if placement.kind == OverlayKind::Modal
    ));

    let plain =
        NewPanePlacement::floating_or_overlay(None, None, None, None, None, None, None, None);
    assert!(matches!(plain, NewPanePlacement::Floating(_)));
}

/// The compatibility shim: a plugin (or keybinding, or layout) that still spells overlay-ness as
/// `overlay`/`modal` keys in the free-form plugin configuration map resolves to the same
/// placement. This is the only place that spelling is understood.
#[test]
fn plugin_configuration_keys_resolve_to_an_overlay_placement() {
    use zellij_utils::data::OverlayKind;

    let mut modal = RunPlugin::from_url("zellij:quit-confirmation").expect("test plugin");
    modal.configuration.insert("modal", "true");
    let modal = RunPluginOrAlias::RunPlugin(modal);
    assert_eq!(
        modal.overlay_placement().map(|placement| placement.kind),
        Some(OverlayKind::Modal)
    );

    let mut dismissible = RunPlugin::from_url("zellij:file-finder").expect("test plugin");
    dismissible.configuration.insert("overlay", "true");
    let dismissible = RunPluginOrAlias::RunPlugin(dismissible);
    assert_eq!(
        dismissible
            .overlay_placement()
            .map(|placement| placement.kind),
        Some(OverlayKind::Dismissible)
    );

    let plain = RunPlugin::from_url("zellij:file-finder").expect("test plugin");
    assert_eq!(
        RunPluginOrAlias::RunPlugin(plain).overlay_placement(),
        None,
        "a plugin that asks for neither is not an overlay"
    );
}

/// A modal is opened for the client who asked for it, but it is drawn for everybody - and a client
/// that attaches afterwards was not in `connected_clients` when the overlay took focus. Without
/// this it sees the dialog over its whole surface and types into the pane behind it, and the modal
/// has no way to learn that anyone is doing so.
#[test]
fn a_client_joining_while_a_modal_is_open_lands_on_the_modal() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);

    let modal_id = add_modal_plugin_overlay(&mut screen, 7);
    screen.add_client(2, false).expect("TEST");

    let tab = screen.get_active_tab_mut(2).unwrap();
    assert_eq!(
        tab.get_active_pane_id(2),
        Some(modal_id),
        "the client that just attached must be on the modal it can see, not on the pane under it"
    );
    assert_eq!(
        tab.get_active_pane_id(1),
        Some(modal_id),
        "and the client that opened it is where it was"
    );
}

/// The same door, opened the way a user actually opens it: no reattach, just walking into a tab
/// that has a modal in it. `switch_active_tab` reaches `Tab::add_client` through
/// `move_clients_between_tabs`, so this is the path that matters in a live session.
#[test]
fn a_client_switching_into_a_tab_with_a_modal_lands_on_it() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");
    screen.switch_active_tab(1, None, true, 2).expect("TEST");
    screen.switch_active_tab(0, None, true, 1).expect("TEST");

    let modal_id = add_modal_plugin_overlay(&mut screen, 7);
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    let tab = screen.get_active_tab_mut(2).unwrap();
    assert_eq!(
        tab.get_active_pane_id(2),
        Some(modal_id),
        "walking into a tab with a modal in it puts the keyboard on the modal"
    );
}

/// The other half of the rule. A dismissible overlay that gave its focus up - which is how
/// `hocus-ask` learns to close itself - stays given up: a joining client inherits what the other
/// clients actually have, and nobody has that overlay. Focusing it here would resurrect a dialog
/// the user already dismissed.
#[test]
fn a_joining_client_does_not_pick_up_a_dismissible_overlay_that_gave_up_focus() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_popup(&mut screen, 2, "POPUP");

    let tab = screen.get_active_tab_mut(1).unwrap();
    tab.new_pane(
        PaneId::Terminal(3),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(1),
        None,
    )
    .expect("TEST");
    assert_eq!(
        tab.get_active_pane_id(1),
        Some(PaneId::Terminal(3)),
        "the popup gave the keyboard up to the pane the user asked for"
    );

    screen.add_client(2, false).expect("TEST");

    let tab = screen.get_active_tab_mut(2).unwrap();
    assert_eq!(
        tab.get_active_pane_id(2),
        Some(PaneId::Terminal(3)),
        "a joining client inherits the tiled focus, not an overlay nobody is on"
    );
}

/// The hit-test already knows the overlay is there - the write has to agree with it. Scroll
/// forwarding used to hand the position to `Tab::write_to_terminal_at`, which resolved it a second
/// time through the floating and tiled layers only: an overlay in alternate-screen mode had its
/// cursor keys delivered to the tiled pane underneath, so scrolling a dialog moved a shell behind
/// it.
#[test]
fn scrolling_a_terminal_overlay_in_alternate_mode_writes_to_the_overlay() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    // the tab copies the bus senders when it is created, so the capture goes in first
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    new_tab(&mut screen, 1, 0);
    open_popup(&mut screen, 3, "OVERLAY");
    // DECSET 1049: the overlay is on the alternate screen, where scrolling is cursor keys
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .handle_pty_bytes(3, b"\x1b[?1049h".to_vec())
        .expect("TEST");
    pty_writer_rx.try_iter().for_each(drop);

    // `overlay_coordinates()` puts the overlay at (2, 2), 60% wide and 4 rows tall
    screen.handle_mouse_event(MouseEvent::new_scroll_up_event(Position::new(3, 4)), 1);

    let writes: Vec<_> = pty_writer_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyWriteInstruction::Write(bytes, terminal_id, _) => Some((terminal_id, bytes)),
            _ => None,
        })
        .collect();
    assert!(
        writes.iter().all(|(terminal_id, _)| *terminal_id == 3),
        "every write goes to the overlay, none to the tiled pane under it: {writes:?}"
    );
    assert_eq!(
        writes
            .into_iter()
            .map(|(_, bytes)| bytes)
            .collect::<Vec<_>>(),
        vec![b"\x1b[A".to_vec(); 3],
        "and the overlay got the cursor keys its alternate screen scrolls with"
    );
}

/// The same delivery, through the other branch: a terminal overlay that tracks the mouse itself
/// gets the SGR report rather than cursor keys, and it has to arrive at the overlay too.
#[test]
fn scrolling_a_terminal_overlay_that_tracks_the_mouse_writes_to_the_overlay() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    // the tab copies the bus senders when it is created, so the capture goes in first
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    new_tab(&mut screen, 1, 0);
    open_popup(&mut screen, 3, "OVERLAY");
    // DECSET 1000 + 1006: report mouse buttons, in SGR encoding
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .handle_pty_bytes(3, b"\x1b[?1000h\x1b[?1006h".to_vec())
        .expect("TEST");
    pty_writer_rx.try_iter().for_each(drop);

    screen.handle_mouse_event(MouseEvent::new_scroll_up_event(Position::new(3, 4)), 1);

    let writes: Vec<_> = pty_writer_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyWriteInstruction::Write(bytes, terminal_id, _) => Some((terminal_id, bytes)),
            _ => None,
        })
        .collect();
    assert!(
        !writes.is_empty(),
        "the overlay tracks the mouse, so the scroll is reported to it"
    );
    assert!(
        writes.iter().all(|(terminal_id, _)| *terminal_id == 3),
        "and every report goes to the overlay, not to the tiled pane under it: {writes:?}"
    );
}

/// `Output` hides and cleans up the kitty image placements of any pane it is not told is visible.
/// An overlay is drawn for as long as it exists, so it belongs in that set - otherwise a terminal
/// overlay showing graphics has them wiped out from under it while it is plainly on screen.
#[test]
fn an_overlay_counts_as_visible_for_kitty_graphics() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "POPUP", true);

    let tab = screen.get_active_tab_mut(1).unwrap();
    assert!(tab.pane_is_overlay(PaneId::Terminal(2)));
    assert!(
        tab.kitty_visible_pane_ids().contains(&PaneId::Terminal(2)),
        "the overlay is on screen, so its kitty placements must not be cleaned up"
    );
}

// ---------------------------------------------------------------------------
// An overlay's geometry is a function of the surface and its coordinates, and of nothing else.
//
// `PaneGeom::adjust_coordinates` writes only the fields the coordinates name, so whatever the
// resolution starts from is what a coordinate that said nothing means. All three resolution
// points used to start from the pane's own geometry - and a pane built for this path arrives
// with `PaneGeom::default()`, whose 100%-constraint dimensions have an `inner` of 1. An overlay
// that named no size was born one cell square.
// ---------------------------------------------------------------------------

/// Open an overlay with exactly these coordinates, saying nothing the caller did not say.
fn open_overlay_with(screen: &mut Screen, pane_id: u32, coordinates: FloatingPaneCoordinates) {
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .new_pane(
            PaneId::Terminal(pane_id),
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(coordinates))),
            Some(1),
            None,
        )
        .expect("TEST");
}

/// `OverlayPlacement::coordinates` is documented as "`None` centres it at a default size", and
/// that is what every overlay born from plugin configuration asks for: `RunPlugin::overlay_placement`
/// returns `modal(None)` / `dismissible(None)` for the `modal`/`overlay` keys, and the fork's own
/// `hocus-new-agent` binding is one of them.
///
/// It used to be born 1x1 - a framed pane with no content area at all - and only recovered if the
/// plugin happened to re-assert its placement afterwards.
#[test]
fn an_overlay_that_named_no_size_gets_the_default_geometry() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_overlay_with(&mut screen, 2, FloatingPaneCoordinates::default());

    let geom = screen.tabs[&0]
        .get_pane_with_id(PaneId::Terminal(2))
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        (geom.cols.as_usize(), geom.rows.as_usize()),
        (40, 10),
        "half the 80x20 session surface, not one cell"
    );
    assert_eq!(
        (geom.x, geom.y),
        (20, 5),
        "centered on the surface, as a floating pane with no coordinates would be"
    );
}

/// Coordinates are resolved per axis: the one the caller named follows the coordinate, the one it
/// did not falls back to the surface default rather than to whatever the pane happened to carry.
#[test]
fn an_overlay_defaults_only_the_axis_its_coordinates_did_not_name() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_overlay_with(
        &mut screen,
        2,
        FloatingPaneCoordinates {
            width: Some(PercentOrFixed::Percent(60)),
            ..Default::default()
        },
    );

    let geom = screen.tabs[&0]
        .get_pane_with_id(PaneId::Terminal(2))
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        geom.cols.as_usize(),
        48,
        "60% of the 80 columns it asked for"
    );
    assert_eq!(
        geom.rows.as_usize(),
        10,
        "and half the surface for the height it did not"
    );
}

/// The default is re-resolved against the surface like any other coordinate: an overlay has no
/// positional history, so a resize leaves it half of whatever the screen is *now* rather than
/// stranded at the absolute the old surface produced.
#[test]
fn an_overlay_that_named_no_size_follows_the_surface_across_a_resize() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_overlay_with(&mut screen, 2, FloatingPaneCoordinates::default());

    screen
        .resize_to_screen(Size {
            cols: 120,
            rows: 20,
        })
        .expect("TEST");

    let geom = screen.tabs[&0]
        .get_pane_with_id(PaneId::Terminal(2))
        .expect("TEST")
        .position_and_size();
    assert_eq!(
        geom.cols.as_usize(),
        60,
        "half of the new 120 columns, not the old 40"
    );
    assert_eq!(geom.x, 30, "and re-centered on the new surface");
}

/// `is_pinned` lives inside the `PaneGeom` rather than beside it, and `Tab::add_overlay_pane` sets
/// it on the pane before handing it over. Seeding the geometry from the surface without carrying
/// the flag across would drop something the caller explicitly asked for.
#[test]
fn an_overlay_that_named_no_size_keeps_the_pinned_flag_it_was_given() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_overlay_with(
        &mut screen,
        2,
        FloatingPaneCoordinates {
            pinned: Some(true),
            ..Default::default()
        },
    );

    let geom = screen.tabs[&0]
        .get_pane_with_id(PaneId::Terminal(2))
        .expect("TEST")
        .position_and_size();
    assert!(
        geom.is_pinned,
        "the pinned flag survives the default geometry"
    );
    assert_eq!(
        (geom.cols.as_usize(), geom.rows.as_usize()),
        (40, 10),
        "and it still got a usable size"
    );
}

/// An overlay is not in the tiled grid, so it cannot be the root of a stack - and saying so has to
/// happen *before* `Screen::stack_panes` extracts the members.
///
/// `has_room_for_stack` used to reject only a floating or suppressed root, and it measures the root
/// through `get_pane_with_id`, which resolves overlays: a tall enough overlay answered "yes, there
/// is room". Screen then pulled every member out of its tab, `TiledPanes::stack_panes` could not
/// find the root, and `Tab::stack_panes` returned - dropping the vec that was by then the only
/// thing holding those panes, while their processes went on running.
#[test]
fn an_overlay_cannot_be_the_root_of_a_stack_and_no_pane_is_lost_refusing() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    for pane_id in [2, 3] {
        screen
            .get_active_tab_mut(1)
            .unwrap()
            .new_pane(
                PaneId::Terminal(pane_id),
                None,
                None,
                false,
                true,
                NewPanePlacement::Tiled {
                    direction: Some(Direction::Down),
                    borderless: None,
                },
                Some(1),
                None,
            )
            .expect("TEST");
    }
    let overlay = PaneId::Terminal(4);
    open_overlay_with(&mut screen, 4, FloatingPaneCoordinates::default());

    // the size gate is not what refuses this: the overlay's default geometry is half the 20-row
    // surface, comfortably more than the `stack_size + MIN_TERMINAL_HEIGHT` the root is measured
    // against. Without the overlay arm in `has_room_for_stack` this stack would be accepted.
    assert!(
        screen.tabs[&0]
            .get_pane_with_id(overlay)
            .expect("TEST")
            .position_and_size()
            .rows
            .as_usize()
            >= 3 + crate::tab::MIN_TERMINAL_HEIGHT
    );

    assert_eq!(
        screen.stack_panes(vec![overlay, PaneId::Terminal(2), PaneId::Terminal(3)]),
        None,
        "an overlay root is refused"
    );

    // still *tiled*, not merely still reachable: `get_pane_with_id` would also find them parked
    // in the suppressed panes, and being parked is the salvage path, not the correct outcome
    let tiled: Vec<PaneId> = screen.tabs[&0]
        .pane_infos()
        .into_iter()
        .filter(|info| !info.is_floating && !info.is_suppressed)
        .map(|info| PaneId::Terminal(info.id))
        .collect();
    for pane_id in [PaneId::Terminal(2), PaneId::Terminal(3)] {
        assert!(
            tiled.contains(&pane_id),
            "{pane_id:?} was never extracted, so it is still tiled in its tab - got {tiled:?}"
        );
    }
    assert!(
        screen.tabs[&0].pane_is_overlay(overlay),
        "and the overlay is untouched"
    );
}

// --- Phase 7: the generic paths that still reached past the collection ------------------------

/// Open a *modal* terminal overlay in the given tab, by id.
///
/// A terminal rather than a plugin deliberately: a plugin's overlay-ness can be re-derived on the
/// far side of a move from its `modal` configuration key, so a plugin modal can survive a path
/// that drops the placement entirely. A terminal modal has no such second source and is the only
/// honest test of whether the placement travelled.
fn open_modal_terminal_in_tab(screen: &mut Screen, tab_id: usize, pane_id: PaneId) {
    screen
        .tabs
        .get_mut(&tab_id)
        .expect("TEST")
        .new_pane(
            pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::modal(Some(overlay_coordinates()))),
            Some(1),
            None,
        )
        .expect("TEST");
}

/// Moving a pane between tabs is the one place overlay-ness can be lost without anybody
/// noticing: `extract_pane` drops the kind and coordinates on the way out, and the destination
/// has no way to re-derive them. `break_multiple_panes_to_tab_with_id` used to ask only
/// `pane_id_is_floating`, which an overlay answers `false` to - so a modal arrived as an
/// ordinary tiled pane, silently stripped of the one property it cannot do without.
#[test]
fn a_modal_broken_out_to_another_tab_arrives_as_a_modal() {
    let mut screen = screen_with_diverged_tab_ids();
    let modal = PaneId::Terminal(42);
    open_modal_terminal_in_tab(&mut screen, 2, modal);
    assert!(screen.tabs[&2].pane_is_overlay(modal), "pre-condition");

    let outcome = screen
        .break_multiple_panes_to_tab_with_id(vec![modal], 3, false, 1)
        .expect("TEST");

    assert!(outcome.succeeded(), "{outcome:?}");
    assert_eq!(outcome.moved, vec![modal]);
    assert!(
        screen.tabs[&3].pane_is_overlay(modal),
        "it is an overlay in the tab it landed in"
    );
    assert!(
        screen.tabs[&3]
            .overlay_placement_for(modal)
            .expect("TEST")
            .is_modal(),
        "and a modal, not merely a dismissible overlay"
    );
    assert!(
        !screen.tabs[&3].are_floating_panes_visible(),
        "an overlay arriving does not raise the destination's floating layer"
    );
}

/// The same reading of "not floating" as "tiled", on the path that makes a brand new tab.
#[test]
fn a_modal_broken_out_to_a_new_tab_arrives_as_a_modal() {
    let mut screen = screen_with_diverged_tab_ids();
    let modal = PaneId::Terminal(42);
    open_modal_terminal_in_tab(&mut screen, 2, modal);

    let new_tab_id = screen
        .break_multiple_panes_to_new_tab(vec![modal], None, false, None, 1)
        .expect("TEST");

    assert!(
        screen.tabs[&new_tab_id].pane_is_overlay(modal),
        "an overlay broken out to a new tab is still an overlay there"
    );
    assert!(screen.tabs[&new_tab_id]
        .overlay_placement_for(modal)
        .expect("TEST")
        .is_modal());
    assert!(
        !screen.tabs[&2].has_pane_with_pid(&modal),
        "and no longer in the tab it came from"
    );
}

/// `Tab::resize` was a two-way floating/tiled split, and an overlay answers `false` to the
/// floating half - so a resize keystroke with a modal up dissolved the tab's stack lists and
/// resized whatever the modal was covering. An overlay is not resizable; the keystroke does
/// nothing at all.
#[test]
fn a_resize_keystroke_on_an_overlay_leaves_the_pane_behind_it_alone() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .vertical_split(PaneId::Terminal(2), None, 1, None, None)
        .expect("TEST");
    open_floating_pane(&mut screen, 0, 3, "OVERLAY", true);

    let before: Vec<PaneGeom> = [PaneId::Terminal(1), PaneId::Terminal(2)]
        .iter()
        .map(|id| {
            screen.tabs[&0]
                .get_pane_with_id(*id)
                .expect("TEST")
                .position_and_size()
        })
        .collect();
    assert_eq!(
        screen.tabs[&0].get_active_pane_id(1),
        Some(PaneId::Terminal(3)),
        "pre-condition: the overlay has the keyboard"
    );

    screen
        .get_active_tab_mut(1)
        .unwrap()
        .resize(
            1,
            ResizeStrategy::new(Resize::Increase, Some(Direction::Left)),
        )
        .expect("TEST");

    let after: Vec<PaneGeom> = [PaneId::Terminal(1), PaneId::Terminal(2)]
        .iter()
        .map(|id| {
            screen.tabs[&0]
                .get_pane_with_id(*id)
                .expect("TEST")
                .position_and_size()
        })
        .collect();
    assert_eq!(
        before, after,
        "the tiled panes behind the overlay are untouched"
    );
}

/// `active_pane_is_flush_with_edge` is the sole input to
/// `Screen::resize_global_pane_from_adjacent_tab_pane`, and it excused only *floating* active
/// panes. An overlay that happened to reach an edge of the surface therefore let a resize
/// keystroke aimed at nothing in particular resize a global pane instead.
#[test]
fn a_resize_keystroke_on_an_overlay_does_not_reach_a_global_pane() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    // an overlay pinned to the very top-left of the surface, so it is flush with two edges
    open_overlay_with(
        &mut screen,
        2,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(0)),
            y: Some(PercentOrFixed::Fixed(0)),
            width: Some(PercentOrFixed::Percent(50)),
            height: Some(PercentOrFixed::Percent(50)),
            ..Default::default()
        },
    );

    for direction in [Direction::Left, Direction::Up] {
        assert!(
            !screen.tabs[&0].active_pane_is_flush_with_edge(direction, 1),
            "an overlay is never flush with anything, so nothing hands the keystroke to a \
             global pane ({direction:?})"
        );
    }
}

/// With `visual_bell` off, `check_and_consume_bells_without_visual_notification` is the *only*
/// thing that sees a bell - and its iterator chain was the visual one minus the overlay link.
/// A BEL from a modal was therefore neither forwarded to the client nor consumed off the pane.
#[test]
fn a_bell_in_an_overlay_is_heard_without_visual_notification() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "OVERLAY", true);

    let tab = screen.get_active_tab_mut(1).unwrap();
    tab.handle_pty_bytes(2, b"\x07".to_vec()).expect("TEST");

    assert!(
        tab.check_and_consume_bells_without_visual_notification(),
        "the bell rung inside the overlay is heard"
    );
    assert!(
        !tab.check_and_consume_bells_without_visual_notification(),
        "and consumed, so it does not ring forever"
    );
}

/// An overlay's frame is drawn over whatever is under it, and an overlay is not draggable. But
/// the mouse pipeline decided "start moving a floating pane" from the *global* layer-visibility
/// flag rather than from the clicked pane, and the executor then threw the pane id away and
/// re-resolved the position through `FloatingPanes` alone. Pressing a modal's frame therefore
/// picked up the ordinary floating pane whose own frame lay beneath it and dragged that.
#[test]
fn a_press_on_an_overlay_frame_does_not_drag_the_floating_pane_beneath_it() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);
    assert!(
        screen.tabs[&0].are_floating_panes_visible(),
        "pre-condition: the layer is up, which is what the buggy test consulted"
    );

    // an overlay whose top frame row lies exactly on the floating pane's own top frame row
    let floating = floating_geom(&screen, 2);
    open_overlay_with(
        &mut screen,
        3,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(floating.x)),
            y: Some(PercentOrFixed::Fixed(floating.y)),
            width: Some(PercentOrFixed::Fixed(floating.cols.as_usize())),
            height: Some(PercentOrFixed::Fixed(4)),
            ..Default::default()
        },
    );

    let row = floating.y as i32;
    let from = floating.x as u16 + 2;
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(row, from)),
        1,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(Position::new(row, from - 5)),
        1,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(Position::new(row, from - 5)),
        1,
    );

    assert_eq!(
        floating_geom(&screen, 2),
        floating,
        "the floating pane under the overlay did not move"
    );
    assert_eq!(
        screen.active_pane_id(1),
        Some(PaneId::Terminal(3)),
        "and the press landed on the overlay, which is what was drawn there"
    );
}

/// The second door into the same drag, and the reason swallowing the click in `focus_pane_at`
/// was not enough on its own: when the pressed overlay is not the focused one the press
/// classifies as an ordinary focus, and `execute_focus_pane` went on to call
/// `move_pane_with_mouse` anyway - which re-resolves the position through `FloatingPanes` and
/// finds the pane the overlay is covering.
#[test]
fn focusing_one_overlay_by_its_frame_does_not_drag_the_floating_pane_beneath_it() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);

    let floating = floating_geom(&screen, 2);
    open_overlay_with(
        &mut screen,
        3,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(floating.x)),
            y: Some(PercentOrFixed::Fixed(floating.y)),
            width: Some(PercentOrFixed::Fixed(floating.cols.as_usize())),
            height: Some(PercentOrFixed::Fixed(4)),
            ..Default::default()
        },
    );
    // a second overlay, elsewhere, which takes the focus - so the press below lands on an
    // overlay that is *not* active and classifies as a focus rather than a selection
    open_overlay_with(
        &mut screen,
        4,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(0)),
            y: Some(PercentOrFixed::Fixed(0)),
            width: Some(PercentOrFixed::Fixed(10)),
            height: Some(PercentOrFixed::Fixed(3)),
            ..Default::default()
        },
    );
    assert_eq!(
        screen.active_pane_id(1),
        Some(PaneId::Terminal(4)),
        "pre-condition: the overlay about to be pressed does not have the focus"
    );

    let row = floating.y as i32;
    let from = floating.x as u16 + 2;
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(row, from)),
        1,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(Position::new(row, from - 5)),
        1,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(Position::new(row, from - 5)),
        1,
    );

    assert_eq!(
        floating_geom(&screen, 2),
        floating,
        "the floating pane under the overlay did not move"
    );
    assert_eq!(
        screen.active_pane_id(1),
        Some(PaneId::Terminal(3)),
        "and the press did focus the overlay it landed on"
    );
}

/// `break_pane_to_new_tab` moves the *active* pane, which is the overlay whenever one is up, and
/// it chose between floating and tiled from `are_floating_panes_visible` alone. A modal broken
/// out this way arrived in the neighbouring tab as an ordinary pane.
#[test]
fn a_modal_broken_out_to_the_neighbouring_tab_arrives_as_a_modal() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.switch_tab_prev(None, false, 1).expect("TEST");

    let modal = PaneId::Terminal(3);
    open_modal_terminal_in_tab(&mut screen, 0, modal);
    assert_eq!(screen.active_pane_id(1), Some(modal), "pre-condition");

    screen
        .break_pane_to_new_tab(Direction::Right, 1)
        .expect("TEST");

    assert!(
        screen.tabs[&1].pane_is_overlay(modal),
        "it is an overlay in the tab it landed in"
    );
    assert!(screen.tabs[&1]
        .overlay_placement_for(modal)
        .expect("TEST")
        .is_modal());
    assert!(
        !screen.tabs[&1].are_floating_panes_visible(),
        "and did not raise that tab's floating layer on the way in"
    );
}

/// `execute_focus_on_hover` asked whether the hovered pane was on the *visible floating layer*,
/// and an overlay answers `false` - so with the layer up, focus-follows-mouse over a modal
/// reduced to a hover and left the keyboard where it was. An overlay is painted over everything
/// with no layer precondition, which is the whole reason it is not a floating pane.
#[test]
fn focus_follows_mouse_lands_on_an_overlay_over_a_visible_floating_layer() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "FLOATING", false);
    assert!(
        screen.tabs[&0].are_floating_panes_visible(),
        "pre-condition"
    );

    let floating = floating_geom(&screen, 2);
    open_overlay_with(
        &mut screen,
        3,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(floating.x)),
            y: Some(PercentOrFixed::Fixed(floating.y)),
            width: Some(PercentOrFixed::Fixed(floating.cols.as_usize())),
            height: Some(PercentOrFixed::Fixed(4)),
            ..Default::default()
        },
    );
    // a second overlay elsewhere takes the focus, so there is somewhere for the hover to move it
    // *from*
    open_overlay_with(
        &mut screen,
        4,
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(0)),
            y: Some(PercentOrFixed::Fixed(0)),
            width: Some(PercentOrFixed::Fixed(10)),
            height: Some(PercentOrFixed::Fixed(3)),
            ..Default::default()
        },
    );
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .update_focus_follows_mouse(true);
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(4)));

    // the body of the first overlay, not its frame
    screen.handle_mouse_event(
        MouseEvent::new_buttonless_motion(Position::new(
            floating.y as i32 + 2,
            floating.x as u16 + 2,
        )),
        1,
    );

    assert_eq!(
        screen.active_pane_id(1),
        Some(PaneId::Terminal(3)),
        "the hover focused the overlay under the cursor"
    );
}

// ---------------------------------------------------------------------------
// A live modal owns the keyboard and the top of the stack.
//
// A modal has no way to learn that it lost focus, so an overlay opened after it must not take
// either. Both halves matter: focus alone would leave the modal holding a keyboard the user
// cannot see it holding, buried under whatever opened last.
// ---------------------------------------------------------------------------

/// Open an overlay of an explicit kind, overlapping the region `open_popup` uses so that whichever
/// one is on top is the one that shows.
fn open_overlay_of_kind(screen: &mut Screen, pane_id: u32, contents: &str, kind: OverlayKind) {
    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(2)),
        y: Some(PercentOrFixed::Fixed(2)),
        width: Some(PercentOrFixed::Percent(60)),
        height: Some(PercentOrFixed::Fixed(4)),
        ..Default::default()
    };
    let placement = match kind {
        OverlayKind::Modal => OverlayPlacement::modal(Some(coordinates)),
        OverlayKind::Dismissible => OverlayPlacement::dismissible(Some(coordinates)),
    };
    let tab = screen.tabs.get_mut(&0).expect("TEST");
    tab.new_pane(
        PaneId::Terminal(pane_id),
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(placement),
        Some(1),
        None,
    )
    .expect("TEST");
    tab.handle_pty_bytes(pane_id, contents.as_bytes().to_vec())
        .expect("TEST");
}

#[test]
fn an_overlay_opened_over_a_modal_takes_neither_the_keyboard_nor_the_top_of_the_stack() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_overlay_of_kind(&mut screen, 2, "THEMODAL", OverlayKind::Modal);
    open_overlay_of_kind(&mut screen, 3, "LATEROVERLAY", OverlayKind::Dismissible);

    assert_eq!(
        screen.active_pane_id(1),
        Some(PaneId::Terminal(2)),
        "the modal keeps the keyboard"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        frame.contains("THEMODAL"),
        "and stays visible, rather than being buried under the overlay that opened after it, \
         got:\n{frame}"
    );
    assert!(
        !frame.contains("LATEROVERLAY"),
        "the later overlay is behind the modal, which covers it entirely, got:\n{frame}"
    );
    assert_snapshot!(frame);
}

#[test]
fn a_modal_opened_over_a_modal_takes_both_and_hands_them_back_when_it_closes() {
    // the deliberate exception: without it a quit-confirmation raised over an already open modal
    // would come up unfocused and unable to close itself
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_overlay_of_kind(&mut screen, 2, "FIRSTMODAL", OverlayKind::Modal);
    open_overlay_of_kind(&mut screen, 3, "SECONDMODAL", OverlayKind::Modal);

    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(3)));
    let frame = rendered_frame(&mut screen, size);
    assert!(
        frame.contains("SECONDMODAL"),
        "the newer modal is the one on top, got:\n{frame}"
    );

    screen
        .get_active_tab_mut(1)
        .unwrap()
        .close_pane(PaneId::Terminal(3), false, None);
    assert_eq!(
        screen.active_pane_id(1),
        Some(PaneId::Terminal(2)),
        "closing it walks the keyboard back down to the modal underneath"
    );
}

/// An overlay outranks the floating layer in `get_active_pane_id`, so a dismissible overlay that
/// keeps its focus when a floating pane opens leaves that pane behind the popup, with the keyboard
/// still on the popup - and the popup, which learns it should close by seeing itself unfocused,
/// never finds out.
#[test]
fn a_focused_floating_pane_opened_over_a_dismissible_overlay_takes_the_keyboard() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_overlay_of_kind(&mut screen, 2, "THEPOPUP", OverlayKind::Dismissible);
    open_floating_pane(&mut screen, 0, 3, "THEFLOATER", false);

    assert_eq!(
        screen.active_pane_id(1),
        Some(PaneId::Terminal(3)),
        "the floating pane the user just asked for has the keyboard, rather than the popup \
         it opened over - an overlay that still held focus would win this lookup"
    );
}

/// The modal half of the same rule: a modal has no way to learn it lost focus, so a floating pane
/// opened over it comes up behind it rather than stranding it on screen with the input elsewhere.
#[test]
fn a_focused_floating_pane_opened_over_a_modal_leaves_the_keyboard_alone() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_overlay_of_kind(&mut screen, 2, "THEMODAL", OverlayKind::Modal);
    open_floating_pane(&mut screen, 0, 3, "THEFLOATER", false);

    assert_eq!(
        screen.active_pane_id(1),
        Some(PaneId::Terminal(2)),
        "the modal keeps the keyboard"
    );
    let frame = rendered_frame(&mut screen, size);
    assert!(
        frame.contains("THEMODAL"),
        "and stays visible over the floating pane that opened under it, got:\n{frame}"
    );
}

/// `set_pane_selectable(false)` moves clients off an overlay; a later `FocusPaneWithId` used to
/// walk straight past that and re-insert it as the active pane, routing input to a pane that had
/// explicitly asked for none.
#[test]
fn a_non_selectable_overlay_cannot_be_focused_again() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_overlay_of_kind(&mut screen, 2, "THEPOPUP", OverlayKind::Dismissible);

    let tab = screen.get_active_tab_mut(1).expect("TEST");
    tab.set_pane_selectable(PaneId::Terminal(2), false);
    assert_ne!(
        tab.get_active_pane_id(1),
        Some(PaneId::Terminal(2)),
        "making it non-selectable moved the client off it"
    );

    tab.focus_pane_with_id(PaneId::Terminal(2), false, false, 1)
        .expect("TEST");
    assert_ne!(
        tab.get_active_pane_id(1),
        Some(PaneId::Terminal(2)),
        "and focusing it by id does not put the client back onto a pane that wants no input - \
         an active overlay would outrank every other layer in this lookup"
    );
}

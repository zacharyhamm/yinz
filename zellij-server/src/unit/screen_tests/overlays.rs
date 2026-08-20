use super::*;
use std::collections::BTreeMap;

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
        NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
            overlay: Some(true),
            ..Default::default()
        })),
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
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Percent(20)),
                width: Some(PercentOrFixed::Percent(60)),
                height: Some(PercentOrFixed::Fixed(4)),
                overlay: Some(true),
                ..Default::default()
            })),
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
        overlay: Some(true),
        ..Default::default()
    };
    let first_tab = screen.tabs.get_mut(&0).expect("TEST");
    first_tab.show_floating_panes();
    first_tab
        .add_floating_pane(popup, popup_id, Some(coordinates), true, Some(1))
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

/// Coordinates that turn a floating pane into an overlay, as the fork's popup plugins do from
/// their `load()` - they are launched as ordinary floating panes and only upgrade themselves
/// once they are running.
fn overlay_coordinates() -> FloatingPaneCoordinates {
    FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(2)),
        y: Some(PercentOrFixed::Fixed(2)),
        width: Some(PercentOrFixed::Percent(60)),
        height: Some(PercentOrFixed::Fixed(4)),
        overlay: Some(true),
        ..Default::default()
    }
}

/// Opening a floating pane shows the floating layer. A plugin that then turns that pane into an
/// overlay leaves the layer flag set behind it, and the whole of zellij reads that flag as "the
/// user has floating panes up": the status bar lights its `<f>` ribbon, the next new pane joins
/// the floating layer, splits and focus moves get swallowed. Only the overlay is up, so the
/// layer must go back down.
#[test]
fn a_floating_pane_that_becomes_an_overlay_takes_the_floating_layer_down_with_it() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    open_floating_pane(&mut screen, 0, 2, "POPUP", false);

    let tab = screen.get_active_tab_mut(1).unwrap();
    assert!(
        tab.are_floating_panes_visible(),
        "opening an ordinary floating pane shows the floating layer"
    );

    tab.change_floating_pane_coordinates(&PaneId::Terminal(2), overlay_coordinates())
        .expect("TEST");

    assert!(tab.pane_is_overlay(PaneId::Terminal(2)));
    assert!(
        !tab.are_floating_panes_visible(),
        "an overlay is not the floating layer"
    );
    assert_eq!(
        tab.get_active_pane_id(1),
        Some(PaneId::Terminal(2)),
        "the overlay keeps focus - the layer went away, it did not"
    );
}

/// The two ways to arrive at an overlay - born one, or turned into one - have to land in the same
/// place, or the fix above is only a fix for the path it was written against.
#[test]
fn a_pane_that_becomes_an_overlay_lands_where_a_pane_born_an_overlay_already_is() {
    let size = Size { cols: 80, rows: 12 };

    let mut born = create_non_mirrored_screen(size);
    new_tab(&mut born, 1, 0);
    open_floating_pane(&mut born, 0, 2, "POPUP", true);

    let mut turned = create_non_mirrored_screen(size);
    new_tab(&mut turned, 1, 0);
    open_floating_pane(&mut turned, 0, 2, "POPUP", false);
    turned
        .get_active_tab_mut(1)
        .unwrap()
        .change_floating_pane_coordinates(&PaneId::Terminal(2), overlay_coordinates())
        .expect("TEST");

    for screen in [&mut born, &mut turned] {
        let tab = screen.get_active_tab_mut(1).unwrap();
        assert!(tab.pane_is_overlay(PaneId::Terminal(2)));
        assert!(!tab.are_floating_panes_visible());
        assert_eq!(tab.get_active_pane_id(1), Some(PaneId::Terminal(2)));
    }

    let render = |screen: &mut Screen| {
        let (server_sender, server_receiver) = channels::unbounded();
        screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
        let mut grid = grid_for_render_snapshot(size);
        render_frame_into(screen, &server_receiver, 1, &mut grid)
    };
    assert_eq!(
        render(&mut turned),
        render(&mut born),
        "a pane turned into an overlay must render as one born an overlay"
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
    open_floating_pane(&mut screen, 0, 3, "POPUP", false);

    let tab = screen.get_active_tab_mut(1).unwrap();
    tab.change_floating_pane_coordinates(&PaneId::Terminal(3), overlay_coordinates())
        .expect("TEST");

    assert!(tab.pane_is_overlay(PaneId::Terminal(3)));
    assert!(
        tab.are_floating_panes_visible(),
        "the floating pane behind the overlay is still up"
    );
}

/// Overlay status is permanent for the pane's lifetime. A coordinate update must not turn an
/// overlay back into an ordinary floating pane rendered on a hidden layer.
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
            overlay: Some(false),
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

/// A modal overlay (e.g. hocus-new-agent) only closes on Esc/submit/`Event::Visible(false)` - it
/// has no way to learn it lost focus, unlike a dismissible overlay that watches `PaneUpdate`.
/// Stealing its focus the way `a_new_pane_opened_over_a_modal_is_tiled` does for a plain overlay
/// would leave it stuck on screen while input goes to the pane underneath, so it must keep focus.
#[test]
fn a_new_pane_opened_over_a_modal_plugin_overlay_does_not_steal_its_focus() {
    let size = Size { cols: 80, rows: 12 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);

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
        7,
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
    let modal_id = PaneId::Plugin(7);
    let tab = screen.get_active_tab_mut(1).unwrap();
    tab.add_floating_pane(modal, modal_id, Some(overlay_coordinates()), true, Some(1))
        .expect("TEST");
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
    open_floating_pane(&mut screen, 0, 2, "POPUP", false);

    let tab = screen.get_active_tab_mut(1).unwrap();
    // DECSET 1004: the pane asks to be told when it gains and loses focus
    tab.handle_pty_bytes(2, b"\x1b[?1004h".to_vec())
        .expect("TEST");

    tab.change_floating_pane_coordinates(&PaneId::Terminal(2), overlay_coordinates())
        .expect("TEST");

    assert!(!tab.are_floating_panes_visible());
    assert_eq!(
        tab.get_active_pane_id(1),
        Some(PaneId::Terminal(2)),
        "the overlay is still the active pane"
    );
}

/// Open a floating pane and then have it turn itself into an overlay, which is how every popup
/// plugin in this fork arrives: `hocus-new-agent`, `hocus-ask` and `quit-confirmation` all call
/// `change_floating_panes_coordinates(.. .as_overlay())` from their `load()`, after the server
/// has already placed them as ordinary floating panes.
fn open_pane_that_becomes_an_overlay(screen: &mut Screen, pane_id: u32, contents: &str) {
    open_floating_pane(screen, 0, pane_id, contents, false);
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .change_floating_pane_coordinates(&PaneId::Terminal(pane_id), overlay_coordinates())
        .expect("TEST");
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

    open_pane_that_becomes_an_overlay(&mut screen, 3, "MODAL");

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

    open_pane_that_becomes_an_overlay(&mut screen, 4, "MODAL");

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

    open_pane_that_becomes_an_overlay(&mut screen, 3, "MODAL");

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

    open_pane_that_becomes_an_overlay(&mut screen, 3, "MODAL");
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

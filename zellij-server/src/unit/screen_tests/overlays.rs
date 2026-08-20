use super::*;

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

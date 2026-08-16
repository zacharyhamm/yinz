use super::*;

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

/// A pane's z-index has to be its place in the occlusion stack, but each render pass draws only
/// half the floating panes (the overlays or the rest). Taking the index from the pass rather than
/// from the stack clips the overlay against its own geometry, dropping every one of its chunks -
/// the popup renders as nothing at all and whatever sits behind it shows through.
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
        .focus_plugin_pane(&run_plugin, true, true, false, 1, &mut None)
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

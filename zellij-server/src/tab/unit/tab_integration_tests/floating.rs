use super::*;
use zellij_utils::input::layout::{Run, RunPlugin};

#[test]
fn new_floating_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
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
fn overlay_does_not_show_hidden_floating_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let other_client_id = 2;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let floating_pane_id = PaneId::Terminal(2);
    let overlay_pane_id = PaneId::Terminal(3);
    let other_client_pane_id = PaneId::Terminal(4);

    tab.new_floating_pane(
        floating_pane_id,
        None,
        None,
        false,
        true,
        Some(FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(80)),
            y: Some(PercentOrFixed::Fixed(2)),
            width: Some(PercentOrFixed::Fixed(20)),
            height: Some(PercentOrFixed::Fixed(8)),
            ..Default::default()
        }),
        None,
    )
    .unwrap();
    assert!(tab.floating_panes.all_panes_are_visible());
    tab.hide_floating_panes();
    tab.add_client(other_client_id, None).unwrap();
    tab.new_pane(
        overlay_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(10)),
                y: Some(PercentOrFixed::Fixed(2)),
                width: Some(PercentOrFixed::Fixed(20)),
                height: Some(PercentOrFixed::Fixed(8)),
                ..Default::default()
            },
        ))),
        Some(client_id),
        None,
    )
    .unwrap();
    // the other client is looking at the hidden floating pane rather than at the overlay. Taking
    // it off the overlay is now a separate step: an overlay masks every client's focus while it is
    // up (see `an_overlay_masks_each_client_s_tiled_focus_until_it_closes`), and that record lives
    // in its own collection rather than in the same map this line writes to.
    tab.overlay_panes.defocus_pane(other_client_id);
    tab.floating_panes
        .set_active_pane_for_client(floating_pane_id, other_client_id);

    assert!(tab.pane_is_overlay(overlay_pane_id));
    assert!(!tab.floating_panes.all_panes_are_visible());
    assert!(!tab.floating_panes.panes_are_visible());
    assert!(
        tab.floating_panes.rendered_pane_ids().is_empty(),
        "nothing on the floating layer is rendered while it is hidden"
    );
    assert_eq!(
        tab.overlay_panes.rendered_pane_ids(),
        vec![overlay_pane_id],
        "and the overlay, which is not on that layer, is rendered regardless"
    );
    assert_eq!(tab.get_active_pane_id(client_id), Some(overlay_pane_id));
    assert_eq!(
        tab.get_active_pane(client_id).unwrap().pid(),
        overlay_pane_id
    );

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    assert_eq!(
        tab.floating_panes.rendered_pane_ids(),
        vec![floating_pane_id]
    );
    assert_eq!(tab.overlay_panes.rendered_pane_ids(), vec![overlay_pane_id]);
    assert_eq!(tab.get_active_pane_id(client_id), Some(overlay_pane_id));

    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    assert!(tab.floating_panes.rendered_pane_ids().is_empty());
    assert_eq!(tab.overlay_panes.rendered_pane_ids(), vec![overlay_pane_id]);
    assert_eq!(tab.get_active_pane_id(client_id), Some(overlay_pane_id));

    tab.show_floating_panes();
    assert_eq!(
        tab.floating_panes.rendered_pane_ids(),
        vec![floating_pane_id]
    );
    assert_eq!(tab.overlay_panes.rendered_pane_ids(), vec![overlay_pane_id]);
    assert_eq!(tab.get_active_pane_id(client_id), Some(overlay_pane_id));

    tab.hide_floating_panes();
    assert!(tab.floating_panes.rendered_pane_ids().is_empty());
    assert_eq!(tab.overlay_panes.rendered_pane_ids(), vec![overlay_pane_id]);
    assert_eq!(tab.get_active_pane_id(client_id), Some(overlay_pane_id));

    assert_eq!(
        tab.get_active_pane(other_client_id).unwrap().pid(),
        PaneId::Terminal(1),
        "the other client must stay on its visible tiled pane"
    );
    assert!(!tab.move_focus_right(client_id).unwrap());
    assert_eq!(
        tab.get_active_pane_id(client_id),
        Some(overlay_pane_id),
        "focus must not move to the hidden floating pane"
    );

    let hidden_floating_pane_geom = tab
        .get_pane_with_id(floating_pane_id)
        .unwrap()
        .position_and_size();
    tab.resize(other_client_id, ResizeStrategy::new(Resize::Increase, None))
        .unwrap();
    assert_eq!(
        tab.get_pane_with_id(floating_pane_id)
            .unwrap()
            .position_and_size(),
        hidden_floating_pane_geom,
        "resizing from the other client must not resize its hidden floating pane"
    );

    tab.new_pane(
        other_client_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(other_client_id),
        None,
    )
    .unwrap();
    assert!(tab.tiled_panes.panes_contain(&other_client_pane_id));
    tab.close_focused_pane(other_client_id, None).unwrap();
    assert!(tab.get_pane_with_id(other_client_pane_id).is_none());
    assert!(tab.get_pane_with_id(overlay_pane_id).is_some());
    assert!(tab.get_pane_with_id(floating_pane_id).is_some());

    tab.close_focused_pane(client_id, None).unwrap();
    assert!(tab.get_pane_with_id(overlay_pane_id).is_none());
    assert!(tab.floating_panes.rendered_pane_ids().is_empty());
    assert!(tab.get_pane_with_id(PaneId::Terminal(1)).is_some());
    assert!(tab.get_pane_with_id(floating_pane_id).is_some());
}

/// A new pane opened while a modal holds the focus goes to the tiled surface.
///
/// It used to join the floating layer and show it, which is the reading of a focused overlay as
/// "this client is working on the floating layer" - and that reveals every hidden floating pane
/// and lights the status bar's `<f>` on the way. Tiled keeps what that behaviour was actually
/// for, which is that the new pane is visible rather than stranded on a layer that is down.
#[test]
fn default_new_pane_from_overlay_is_tiled_and_visible() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let overlay_pane_id = PaneId::Terminal(2);
    let new_pane_id = PaneId::Terminal(3);

    tab.new_pane(
        overlay_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
            FloatingPaneCoordinates {
                ..Default::default()
            },
        ))),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();

    assert!(
        !tab.floating_panes.all_panes_are_visible(),
        "a modal must not put the tab into 'floating panes visible'"
    );
    assert!(tab.tiled_panes.panes_contain(&new_pane_id));
    assert!(!tab.floating_panes.panes_contain(&new_pane_id));
    assert!(
        tab.overlay_panes
            .rendered_pane_ids()
            .contains(&overlay_pane_id),
        "the overlay is still drawn over it"
    );
    assert_eq!(
        tab.get_active_pane_id(client_id),
        Some(new_pane_id),
        "the pane the user just asked for takes the keyboard from the modal"
    );
}

/// A UI-covering fullscreen tiled pane used to hide overlays, and focus followed - which left a
/// modal focused and invisible, with no way out: the thing holding the keyboard was the thing that
/// could not be seen. An overlay now outranks fullscreen, as it outranks a pinned pane.
#[test]
fn an_overlay_outranks_a_ui_covering_fullscreen_tiled_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let overlay_pane_id = PaneId::Terminal(2);

    tab.toggle_active_pane_no_ui_fullscreen(client_id);
    tab.new_pane(
        overlay_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::modal(Some(FloatingPaneCoordinates {
            ..Default::default()
        }))),
        Some(client_id),
        None,
    )
    .unwrap();

    assert!(tab.fullscreen_covers_ui());
    assert!(tab.overlays_are_rendered());
    assert_eq!(tab.overlay_panes.rendered_pane_ids(), vec![overlay_pane_id]);
    assert_eq!(tab.get_active_pane_id(client_id), Some(overlay_pane_id));

    tab.toggle_active_pane_no_ui_fullscreen(client_id);

    assert!(
        tab.tiled_panes.fullscreen_covers_ui(),
        "the fullscreen keystroke was about the overlay - a modal, which cannot be expanded - so \
         the tiled pane behind it is left exactly as it was"
    );
    assert!(tab.overlays_are_rendered());
    assert_eq!(tab.get_active_pane_id(client_id), Some(overlay_pane_id));
}

/// The by-id fullscreen paths used to fall through to the tiled collection and log "no tiled pane
/// with id", which reads as a lookup failure rather than a refusal; then they refused. Now they
/// expand the overlay, and put it back on the second press - and the tiled layout, which the
/// keystroke was never about, is untouched throughout.
#[test]
fn an_overlay_is_expanded_over_the_screen_by_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let overlay_pane_id = PaneId::Terminal(2);

    tab.new_pane(
        overlay_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
        Some(client_id),
        None,
    )
    .unwrap();

    let before = tab
        .overlay_panes
        .get(&overlay_pane_id)
        .unwrap()
        .position_and_size();

    tab.toggle_pane_fullscreen(overlay_pane_id);
    assert!(
        tab.overlay_panes.pane_is_fullscreen(&overlay_pane_id),
        "the overlay is the thing the keystroke was about"
    );
    assert!(
        !tab.is_fullscreen_active(),
        "and the tab's own layout is not collapsed to one pane by it"
    );
    let viewport = *tab.viewport.borrow();
    let expanded = tab
        .overlay_panes
        .get(&overlay_pane_id)
        .unwrap()
        .current_geom();
    assert_eq!(
        (
            expanded.x,
            expanded.y,
            expanded.cols.as_usize(),
            expanded.rows.as_usize()
        ),
        (viewport.x, viewport.y, viewport.cols, viewport.rows),
        "it fills the viewport - whatever the chrome leaves, and no more"
    );

    tab.toggle_pane_no_ui_fullscreen(overlay_pane_id);
    assert!(
        tab.overlay_panes.fullscreen_covers_ui(),
        "the other keystroke switches to covering the chrome rather than collapsing"
    );

    tab.toggle_pane_no_ui_fullscreen(overlay_pane_id);
    assert!(!tab.overlay_panes.fullscreen_is_active());
    assert_eq!(
        tab.overlay_panes
            .get(&overlay_pane_id)
            .unwrap()
            .position_and_size(),
        before,
        "and it comes back to the rectangle its coordinates always said"
    );

    assert_eq!(tab.get_active_pane_id(client_id), Some(overlay_pane_id));
}

/// The active-pane paths never consulted `get_active_pane_id`, so with an overlay focused the
/// keystroke fullscreened the tiled pane *behind* it - a pane the user was not looking at.
#[test]
fn a_fullscreen_keystroke_on_an_overlay_leaves_the_pane_behind_it_alone() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let overlay_pane_id = PaneId::Terminal(2);

    tab.new_pane(
        overlay_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::modal(None)),
        Some(client_id),
        None,
    )
    .unwrap();

    tab.toggle_active_pane_fullscreen(client_id);
    assert!(
        !tab.is_fullscreen_active(),
        "the tiled pane behind the modal must not be fullscreened by a keystroke aimed at it"
    );

    tab.toggle_active_pane_no_ui_fullscreen(client_id);
    assert!(!tab.is_fullscreen_active());
}

/// A modal that goes into the suppressed panes and comes back out lands in `overlay_panes`,
/// because `add_floating_pane` redirects it there by configuration. The focus call that follows
/// must not name `floating_panes`: that collection does not hold the pane, and `ActivePanes` and
/// `z_indices` take an id without checking, so the entry would be a phantom that outlives the
/// overlay - leaving `has_active_panes()` true over an empty collection, which is what decides
/// whether a fullscreen keystroke is routed to the floating layer.
#[test]
fn a_suppressed_modal_coming_back_out_leaves_no_phantom_in_the_floating_collection() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    // a plugin pane needs somewhere to send to before it can be built
    let (plugin_sender, _plugin_receiver): ChannelWithContext<PluginInstruction> =
        channels::unbounded();
    tab.senders
        .replace_to_plugin(SenderWithContext::new(plugin_sender));
    let modal_pane_id = PaneId::Plugin(2);

    let mut modal = RunPlugin::from_url("zellij:quit-confirmation").expect("test plugin");
    modal.configuration.insert("modal", "true");
    let modal = Run::Plugin(RunPluginOrAlias::RunPlugin(modal));

    tab.new_pane(
        modal_pane_id,
        None,
        Some(modal),
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::modal(None)),
        Some(client_id),
        None,
    )
    .unwrap();
    assert!(tab.overlay_panes.panes_contain(&modal_pane_id));

    // the modal hides itself, as a plugin does with `hide_self()`
    tab.suppress_pane(modal_pane_id, Some(client_id));
    assert!(
        tab.suppressed_panes.contains_key(&modal_pane_id),
        "the modal is suppressed"
    );
    assert!(!tab.overlay_panes.panes_contain(&modal_pane_id));

    tab.focus_suppressed_pane_for_all_clients(modal_pane_id);

    assert!(
        tab.overlay_panes.panes_contain(&modal_pane_id),
        "the modal comes back out as an overlay"
    );
    assert_eq!(
        tab.overlay_panes.active_pane_id(client_id),
        Some(modal_pane_id),
        "and it is focused there"
    );
    assert!(
        !tab.floating_panes.has_active_panes(),
        "the floating collection holds no pane, so it must claim no active one either"
    );
    assert!(
        !tab.floating_panes.panes_contain(&modal_pane_id),
        "the modal is not a floating pane"
    );
}

#[test]
fn new_stacked_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.new_pane(
        new_pane_id,
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
fn floating_panes_persist_across_toggles() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    // here we send bytes to the pane when it's not visible to make sure they're still handled and
    // we see them once we toggle the panes back
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.toggle_floating_panes(Some(client_id), None, None)
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
fn toggle_floating_panes_off() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.toggle_floating_panes(Some(client_id), None, None)
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
fn toggle_floating_panes_on() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.toggle_floating_panes(Some(client_id), None, None)
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
fn five_new_floating_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(4, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(5, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(6, Vec::from("\u{1b}#8".as_bytes()))
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
fn increase_floating_pane_size() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
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
fn decrease_floating_pane_size() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.resize(client_id, ResizeStrategy::new(Resize::Decrease, None))
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
fn resize_floating_pane_left() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
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
fn resize_floating_pane_right() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
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
fn resize_floating_pane_up() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
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
fn resize_floating_pane_down() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
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
fn move_floating_pane_focus_left() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(4, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(5, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(6, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.move_focus_left(client_id).unwrap();
    tab.render(&mut output, None).unwrap();
    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((3, 3)),
        "cursor coordinates moved to the pane on the left"
    );

    assert_snapshot!(snapshot);
}

#[test]
fn move_floating_pane_focus_right() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(4, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(5, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(6, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.move_focus_left(client_id).unwrap();
    tab.move_focus_right(client_id).unwrap();
    tab.render(&mut output, None).unwrap();
    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((5, 5)),
        "cursor coordinates moved to the pane on the right"
    );

    assert_snapshot!(snapshot);
}

#[test]
fn move_floating_pane_focus_up() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(4, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(5, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(6, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.move_focus_up(client_id).unwrap();
    tab.render(&mut output, None).unwrap();
    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((3, 3)),
        "cursor coordinates moved to the pane above"
    );

    assert_snapshot!(snapshot);
}

#[test]
fn move_floating_pane_focus_down() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(4, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(5, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(6, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.move_focus_up(client_id).unwrap();
    tab.move_focus_down(client_id).unwrap();
    tab.render(&mut output, None).unwrap();
    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((5, 5)),
        "cursor coordinates moved to the pane below"
    );

    assert_snapshot!(snapshot);
}

#[test]
fn move_floating_pane_focus_with_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(4, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(5, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(6, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(9, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(9, 71)),
        client_id,
    )
    .unwrap();
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
        "cursor coordinates moved to the clicked pane"
    );

    assert_snapshot!(snapshot);
}

#[test]
fn move_floating_pane_focus_to_last_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(4, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(5, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(6, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.move_focus_up(client_id).unwrap();
    tab.focus_last_pane(client_id);
    tab.render(&mut output, None).unwrap();
    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((5, 5)),
        "cursor coordinates moved back to the last pane below"
    );

    assert_snapshot!(snapshot);
}

#[test]
fn move_pane_focus_with_mouse_to_non_floating_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(4, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(5, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(6, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(4, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(4, 71)),
        client_id,
    )
    .unwrap();
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
        "cursor coordinates moved to the clicked pane"
    );

    assert_snapshot!(snapshot);
}

#[test]
fn drag_pane_with_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(4, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(5, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(6, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(5, 71)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_motion_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(7, 75)),
        client_id,
    )
    .unwrap();
    tab.render(&mut output, None).unwrap();
    let (snapshot, _cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn mark_text_inside_floating_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(4, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(5, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(6, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_mouse_event(
        &MouseEvent::new_left_press_event(Position::new(6, 30)),
        client_id,
    )
    .unwrap();
    assert!(
        tab.selecting_with_mouse_in_pane.is_some(),
        "started selecting with mouse on click"
    );
    tab.handle_mouse_event(
        &MouseEvent::new_left_release_event(Position::new(5, 15)),
        client_id,
    )
    .unwrap();
    assert!(
        tab.selecting_with_mouse_in_pane.is_none(),
        "stopped selecting with mouse on release"
    );
    tab.render(&mut output, None).unwrap();
    let (snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_eq!(
        cursor_coordinates,
        Some((5, 5)),
        "cursor coordinates stayed in clicked pane"
    );

    assert_snapshot!(snapshot);
}

#[test]
fn resize_tab_with_floating_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(4, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(5, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(6, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.resize_whole_tab(Size {
        cols: 100,
        rows: 10,
    })
    .unwrap();
    tab.render(&mut output, None).unwrap();
    let (snapshot, _cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn shrink_whole_tab_with_floating_panes_horizontally_and_vertically() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(4, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(5, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(6, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.resize_whole_tab(Size { cols: 50, rows: 10 }).unwrap();
    tab.render(&mut output, None).unwrap();
    let (snapshot, _cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn shrink_whole_tab_with_floating_panes_horizontally_and_vertically_and_expand_back() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id_1 = PaneId::Terminal(2);
    let new_pane_id_2 = PaneId::Terminal(3);
    let new_pane_id_3 = PaneId::Terminal(4);
    let new_pane_id_4 = PaneId::Terminal(5);
    let new_pane_id_5 = PaneId::Terminal(6);
    let mut output = Output::default();
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
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(4, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(5, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(6, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.resize_whole_tab(Size { cols: 50, rows: 10 }).unwrap();
    tab.resize_whole_tab(Size {
        cols: 121,
        rows: 20,
    })
    .unwrap();
    tab.render(&mut output, None).unwrap();
    let (snapshot, _cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    assert_snapshot!(snapshot);
}

#[test]
fn embed_floating_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.toggle_pane_embed_or_floating(client_id).unwrap();
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
fn float_embedded_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am an embedded pane".as_bytes()),
    )
    .unwrap();
    tab.toggle_pane_embed_or_floating(client_id).unwrap();
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
fn embed_floating_pane_without_pane_frames() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.set_pane_frames(PaneFrameStyle::None);
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am scratch terminal".as_bytes()),
    )
    .unwrap();
    tab.toggle_pane_embed_or_floating(client_id).unwrap();
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
fn float_embedded_pane_without_pane_frames() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(2);
    let mut output = Output::default();
    tab.set_pane_frames(PaneFrameStyle::None);
    tab.new_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am an embedded pane".as_bytes()),
    )
    .unwrap();
    tab.toggle_pane_embed_or_floating(client_id).unwrap();
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
fn floating_fullscreen_full_frame_style() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = setup_tab_with_focused_floating_pane(size, client_id, PaneFrameStyle::Full);
    let mut output = Output::default();
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

#[test]
fn floating_fullscreen_titles_frame_style() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = setup_tab_with_focused_floating_pane(size, client_id, PaneFrameStyle::Titles);
    let mut output = Output::default();
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

#[test]
fn floating_fullscreen_none_frame_style() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = setup_tab_with_focused_floating_pane(size, client_id, PaneFrameStyle::None);
    let mut output = Output::default();
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

#[test]
fn changing_frame_style_while_floating_fullscreen_reflows_content() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = setup_tab_with_focused_floating_pane(size, client_id, PaneFrameStyle::Full);
    let mut output = Output::default();
    tab.toggle_active_pane_fullscreen(client_id);
    tab.set_pane_frames(PaneFrameStyle::None);
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
fn exiting_floating_fullscreen_clears_background() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.handle_pty_bytes(1, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        PaneId::Terminal(2),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(
        2,
        Vec::from("\n\n\n                   I am a floating pane".as_bytes()),
    )
    .unwrap();
    let mut output = Output::default();
    tab.toggle_active_pane_fullscreen(client_id);
    tab.render(&mut output, None).unwrap();
    let mut output = Output::default();
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

#[test]
fn floating_no_ui_fullscreen_covers_tab_and_status_bar() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = setup_tab_with_focused_floating_pane(size, client_id, PaneFrameStyle::Full);
    let mut output = Output::default();
    tab.toggle_active_pane_no_ui_fullscreen(client_id);
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
fn floating_fullscreen_hides_other_floating_and_pinned_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        PaneId::Terminal(2),
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
        PaneId::Terminal(3),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.set_floating_pane_pinned(PaneId::Terminal(3), true);
    tab.handle_pty_bytes(2, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    tab.handle_pty_bytes(3, Vec::from("\u{1b}#8".as_bytes()))
        .unwrap();
    let _ = tab.focus_pane_with_id(PaneId::Terminal(2), false, false, client_id);
    let mut output = Output::default();
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

// ---------------------------------------------------------------------------
// The floating collection's counts, and what an overlay does to them.
//
// These reach into `tab.floating_panes` directly, which the screen-level tests
// in `unit/screen_tests/overlays.rs` cannot do. They were written ahead of the
// modals/overlays refactor to pin the places where the floating collection
// counted overlays as its own, and flipped by it.
// ---------------------------------------------------------------------------

/// An overlay is not on the floating layer, so no count taken off that collection includes it.
///
/// It used to, because an overlay *was* a floating pane. `visible_panes_count` is one such count
/// and it gates the auto-relayout in `Tab::extract_pane`: with only an overlay left, the count
/// was 1 and a relayout ran over a layer that had nothing on it.
#[test]
fn an_overlay_is_not_counted_by_the_floating_collection() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, ModeInfo::default());
    let overlay_pane_id = PaneId::Terminal(2);

    tab.new_overlay_pane(
        overlay_pane_id,
        None,
        None,
        false,
        true,
        OverlayPlacement::dismissible(Some(FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(10)),
            y: Some(PercentOrFixed::Fixed(2)),
            width: Some(PercentOrFixed::Fixed(20)),
            height: Some(PercentOrFixed::Fixed(8)),
            ..Default::default()
        })),
        None,
    )
    .unwrap();

    assert!(tab.pane_is_overlay(overlay_pane_id));
    assert_eq!(
        tab.floating_panes.visible_panes_count(),
        0,
        "the overlay is not a floating pane"
    );
    assert!(
        !tab.floating_panes.has_panes(),
        "and it does not make the floating collection look non-empty"
    );
}

/// Closing the last *real* floating pane while an overlay is up takes the floating layer down -
/// there is nothing on it any more.
///
/// It used to leave the layer flagged visible with nothing to show: `close_pane` only takes it
/// down when the floating collection has no selectable panes left, and the overlay - being one of
/// them - kept that false.
#[test]
fn closing_the_last_floating_pane_under_an_overlay_takes_the_layer_down() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, ModeInfo::default());
    let floating_pane_id = PaneId::Terminal(2);
    let overlay_pane_id = PaneId::Terminal(3);

    tab.new_floating_pane(floating_pane_id, None, None, false, true, None, None)
        .unwrap();
    tab.new_overlay_pane(
        overlay_pane_id,
        None,
        None,
        false,
        true,
        OverlayPlacement::dismissible(Some(FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(10)),
            y: Some(PercentOrFixed::Fixed(2)),
            width: Some(PercentOrFixed::Fixed(20)),
            height: Some(PercentOrFixed::Fixed(8)),
            ..Default::default()
        })),
        None,
    )
    .unwrap();
    assert!(tab.floating_panes.panes_are_visible());

    tab.close_pane(floating_pane_id, false, None);

    assert!(
        !tab.floating_panes.panes_are_visible(),
        "the overlay does not keep the layer flagged visible"
    );
}

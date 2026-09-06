use super::*;
use crate::os_input_output::{AsyncReader, ServerOsApi};
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::sixel::SixelImageStore;
use crate::panes::{LinkHandler, PaneId, TerminalPane};
use crate::tab::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};
use interprocess::local_socket::Stream as LocalSocketStream;
use std::path::PathBuf;
use zellij_types::internal_ids::TerminalId;
use zellij_types::style::Palette;
use zellij_utils::data::{Direction, Resize};
use zellij_utils::input::command::{RunCommand, TerminalAction};
use zellij_utils::ipc::{ClientToServerMsg, IpcReceiverWithContext, ServerToClientMsg};

#[derive(Clone)]
struct FakeInputOutput {}

impl ServerOsApi for FakeInputOutput {
    fn set_terminal_size_using_terminal_id(
        &self,
        _id: TerminalId,
        _cols: u16,
        _rows: u16,
        _width_in_pixels: Option<u16>,
        _height_in_pixels: Option<u16>,
    ) -> Result<()> {
        Ok(())
    }
    fn spawn_terminal(
        &self,
        _file_to_open: TerminalAction,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        _default_editor: Option<PathBuf>,
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<u32>)> {
        unimplemented!()
    }
    fn write_to_tty_stdin(&self, _id: TerminalId, _buf: &[u8]) -> Result<usize> {
        unimplemented!()
    }
    fn tcdrain(&self, _id: TerminalId) -> Result<()> {
        unimplemented!()
    }
    fn kill(&self, _pid: u32) -> Result<()> {
        unimplemented!()
    }
    fn force_kill(&self, _pid: u32) -> Result<()> {
        unimplemented!()
    }
    fn box_clone(&self) -> Box<dyn ServerOsApi> {
        Box::new((*self).clone())
    }
    fn send_to_client(&self, _client_id: ClientId, _msg: ServerToClientMsg) -> Result<()> {
        unimplemented!()
    }
    fn new_client(
        &mut self,
        _client_id: ClientId,
        _stream: LocalSocketStream,
    ) -> Result<IpcReceiverWithContext<ClientToServerMsg>> {
        unimplemented!()
    }
    fn remove_client(&mut self, _client_id: ClientId) -> Result<()> {
        unimplemented!()
    }
    fn load_palette(&self) -> Palette {
        unimplemented!()
    }
    fn get_cwd(&self, _pid: u32) -> Option<PathBuf> {
        unimplemented!()
    }
    fn write_to_file(&mut self, _buf: String, _name: Option<String>) -> Result<()> {
        unimplemented!()
    }
    fn re_run_command_in_terminal(
        &self,
        _terminal_id: TerminalId,
        _run_command: RunCommand,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<u32>)> {
        unimplemented!()
    }
    fn clear_terminal_id(&self, _terminal_id: TerminalId) -> Result<()> {
        unimplemented!()
    }
    fn send_sigint(&self, _pid: u32) -> Result<()> {
        unimplemented!()
    }
    fn send_sigcont(&self, _pid: u32) -> Result<()> {
        unimplemented!()
    }
}

/// A real `TerminalPane` rather than a mock: `OverlayPanes` asks a pane for its selectability,
/// borderlessness and content offset, and the two mocks already in this crate
/// (`tiled_panes/unit/mock_pane.rs` and the one inside `stacked_panes_tests.rs`) are
/// geometry-only and panic on all three.
fn mock_pane(pane_id: PaneId) -> Box<dyn Pane> {
    let PaneId::Terminal(pid) = pane_id else {
        unreachable!("overlay test panes are terminals")
    };
    Box::new(TerminalPane::new(
        pid,
        PaneGeom::default(),
        Style::default(),
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        None,
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    ))
}

fn coordinates(width: &str, height: &str) -> FloatingPaneCoordinates {
    FloatingPaneCoordinates::new(
        None,
        None,
        Some(width.to_owned()),
        Some(height.to_owned()),
        None,
        None,
    )
    .unwrap()
}

fn borderless_coordinates(borderless: bool) -> FloatingPaneCoordinates {
    FloatingPaneCoordinates::new(
        None,
        None,
        Some("60%".to_owned()),
        Some("50%".to_owned()),
        None,
        Some(borderless),
    )
    .unwrap()
}

/// A collection over a `cols` x `rows` surface with no clients.
fn overlay_panes(cols: usize, rows: usize) -> OverlayPanes {
    overlay_panes_with(
        cols,
        rows,
        Viewport {
            x: 0,
            y: 0,
            cols,
            rows,
        },
    )
    .0
}

/// The collection plus the shared `display_area` cell, so a test can grow the surface under it.
/// `tab_viewport` is the tab's own viewport, which an overlay is free to paint outside of.
fn overlay_panes_with(
    cols: usize,
    rows: usize,
    tab_viewport: Viewport,
) -> (OverlayPanes, Rc<RefCell<Size>>) {
    let display_area = Rc::new(RefCell::new(Size { cols, rows }));
    let viewport = Rc::new(RefCell::new(tab_viewport));
    let panes = OverlayPanes::new(
        display_area.clone(),
        viewport,
        Rc::new(RefCell::new(HashSet::new())),
        Rc::new(RefCell::new(HashSet::new())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(None)),
        PaneFrameStyle::default(),
        false,
        ModeInfo::default(),
        Style::default(),
        Box::new(FakeInputOutput {}),
        ThreadSenders::default().silently_fail_on_send(),
    );
    (panes, display_area)
}

fn add(panes: &mut OverlayPanes, id: u32, kind: OverlayKind, coords: FloatingPaneCoordinates) {
    let pane_id = PaneId::Terminal(crate::TerminalId(id));
    panes.add_pane(
        pane_id,
        mock_pane(pane_id),
        kind,
        coords,
        OverlayCarry::default(),
    );
}

#[test]
fn an_overlay_resolves_its_coordinates_against_the_session_surface() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    let pane = panes.get(&PaneId::Terminal(crate::TerminalId(1))).unwrap();
    assert_eq!(pane.position_and_size().cols.as_usize(), 48);
    assert_eq!(pane.position_and_size().rows.as_usize(), 10);
}

#[test]
fn reapplying_coordinates_re_resolves_against_the_surface_as_it_is_now() {
    let (mut panes, display_area) = overlay_panes_with(
        80,
        20,
        Viewport {
            x: 0,
            y: 0,
            cols: 80,
            rows: 20,
        },
    );
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    assert_eq!(
        panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        48
    );

    // the terminal grows 80 -> 120; a 60%-wide overlay is 60% of what the screen is *now*
    *display_area.borrow_mut() = Size {
        cols: 120,
        rows: 20,
    };
    panes.reapply_coordinates();
    assert_eq!(
        panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        72,
        "a 60%-wide overlay follows the surface it was placed on"
    );
}

#[test]
fn changing_an_overlay_s_coordinates_keeps_it_an_overlay_of_the_same_kind() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));
    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId(1)),
            coordinates("30%", "25%"),
            false,
        )
        .unwrap();
    assert_eq!(
        panes.kind_of(&PaneId::Terminal(crate::TerminalId(1))),
        Some(OverlayKind::Modal),
        "a coordinate change must not be able to demote a modal"
    );
    assert!(panes.is_modal(&PaneId::Terminal(crate::TerminalId(1))));
    assert_eq!(
        panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        24
    );
}

#[test]
fn the_occlusion_stack_is_add_order_and_agrees_with_every_pane_s_z_index() {
    let mut panes = overlay_panes(80, 20);
    for id in 1..=3 {
        add(
            &mut panes,
            id,
            OverlayKind::Dismissible,
            coordinates("50%", "50%"),
        );
    }
    let stack = panes.stack_layer_ids();
    assert_eq!(
        stack,
        vec![
            PaneId::Terminal(crate::TerminalId(1)),
            PaneId::Terminal(crate::TerminalId(2)),
            PaneId::Terminal(crate::TerminalId(3))
        ]
    );
    // the same property `Tab::assert_invariants` checks for the floating collection: a pane given
    // an index that is not its own is clipped against its own geometry and erases itself
    for (index, pane_id) in stack.iter().enumerate() {
        assert_eq!(panes.get_pane_z_index(*pane_id), Some(index));
    }
    assert_eq!(panes.layers().len(), 3);
}

#[test]
fn an_overlay_is_always_in_the_stack_there_being_no_layer_to_lower() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    // no show/hide to toggle: existence is visibility
    assert_eq!(
        panes.rendered_pane_ids(),
        vec![PaneId::Terminal(crate::TerminalId(1))]
    );
}

#[test]
fn removing_a_pane_drops_it_from_the_stack_as_well_as_the_map() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(
        &mut panes,
        2,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    assert!(panes
        .remove_pane(PaneId::Terminal(crate::TerminalId(1)))
        .is_some());
    assert!(!panes.panes_contain(&PaneId::Terminal(crate::TerminalId(1))));
    assert_eq!(
        panes.stack_layer_ids(),
        vec![PaneId::Terminal(crate::TerminalId(2))]
    );
    assert_eq!(
        panes.get_pane_z_index(PaneId::Terminal(crate::TerminalId(2))),
        Some(0)
    );
}

#[test]
fn replacing_a_pane_keeps_its_slot_its_kind_and_its_coordinates() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));
    add(
        &mut panes,
        2,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    // a second modal above the one being replaced, so that its slot is not the top of the stack
    // and "kept its slot" is distinguishable from "was pushed on top"
    add(&mut panes, 3, OverlayKind::Modal, coordinates("40%", "30%"));

    let replacement = PaneId::Terminal(crate::TerminalId(9));
    panes
        .replace_pane(
            PaneId::Terminal(crate::TerminalId(1)),
            mock_pane(replacement),
        )
        .unwrap();

    assert_eq!(
        panes.stack_layer_ids(),
        vec![
            PaneId::Terminal(crate::TerminalId(2)),
            replacement,
            PaneId::Terminal(crate::TerminalId(3))
        ],
        "the replacement takes the slot rather than being pushed on top"
    );
    assert_eq!(panes.kind_of(&replacement), Some(OverlayKind::Modal));

    // the carried-over coordinates are what a later reapply resolves against
    panes.reapply_coordinates();
    assert_eq!(
        panes
            .get(&replacement)
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        48
    );
}

#[test]
fn a_client_keeps_its_overlay_focused_with_no_visibility_precondition() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(1)), crate::ClientId(1));

    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(1)))
    );
    assert!(panes.has_active_panes());
}

#[test]
fn closing_the_focused_overlay_falls_through_to_the_one_below_it() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(
        &mut panes,
        2,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(2)), crate::ClientId(1));

    panes.move_clients_out_of_pane(PaneId::Terminal(crate::TerminalId(2)));
    panes.remove_pane(PaneId::Terminal(crate::TerminalId(2)));
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(1)))
    );

    panes.move_clients_out_of_pane(PaneId::Terminal(crate::TerminalId(1)));
    panes.remove_pane(PaneId::Terminal(crate::TerminalId(1)));
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        None,
        "the last overlay leaves nothing behind to focus"
    );
}

#[test]
fn an_overlay_reports_the_cells_it_owns_outside_the_tab_s_viewport() {
    // the tab's viewport is inset by a row of chrome top and bottom; an overlay across the whole
    // surface therefore paints cells the tab's own viewport does not cover
    let (mut panes, _display_area) = overlay_panes_with(
        80,
        20,
        Viewport {
            x: 0,
            y: 1,
            cols: 80,
            rows: 18,
        },
    );
    assert!(!panes.has_panes_outside_viewport());

    add(
        &mut panes,
        1,
        OverlayKind::Modal,
        coordinates("100%", "100%"),
    );
    assert!(panes.has_panes_outside_viewport());
    assert!(panes
        .geoms_outside_viewport()
        .contains_key(&PaneId::Terminal(crate::TerminalId(1))));
}

#[test]
fn an_overlay_asked_for_borderless_is_born_without_a_frame() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        borderless_coordinates(true),
    );
    let pane = panes.get(&PaneId::Terminal(crate::TerminalId(1))).unwrap();
    assert!(
        pane.borderless(),
        "the coordinate was asked for and honoured"
    );
    assert_eq!(
        pane.get_content_offset(),
        Offset::default(),
        "a frameless overlay gives its whole geometry to its contents"
    );
}

#[test]
fn an_overlay_that_said_nothing_about_borders_gets_a_frame() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    let pane = panes.get(&PaneId::Terminal(crate::TerminalId(1))).unwrap();
    assert!(!pane.borderless());
    assert_eq!(pane.get_content_offset(), Offset::frame(1));
}

#[test]
fn setting_an_overlay_borderless_travels_with_its_placement() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));

    assert!(panes.set_borderless(&PaneId::Terminal(crate::TerminalId(1)), true));
    let pane = panes.get(&PaneId::Terminal(crate::TerminalId(1))).unwrap();
    assert!(pane.borderless());
    assert_eq!(pane.get_content_offset(), Offset::default());
    assert_eq!(
        panes
            .placement_of(&PaneId::Terminal(crate::TerminalId(1)))
            .and_then(|placement| placement.coordinates)
            .and_then(|coordinates| coordinates.borderless),
        Some(true),
        "the placement is what follows the pane to another tab"
    );

    assert!(panes.toggle_borderless(&PaneId::Terminal(crate::TerminalId(1))));
    let pane = panes.get(&PaneId::Terminal(crate::TerminalId(1))).unwrap();
    assert!(!pane.borderless());
    assert_eq!(pane.get_content_offset(), Offset::frame(1));

    assert!(
        !panes.set_borderless(&PaneId::Terminal(crate::TerminalId(7)), true),
        "a pane that is not here is reported as not here, so the caller can look elsewhere"
    );
}

#[test]
fn a_borderless_overlay_stays_borderless_when_its_pane_is_replaced() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Modal,
        borderless_coordinates(true),
    );

    let replacement = PaneId::Terminal(crate::TerminalId(9));
    panes
        .replace_pane(
            PaneId::Terminal(crate::TerminalId(1)),
            mock_pane(replacement),
        )
        .unwrap();

    let pane = panes.get(&replacement).unwrap();
    assert!(pane.borderless());
    assert_eq!(pane.get_content_offset(), Offset::default());
}

#[test]
fn a_rounded_corners_update_reaches_the_style_the_overlay_frames_are_drawn_with() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));
    assert!(
        !panes.style.rounded_corners,
        "sanity: the collection starts with square corners"
    );

    panes.update_pane_rounded_corners(true);

    // `render` draws every overlay frame with `self.style`, so a pane-only update would leave
    // the frames square while the rest of the session went round.
    assert!(panes.style.rounded_corners);
}

#[test]
fn an_overlay_opened_over_a_live_modal_neither_takes_focus_nor_covers_it() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(1)), crate::ClientId(1));

    add(
        &mut panes,
        2,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );

    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(1))),
        "the modal keeps the keyboard - it has no way to learn it lost it"
    );
    assert_eq!(
        panes.stack_layer_ids(),
        vec![
            PaneId::Terminal(crate::TerminalId(2)),
            PaneId::Terminal(crate::TerminalId(1))
        ],
        "and stays drawn over the overlay that opened after it, rather than under it"
    );
}

#[test]
fn focusing_an_overlay_under_a_live_modal_leaves_the_keyboard_where_it_is() {
    // the mouse-click and focus-by-id door onto the same rule: the overlay was added first, so
    // this is not the add path declining but `focus_pane` itself
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(&mut panes, 2, OverlayKind::Modal, coordinates("60%", "50%"));
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(2)), crate::ClientId(1));

    panes.focus_pane(PaneId::Terminal(crate::TerminalId(1)), crate::ClientId(1));
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(2)))
    );
    assert_eq!(
        panes.stack_layer_ids(),
        vec![
            PaneId::Terminal(crate::TerminalId(1)),
            PaneId::Terminal(crate::TerminalId(2))
        ],
        "a declined focus raises nothing either"
    );

    panes.focus_pane_for_all_clients(PaneId::Terminal(crate::TerminalId(1)));
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(2)))
    );

    assert!(panes
        .focus_pane_if_exists(PaneId::Terminal(crate::TerminalId(1)), crate::ClientId(1))
        .is_ok());
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(2))),
        "`focus_pane_if_exists` reports the pane is ours even when focus was declined - an Err \
         would send `Tab::focus_pane_with_id` on to focus a floating pane instead"
    );
}

#[test]
fn a_modal_opened_over_a_modal_takes_focus_and_gives_it_back_when_it_closes() {
    // the one thing that may outrank a live modal, and the reason it may: closing it walks focus
    // back down to the modal underneath, so a quit-confirmation over an open modal stays usable
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(1)), crate::ClientId(1));

    add(&mut panes, 2, OverlayKind::Modal, coordinates("40%", "30%"));
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(2)), crate::ClientId(1));
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(2)))
    );
    assert_eq!(
        panes.stack_layer_ids(),
        vec![
            PaneId::Terminal(crate::TerminalId(1)),
            PaneId::Terminal(crate::TerminalId(2))
        ]
    );

    panes.move_clients_out_of_pane(PaneId::Terminal(crate::TerminalId(2)));
    panes.remove_pane(PaneId::Terminal(crate::TerminalId(2)));
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(1)))
    );
}

#[test]
fn a_modal_is_the_top_of_the_stack_and_not_merely_the_thing_that_outranks_it() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(&mut panes, 2, OverlayKind::Modal, coordinates("60%", "50%"));
    add(
        &mut panes,
        3,
        OverlayKind::Dismissible,
        coordinates("30%", "30%"),
    );
    add(&mut panes, 4, OverlayKind::Modal, coordinates("40%", "30%"));
    add(
        &mut panes,
        5,
        OverlayKind::Dismissible,
        coordinates("20%", "20%"),
    );

    assert_eq!(
        panes.stack_layer_ids(),
        vec![
            PaneId::Terminal(crate::TerminalId(1)),
            PaneId::Terminal(crate::TerminalId(3)),
            PaneId::Terminal(crate::TerminalId(5)),
            PaneId::Terminal(crate::TerminalId(2)),
            PaneId::Terminal(crate::TerminalId(4)),
        ],
        "modals above non-modals, add order within each class"
    );
    assert_eq!(
        panes.topmost_modal_pane_id(),
        Some(PaneId::Terminal(crate::TerminalId(4)))
    );
    for (index, pane_id) in panes.stack_layer_ids().iter().enumerate() {
        assert_eq!(panes.get_pane_z_index(*pane_id), Some(index));
    }
}

// ---- being dragged with the mouse -----------------------------------------------------------

fn placed_coordinates(x: &str, y: &str, width: &str, height: &str) -> FloatingPaneCoordinates {
    FloatingPaneCoordinates::new(
        Some(x.to_owned()),
        Some(y.to_owned()),
        Some(width.to_owned()),
        Some(height.to_owned()),
        None,
        None,
    )
    .unwrap()
}

/// The top-left cell of a pane is on its frame.
fn frame_corner(panes: &OverlayPanes, id: u32) -> Position {
    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId(id)))
        .unwrap()
        .position_and_size();
    Position::new(geom.y as i32, geom.x as u16)
}

fn xy(panes: &OverlayPanes, id: u32) -> (usize, usize) {
    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId(id)))
        .unwrap()
        .position_and_size();
    (geom.x, geom.y)
}

#[test]
fn a_frame_drag_moves_an_overlay_by_the_pointer_delta() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    let (start_x, start_y) = xy(&panes, 1);

    let press = frame_corner(&panes, 1);
    assert!(panes.move_pane_with_mouse(press), "the frame press is ours");
    assert!(panes.pane_is_being_moved_with_mouse());

    panes.move_pane_with_mouse(Position::new(
        press.line() as i32 + 2,
        press.column() as u16 + 4,
    ));
    assert_eq!(xy(&panes, 1), (start_x + 4, start_y + 2));

    let never_moved = panes.stop_moving_pane_with_mouse(Position::new(
        press.line() as i32 + 2,
        press.column() as u16 + 4,
    ));
    assert!(!never_moved, "the pointer left the cell it was pressed on");
    assert!(!panes.pane_is_being_moved_with_mouse());
}

/// The drag clamps against the *session surface*, so an overlay carried past the edge of the
/// screen stops at it rather than being lost off it.
#[test]
fn a_frame_drag_clamps_an_overlay_inside_the_session_surface() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    let press = frame_corner(&panes, 1);

    panes.move_pane_with_mouse(press);
    // far past the bottom-right corner of an 80x20 surface
    panes.move_pane_with_mouse(Position::new(400, 400));
    panes.stop_moving_pane_with_mouse(Position::new(400, 400));

    // a 48x10 overlay on an 80x20 surface can go no further than (32, 10)
    assert_eq!(xy(&panes, 1), (32, 10));

    let press = frame_corner(&panes, 1);
    panes.move_pane_with_mouse(press);
    panes.move_pane_with_mouse(Position::new(-400, 0));
    panes.stop_moving_pane_with_mouse(Position::new(-400, 0));
    assert_eq!(xy(&panes, 1), (0, 0), "and no further than the origin");
}

/// The write-back is what makes this work: the drag rewrites the overlay's *coordinates*, so the
/// position it was dropped at is re-resolved on the next surface change rather than discarded.
/// Without it the overlay re-centres the moment the terminal is resized.
#[test]
fn a_dragged_overlay_stays_where_it_was_dropped_when_the_surface_grows() {
    let (mut panes, display_area) = overlay_panes_with(
        80,
        20,
        Viewport {
            x: 0,
            y: 0,
            cols: 80,
            rows: 20,
        },
    );
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );

    let press = frame_corner(&panes, 1);
    let target = Position::new(press.line() as i32 + 2, press.column() as u16 + 4);
    panes.move_pane_with_mouse(press);
    panes.move_pane_with_mouse(target);
    panes.stop_moving_pane_with_mouse(target);
    let dropped_at = xy(&panes, 1);

    *display_area.borrow_mut() = Size {
        cols: 120,
        rows: 40,
    };
    panes.reapply_coordinates();

    assert_eq!(
        xy(&panes, 1),
        dropped_at,
        "the dragged position survives the resize"
    );
    let resized = panes
        .get(&PaneId::Terminal(crate::TerminalId(1)))
        .unwrap()
        .position_and_size();
    assert_eq!(
        (resized.cols.as_usize(), resized.rows.as_usize()),
        (72, 20),
        "but the size is still a percentage of the surface - only the position became the user's"
    );
}

/// `placement_of` is what an overlay travels on when it follows a client to another tab, so the
/// dragged position has to be visible there too.
#[test]
fn the_dragged_position_is_reported_by_placement_of() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));

    let press = frame_corner(&panes, 1);
    let target = Position::new(press.line() as i32 + 3, press.column() as u16 + 5);
    panes.move_pane_with_mouse(press);
    panes.move_pane_with_mouse(target);
    panes.stop_moving_pane_with_mouse(target);
    let (x, y) = xy(&panes, 1);

    let placement = panes
        .placement_of(&PaneId::Terminal(crate::TerminalId(1)))
        .unwrap();
    assert_eq!(placement.kind, OverlayKind::Modal, "still a modal");
    let coordinates = placement.coordinates.unwrap();
    assert_eq!(coordinates.x, Some(PercentOrFixed::Fixed(x)));
    assert_eq!(coordinates.y, Some(PercentOrFixed::Fixed(y)));
}

/// A press that never left its cell is a click, and the caller turns it back into a focus.
#[test]
fn a_press_that_never_moved_reports_never_moved() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    let before = xy(&panes, 1);

    let press = frame_corner(&panes, 1);
    panes.move_pane_with_mouse(press);
    assert!(panes.stop_moving_pane_with_mouse(press));
    assert_eq!(xy(&panes, 1), before, "and nothing moved");
}

/// The drag is the frame's. A press in the body is a press in a live terminal or plugin.
#[test]
fn a_press_inside_an_overlay_does_not_start_a_drag() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId(1)))
        .unwrap()
        .position_and_size();
    let inside = Position::new(geom.y as i32 + 3, geom.x as u16 + 10);

    assert!(!panes.move_pane_with_mouse(inside));
    assert!(!panes.pane_is_being_moved_with_mouse());
}

/// A borderless overlay has no frame to grab, exactly as a borderless floating pane has none.
#[test]
fn a_borderless_overlay_has_no_frame_to_be_dragged_by() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        borderless_coordinates(true),
    );
    let press = frame_corner(&panes, 1);

    assert!(!panes.move_pane_with_mouse(press));
    assert!(!panes.pane_is_being_moved_with_mouse());
}

/// A press outside every overlay is not the collection's, however far the drag then travels.
#[test]
fn a_press_that_is_not_on_an_overlay_is_not_ours() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        placed_coordinates("40", "10", "20", "5"),
    );

    assert!(!panes.move_pane_with_mouse(Position::new(0, 0)));
    assert!(!panes.pane_is_being_moved_with_mouse());
}

/// A popup can dismiss itself while its own frame is held down. A drag left pointing at a pane
/// that is gone would swallow every mouse event until a release arrived to clear it.
#[test]
fn closing_an_overlay_mid_drag_releases_the_mouse() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );

    let press = frame_corner(&panes, 1);
    panes.move_pane_with_mouse(press);
    assert!(panes.pane_is_being_moved_with_mouse(), "pre-condition");

    panes.remove_pane(PaneId::Terminal(crate::TerminalId(1)));
    assert!(!panes.pane_is_being_moved_with_mouse());
}

// ---- a dragged position is the user's ---------------------------------------------------------

/// What a plugin re-asserting its placement sends: a size, a `y`, and no `x` at all - the shape
/// `hocus-new-agent` sends on every `TabUpdate`.
fn plugin_placement(y: &str, width: &str, height: &str) -> FloatingPaneCoordinates {
    FloatingPaneCoordinates::new(
        None,
        Some(y.to_owned()),
        Some(width.to_owned()),
        Some(height.to_owned()),
        None,
        None,
    )
    .unwrap()
}

fn drag_by(panes: &mut OverlayPanes, id: u32, columns: u16, lines: i32) {
    let press = frame_corner(panes, id);
    let target = Position::new(press.line() as i32 + lines, press.column() as u16 + columns);
    panes.move_pane_with_mouse(press);
    panes.move_pane_with_mouse(target);
    panes.stop_moving_pane_with_mouse(target);
}

/// A drag reports session state, session state is a `TabUpdate`, and a plugin that re-places
/// itself on `TabUpdate` answers every step of its own drag by putting the overlay back. That is
/// what made the new-agent modal look immovable while the file finder moved.
#[test]
fn a_plugin_re_asserting_its_placement_does_not_undo_a_drag() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));

    drag_by(&mut panes, 1, 5, 3);
    let dropped_at = xy(&panes, 1);

    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId(1)),
            plugin_placement("20%", "60%", "10"),
            false,
        )
        .unwrap();

    assert_eq!(
        xy(&panes, 1),
        dropped_at,
        "the position the user dragged to outranks the one the plugin asks for"
    );
}

/// Only the *position* becomes the user's. A plugin whose dialog grew still grows the overlay -
/// in place.
#[test]
fn a_plugin_resize_after_a_drag_still_resizes() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));

    drag_by(&mut panes, 1, 5, 3);
    let dropped_at = xy(&panes, 1);

    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId(1)),
            plugin_placement("20%", "40", "12"),
            false,
        )
        .unwrap();

    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId(1)))
        .unwrap()
        .position_and_size();
    assert_eq!(
        (geom.cols.as_usize(), geom.rows.as_usize()),
        (40, 12),
        "the size the plugin asked for"
    );
    assert_eq!(xy(&panes, 1), dropped_at, "at the position the user chose");
}

// ---- unless the placement is the user's own -------------------------------------------------

/// A key the user pressed to move the overlay is as much theirs as the drag was, and later. The
/// plugin says so with `override_user_placement`, and the overlay goes where the key said.
#[test]
fn a_placement_the_user_asked_for_reclaims_a_dragged_position() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));

    drag_by(&mut panes, 1, 5, 3);
    let dropped_at = xy(&panes, 1);

    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId(1)),
            placed_coordinates("10", "5", "40", "10"),
            true,
        )
        .unwrap();

    assert_ne!(xy(&panes, 1), dropped_at, "the drag no longer wins");
    assert_eq!(xy(&panes, 1), (10, 5), "the position the key asked for");
}

/// Moving an overlay is not resizing it: the independent size claim remains the user's.
#[test]
fn a_placement_the_user_asked_for_keeps_a_resized_size() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );

    panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[grow(Direction::Right)],
            (5, 2),
        )
        .unwrap();
    let resized_to = size(&panes, 1);

    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId(1)),
            placed_coordinates("10", "5", "40", "10"),
            true,
        )
        .unwrap();

    assert_eq!(xy(&panes, 1), (10, 5), "the position the key asked for");
    assert_eq!(
        size(&panes, 1),
        resized_to,
        "the earlier resize remains independent of the move"
    );
}

/// Cleared, not bypassed once: after the key has moved the overlay, the plugin's ordinary
/// placements move it again - a resize of the terminal re-places it where the plugin says, not
/// back where the drag left it.
#[test]
fn after_an_override_the_next_plain_placement_moves_the_overlay_again() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));

    drag_by(&mut panes, 1, 5, 3);
    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId(1)),
            placed_coordinates("10", "5", "40", "10"),
            true,
        )
        .unwrap();

    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId(1)),
            placed_coordinates("30", "5", "40", "10"),
            false,
        )
        .unwrap();

    assert_eq!(
        xy(&panes, 1),
        (30, 5),
        "the plugin places the overlay again, the drag's claim having been released"
    );
}

/// The replace semantics are unchanged for an overlay nobody dragged: an axis the call does not
/// name is resolved from the surface default rather than from what happened to be there.
#[test]
fn an_overlay_nobody_dragged_is_still_re_placed_by_a_plugin() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Modal,
        placed_coordinates("10", "15", "60%", "10"),
    );
    assert_eq!(xy(&panes, 1), (10, 15), "pre-condition");

    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId(1)),
            plugin_placement("20%", "40", "10"),
            false,
        )
        .unwrap();

    // x unnamed => centred on the surface; y named => 20% of it
    assert_eq!(xy(&panes, 1), (20, 4));
}

/// A pane swapped into a dragged overlay's slot is the pane the user dragged, so the slot keeps
/// where they put it - as it already keeps its kind and its coordinates.
#[test]
fn a_replaced_pane_keeps_the_dragged_position() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));

    drag_by(&mut panes, 1, 5, 3);
    let dropped_at = xy(&panes, 1);

    let replacement = PaneId::Terminal(crate::TerminalId(9));
    panes
        .replace_pane(
            PaneId::Terminal(crate::TerminalId(1)),
            mock_pane(replacement),
        )
        .unwrap();
    panes
        .change_pane_coordinates(replacement, plugin_placement("20%", "60%", "10"), false)
        .unwrap();

    assert_eq!(xy(&panes, 9), dropped_at);
}

// ---- being resized ----------------------------------------------------------------------

fn grow(direction: Direction) -> ResizeStrategy {
    ResizeStrategy {
        resize: Resize::Increase,
        direction: Some(direction),
        invert_on_boundaries: false,
    }
}

fn shrink(direction: Direction) -> ResizeStrategy {
    ResizeStrategy {
        resize: Resize::Decrease,
        direction: Some(direction),
        invert_on_boundaries: false,
    }
}

/// The strategy ctrl+MouseScroll sends: all four borders out, `geom_resized_within` halving
/// the step for each.
fn grow_all() -> ResizeStrategy {
    ResizeStrategy {
        resize: Resize::Increase,
        direction: None,
        invert_on_boundaries: false,
    }
}

fn shrink_all() -> ResizeStrategy {
    ResizeStrategy {
        resize: Resize::Decrease,
        direction: None,
        invert_on_boundaries: false,
    }
}

fn size(panes: &OverlayPanes, id: u32) -> (usize, usize) {
    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId(id)))
        .unwrap()
        .position_and_size();
    (geom.cols.as_usize(), geom.rows.as_usize())
}

/// The resize has to reach the stored *coordinates*, not only the geometry: this collection keeps
/// no geometry of record beside the panes, so a size that lived only on the geom would be undone
/// by the next `reapply_coordinates`.
#[test]
fn a_resize_writes_the_new_size_back_into_the_overlay_s_coordinates() {
    let (mut panes, display_area) = overlay_panes_with(
        80,
        20,
        Viewport {
            x: 0,
            y: 0,
            cols: 80,
            rows: 20,
        },
    );
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    assert_eq!(size(&panes, 1), (48, 10));

    assert!(panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[grow(Direction::Right)],
            (5, 2)
        )
        .unwrap());
    assert_eq!(size(&panes, 1), (53, 10));

    // the terminal grows 80 -> 120. An untouched 60%-wide overlay would re-resolve to 72; this
    // one keeps the size the user gave it
    *display_area.borrow_mut() = Size {
        cols: 120,
        rows: 20,
    };
    panes.reapply_coordinates();
    assert_eq!(
        size(&panes, 1),
        (53, 10),
        "the resized size survives a re-resolution"
    );
}

/// A resize from the left or upper border moves the pane as well as sizing it, so that half has to
/// be written back too - otherwise the next re-resolution snaps the overlay back to where its
/// coordinates still say it is, and the resize looks like it slid sideways.
#[test]
fn a_resize_from_the_upper_left_pins_the_position_as_well_as_the_size() {
    let (mut panes, display_area) = overlay_panes_with(
        80,
        20,
        Viewport {
            x: 0,
            y: 0,
            cols: 80,
            rows: 20,
        },
    );
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    let (start_x, start_y) = xy(&panes, 1);

    panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[grow(Direction::Left), grow(Direction::Up)],
            (5, 2),
        )
        .unwrap();
    assert_eq!(xy(&panes, 1), (start_x - 5, start_y - 2));
    assert_eq!(size(&panes, 1), (53, 12));

    *display_area.borrow_mut() = Size {
        cols: 120,
        rows: 40,
    };
    panes.reapply_coordinates();
    assert_eq!(xy(&panes, 1), (start_x - 5, start_y - 2));
    assert_eq!(size(&panes, 1), (53, 12));
}

/// A modal is sized to the question it is asking. It is still draggable - only its size is the
/// plugin's.
#[test]
fn a_modal_refuses_to_be_resized() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));
    let before = panes
        .get(&PaneId::Terminal(crate::TerminalId(1)))
        .unwrap()
        .position_and_size();

    assert!(!panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[grow(Direction::Right)],
            (5, 2)
        )
        .unwrap());
    assert_eq!(
        panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size(),
        before
    );
    assert!(!panes.is_resizable(&PaneId::Terminal(crate::TerminalId(1))));
    assert!(
        !panes.is_resizable(&PaneId::Terminal(crate::TerminalId(7))),
        "not an overlay"
    );
}

/// The size twin of `a_plugin_re_asserting_its_placement_does_not_undo_a_drag`: a plugin that
/// re-places itself on every `TabUpdate` would otherwise answer each step of the resize by putting
/// the overlay back.
#[test]
fn a_plugin_re_asserting_its_placement_does_not_undo_a_resize() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );

    panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[grow(Direction::Right)],
            (5, 2),
        )
        .unwrap();
    let resized_to = size(&panes, 1);

    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId(1)),
            plugin_placement("20%", "60%", "10"),
            false,
        )
        .unwrap();

    assert_eq!(
        size(&panes, 1),
        resized_to,
        "the size the user chose outranks the one the plugin asks for"
    );
}

/// The two claims are independent: a resize does not hand the plugin's position to the user.
#[test]
fn a_resize_alone_leaves_the_position_the_plugin_s() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );

    panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[grow(Direction::Right)],
            (5, 2),
        )
        .unwrap();
    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId(1)),
            plugin_placement("50%", "60%", "10"),
            false,
        )
        .unwrap();

    assert_eq!(xy(&panes, 1).1, 10, "the plugin still places the overlay");
}

/// The resize clamps against the *session surface*, the same bound the drag clamps against, and
/// never below the minimum a pane can be drawn at.
#[test]
fn a_resize_is_clamped_to_the_surface_and_to_the_minimum_pane_size() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    let (start_x, _) = xy(&panes, 1);

    panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[grow(Direction::Right)],
            (400, 0),
        )
        .unwrap();
    assert_eq!(
        size(&panes, 1).0,
        80 - start_x,
        "no further than the right edge of the surface"
    );

    panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[shrink(Direction::Right), shrink(Direction::Down)],
            (400, 400),
        )
        .unwrap();
    assert_eq!(
        size(&panes, 1),
        (MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT),
        "and no smaller than a pane can be drawn"
    );
}

/// A resize that changes nothing is not a state change - the mouse pipeline asks this question on
/// every motion event of a drag.
#[test]
fn a_resize_that_changes_nothing_reports_so() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    // already as small as a pane can be drawn, and shrinking does not invert the way growing on a
    // boundary does
    panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[shrink(Direction::Right)],
            (400, 0),
        )
        .unwrap();
    assert!(!panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[shrink(Direction::Right)],
            (5, 0)
        )
        .unwrap());
}

/// Growing against the edge of the surface inverts into a shrink, exactly as it does for a
/// floating pane against the edge of the viewport - the arithmetic is literally the same function.
/// This is the keyboard strategy, which asks for the inversion.
#[test]
fn growing_an_overlay_that_is_already_on_the_boundary_inverts() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("100%", "50%"),
    );
    // 100% wide clamps to the surface, so the right border has nowhere left to go
    let (cols, _) = size(&panes, 1);
    panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[ResizeStrategy::new(
                Resize::Increase,
                Some(Direction::Right),
            )],
            (5, 0),
        )
        .unwrap();
    assert_eq!(size(&panes, 1).0, cols - 5);
}

/// A drag does *not* get that inversion: the mouse builds its strategies with
/// `invert_on_boundaries: false`, and a pointer pushing the right edge outward past the edge of the
/// surface means "no further", not "shrink from the left". Inverting here would slide the overlay
/// out from under the pointer that is pushing it.
#[test]
fn dragging_an_overlay_edge_outward_on_the_boundary_does_not_invert() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("100%", "50%"),
    );
    let before = size(&panes, 1);
    assert!(!panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[grow(Direction::Right)],
            (5, 0)
        )
        .unwrap());
    assert_eq!(size(&panes, 1), before);
}

/// The keyboard entry point resizes the overlay the client is on, and answers `false` rather than
/// falling through to another collection when the client is not on one.
#[test]
fn the_keyboard_path_resizes_the_client_s_own_overlay() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    assert!(
        !panes
            .resize_active_pane(crate::ClientId(1), &grow(Direction::Right))
            .unwrap(),
        "the client is not on an overlay"
    );

    panes.focus_pane(PaneId::Terminal(crate::TerminalId(1)), crate::ClientId(1));
    assert!(panes
        .resize_active_pane(crate::ClientId(1), &grow(Direction::Right))
        .unwrap());
    assert_eq!(size(&panes, 1), (53, 10));
}

// ---- per-pane suppression --------------------------------------------------------------------
//
// `hide_pane` sets one overlay aside without unloading it, so hocus's Esc can put the sidebar away
// and keep the conversation. What these pin is the line between the queries that must stop seeing
// a hidden overlay (everything that draws, occludes, hit-tests or focuses) and the ones that must
// keep seeing it (everything that identifies it, so a `LaunchOrFocusPlugin` can find it again).

#[test]
fn a_hidden_overlay_is_drawn_nowhere_but_is_still_here() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    assert!(panes.hide_pane(PaneId::Terminal(crate::TerminalId(1))));

    assert!(
        panes.stack_layer_ids().is_empty(),
        "out of the occlusion stack, which is what render, layers and hit-testing all read"
    );
    assert!(panes.layers().is_empty());
    assert!(panes.rendered_pane_ids().is_empty());
    assert!(
        !panes.has_panes(),
        "a collection holding only a hidden overlay draws nothing, so the tab must behave as if \
         it had no overlay at all"
    );
    assert!(
        !panes.is_empty(),
        "but it is not empty - the pane is still loaded, which is the whole point"
    );

    assert!(panes.panes_contain(&PaneId::Terminal(crate::TerminalId(1))));
    assert!(panes.get(&PaneId::Terminal(crate::TerminalId(1))).is_some());
    assert!(
        panes
            .placement_of(&PaneId::Terminal(crate::TerminalId(1)))
            .is_some(),
        "still an overlay, and still able to say how it is placed"
    );
    assert!(panes.is_hidden(&PaneId::Terminal(crate::TerminalId(1))));
}

#[test]
fn hiding_an_overlay_moves_its_clients_to_the_one_below_it() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(
        &mut panes,
        2,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(2)), crate::ClientId(1));

    panes.hide_pane(PaneId::Terminal(crate::TerminalId(2)));
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(1))),
        "focus walks down to the overlay still on screen"
    );

    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        None,
        "hiding the last one leaves nobody holding a keyboard nobody can see - this is what lets \
         every focus query stay unfiltered"
    );
}

#[test]
fn a_hidden_overlay_may_not_be_focused_until_it_is_shown() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));

    assert!(!panes.may_be_focused(&PaneId::Terminal(crate::TerminalId(1))));
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(1)), crate::ClientId(1));
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        None,
        "the focus was refused"
    );

    assert!(panes.show_pane(PaneId::Terminal(crate::TerminalId(1))));
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(1)), crate::ClientId(1));
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(1)))
    );
}

#[test]
fn focusing_a_hidden_overlay_by_id_reveals_it() {
    // the reveal path: `LaunchOrFocusPlugin` naming a hidden hocus is the user asking for it back,
    // and `Tab::focus_pane_with_id` routes through here
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));

    panes
        .focus_pane_if_exists(PaneId::Terminal(crate::TerminalId(1)), crate::ClientId(1))
        .expect("the overlay is ours");
    assert!(
        !panes.is_hidden(&PaneId::Terminal(crate::TerminalId(1))),
        "shown"
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(1))),
        "and focused - being shown without being focused would leave the user's request half done"
    );
}

#[test]
fn hiding_an_overlay_mid_drag_releases_the_mouse() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    let corner = frame_corner(&panes, 1);
    panes.set_pane_being_moved_with_mouse(PaneId::Terminal(crate::TerminalId(1)), corner);
    assert!(panes.pane_is_being_moved_with_mouse());

    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));
    assert!(
        !panes.pane_is_being_moved_with_mouse(),
        "a drag left pointing at a pane that is no longer painted would swallow every mouse event \
         until a release arrived"
    );
}

#[test]
fn a_hidden_overlay_does_not_take_the_mouse_or_the_cells_under_it() {
    let (mut panes, _display_area) = overlay_panes_with(
        90,
        20,
        Viewport {
            x: 0,
            y: 1,
            cols: 90,
            rows: 18,
        },
    );
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("100%", "100%"),
    );
    let inside = Position::new(5, 5);
    assert_eq!(
        panes.get_pane_id_at(&inside),
        Some(PaneId::Terminal(crate::TerminalId(1)))
    );
    assert!(panes.has_panes_outside_viewport());

    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));
    assert_eq!(
        panes.get_pane_id_at(&inside),
        None,
        "the click goes to whatever is underneath"
    );
    assert!(
        !panes.has_panes_outside_viewport(),
        "a hidden overlay owns no cells - counting one would leave the tab painting over the \
         chrome with nothing to paint"
    );
}

#[test]
fn a_hidden_overlay_reports_itself_suppressed_and_unfocused() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(1)), crate::ClientId(1));
    let visible = panes.pane_info(&HashMap::new());
    assert!(!visible[0].is_suppressed);

    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));
    let hidden = panes.pane_info(&HashMap::new());
    assert_eq!(
        hidden.len(),
        1,
        "still reported, so a plugin can see the state"
    );
    assert!(
        hidden[0].is_suppressed,
        "the one bit a plugin can read to tell it was hidden - hocus keys its reveal off this"
    );
    assert!(!hidden[0].is_focused);
}

#[test]
fn a_hidden_overlay_is_still_re_resolved_against_a_new_surface() {
    let (mut panes, display_area) = overlay_panes_with(
        80,
        20,
        Viewport {
            x: 0,
            y: 0,
            cols: 80,
            rows: 20,
        },
    );
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));

    *display_area.borrow_mut() = Size {
        rows: 40,
        cols: 160,
    };
    panes.reapply_coordinates();
    panes.show_pane(PaneId::Terminal(crate::TerminalId(1)));

    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId(1)))
        .unwrap()
        .position_and_size();
    assert_eq!(
        (geom.cols.as_usize(), geom.rows.as_usize()),
        (80, 20),
        "it comes back where its coordinates put it on the surface as it is now, not where it was \
         when it was hidden"
    );
}

#[test]
fn a_replaced_hidden_overlay_is_still_hidden() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));
    panes
        .replace_pane(
            PaneId::Terminal(crate::TerminalId(1)),
            mock_pane(PaneId::Terminal(crate::TerminalId(2))),
        )
        .expect("replaced");

    assert!(
        panes.is_hidden(&PaneId::Terminal(crate::TerminalId(2))),
        "the slot's hidden-ness travels with the slot, as its kind and coordinates already do - a \
         pane swapped into a hidden overlay is not suddenly on screen"
    );
    assert!(panes.stack_layer_ids().is_empty());
}

#[test]
fn a_modal_opened_while_an_overlay_is_hidden_still_outranks_it_when_it_comes_back() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));
    add(&mut panes, 2, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(2)), crate::ClientId(1));

    assert_eq!(
        panes.topmost_modal_pane_id(),
        Some(PaneId::Terminal(crate::TerminalId(2))),
        "a hidden overlay is not in the running for the top of the stack either way"
    );
    panes.show_pane(PaneId::Terminal(crate::TerminalId(1)));
    assert_eq!(
        *panes.stack_layer_ids().last().unwrap(),
        PaneId::Terminal(crate::TerminalId(2)),
        "coming back puts it at the top of its own class, which is still below the modal"
    );
    assert!(
        !panes.may_be_focused(&PaneId::Terminal(crate::TerminalId(1))),
        "and the modal keeps the keyboard"
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(2)))
    );
}

// ---- a modal that was set aside ----------------------------------------------------------------
//
// Suppression and modality pull against each other, and the seam between them is `stack_layer_ids`:
// the modal lock is read off it, and it filters `hidden`. So hiding a modal quietly lifts the lock
// and showing it quietly puts it back - which means neither statement can be only about drawing.
// A modal *is* its keyboard: it has no channel to learn it lost focus, so "on screen" and "holding
// the keyboard" have to move together for one, exactly as "not drawn" and "not focused" already do
// for every overlay.

#[test]
fn showing_a_hidden_modal_takes_the_keyboard_back() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(&mut panes, 2, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(2)), crate::ClientId(1));

    panes.hide_pane(PaneId::Terminal(crate::TerminalId(2)));
    panes.show_pane(PaneId::Terminal(crate::TerminalId(2)));

    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(2))),
        "the modal is back, and a modal that is back is a modal that is focused"
    );
    assert_eq!(
        panes.stack_layer_ids(),
        vec![
            PaneId::Terminal(crate::TerminalId(1)),
            PaneId::Terminal(crate::TerminalId(2))
        ],
        "and back at the top of the stack, above the dismissible it outranks"
    );
}

/// The bug this is all for. Before, the round trip left a dialog drawn over everything, holding
/// the lock that refuses every other overlay the keyboard, while the keyboard itself had fallen
/// through to whatever was underneath - so nobody could type into the modal and nothing else could
/// be focused past it.
#[test]
fn a_modal_put_back_locks_the_others_out_and_holds_the_keyboard_itself() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(&mut panes, 2, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(2)), crate::ClientId(1));

    panes.hide_pane(PaneId::Terminal(crate::TerminalId(2)));
    panes.show_pane(PaneId::Terminal(crate::TerminalId(2)));

    panes.focus_pane(PaneId::Terminal(crate::TerminalId(1)), crate::ClientId(1));
    assert!(
        !panes.may_be_focused(&PaneId::Terminal(crate::TerminalId(1))),
        "the lock is armed again - the modal is back in the stack"
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(2))),
        "and the pane holding that lock is the one holding the keyboard, rather than the two \
         having come apart"
    );
}

#[test]
fn hiding_a_modal_does_not_hand_the_keyboard_to_the_overlay_under_it() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(&mut panes, 2, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(2)), crate::ClientId(1));

    panes.hide_pane(PaneId::Terminal(crate::TerminalId(2)));

    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        None,
        "setting a modal aside is not the user withdrawing the refusal it was making - the \
         dismissible underneath was not focusable a moment ago and does not become so now"
    );
}

/// A modal *closing* does walk focus down (`move_clients_out_of_pane`), and a modal below is the
/// one thing it may walk down to. Hiding follows the same rule for the same reason.
#[test]
fn hiding_a_modal_over_a_modal_walks_the_keyboard_to_the_modal_below() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(&mut panes, 1, OverlayKind::Modal, coordinates("50%", "50%"));
    add(&mut panes, 2, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(2)), crate::ClientId(1));

    panes.hide_pane(PaneId::Terminal(crate::TerminalId(2)));

    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(1)))
    );
}

#[test]
fn showing_a_modal_gives_it_to_every_connected_client() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(2));
    add(&mut panes, 1, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.focus_pane_for_all_clients(PaneId::Terminal(crate::TerminalId(1)));

    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));
    panes.show_pane(PaneId::Terminal(crate::TerminalId(1)));

    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(1)))
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId(2)),
        Some(PaneId::Terminal(crate::TerminalId(1))),
        "a modal is focused for everybody, and coming back is no different from opening"
    );
}

/// The other half of the contract, and the one hocus depends on: showing an *ordinary* overlay is
/// still only about drawing it. hocus restores its agent list beside itself without wanting the
/// keyboard to leave the conversation.
#[test]
fn showing_a_hidden_dismissible_overlay_still_leaves_the_keyboard_alone() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(
        &mut panes,
        2,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.focus_pane(PaneId::Terminal(crate::TerminalId(1)), crate::ClientId(1));
    panes.hide_pane(PaneId::Terminal(crate::TerminalId(2)));

    panes.show_pane(PaneId::Terminal(crate::TerminalId(2)));

    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(1))),
        "the pane that was drawn again is not the pane that is focused"
    );
}

/// `focus_pane_if_exists` reveals through `show_pane`, so a modal now arrives focused twice over.
/// Its contract is unchanged: `Ok` still means "this overlay is mine", which is what keeps
/// `Tab::focus_pane_with_id`'s `.or_else` chain from going on to focus something else.
#[test]
fn focusing_a_hidden_modal_by_id_reveals_it_and_still_reports_it_is_ours() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId(1));
    add(&mut panes, 1, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));

    assert!(panes
        .focus_pane_if_exists(PaneId::Terminal(crate::TerminalId(1)), crate::ClientId(1))
        .is_ok());
    assert!(!panes.is_hidden(&PaneId::Terminal(crate::TerminalId(1))));
    assert_eq!(
        panes.active_pane_id(crate::ClientId(1)),
        Some(PaneId::Terminal(crate::TerminalId(1)))
    );
}

// ---- an overlay that is placed by its owner ---------------------------------------------------
//
// A launch that named no coordinates is born half the surface, *centered*. For hocus's pair of
// corner panels that is squarely on top of each other, and only their owner moves them out of
// there - a round trip later, once the pane is on a tab for the placement to find. The flag says
// "do not paint me until somebody has placed me"; see `OverlayCarry::awaiting_placement`.

fn add_awaiting_placement(panes: &mut OverlayPanes, id: u32) {
    let pane_id = PaneId::Terminal(crate::TerminalId(id));
    panes.add_pane(
        pane_id,
        mock_pane(pane_id),
        OverlayKind::Dismissible,
        FloatingPaneCoordinates::default(),
        OverlayCarry {
            awaiting_placement: true,
            ..Default::default()
        },
    );
}

#[test]
fn an_overlay_awaiting_placement_is_not_drawn() {
    let mut panes = overlay_panes(80, 20);
    add_awaiting_placement(&mut panes, 1);

    assert!(
        panes.rendered_pane_ids().is_empty(),
        "nothing is painted for it"
    );
    assert!(
        panes.layers().is_empty(),
        "and nothing is clipped against it either, or it would erase what is underneath"
    );
    assert!(
        panes.panes_contain(&PaneId::Terminal(crate::TerminalId(1))),
        "it exists all the same - this is not `hide_pane`"
    );
}

/// Not `hidden`, which is the user setting a pane aside and is reported as `is_suppressed`. A
/// plugin reads that field to tell that it was hidden or revealed, and a pane a frame old has been
/// neither.
#[test]
fn an_overlay_awaiting_placement_does_not_report_itself_hidden() {
    let mut panes = overlay_panes(80, 20);
    add_awaiting_placement(&mut panes, 1);

    assert!(!panes.is_hidden(&PaneId::Terminal(crate::TerminalId(1))));
    let info = panes.pane_info(&HashMap::new());
    assert_eq!(info.len(), 1);
    assert!(!info[0].is_suppressed);
}

#[test]
fn the_first_placement_is_what_draws_it() {
    let mut panes = overlay_panes(80, 20);
    add_awaiting_placement(&mut panes, 1);

    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId(1)),
            coordinates("60%", "50%"),
            false,
        )
        .unwrap();

    assert_eq!(
        panes.rendered_pane_ids(),
        vec![PaneId::Terminal(crate::TerminalId(1))]
    );
    assert_eq!(
        panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        48,
        "and it is drawn where it was placed, not where it was born"
    );
}

/// The safety valve. An overlay whose owner never places it would otherwise be invisible for as
/// long as it lived; a resize shows it where it was born, which is what it would have done without
/// the flag at all.
#[test]
fn a_resize_releases_an_overlay_nobody_ever_placed() {
    let (mut panes, display_area) = overlay_panes_with(
        80,
        20,
        Viewport {
            x: 0,
            y: 0,
            cols: 80,
            rows: 20,
        },
    );
    add_awaiting_placement(&mut panes, 1);
    assert!(panes.rendered_pane_ids().is_empty());

    *display_area.borrow_mut() = Size {
        cols: 100,
        rows: 30,
    };
    panes.reapply_coordinates();

    assert_eq!(
        panes.rendered_pane_ids(),
        vec![PaneId::Terminal(crate::TerminalId(1))]
    );
}

/// It travels, so a pane that was still waiting when the user switched tabs is still waiting when
/// it arrives - and one that had been placed does not go blank on the way across.
#[test]
fn awaiting_placement_survives_a_move_between_collections() {
    let mut panes = overlay_panes(80, 20);
    add_awaiting_placement(&mut panes, 1);
    let carry = panes.carry_of(&PaneId::Terminal(crate::TerminalId(1)));
    assert!(carry.awaiting_placement);

    let pane = panes
        .remove_pane(PaneId::Terminal(crate::TerminalId(1)))
        .unwrap();
    let mut elsewhere = overlay_panes(80, 20);
    elsewhere.add_pane(
        PaneId::Terminal(crate::TerminalId(1)),
        pane,
        OverlayKind::Dismissible,
        FloatingPaneCoordinates::default(),
        carry,
    );
    assert!(elsewhere.rendered_pane_ids().is_empty());

    elsewhere
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId(1)),
            coordinates("60%", "50%"),
            false,
        )
        .unwrap();
    let carry = elsewhere.carry_of(&PaneId::Terminal(crate::TerminalId(1)));
    assert!(
        !carry.awaiting_placement,
        "and a placed overlay carries that across rather than going blank on the next move"
    );
}

// ---- linked panels and the seam between them ---------------------------------------------------
//
// hocus draws its agent list and its conversation as two overlays that butt together, and the two
// touching frame columns are the divider. What these pin: only a pair placed *together* is a
// panel, the seam drag moves the divider and nothing else and claims nothing, and a frame drag
// carries both halves.

/// A 30-column half at `x`, both halves along row 2 of an 80x20 surface.
fn half(x: usize) -> FloatingPaneCoordinates {
    placed_coordinates(&x.to_string(), "2", "30", "10")
}

/// Two overlays placed edge to edge in one batch, as hocus places its panel: 1 on the left, 2 on
/// the right, linked.
fn linked_panel() -> OverlayPanes {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    add(&mut panes, 2, OverlayKind::Dismissible, half(32));
    panes.link_touching(&[
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
    ]);
    panes
}

fn geom(panes: &OverlayPanes, id: u32) -> (usize, usize, usize, usize) {
    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId(id)))
        .unwrap()
        .position_and_size();
    (geom.x, geom.y, geom.cols.as_usize(), geom.rows.as_usize())
}

/// A cell on the left half's right-hand frame column, which is the seam.
fn seam_cell(panes: &OverlayPanes) -> Position {
    let (x, y, cols, _) = geom(panes, 1);
    Position::new(y as i32 + 3, (x + cols - 1) as u16)
}

#[test]
fn a_batch_that_lands_two_overlays_edge_to_edge_links_them() {
    let panes = linked_panel();

    assert_eq!(
        panes.seam_at(&seam_cell(&panes)),
        Some((
            PaneId::Terminal(crate::TerminalId(1)),
            PaneId::Terminal(crate::TerminalId(2))
        )),
        "the left half's right frame column is the seam"
    );
    let (right_x, y, ..) = geom(&panes, 2);
    assert_eq!(
        panes.seam_at(&Position::new(y as i32 + 3, right_x as u16)),
        Some((
            PaneId::Terminal(crate::TerminalId(1)),
            PaneId::Terminal(crate::TerminalId(2))
        )),
        "and so is the right half's left frame column"
    );
    assert_eq!(
        panes.seam_at(&frame_corner(&panes, 1)),
        None,
        "the left half's own left frame is not"
    );
    assert_eq!(
        panes.seam_at(&Position::new(y as i32 + 3, (right_x + 5) as u16)),
        None,
        "nor is a cell inside either half"
    );
    assert_eq!(
        panes.linked_partner(&PaneId::Terminal(crate::TerminalId(1))),
        Some(PaneId::Terminal(crate::TerminalId(2)))
    );
}

/// Adjacency is not linkage: two unrelated overlays the user dragged next to each other must keep
/// moving on a frame drag rather than start resizing each other.
#[test]
fn two_overlays_that_merely_touch_are_not_linked() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    add(&mut panes, 2, OverlayKind::Dismissible, half(32));

    assert_eq!(panes.seam_at(&seam_cell(&panes)), None);
    assert_eq!(
        panes.linked_partner(&PaneId::Terminal(crate::TerminalId(1))),
        None
    );

    // a batch naming one pane leaves links alone, however the panes sit
    panes.link_touching(&[PaneId::Terminal(crate::TerminalId(1))]);
    assert_eq!(panes.seam_at(&seam_cell(&panes)), None);

    let before = geom(&panes, 2);
    drag_by(&mut panes, 1, 4, 2);
    assert_eq!(geom(&panes, 1), (6, 4, 30, 10), "the dragged one moved");
    assert_eq!(geom(&panes, 2), before, "and its neighbour did not");
}

#[test]
fn dragging_the_seam_moves_the_divider_and_nothing_else() {
    let mut panes = linked_panel();
    let press = seam_cell(&panes);
    panes.start_seam_drag(
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
        press,
    );
    assert!(panes.seam_is_being_dragged());

    let moved = panes.drag_seam_to(&Position::new(
        press.line() as i32,
        press.column() as u16 + 4,
    ));
    assert!(moved);

    assert_eq!(
        geom(&panes, 1),
        (2, 2, 34, 10),
        "the left half grew by four"
    );
    assert_eq!(
        geom(&panes, 2),
        (36, 2, 26, 10),
        "the right half started four columns later and gave up the same four"
    );
    assert_eq!(
        panes.seam_at(&Position::new(5, 35)),
        Some((
            PaneId::Terminal(crate::TerminalId(1)),
            PaneId::Terminal(crate::TerminalId(2))
        )),
        "the seam is where the divider is now"
    );

    let carry = panes.carry_of(&PaneId::Terminal(crate::TerminalId(1)));
    assert!(
        !carry.size_is_user_stated && !carry.position_is_user_stated,
        "a seam drag is not a claim: the split is the owning plugin's to re-derive"
    );
    let carry = panes.carry_of(&PaneId::Terminal(crate::TerminalId(2)));
    assert!(!carry.size_is_user_stated && !carry.position_is_user_stated);

    // the split is written back as fixed coordinates, so a re-resolution keeps it
    let placement = panes
        .placement_of(&PaneId::Terminal(crate::TerminalId(2)))
        .unwrap();
    let coordinates = placement.coordinates.unwrap();
    assert_eq!(coordinates.x, Some(PercentOrFixed::Fixed(36)));
    assert_eq!(coordinates.width, Some(PercentOrFixed::Fixed(26)));
    panes.reapply_coordinates();
    assert_eq!(geom(&panes, 1), (2, 2, 34, 10));
    assert_eq!(geom(&panes, 2), (36, 2, 26, 10));

    panes.stop_seam_drag(Position::new(
        press.line() as i32,
        press.column() as u16 + 4,
    ));
    assert!(!panes.seam_is_being_dragged());
}

/// A drag that goes nowhere - the same column again, or a drag with no seam held - reports so,
/// so the caller can skip the render.
#[test]
fn a_seam_drag_that_changes_nothing_reports_so() {
    let mut panes = linked_panel();
    let press = seam_cell(&panes);
    assert!(!panes.drag_seam_to(&press), "nothing is being dragged");
    panes.start_seam_drag(
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
        press,
    );
    assert!(!panes.drag_seam_to(&press), "the pointer has not moved");
    assert!(
        !panes.drag_seam_to(&Position::new(
            press.line() as i32 + 3,
            press.column() as u16
        )),
        "and a vertical move is not a seam move"
    );
}

/// Each half keeps the floor a resize keeps, and the panel's outer edges never move.
#[test]
fn a_seam_drag_clamps_at_the_floor() {
    let mut panes = linked_panel();
    let press = seam_cell(&panes);
    panes.start_seam_drag(
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
        press,
    );

    panes.drag_seam_to(&Position::new(press.line() as i32, 0));
    assert_eq!(
        geom(&panes, 1),
        (2, 2, MIN_TERMINAL_WIDTH, 10),
        "the left half stops at the floor"
    );
    assert_eq!(
        geom(&panes, 2),
        (2 + MIN_TERMINAL_WIDTH, 2, 60 - MIN_TERMINAL_WIDTH, 10),
        "and the right half has the rest, ending where the panel always ended"
    );

    panes.drag_seam_to(&Position::new(press.line() as i32, 400));
    assert_eq!(
        geom(&panes, 2),
        (62 - MIN_TERMINAL_WIDTH, 2, MIN_TERMINAL_WIDTH, 10)
    );
    assert_eq!(geom(&panes, 1), (2, 2, 60 - MIN_TERMINAL_WIDTH, 10));
}

/// A hidden half has no seam to drag: the drag holding it is released with it. The link stays -
/// the hidden half is still half of its panel, and the placement that reveals it needs the pair
/// to map the panel's layout onto - so the seam is back the moment the half is shown.
#[test]
fn hiding_a_half_ends_the_seam_drag_and_keeps_the_link() {
    let mut panes = linked_panel();
    let press = seam_cell(&panes);
    panes.start_seam_drag(
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
        press,
    );

    assert!(panes.hide_pane(PaneId::Terminal(crate::TerminalId(1))));
    assert!(!panes.seam_is_being_dragged());
    assert_eq!(
        panes.linked_partner(&PaneId::Terminal(crate::TerminalId(2))),
        Some(PaneId::Terminal(crate::TerminalId(1))),
        "still a panel"
    );
    assert_eq!(
        panes.seam_at(&seam_cell(&panes)),
        None,
        "but with nothing to divide while one half is away"
    );

    panes.show_pane(PaneId::Terminal(crate::TerminalId(1)));
    assert!(
        panes.seam_at(&seam_cell(&panes)).is_some(),
        "shown again, the seam is where it was, with no placement needed"
    );
}

#[test]
fn closing_a_half_ends_the_seam_drag_and_the_link() {
    let mut panes = linked_panel();
    panes.start_seam_drag(
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
        seam_cell(&panes),
    );
    panes.remove_pane(PaneId::Terminal(crate::TerminalId(2)));
    assert!(!panes.seam_is_being_dragged());
    assert_eq!(
        panes.linked_partner(&PaneId::Terminal(crate::TerminalId(1))),
        None
    );
}

/// A batch that places the halves apart - one hidden, the other given the whole panel, or the
/// two put in different corners - is the end of the panel.
#[test]
fn a_batch_that_places_the_halves_apart_unlinks_them() {
    let mut panes = linked_panel();
    panes
        .change_pane_coordinates(PaneId::Terminal(crate::TerminalId(2)), half(40), false)
        .unwrap();
    panes.link_touching(&[
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
    ]);
    assert_eq!(
        panes.linked_partner(&PaneId::Terminal(crate::TerminalId(1))),
        None
    );

    // and back together again
    panes
        .change_pane_coordinates(PaneId::Terminal(crate::TerminalId(2)), half(32), false)
        .unwrap();
    panes.link_touching(&[
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
    ]);
    assert_eq!(
        panes.linked_partner(&PaneId::Terminal(crate::TerminalId(1))),
        Some(PaneId::Terminal(crate::TerminalId(2)))
    );
}

/// The panel moves as one: a frame drag on either half carries the other by the same delta,
/// clamped so that neither half leaves the surface.
#[test]
fn a_frame_drag_moves_both_halves_of_a_linked_panel() {
    let mut panes = linked_panel();

    drag_by(&mut panes, 2, 4, 3);
    assert_eq!(geom(&panes, 1), (6, 5, 30, 10), "the other half came along");
    assert_eq!(geom(&panes, 2), (36, 5, 30, 10));
    assert!(
        panes
            .carry_of(&PaneId::Terminal(crate::TerminalId(1)))
            .position_is_user_stated
            && panes
                .carry_of(&PaneId::Terminal(crate::TerminalId(2)))
                .position_is_user_stated,
        "both positions are the user's, as a single dragged overlay's is"
    );
    assert!(
        panes.seam_at(&seam_cell(&panes)).is_some(),
        "and the seam moved with them"
    );

    // far past the right-hand edge: the *panel* stops at the edge, so the left half stops short
    // of it by the right half's width rather than being carried under it
    drag_by(&mut panes, 1, 400, 0);
    assert_eq!(
        geom(&panes, 2),
        (50, 5, 30, 10),
        "the right half is flush with the edge"
    );
    assert_eq!(
        geom(&panes, 1),
        (20, 5, 30, 10),
        "and the left half is still beside it"
    );
}

/// A placement batch naming both halves: `(id, x, width)` along row 2, 10 rows tall.
fn batch(halves: &[(u32, usize, usize)]) -> Vec<(PaneId, FloatingPaneCoordinates)> {
    halves
        .iter()
        .map(|(id, x, width)| {
            (
                PaneId::Terminal(crate::TerminalId(*id)),
                placed_coordinates(&x.to_string(), "2", &width.to_string(), "10"),
            )
        })
        .collect()
}

/// A batch is applied as a batch and then linked, as `Screen::change_overlay_coordinates` does.
fn place(
    panes: &mut OverlayPanes,
    batch: Vec<(PaneId, FloatingPaneCoordinates)>,
    overriding: bool,
) {
    let ids: Vec<PaneId> = batch.iter().map(|(id, _)| *id).collect();
    panes
        .change_pane_coordinates_batch(batch, overriding)
        .unwrap();
    panes.link_touching(&ids);
}

/// The plugin computes both halves from one panel rectangle and never learns where the user
/// dragged it to, so a batch after a drag names the home corner: the batch's layout - here a
/// different split - is mapped onto the panel where the user left it, and the halves still abut.
/// Only the position was claimed, so the batch's *size* is taken.
#[test]
fn a_dragged_panel_is_re_placed_where_the_user_left_it() {
    let mut panes = linked_panel();
    drag_by(&mut panes, 2, 4, 3);
    assert_eq!(geom(&panes, 1), (6, 5, 30, 10));

    place(&mut panes, batch(&[(2, 22, 40), (1, 2, 20)]), false);
    assert_eq!(
        geom(&panes, 1),
        (6, 5, 20, 10),
        "the new split, at the dragged origin"
    );
    assert_eq!(
        geom(&panes, 2),
        (26, 5, 40, 10),
        "and the right half still abuts"
    );
    assert!(panes.seam_at(&seam_cell(&panes)).is_some());
    assert!(
        panes
            .carry_of(&PaneId::Terminal(crate::TerminalId(2)))
            .position_is_user_stated
    );
    assert!(
        !panes
            .carry_of(&PaneId::Terminal(crate::TerminalId(2)))
            .size_is_user_stated
    );

    // a panel that was only dragged still takes a plugin's size, as a single overlay does
    let taller = vec![
        (
            PaneId::Terminal(crate::TerminalId(1)),
            placed_coordinates("2", "2", "25", "12"),
        ),
        (
            PaneId::Terminal(crate::TerminalId(2)),
            placed_coordinates("27", "2", "25", "12"),
        ),
    ];
    place(&mut panes, taller, false);
    assert_eq!(geom(&panes, 1), (6, 5, 25, 12));
    assert_eq!(geom(&panes, 2), (31, 5, 25, 12));
}

/// Hiding a half after a drag: the plugin asks for the shown half to have the whole panel and
/// keeps the hidden one at its share, and both land inside the dragged panel. The reveal then
/// cuts that same panel at whatever split the plugin asks for.
#[test]
fn a_hidden_half_of_a_dragged_panel_gives_its_width_up_and_the_reveal_cuts_it_again() {
    let mut panes = linked_panel();
    drag_by(&mut panes, 2, 4, 3);

    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));
    place(&mut panes, batch(&[(2, 2, 60), (1, 2, 30)]), false);
    assert_eq!(
        geom(&panes, 2),
        (6, 5, 60, 10),
        "the shown half has the dragged panel"
    );
    assert_eq!(
        geom(&panes, 1),
        (6, 5, 30, 10),
        "the hidden one keeps its share inside it"
    );
    assert_eq!(
        panes.linked_partner(&PaneId::Terminal(crate::TerminalId(2))),
        Some(PaneId::Terminal(crate::TerminalId(1))),
        "a hidden share inside the shown half is still the panel"
    );

    panes.show_pane(PaneId::Terminal(crate::TerminalId(1)));
    place(&mut panes, batch(&[(2, 22, 40), (1, 2, 20)]), false);
    assert_eq!(geom(&panes, 1), (6, 5, 20, 10));
    assert_eq!(geom(&panes, 2), (26, 5, 40, 10));
    assert!(panes.seam_at(&seam_cell(&panes)).is_some());
}

/// A pair carried to another tab with one half hidden arrives without its link and is relinked
/// by shape alone: the hidden share inside the shown half, the one starting further left first.
#[test]
fn a_hidden_share_inside_the_shown_half_links_as_a_panel() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        2,
        OverlayKind::Dismissible,
        placed_coordinates("2", "2", "60", "10"),
    );
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));

    panes.link_touching(&[
        PaneId::Terminal(crate::TerminalId(2)),
        PaneId::Terminal(crate::TerminalId(1)),
    ]);
    assert_eq!(
        panes.link_of(&PaneId::Terminal(crate::TerminalId(2))),
        Some((
            PaneId::Terminal(crate::TerminalId(1)),
            PaneId::Terminal(crate::TerminalId(2))
        ))
    );

    // a hidden overlay that merely overlaps is not a share of anything
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    add(&mut panes, 2, OverlayKind::Dismissible, half(20));
    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));
    panes.link_touching(&[
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
    ]);
    assert_eq!(panes.link_of(&PaneId::Terminal(crate::TerminalId(2))), None);
}

/// A surface that shrank pulls each half's fixed coordinates back onto it one at a time, which
/// can leave the two overlapping; the next batch re-cuts the panel inside the surface, whole.
#[test]
fn a_dragged_panel_re_resolved_onto_a_smaller_surface_is_re_cut_whole() {
    let (mut panes, display_area) = overlay_panes_with(
        80,
        20,
        Viewport {
            x: 0,
            y: 0,
            cols: 80,
            rows: 20,
        },
    );
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    add(&mut panes, 2, OverlayKind::Dismissible, half(32));
    panes.link_touching(&[
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
    ]);
    drag_by(&mut panes, 2, 10, 3);
    assert_eq!(geom(&panes, 2), (42, 5, 30, 10));

    *display_area.borrow_mut() = Size { cols: 60, rows: 20 };
    panes.reapply_coordinates();
    assert_eq!(
        geom(&panes, 2),
        (42, 5, 18, 10),
        "cut down to the surface on its own, with the seam still where it was"
    );

    place(&mut panes, batch(&[(2, 32, 30), (1, 2, 30)]), false);
    let (left_x, _, left_cols, _) = geom(&panes, 1);
    let (right_x, _, right_cols, _) = geom(&panes, 2);
    assert_eq!(left_x + left_cols, right_x, "abutting");
    assert!(right_x + right_cols <= 60, "and on the surface");
    assert!(panes.seam_at(&seam_cell(&panes)).is_some());
}

/// A ctrl-resize of one half is a resize of the panel: the other half's rows follow, and the
/// size becomes the user's on both, so the next placement keeps it.
#[test]
fn a_resize_of_one_half_sizes_the_panel() {
    let mut panes = linked_panel();
    assert!(panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[grow(Direction::Up)],
            (5, 2)
        )
        .unwrap());
    assert_eq!(geom(&panes, 1), (2, 0, 30, 12));
    assert_eq!(
        geom(&panes, 2),
        (32, 0, 30, 12),
        "the other half's rows came along"
    );
    for id in [1, 2] {
        let carry = panes.carry_of(&PaneId::Terminal(crate::TerminalId(id)));
        assert!(carry.size_is_user_stated && carry.position_is_user_stated);
    }

    place(&mut panes, batch(&[(2, 32, 30), (1, 2, 30)]), false);
    assert_eq!(
        geom(&panes, 1),
        (2, 0, 30, 12),
        "the user's size outranks the plugin's"
    );
    assert_eq!(geom(&panes, 2), (32, 0, 30, 12));

    // an overriding placement - the user's own key - takes the position and keeps the size
    place(&mut panes, batch(&[(2, 40, 30), (1, 10, 30)]), true);
    assert_eq!(geom(&panes, 1), (10, 2, 30, 12));
    assert_eq!(geom(&panes, 2), (40, 2, 30, 12));
    let carry = panes.carry_of(&PaneId::Terminal(crate::TerminalId(1)));
    assert!(!carry.position_is_user_stated && carry.size_is_user_stated);
}

/// The seam is placed from where the pointer is, not moved by where it went: past the floor the
/// seam waits at the floor, and on the way back it is only picked up again where it was left.
#[test]
fn a_seam_dragged_past_the_floor_comes_back_under_the_pointer() {
    let mut panes = linked_panel();
    let press = seam_cell(&panes);
    panes.start_seam_drag(
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
        press,
    );

    panes.drag_seam_to(&Position::new(press.line() as i32, 0));
    assert_eq!(geom(&panes, 1).2, MIN_TERMINAL_WIDTH);

    panes.drag_seam_to(&Position::new(press.line() as i32, 20));
    assert_eq!(
        geom(&panes, 1),
        (2, 2, 19, 10),
        "the seam is on column 20, where the pointer is"
    );
    assert_eq!(geom(&panes, 2), (21, 2, 41, 10));
}

/// A placement batch arriving mid-drag - a terminal resize, a half's visibility changing - that
/// keeps the pair together leaves the drag in flight; one that parts them ends it.
#[test]
fn a_placement_that_keeps_the_pair_leaves_a_seam_drag_in_flight() {
    let mut panes = linked_panel();
    panes.start_seam_drag(
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
        seam_cell(&panes),
    );

    place(&mut panes, batch(&[(1, 2, 30), (2, 32, 30)]), false);
    assert!(panes.seam_is_being_dragged());

    place(&mut panes, batch(&[(1, 2, 30), (2, 40, 30)]), false);
    assert!(!panes.seam_is_being_dragged());
}

/// Whether the halves of `linked_panel` carry a size claim and a position claim.
fn claims(panes: &OverlayPanes, id: u32) -> (bool, bool) {
    let carry = panes.carry_of(&PaneId::Terminal(crate::TerminalId(id)));
    (carry.size_is_user_stated, carry.position_is_user_stated)
}

#[test]
fn a_seam_side_resize_of_the_left_half_moves_the_seam() {
    let mut panes = linked_panel();
    assert!(panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[grow(Direction::Right)],
            (5, 2)
        )
        .unwrap());
    assert_eq!(geom(&panes, 1), (2, 2, 35, 10));
    assert_eq!(
        geom(&panes, 2),
        (37, 2, 25, 10),
        "the right half gave up what the left half took, and kept its outer edge"
    );
    assert_eq!(
        claims(&panes, 1),
        (false, false),
        "a seam move is not a claim"
    );
    assert_eq!(claims(&panes, 2), (false, false));
}

#[test]
fn a_seam_side_resize_of_the_right_half_moves_the_seam() {
    let mut panes = linked_panel();
    assert!(panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(2)),
            &[grow(Direction::Left)],
            (5, 2)
        )
        .unwrap());
    assert_eq!(geom(&panes, 2), (27, 2, 35, 10));
    assert_eq!(geom(&panes, 1), (2, 2, 25, 10));
    assert_eq!(
        claims(&panes, 2),
        (false, false),
        "the right half's x moved, but the panel did not"
    );
    assert_eq!(claims(&panes, 1), (false, false));
}

#[test]
fn a_seam_side_shrink_moves_the_seam_the_other_way() {
    let mut panes = linked_panel();
    assert!(panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[shrink(Direction::Right)],
            (5, 2)
        )
        .unwrap());
    assert_eq!(geom(&panes, 1), (2, 2, 25, 10));
    assert_eq!(geom(&panes, 2), (27, 2, 35, 10));
    assert_eq!(claims(&panes, 1), (false, false));
}

#[test]
fn a_seam_side_resize_stops_at_the_partner_s_floor() {
    let mut panes = linked_panel();
    for _ in 0..20 {
        if !panes
            .resize_pane_with_strategies(
                PaneId::Terminal(crate::TerminalId(1)),
                &[grow(Direction::Right)],
                (5, 2),
            )
            .unwrap()
        {
            break;
        }
    }
    assert_eq!(geom(&panes, 1), (2, 2, 60 - MIN_TERMINAL_WIDTH, 10));
    assert_eq!(
        geom(&panes, 2),
        (62 - MIN_TERMINAL_WIDTH, 2, MIN_TERMINAL_WIDTH, 10),
        "the partner keeps the floor and its outer edge"
    );
    assert!(
        !panes
            .resize_pane_with_strategies(
                PaneId::Terminal(crate::TerminalId(1)),
                &[grow(Direction::Right)],
                (5, 2)
            )
            .unwrap(),
        "at the floor there is nothing left to take"
    );
    assert_eq!(geom(&panes, 1), (2, 2, 60 - MIN_TERMINAL_WIDTH, 10));
    assert_eq!(claims(&panes, 1), (false, false));
}

#[test]
fn an_outer_edge_resize_sizes_the_panel() {
    let mut panes = linked_panel();
    assert!(panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[grow(Direction::Left)],
            (5, 2)
        )
        .unwrap());
    assert_eq!(geom(&panes, 1), (0, 2, 32, 10), "clamped to the surface");
    assert_eq!(geom(&panes, 2), (32, 2, 30, 10), "the seam did not move");
    for id in [1, 2] {
        assert_eq!(
            claims(&panes, id),
            (true, true),
            "the panel grew and its origin moved, and both halves say so"
        );
    }
}

/// Ctrl+MouseScroll over either half: the panel's four outer borders each move by half the
/// step, as a lone overlay's would, and the seam keeps its share of the width. Which half the
/// pointer was on does not matter.
#[test]
fn an_undirected_resize_grows_the_panel_and_keeps_the_split() {
    for under_the_pointer in [1, 2] {
        let mut panes = linked_panel();
        assert!(panes
            .resize_pane_with_strategies(
                PaneId::Terminal(crate::TerminalId(under_the_pointer)),
                &[grow_all()],
                (5, 2)
            )
            .unwrap());
        assert_eq!(
            geom(&panes, 1),
            (0, 1, 32, 12),
            "half {under_the_pointer}: the panel grew 2 columns and 1 row on every side"
        );
        assert_eq!(
            geom(&panes, 2),
            (32, 1, 32, 12),
            "half {under_the_pointer}: the seam stayed at half the width"
        );
        assert!(panes.seam_at(&seam_cell(&panes)).is_some());
        for id in [1, 2] {
            assert_eq!(claims(&panes, id), (true, true));
        }
    }
}

#[test]
fn an_undirected_shrink_shrinks_the_panel_and_keeps_the_split() {
    let mut panes = linked_panel();
    assert!(panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(2)),
            &[shrink_all()],
            (5, 2)
        )
        .unwrap());
    assert_eq!(geom(&panes, 1), (4, 3, 28, 8));
    assert_eq!(geom(&panes, 2), (32, 3, 28, 8));
    for id in [1, 2] {
        assert_eq!(claims(&panes, id), (true, true));
    }
}

/// A split the user dragged off-centre is carried onto the new width in proportion, not reset
/// and not moved by the step.
#[test]
fn an_undirected_resize_keeps_an_uneven_split_in_proportion() {
    let mut panes = linked_panel();
    let press = seam_cell(&panes);
    panes.start_seam_drag(
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
        press,
    );
    panes.drag_seam_to(&Position::new(press.line() as i32, 21));
    panes.stop_seam_drag(Position::new(press.line() as i32, 21));
    assert_eq!(geom(&panes, 1), (2, 2, 20, 10), "the left half is 20 of 60");
    assert_eq!(geom(&panes, 2), (22, 2, 40, 10));

    assert!(panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId(1)),
            &[grow_all()],
            (5, 2)
        )
        .unwrap());
    // the seam at 20/60 of the old box lands at 21/64 of the new one
    assert_eq!(geom(&panes, 1), (0, 1, 21, 12));
    assert_eq!(geom(&panes, 2), (21, 1, 43, 12));
    assert!(panes.seam_at(&seam_cell(&panes)).is_some());
}

/// Shrinking stops where a seam drag stops: each half keeps `MIN_TERMINAL_WIDTH`, and once the
/// panel is two of those wide a further shrink takes rows only, then nothing.
#[test]
fn an_undirected_shrink_stops_at_the_halves_floor() {
    let mut panes = linked_panel();
    for _ in 0..40 {
        if !panes
            .resize_pane_with_strategies(
                PaneId::Terminal(crate::TerminalId(1)),
                &[shrink_all()],
                (5, 2),
            )
            .unwrap()
        {
            break;
        }
        let (left_x, _, left_cols, _) = geom(&panes, 1);
        let (right_x, _, right_cols, _) = geom(&panes, 2);
        assert_eq!(left_x + left_cols, right_x, "still abutting");
        assert!(left_cols >= MIN_TERMINAL_WIDTH && right_cols >= MIN_TERMINAL_WIDTH);
    }
    let (_, _, left_cols, rows) = geom(&panes, 1);
    let (_, _, right_cols, _) = geom(&panes, 2);
    assert_eq!(left_cols + right_cols, 2 * MIN_TERMINAL_WIDTH);
    assert_eq!(rows, MIN_TERMINAL_HEIGHT);
    let before = (geom(&panes, 1), geom(&panes, 2));
    assert!(
        !panes
            .resize_pane_with_strategies(
                PaneId::Terminal(crate::TerminalId(2)),
                &[shrink_all()],
                (5, 2)
            )
            .unwrap(),
        "at the floor there is nothing left to take"
    );
    assert_eq!((geom(&panes, 1), geom(&panes, 2)), before);
}

#[test]
fn a_pane_abutting_two_others_is_in_one_panel() {
    let mut panes = overlay_panes(80, 20);
    for (id, x) in [(1, 2), (2, 22), (3, 42)] {
        add(
            &mut panes,
            id,
            OverlayKind::Dismissible,
            placed_coordinates(&x.to_string(), "2", "20", "10"),
        );
    }
    let (a, b, c) = (
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
        PaneId::Terminal(crate::TerminalId(3)),
    );

    panes.link_touching(&[a, b, c]);
    assert_eq!(
        panes.link_of(&b),
        Some((a, b)),
        "the first pair in the batch's order"
    );
    assert_eq!(panes.linked_partner(&a), Some(b));
    assert_eq!(
        panes.link_of(&c),
        None,
        "the pair that would name the middle pane again is not made"
    );

    panes.link_touching(&[c, b, a]);
    assert_eq!(panes.link_of(&b), Some((b, c)));
    assert_eq!(panes.link_of(&a), None);
}

// ---- expanded over the screen ---------------------------------------------------------------

/// A tab whose chrome takes a row off the top and one off the bottom, which is what makes the
/// difference between the two fullscreen keystrokes visible.
fn overlay_panes_with_chrome(cols: usize, rows: usize) -> OverlayPanes {
    overlay_panes_with(
        cols,
        rows,
        Viewport {
            x: 0,
            y: 1,
            cols,
            rows: rows - 2,
        },
    )
    .0
}

/// The rectangle a pane is *drawn* with, override included - what `geom` is to the base.
fn drawn(panes: &OverlayPanes, id: u32) -> (usize, usize, usize, usize) {
    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId(id)))
        .unwrap()
        .current_geom();
    (geom.x, geom.y, geom.cols.as_usize(), geom.rows.as_usize())
}

#[test]
fn an_expanded_overlay_fills_the_viewport_and_leaves_the_chrome() {
    let mut panes = overlay_panes_with_chrome(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        placed_coordinates("10", "5", "30", "8"),
    );

    assert!(panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1))));

    assert_eq!(
        drawn(&panes, 1),
        (0, 1, 80, 18),
        "the tab bar and the status bar keep their rows, as they do for a tiled pane"
    );
    assert!(panes.pane_is_fullscreen(&PaneId::Terminal(crate::TerminalId(1))));
    assert!(!panes.fullscreen_covers_ui());
}

#[test]
fn the_other_keystroke_covers_the_chrome_too() {
    let mut panes = overlay_panes_with_chrome(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        placed_coordinates("10", "5", "30", "8"),
    );

    assert!(panes.toggle_pane_no_ui_fullscreen(PaneId::Terminal(crate::TerminalId(1))));

    assert_eq!(drawn(&panes, 1), (0, 0, 80, 20));
    assert!(panes.fullscreen_covers_ui());
}

/// Pressing the *other* keystroke while expanded switches between the two rather than collapsing,
/// as the floating layer's pair does.
#[test]
fn the_two_keystrokes_switch_between_each_other() {
    let mut panes = overlay_panes_with_chrome(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));
    panes.toggle_pane_no_ui_fullscreen(PaneId::Terminal(crate::TerminalId(1)));
    assert!(panes.fullscreen_covers_ui());
    assert_eq!(drawn(&panes, 1), (0, 0, 80, 20));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));
    assert!(panes.fullscreen_is_active());
    assert!(!panes.fullscreen_covers_ui());
    assert_eq!(drawn(&panes, 1), (0, 1, 80, 18));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));
    assert!(!panes.fullscreen_is_active());
}

/// The claim the whole design rests on: an expansion is an override over a geometry that is still
/// a function of the overlay's coordinates and the surface, so the way back is exact - even when
/// the surface changed size while it was expanded.
#[test]
fn the_way_back_is_the_rectangle_the_coordinates_always_said() {
    let mut panes = overlay_panes(80, 20);
    let mut untouched = overlay_panes(80, 20);
    for panes in [&mut panes, &mut untouched] {
        add(
            panes,
            1,
            OverlayKind::Dismissible,
            coordinates("50%", "40%"),
        );
    }

    // the no-UI keystroke, whose area is the surface itself, so that growing the surface under
    // it is the whole of the change
    panes.toggle_pane_no_ui_fullscreen(PaneId::Terminal(crate::TerminalId(1)));
    *panes.display_area.borrow_mut() = Size {
        cols: 120,
        rows: 40,
    };
    *untouched.display_area.borrow_mut() = Size {
        cols: 120,
        rows: 40,
    };
    panes.reapply_coordinates();
    untouched.reapply_coordinates();
    assert_eq!(
        drawn(&panes, 1),
        (0, 0, 120, 40),
        "the expansion follows the surface while it lasts"
    );

    panes.toggle_pane_no_ui_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    assert_eq!(
        geom(&panes, 1),
        geom(&untouched, 1),
        "and what comes back is what a never-expanded twin resolved to on the same surface"
    );
    assert_eq!(drawn(&panes, 1), geom(&panes, 1), "with no override left");
}

/// An expansion is not a placement, so it claims nothing: the plugin that owns the overlay keeps
/// placing it, and what it places is the rectangle the expansion comes back to.
#[test]
fn an_expansion_claims_nothing_and_a_plugin_keeps_placing_underneath_it() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));
    assert_eq!(
        claims(&panes, 1),
        (false, false),
        "an expansion claims nothing"
    );

    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId(1)),
            placed_coordinates("40", "6", "20", "6"),
            false,
        )
        .unwrap();
    assert_eq!(
        drawn(&panes, 1),
        (0, 0, 80, 20),
        "the placement lands underneath, and the overlay is still expanded"
    );
    assert_eq!(geom(&panes, 1), (40, 6, 20, 6));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));
    assert_eq!(
        geom(&panes, 1),
        (40, 6, 20, 6),
        "so the way back is where the plugin last put it, not where it was when it was expanded"
    );
}

/// hocus takes its panel's *size* from the manifest - that is how a user resize reaches it - so an
/// expansion reported as a rectangle would be answered by re-placing the panel at the size of the
/// screen, overwriting the very rectangle the expansion exists to come back to.
#[test]
fn an_expanded_overlay_reports_the_rectangle_underneath_it() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    let group = HashMap::new();

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    let info = panes.pane_info(&group).into_iter().next().unwrap();
    assert!(info.is_fullscreen, "it says that it is expanded");
    assert_eq!(
        (info.pane_x, info.pane_y, info.pane_columns, info.pane_rows),
        (2, 2, 30, 10),
        "and reports the rectangle it will come back to, not the one it is drawn with"
    );
    assert_eq!(
        (
            info.pane_content_x,
            info.pane_content_y,
            info.pane_content_columns,
            info.pane_content_rows
        ),
        (3, 3, 28, 8),
        "content area included - the frame it would wear there, not the one it wears now"
    );
}

#[test]
fn a_modal_may_not_be_expanded() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, half(2));

    assert!(!panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1))));
    assert!(!panes.fullscreen_is_active());
    assert_eq!(drawn(&panes, 1), geom(&panes, 1));
}

#[test]
fn a_hidden_overlay_may_not_be_expanded() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));

    assert!(!panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1))));
    assert!(!panes.fullscreen_is_active());
}

/// Setting the pane the user is looking at aside is asking for it to go away, not for a screenful
/// of overlay with no way back.
#[test]
fn setting_an_expanded_overlay_aside_puts_it_back_first() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));

    assert!(!panes.fullscreen_is_active());
    assert_eq!(geom(&panes, 1), (2, 2, 30, 10));
    assert_eq!(drawn(&panes, 1), geom(&panes, 1));
}

#[test]
fn closing_an_expanded_overlay_ends_the_expansion() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    add(&mut panes, 2, OverlayKind::Dismissible, half(40));
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    panes.remove_pane(PaneId::Terminal(crate::TerminalId(1)));

    assert!(!panes.fullscreen_is_active());
    assert_eq!(
        drawn(&panes, 2),
        geom(&panes, 2),
        "and the overlay it was covering is drawn as itself again"
    );
}

/// A pane swapped into an expanded slot is expanded, and the slot still knows where to come back
/// to - the base geometry travels, not the override.
#[test]
fn a_pane_swapped_into_an_expanded_slot_is_expanded() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    panes
        .replace_pane(
            PaneId::Terminal(crate::TerminalId(1)),
            mock_pane(PaneId::Terminal(crate::TerminalId(9))),
        )
        .unwrap();

    assert!(panes.pane_is_fullscreen(&PaneId::Terminal(crate::TerminalId(9))));
    assert_eq!(drawn(&panes, 9), (0, 0, 80, 20));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(9)));
    assert_eq!(geom(&panes, 9), (2, 2, 30, 10));
}

#[test]
fn an_expanded_overlay_is_not_dragged_resized_or_seam_dragged() {
    let mut panes = linked_panel();
    let seam = seam_cell(&panes);
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));
    let before = (geom(&panes, 1), geom(&panes, 2));

    assert!(
        !panes.move_pane_with_mouse(Position::new(0, 0)),
        "there is nowhere to move a pane that fills the screen"
    );
    assert!(!panes.is_resizable(&PaneId::Terminal(crate::TerminalId(1))));
    assert!(!panes
        .resize_pane_with_id(
            PaneId::Terminal(crate::TerminalId(1)),
            grow(Direction::Right)
        )
        .unwrap());
    assert_eq!(panes.seam_at(&seam), None, "and no divider to drag");

    assert_eq!(
        (geom(&panes, 1), geom(&panes, 2)),
        before,
        "so the rectangle underneath is exactly as it was"
    );
}

/// A linked pair is one panel to every gesture, and this is a gesture.
#[test]
fn a_linked_panel_is_expanded_as_a_panel() {
    let mut panes = linked_panel();

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(2)));

    assert_eq!(
        (drawn(&panes, 1), drawn(&panes, 2)),
        ((0, 0, 40, 20), (40, 0, 40, 20)),
        "both halves grow, and the divider keeps its share of the panel"
    );
    assert!(panes.pane_is_fullscreen(&PaneId::Terminal(crate::TerminalId(1))));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(2)));
    assert_eq!(
        (geom(&panes, 1), geom(&panes, 2)),
        ((2, 2, 30, 10), (32, 2, 30, 10)),
        "and the panel comes back whole"
    );
}

/// An uneven split stays uneven: the divider is carried in proportion, exactly as an undirected
/// resize of the panel carries it.
#[test]
fn an_expanded_panel_keeps_an_uneven_split_in_proportion() {
    let mut panes = linked_panel();
    place(&mut panes, batch(&[(1, 2, 45), (2, 47, 15)]), false);

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    assert_eq!(
        (drawn(&panes, 1).2, drawn(&panes, 2).2),
        (60, 20),
        "three quarters and one quarter of the panel become three quarters and one of the screen"
    );
}

/// The owning plugin keeps re-cutting the panel while it is expanded, and the expansion follows -
/// this is the same `set_pane_frames` step that keeps it filling a screen that changed size.
#[test]
fn a_batch_that_re_cuts_an_expanded_panel_moves_its_divider() {
    let mut panes = linked_panel();
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    place(&mut panes, batch(&[(1, 2, 45), (2, 47, 15)]), false);

    assert_eq!(
        (drawn(&panes, 1).2, drawn(&panes, 2).2),
        (60, 20),
        "the divider is re-cut where the batch asked, at the size of the screen"
    );
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));
    assert_eq!(
        (geom(&panes, 1), geom(&panes, 2)),
        ((2, 2, 45, 10), (47, 2, 15, 10)),
        "and the way back is the batch's, not the rectangle from before it"
    );
}

/// A pair with a hidden half is the shown half taking the whole panel, which is how every other
/// placement already reads that shape.
#[test]
fn a_panel_with_a_hidden_half_is_expanded_as_the_shown_half() {
    let mut panes = linked_panel();
    panes.hide_pane(PaneId::Terminal(crate::TerminalId(1)));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(2)));

    assert_eq!(drawn(&panes, 2), (0, 0, 80, 20));
    assert_eq!(
        drawn(&panes, 1),
        geom(&panes, 1),
        "the hidden half keeps the rectangle it will come back to"
    );
    assert!(!panes.pane_is_fullscreen(&PaneId::Terminal(crate::TerminalId(1))));
}

/// The same shape reached from the other side: the panel is expanded first and a half is set
/// aside underneath it. The survivor is the whole panel from that moment on, so it takes the
/// whole screen - `hide_pane` has to re-derive the expansion, not only set the flag.
#[test]
fn hiding_a_half_of_an_expanded_panel_leaves_the_survivor_filling_the_screen() {
    let mut panes = linked_panel();
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    panes.hide_pane(PaneId::Terminal(crate::TerminalId(2)));

    assert!(
        panes.pane_is_fullscreen(&PaneId::Terminal(crate::TerminalId(1))),
        "hiding the other half does not collapse the expansion"
    );
    assert_eq!(
        drawn(&panes, 1),
        (0, 0, 80, 20),
        "and the half left on screen grows into the whole of it"
    );
    assert_eq!(
        drawn(&panes, 2),
        geom(&panes, 2),
        "while the hidden half keeps the rectangle it will come back to"
    );
}

/// A modal is drawn because something needs an answer now, and a keystroke that fills the screen
/// with something else does not suspend that.
#[test]
fn an_expanded_overlay_does_not_cover_a_live_modal() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    add(&mut panes, 2, OverlayKind::Modal, half(40));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    assert_eq!(
        panes.stack_layer_ids(),
        vec![
            PaneId::Terminal(crate::TerminalId(1)),
            PaneId::Terminal(crate::TerminalId(2))
        ],
        "the modal is still the top of the stack"
    );
}

/// The frame is nothing but the resize hint, and an expanded overlay is not resizable, so it must
/// not offer one.
#[test]
fn an_expanded_overlay_stops_offering_a_resize() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    assert!(panes.is_resizable(&PaneId::Terminal(crate::TerminalId(1))));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    assert!(!panes.is_resizable(&PaneId::Terminal(crate::TerminalId(1))));
}

/// A fullscreen press names a pane, and it is that pane that is expanded. The expansion used to
/// be dropped and the requested pane never expanded at all, so a second overlay could not be
/// looked at closely without first putting the first one back by hand.
#[test]
fn a_fullscreen_press_on_another_overlay_expands_that_one() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    add(&mut panes, 2, OverlayKind::Dismissible, half(40));
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    assert!(panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(2))));

    assert!(panes.pane_is_fullscreen(&PaneId::Terminal(crate::TerminalId(2))));
    assert_eq!(drawn(&panes, 2), (0, 0, 80, 20));
    assert!(!panes.pane_is_fullscreen(&PaneId::Terminal(crate::TerminalId(1))));
    assert_eq!(
        drawn(&panes, 1),
        geom(&panes, 1),
        "and the one that was expanded is back where its coordinates say"
    );

    assert!(panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(2))));
    assert!(
        !panes.fullscreen_is_active(),
        "a press naming the expanded panel still collapses it"
    );
}

/// A drag in flight has nothing left to carry once the pane fills the screen, and leaving it in
/// flight would swallow every mouse event until the release.
#[test]
fn expanding_an_overlay_releases_a_drag_in_flight() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    assert!(panes.move_pane_with_mouse(frame_corner(&panes, 1)));
    assert!(panes.pane_is_being_moved_with_mouse());

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    assert!(!panes.pane_is_being_moved_with_mouse());
}

/// And the mover refuses a step of its own, for a drag whose state the caller is holding: the
/// rectangle it would write is the one the expansion comes back to.
#[test]
fn an_expanded_overlay_is_not_carried_by_a_drag_that_outlived_the_press() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));
    let before = geom(&panes, 1);

    panes.set_pane_being_moved_with_mouse(
        PaneId::Terminal(crate::TerminalId(1)),
        Position::new(2, 2),
    );

    assert!(!panes.move_pane_to_position(&Position::new(9, 40)));
    assert_eq!(geom(&panes, 1), before);
}

/// The same for the divider: `seam_at` refuses to start one, this refuses to continue one that
/// was in flight when the expansion began.
#[test]
fn expanding_a_panel_releases_a_seam_drag_and_refuses_the_next_step() {
    let mut panes = linked_panel();
    let seam = seam_cell(&panes);
    let (left, right) = panes.seam_at(&seam).unwrap();
    panes.start_seam_drag(left, right, seam);
    assert!(panes.seam_is_being_dragged());

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));
    assert!(!panes.seam_is_being_dragged());

    let before = (geom(&panes, 1), geom(&panes, 2));
    panes.start_seam_drag(left, right, seam);
    assert!(!panes.drag_seam_to(&Position::new(5, 10)));
    assert_eq!(
        (geom(&panes, 1), geom(&panes, 2)),
        before,
        "the split underneath is untouched"
    );
}

/// Either half. The guard used to run after `unlink`, and `fullscreen_pane_ids` is derived from
/// the link - so once the pair was forgotten it named the anchor alone and the other half slipped
/// past, leaving a panel expanded with one half of it gone.
#[test]
fn closing_the_other_half_of_an_expanded_panel_ends_the_expansion() {
    let mut panes = linked_panel();
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    panes.remove_pane(PaneId::Terminal(crate::TerminalId(2)));

    assert!(!panes.fullscreen_is_active());
    assert_eq!(
        drawn(&panes, 1),
        geom(&panes, 1),
        "and the half that is left is drawn as itself again"
    );
}

/// The pane handed back to the caller is out of the collection, so `refresh_fullscreen`'s reset
/// loop - which owns every override here - can no longer reach it. It must not leave wearing one.
#[test]
fn a_pane_taken_out_of_an_expanded_slot_comes_back_without_the_expansion() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    let removed = panes
        .replace_pane(
            PaneId::Terminal(crate::TerminalId(1)),
            mock_pane(PaneId::Terminal(crate::TerminalId(9))),
        )
        .unwrap();

    assert!(removed.geom_override().is_none());
    assert_eq!(
        removed.position_and_size(),
        removed.current_geom(),
        "so it is drawn at the rectangle it always had"
    );
}

/// A surface a single column wide has no room for two halves, and the panel is still a panel: the
/// seam is clamped to the one column there is, and the right half takes the floor `resize_panel`
/// gives it rather than being drawn at no width at all.
#[test]
fn an_expanded_panel_on_a_surface_one_column_wide_keeps_both_halves() {
    let (mut panes, display_area) = overlay_panes_with(
        80,
        20,
        Viewport {
            x: 0,
            y: 0,
            cols: 80,
            rows: 20,
        },
    );
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    add(&mut panes, 2, OverlayKind::Dismissible, half(32));
    panes.link_touching(&[
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
    ]);
    panes.toggle_pane_no_ui_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    *display_area.borrow_mut() = Size { cols: 1, rows: 20 };
    panes.set_pane_frames().unwrap();

    assert_eq!(
        (drawn(&panes, 1).2, drawn(&panes, 2).2),
        (1, 1),
        "neither half is drawn at no width"
    );
    assert!(panes.pane_is_fullscreen(&PaneId::Terminal(crate::TerminalId(1))));
}

/// A surface with no columns has no room for a divider either: `seam_mapped_onto` clamps its
/// answer to a column that is already past the right edge, and there is no rectangle to answer
/// with at all - so the expansion is over, rather than remembered with nothing to show for it.
#[test]
fn an_expanded_panel_on_a_surface_with_no_columns_is_drawn_at_nothing() {
    let (mut panes, display_area) = overlay_panes_with(
        80,
        20,
        Viewport {
            x: 0,
            y: 0,
            cols: 80,
            rows: 20,
        },
    );
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    add(&mut panes, 2, OverlayKind::Dismissible, half(32));
    panes.link_touching(&[
        PaneId::Terminal(crate::TerminalId(1)),
        PaneId::Terminal(crate::TerminalId(2)),
    ]);
    panes.toggle_pane_no_ui_fullscreen(PaneId::Terminal(crate::TerminalId(1)));

    *display_area.borrow_mut() = Size { cols: 0, rows: 0 };
    panes.set_pane_frames().unwrap();

    assert!(
        panes
            .get(&PaneId::Terminal(crate::TerminalId(1)))
            .unwrap()
            .geom_override()
            .is_none(),
        "nothing is expanded into a surface this size"
    );
    assert!(
        !panes.fullscreen_is_active(),
        "and the expansion is dropped rather than left claiming panes it cannot draw"
    );
    assert!(
        !panes.pane_is_fullscreen(&PaneId::Terminal(crate::TerminalId(1))),
        "which is what every resize, drag and move of the panel goes on to ask"
    );
}

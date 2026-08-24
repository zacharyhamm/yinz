use super::*;
use crate::os_input_output::{AsyncReader, ServerOsApi};
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::sixel::SixelImageStore;
use crate::panes::{LinkHandler, PaneId, TerminalPane};
use interprocess::local_socket::Stream as LocalSocketStream;
use std::path::PathBuf;
use zellij_utils::data::{GlobalPaneEdge, Palette};
use zellij_utils::input::command::{RunCommand, TerminalAction};
use zellij_utils::ipc::{ClientToServerMsg, IpcReceiverWithContext, ServerToClientMsg};
use zellij_utils::pane_size::Dimension;

#[derive(Clone)]
struct FakeInputOutput {}

impl ServerOsApi for FakeInputOutput {
    fn set_terminal_size_using_terminal_id(
        &self,
        _id: u32,
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
    ) -> Result<(u32, Box<dyn AsyncReader>, Option<u32>)> {
        unimplemented!()
    }
    fn write_to_tty_stdin(&self, _id: u32, _buf: &[u8]) -> Result<usize> {
        unimplemented!()
    }
    fn tcdrain(&self, _id: u32) -> Result<()> {
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
        _terminal_id: u32,
        _run_command: RunCommand,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<u32>)> {
        unimplemented!()
    }
    fn clear_terminal_id(&self, _terminal_id: u32) -> Result<()> {
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

fn geom(x: usize, y: usize, cols: usize, rows: usize) -> PaneGeom {
    PaneGeom {
        x,
        y,
        cols: Dimension::fixed(cols),
        rows: Dimension::fixed(rows),
        ..Default::default()
    }
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
    overlay_panes_with(cols, rows, (0, 0), vec![]).0
}

/// The collection plus the shared `display_area` cell, so a test can grow the surface under it.
fn overlay_panes_with(
    cols: usize,
    rows: usize,
    origin: (usize, usize),
    strips: Vec<GlobalPaneStrip>,
) -> (OverlayPanes, Rc<RefCell<Size>>) {
    let display_area = Rc::new(RefCell::new(Size { cols, rows }));
    let viewport = Rc::new(RefCell::new(Viewport {
        x: origin.0,
        y: origin.1,
        cols,
        rows,
    }));
    let panes = OverlayPanes::new(
        display_area.clone(),
        Rc::new(RefCell::new(origin)),
        viewport,
        Rc::new(RefCell::new(strips)),
        Rc::new(RefCell::new(HashSet::new())),
        Rc::new(RefCell::new(HashMap::new())),
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
    let pane_id = PaneId::Terminal(id);
    panes.add_pane(pane_id, mock_pane(pane_id), kind, coords);
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
    let pane = panes.get(&PaneId::Terminal(1)).unwrap();
    assert_eq!(pane.position_and_size().cols.as_usize(), 48);
    assert_eq!(pane.position_and_size().rows.as_usize(), 10);
}

#[test]
fn an_overlay_resolves_over_the_global_panes_rather_than_beside_them() {
    // a 10-column left strip: the tab's own area starts at x=10, but the surface starts at 0 and
    // an overlay placed on it may cover the strip
    let strip = GlobalPaneStrip {
        geom: geom(0, 0, 10, 20),
        edge: GlobalPaneEdge::Left,
    };
    let (mut panes, _display_area) = overlay_panes_with(80, 20, (10, 0), vec![strip]);
    add(
        &mut panes,
        1,
        OverlayKind::Modal,
        coordinates("100%", "100%"),
    );
    let pane_geom = panes.get(&PaneId::Terminal(1)).unwrap().position_and_size();
    assert_eq!(
        pane_geom.x, 0,
        "the overlay starts at the surface origin, not the tab's"
    );
    assert_eq!(
        pane_geom.cols.as_usize(),
        90,
        "the surface is the display area grown back over the strip"
    );
}

#[test]
fn reapplying_coordinates_re_resolves_against_the_surface_as_it_is_now() {
    let (mut panes, display_area) = overlay_panes_with(80, 20, (0, 0), vec![]);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    assert_eq!(
        panes
            .get(&PaneId::Terminal(1))
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
            .get(&PaneId::Terminal(1))
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
        .change_pane_coordinates(PaneId::Terminal(1), coordinates("30%", "25%"))
        .unwrap();
    assert_eq!(
        panes.kind_of(&PaneId::Terminal(1)),
        Some(OverlayKind::Modal),
        "a coordinate change must not be able to demote a modal"
    );
    assert!(panes.is_modal(&PaneId::Terminal(1)));
    assert_eq!(
        panes
            .get(&PaneId::Terminal(1))
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
            PaneId::Terminal(1),
            PaneId::Terminal(2),
            PaneId::Terminal(3)
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
    assert_eq!(panes.rendered_pane_ids(), vec![PaneId::Terminal(1)]);
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
    assert!(panes.remove_pane(PaneId::Terminal(1)).is_some());
    assert!(!panes.panes_contain(&PaneId::Terminal(1)));
    assert_eq!(panes.stack_layer_ids(), vec![PaneId::Terminal(2)]);
    assert_eq!(panes.get_pane_z_index(PaneId::Terminal(2)), Some(0));
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

    let replacement = PaneId::Terminal(9);
    panes
        .replace_pane(PaneId::Terminal(1), mock_pane(replacement))
        .unwrap();

    assert_eq!(
        panes.stack_layer_ids(),
        vec![PaneId::Terminal(2), replacement, PaneId::Terminal(3)],
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
    panes.connected_clients.borrow_mut().insert(1);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));
    panes.focus_pane(PaneId::Terminal(1), 1);

    assert_eq!(panes.active_pane_id(1), Some(PaneId::Terminal(1)));
    assert!(panes.has_active_panes());
}

#[test]
fn closing_the_focused_overlay_falls_through_to_the_one_below_it() {
    let mut panes = overlay_panes(80, 20);
    panes.connected_clients.borrow_mut().insert(1);
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
    panes.focus_pane(PaneId::Terminal(2), 1);

    panes.move_clients_out_of_pane(PaneId::Terminal(2));
    panes.remove_pane(PaneId::Terminal(2));
    assert_eq!(panes.active_pane_id(1), Some(PaneId::Terminal(1)));

    panes.move_clients_out_of_pane(PaneId::Terminal(1));
    panes.remove_pane(PaneId::Terminal(1));
    assert_eq!(
        panes.active_pane_id(1),
        None,
        "the last overlay leaves nothing behind to focus"
    );
}

#[test]
fn an_overlay_reports_the_cells_it_owns_outside_the_tab_s_viewport() {
    // the tab occupies columns 10..90 of a 90-wide surface; an overlay across the whole surface
    // therefore paints cells the tab's own viewport does not cover
    let strip = GlobalPaneStrip {
        geom: geom(0, 0, 10, 20),
        edge: GlobalPaneEdge::Left,
    };
    let (mut panes, _display_area) = overlay_panes_with(80, 20, (10, 0), vec![strip]);
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
        .contains_key(&PaneId::Terminal(1)));
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
    let pane = panes.get(&PaneId::Terminal(1)).unwrap();
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
    let pane = panes.get(&PaneId::Terminal(1)).unwrap();
    assert!(!pane.borderless());
    assert_eq!(pane.get_content_offset(), Offset::frame(1));
}

#[test]
fn setting_an_overlay_borderless_travels_with_its_placement() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));

    assert!(panes.set_borderless(&PaneId::Terminal(1), true));
    let pane = panes.get(&PaneId::Terminal(1)).unwrap();
    assert!(pane.borderless());
    assert_eq!(pane.get_content_offset(), Offset::default());
    assert_eq!(
        panes
            .placement_of(&PaneId::Terminal(1))
            .and_then(|placement| placement.coordinates)
            .and_then(|coordinates| coordinates.borderless),
        Some(true),
        "the placement is what follows the pane to another tab"
    );

    assert!(panes.toggle_borderless(&PaneId::Terminal(1)));
    let pane = panes.get(&PaneId::Terminal(1)).unwrap();
    assert!(!pane.borderless());
    assert_eq!(pane.get_content_offset(), Offset::frame(1));

    assert!(
        !panes.set_borderless(&PaneId::Terminal(7), true),
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

    let replacement = PaneId::Terminal(9);
    panes
        .replace_pane(PaneId::Terminal(1), mock_pane(replacement))
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
    panes.connected_clients.borrow_mut().insert(1);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));
    panes.focus_pane(PaneId::Terminal(1), 1);

    add(
        &mut panes,
        2,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );

    assert_eq!(
        panes.active_pane_id(1),
        Some(PaneId::Terminal(1)),
        "the modal keeps the keyboard - it has no way to learn it lost it"
    );
    assert_eq!(
        panes.stack_layer_ids(),
        vec![PaneId::Terminal(2), PaneId::Terminal(1)],
        "and stays drawn over the overlay that opened after it, rather than under it"
    );
}

#[test]
fn focusing_an_overlay_under_a_live_modal_leaves_the_keyboard_where_it_is() {
    // the mouse-click and focus-by-id door onto the same rule: the overlay was added first, so
    // this is not the add path declining but `focus_pane` itself
    let mut panes = overlay_panes(80, 20);
    panes.connected_clients.borrow_mut().insert(1);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(&mut panes, 2, OverlayKind::Modal, coordinates("60%", "50%"));
    panes.focus_pane(PaneId::Terminal(2), 1);

    panes.focus_pane(PaneId::Terminal(1), 1);
    assert_eq!(panes.active_pane_id(1), Some(PaneId::Terminal(2)));
    assert_eq!(
        panes.stack_layer_ids(),
        vec![PaneId::Terminal(1), PaneId::Terminal(2)],
        "a declined focus raises nothing either"
    );

    panes.focus_pane_for_all_clients(PaneId::Terminal(1));
    assert_eq!(panes.active_pane_id(1), Some(PaneId::Terminal(2)));

    assert!(panes.focus_pane_if_exists(PaneId::Terminal(1), 1).is_ok());
    assert_eq!(
        panes.active_pane_id(1),
        Some(PaneId::Terminal(2)),
        "`focus_pane_if_exists` reports the pane is ours even when focus was declined - an Err \
         would send `Tab::focus_pane_with_id` on to focus a floating pane instead"
    );
}

#[test]
fn a_modal_opened_over_a_modal_takes_focus_and_gives_it_back_when_it_closes() {
    // the one thing that may outrank a live modal, and the reason it may: closing it walks focus
    // back down to the modal underneath, so a quit-confirmation over an open modal stays usable
    let mut panes = overlay_panes(80, 20);
    panes.connected_clients.borrow_mut().insert(1);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));
    panes.focus_pane(PaneId::Terminal(1), 1);

    add(&mut panes, 2, OverlayKind::Modal, coordinates("40%", "30%"));
    panes.focus_pane(PaneId::Terminal(2), 1);
    assert_eq!(panes.active_pane_id(1), Some(PaneId::Terminal(2)));
    assert_eq!(
        panes.stack_layer_ids(),
        vec![PaneId::Terminal(1), PaneId::Terminal(2)]
    );

    panes.move_clients_out_of_pane(PaneId::Terminal(2));
    panes.remove_pane(PaneId::Terminal(2));
    assert_eq!(panes.active_pane_id(1), Some(PaneId::Terminal(1)));
}

#[test]
fn a_modal_is_the_top_of_the_stack_and_not_merely_the_thing_that_outranks_it() {
    let mut panes = overlay_panes(80, 20);
    panes.connected_clients.borrow_mut().insert(1);
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
            PaneId::Terminal(1),
            PaneId::Terminal(3),
            PaneId::Terminal(5),
            PaneId::Terminal(2),
            PaneId::Terminal(4),
        ],
        "modals above non-modals, add order within each class"
    );
    assert_eq!(panes.topmost_modal_pane_id(), Some(PaneId::Terminal(4)));
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
        .get(&PaneId::Terminal(id))
        .unwrap()
        .position_and_size();
    Position::new(geom.y as i32, geom.x as u16)
}

fn xy(panes: &OverlayPanes, id: u32) -> (usize, usize) {
    let geom = panes
        .get(&PaneId::Terminal(id))
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

/// The point of dragging against the session surface rather than the tab's viewport: a popup can
/// be moved out of the way of what it is covering, including onto the cells a global pane owns.
#[test]
fn an_overlay_can_be_dragged_over_a_global_pane() {
    let strip = GlobalPaneStrip {
        geom: geom(0, 0, 10, 20),
        edge: GlobalPaneEdge::Left,
    };
    let (mut panes, _display_area) = overlay_panes_with(80, 20, (10, 0), vec![strip]);
    add(
        &mut panes,
        1,
        OverlayKind::Modal,
        placed_coordinates("30", "2", "20", "8"),
    );
    // x is stated relative to the surface origin, which starts at the strip
    assert_eq!(xy(&panes, 1), (30, 2), "pre-condition");

    let press = frame_corner(&panes, 1);
    panes.move_pane_with_mouse(press);
    let target = Position::new(press.line() as i32, press.column() as u16 - 28);
    panes.move_pane_with_mouse(target);
    panes.stop_moving_pane_with_mouse(target);

    assert_eq!(
        xy(&panes, 1),
        (2, 2),
        "the modal sits over the global pane's columns"
    );
}

/// The write-back is what makes this work: the drag rewrites the overlay's *coordinates*, so the
/// position it was dropped at is re-resolved on the next surface change rather than discarded.
/// Without it the overlay re-centres the moment the terminal is resized.
#[test]
fn a_dragged_overlay_stays_where_it_was_dropped_when_the_surface_grows() {
    let (mut panes, display_area) = overlay_panes_with(80, 20, (0, 0), vec![]);
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
    let resized = panes.get(&PaneId::Terminal(1)).unwrap().position_and_size();
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

    let placement = panes.placement_of(&PaneId::Terminal(1)).unwrap();
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
    let geom = panes.get(&PaneId::Terminal(1)).unwrap().position_and_size();
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

    panes.remove_pane(PaneId::Terminal(1));
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
        .change_pane_coordinates(PaneId::Terminal(1), plugin_placement("20%", "60%", "10"))
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
        .change_pane_coordinates(PaneId::Terminal(1), plugin_placement("20%", "40", "12"))
        .unwrap();

    let geom = panes.get(&PaneId::Terminal(1)).unwrap().position_and_size();
    assert_eq!(
        (geom.cols.as_usize(), geom.rows.as_usize()),
        (40, 12),
        "the size the plugin asked for"
    );
    assert_eq!(xy(&panes, 1), dropped_at, "at the position the user chose");
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
        .change_pane_coordinates(PaneId::Terminal(1), plugin_placement("20%", "40", "10"))
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

    let replacement = PaneId::Terminal(9);
    panes
        .replace_pane(PaneId::Terminal(1), mock_pane(replacement))
        .unwrap();
    panes
        .change_pane_coordinates(replacement, plugin_placement("20%", "60%", "10"))
        .unwrap();

    assert_eq!(xy(&panes, 9), dropped_at);
}

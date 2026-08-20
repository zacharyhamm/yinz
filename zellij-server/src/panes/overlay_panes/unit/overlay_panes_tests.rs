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
        Some(true),
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

    let replacement = PaneId::Terminal(9);
    panes
        .replace_pane(PaneId::Terminal(1), mock_pane(replacement))
        .unwrap();

    assert_eq!(
        panes.stack_layer_ids(),
        vec![replacement, PaneId::Terminal(2)],
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

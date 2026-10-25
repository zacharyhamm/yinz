use super::*;
use crate::os_input_output::AsyncReader;
use crate::pane::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::sixel::SixelImageStore;
use crate::panes::{LinkHandler, PaneId, TerminalPane};
use crate::PtyWriteInstruction;
use interprocess::local_socket::Stream as LocalSocketStream;
use std::path::PathBuf;
use yinz_bus::os_input_output::{
    ClientTransport, RemotePanes, ServerEnvironment, TerminalIo, TerminalProcesses,
    TerminalSupervision,
};
use yinz_runtime::channels::{unbounded, Receiver, SenderWithContext};
use yinz_types::diagnostics::ErrorContext;
use yinz_types::geometry::{Direction, Resize};
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::TerminalId;
use yinz_types::style::Palette;
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::IpcReceiverWithContext,
    yinz_ipc::ipc::ServerToClientMsg,
};
use {yinz_types::input::command::RunCommand, yinz_types::input::command::TerminalAction};

#[derive(Clone)]
struct FakeInputOutput {}

impl TerminalIo for FakeInputOutput {
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
    fn write_to_tty_stdin(&self, _id: TerminalId, _buf: &[u8]) -> Result<usize> {
        unimplemented!()
    }
    fn tcdrain(&self, _id: TerminalId) -> Result<()> {
        unimplemented!()
    }
}

impl TerminalProcesses for FakeInputOutput {
    fn spawn_terminal(
        &self,
        _file_to_open: TerminalAction,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        _default_editor: Option<PathBuf>,
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<ProcessId>)> {
        unimplemented!()
    }
    fn kill(&self, _pid: ProcessId) -> Result<()> {
        unimplemented!()
    }
    fn force_kill(&self, _pid: ProcessId) -> Result<()> {
        unimplemented!()
    }
    fn re_run_command_in_terminal(
        &self,
        _terminal_id: TerminalId,
        _run_command: RunCommand,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<ProcessId>)> {
        unimplemented!()
    }
    fn clear_terminal_id(&self, _terminal_id: TerminalId) -> Result<()> {
        unimplemented!()
    }
    fn send_sigint(&self, _pid: ProcessId) -> Result<()> {
        unimplemented!()
    }
    fn send_sigcont(&self, _pid: ProcessId) -> Result<()> {
        unimplemented!()
    }
}

impl TerminalSupervision for FakeInputOutput {}

impl RemotePanes for FakeInputOutput {}

impl ClientTransport for FakeInputOutput {
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
}

impl ServerEnvironment for FakeInputOutput {
    fn load_palette(&self) -> Palette {
        unimplemented!()
    }
    fn get_cwd(&self, _pid: ProcessId) -> Option<PathBuf> {
        unimplemented!()
    }
    fn write_to_file(&mut self, _buf: String, _name: Option<String>) -> Result<()> {
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
        Rc::new(RefCell::new(Default::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(None)),
        false,
        ModeInfo::default(),
        Style::default(),
        Box::new(FakeInputOutput {}),
        ThreadSenders::default().silently_fail_on_send(),
    );
    (panes, display_area)
}

/// What the pty writer was told - `resize_pty!` sends `ResizePty` over the senders, never
/// through the os api, so this is the only place a pane's new size can be observed.
type PtyWrites = Receiver<(PtyWriteInstruction, ErrorContext)>;

/// The collection with its pty-writer channel captured, so a test can see which panes were told
/// their new size.
fn overlay_panes_with_pty_capture(cols: usize, rows: usize) -> (OverlayPanes, PtyWrites) {
    let (mut panes, _) = overlay_panes_with(
        cols,
        rows,
        Viewport {
            x: 0,
            y: 0,
            cols,
            rows,
        },
    );
    let (tx, rx) = unbounded();
    panes
        .senders
        .replace_to_pty_writer(SenderWithContext::new(tx));
    (panes, rx)
}

/// Every `ResizePty` sent since the last drain, as `(terminal, cols, rows)`, in order.
fn resized_ptys(rx: &PtyWrites) -> Vec<(TerminalId, u16, u16)> {
    let mut out = Vec::new();
    while let Ok((instruction, _)) = rx.try_recv() {
        if let PtyWriteInstruction::ResizePty(id, cols, rows, _, _) = instruction {
            out.push((id, cols, rows));
        }
    }
    out
}

fn add(panes: &mut OverlayPanes, id: u32, kind: OverlayKind, coords: FloatingPaneCoordinates) {
    let pane_id = PaneId::Terminal(crate::TerminalId::new(id));
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
    let pane = panes
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
        .unwrap();
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
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            PaneId::Terminal(crate::TerminalId::new(1)),
            coordinates("30%", "25%"),
            false,
        )
        .unwrap();
    assert_eq!(
        panes.kind_of(&PaneId::Terminal(crate::TerminalId::new(1))),
        Some(OverlayKind::Modal),
        "a coordinate change must not be able to demote a modal"
    );
    assert!(panes.is_modal(&PaneId::Terminal(crate::TerminalId::new(1))));
    assert_eq!(
        panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            PaneId::Terminal(crate::TerminalId::new(1)),
            PaneId::Terminal(crate::TerminalId::new(2)),
            PaneId::Terminal(crate::TerminalId::new(3))
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
        vec![PaneId::Terminal(crate::TerminalId::new(1))]
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
        .remove_pane(PaneId::Terminal(crate::TerminalId::new(1)))
        .is_some());
    assert!(!panes.panes_contain(&PaneId::Terminal(crate::TerminalId::new(1))));
    assert_eq!(
        panes.stack_layer_ids(),
        vec![PaneId::Terminal(crate::TerminalId::new(2))]
    );
    assert_eq!(
        panes.get_pane_z_index(PaneId::Terminal(crate::TerminalId::new(2))),
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

    let replacement = PaneId::Terminal(crate::TerminalId::new(9));
    panes
        .replace_pane(
            PaneId::Terminal(crate::TerminalId::new(1)),
            mock_pane(replacement),
        )
        .unwrap();

    assert_eq!(
        panes.stack_layer_ids(),
        vec![
            PaneId::Terminal(crate::TerminalId::new(2)),
            replacement,
            PaneId::Terminal(crate::TerminalId::new(3))
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

/// `focus_pane_for_joining_client` is `focus_pane` for a client that has just attached: it gives
/// the client an overlay to be on *unless it already has one*, which `focus_pane` would happily
/// replace. It still refuses what may not be focused, a hidden overlay included.
#[test]
fn a_joining_client_does_not_steal_focus_a_client_already_has() {
    let mut panes = overlay_panes(80, 20);
    for client in 1..=3 {
        panes
            .connected_clients
            .borrow_mut()
            .insert(crate::ClientId::new(client));
    }
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    add(
        &mut panes,
        2,
        OverlayKind::Dismissible,
        coordinates("40%", "40%"),
    );
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        crate::ClientId::new(1),
    );

    panes.focus_pane_for_joining_client(
        PaneId::Terminal(crate::TerminalId::new(2)),
        crate::ClientId::new(1),
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(1))),
        "a client that already has an overlay keeps it"
    );

    panes.focus_pane_for_joining_client(
        PaneId::Terminal(crate::TerminalId::new(2)),
        crate::ClientId::new(2),
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(2)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "a client with no overlay is given the one it joined on"
    );

    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(2)));
    panes.focus_pane_for_joining_client(
        PaneId::Terminal(crate::TerminalId::new(2)),
        crate::ClientId::new(3),
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(3)),
        None,
        "a hidden overlay may not be focused, joining or not"
    );
}

#[test]
fn closing_the_focused_overlay_falls_through_to_the_one_below_it() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId::new(1));
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
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        crate::ClientId::new(1),
    );

    panes.move_clients_out_of_pane(PaneId::Terminal(crate::TerminalId::new(2)));
    panes.remove_pane(PaneId::Terminal(crate::TerminalId::new(2)));
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(1)))
    );

    panes.move_clients_out_of_pane(PaneId::Terminal(crate::TerminalId::new(1)));
    panes.remove_pane(PaneId::Terminal(crate::TerminalId::new(1)));
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
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
        .contains_key(&PaneId::Terminal(crate::TerminalId::new(1))));
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
    let pane = panes
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
        .unwrap();
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
    let pane = panes
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
        .unwrap();
    assert!(!pane.borderless());
    assert_eq!(pane.get_content_offset(), Offset::frame(1));
}

#[test]
fn setting_an_overlay_borderless_travels_with_its_placement() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));

    assert!(panes.set_borderless(&PaneId::Terminal(crate::TerminalId::new(1)), true));
    let pane = panes
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
        .unwrap();
    assert!(pane.borderless());
    assert_eq!(pane.get_content_offset(), Offset::default());
    assert_eq!(
        panes
            .placement_of(&PaneId::Terminal(crate::TerminalId::new(1)))
            .and_then(|placement| placement.coordinates)
            .and_then(|coordinates| coordinates.borderless),
        Some(true),
        "the placement is what follows the pane to another tab"
    );

    assert!(panes.toggle_borderless(&PaneId::Terminal(crate::TerminalId::new(1))));
    let pane = panes
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
        .unwrap();
    assert!(!pane.borderless());
    assert_eq!(pane.get_content_offset(), Offset::frame(1));

    assert!(
        !panes.set_borderless(&PaneId::Terminal(crate::TerminalId::new(7)), true),
        "a pane that is not here is reported as not here, so the caller can look elsewhere"
    );
}

#[test]
fn coordinate_updates_preserve_borderlessness_unless_explicitly_overridden() {
    for direction in [
        None,
        Some(MultipaneDirection::LeftRight),
        Some(MultipaneDirection::TopBottom),
    ] {
        for batch in [false, true] {
            let (mut panes, children) = if let Some(direction) = direction {
                let (panes, children, _) = composed(direction);
                (panes, children.to_vec())
            } else {
                let mut panes = overlay_panes(120, 40);
                add(
                    &mut panes,
                    1,
                    OverlayKind::Dismissible,
                    coordinates("50", "20"),
                );
                (panes, vec![PaneId::Terminal(TerminalId::new(1))])
            };
            for (step, (borderless, expected)) in [
                (Some(true), true),
                (None, true),
                (Some(false), false),
                (None, false),
            ]
            .into_iter()
            .enumerate()
            {
                let mut coordinates = placed_coordinates("5", "3", &(60 + step).to_string(), "20");
                coordinates.borderless = borderless;
                if batch {
                    panes
                        .change_pane_coordinates_batch(
                            children
                                .iter()
                                .map(|id| (*id, coordinates.clone()))
                                .collect(),
                            false,
                        )
                        .unwrap();
                } else {
                    panes
                        .change_pane_coordinates(children[0], coordinates, false)
                        .unwrap();
                }
                panes.reapply_coordinates();
                let outer = panes.outer_geom(children[0]).unwrap();
                assert_eq!(outer.cols.as_usize(), 60 + step);
                assert_eq!(
                    panes.overlay(&children[0]).unwrap().coordinates.borderless,
                    Some(expected)
                );
                assert_eq!(
                    panes.position_is_on_frame(
                        children[0],
                        &Position::new(outer.y as i32, outer.x as u16)
                    ),
                    !expected,
                );
                for child in &children {
                    let pane = panes.get(child).unwrap();
                    assert_eq!(pane.borderless(), expected);
                    assert_eq!(
                        pane.get_content_offset(),
                        if expected || direction.is_some() {
                            Offset::default()
                        } else {
                            Offset::frame(1)
                        }
                    );
                }
                if direction.is_some() {
                    let first = panes.get(&children[0]).unwrap().current_geom();
                    let inset = usize::from(!expected);
                    assert_eq!((first.x, first.y), (outer.x + inset, outer.y + inset));
                }
            }
        }
    }
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

    let replacement = PaneId::Terminal(crate::TerminalId::new(9));
    panes
        .replace_pane(
            PaneId::Terminal(crate::TerminalId::new(1)),
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
        .insert(crate::ClientId::new(1));
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        crate::ClientId::new(1),
    );

    add(
        &mut panes,
        2,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );

    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(1))),
        "the modal keeps the keyboard - it has no way to learn it lost it"
    );
    assert_eq!(
        panes.stack_layer_ids(),
        vec![
            PaneId::Terminal(crate::TerminalId::new(2)),
            PaneId::Terminal(crate::TerminalId::new(1))
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
        .insert(crate::ClientId::new(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(&mut panes, 2, OverlayKind::Modal, coordinates("60%", "50%"));
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        crate::ClientId::new(1),
    );

    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        crate::ClientId::new(1),
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2)))
    );
    assert_eq!(
        panes.stack_layer_ids(),
        vec![
            PaneId::Terminal(crate::TerminalId::new(1)),
            PaneId::Terminal(crate::TerminalId::new(2))
        ],
        "a declined focus raises nothing either"
    );

    panes.focus_pane_for_all_clients(PaneId::Terminal(crate::TerminalId::new(1)));
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2)))
    );

    assert!(panes
        .focus_pane_if_exists(
            PaneId::Terminal(crate::TerminalId::new(1)),
            crate::ClientId::new(1)
        )
        .is_ok());
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
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
        .insert(crate::ClientId::new(1));
    add(&mut panes, 1, OverlayKind::Modal, coordinates("60%", "50%"));
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        crate::ClientId::new(1),
    );

    add(&mut panes, 2, OverlayKind::Modal, coordinates("40%", "30%"));
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        crate::ClientId::new(1),
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2)))
    );
    assert_eq!(
        panes.stack_layer_ids(),
        vec![
            PaneId::Terminal(crate::TerminalId::new(1)),
            PaneId::Terminal(crate::TerminalId::new(2))
        ]
    );

    panes.move_clients_out_of_pane(PaneId::Terminal(crate::TerminalId::new(2)));
    panes.remove_pane(PaneId::Terminal(crate::TerminalId::new(2)));
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(1)))
    );
}

#[test]
fn a_modal_is_the_top_of_the_stack_and_not_merely_the_thing_that_outranks_it() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId::new(1));
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
            PaneId::Terminal(crate::TerminalId::new(1)),
            PaneId::Terminal(crate::TerminalId::new(3)),
            PaneId::Terminal(crate::TerminalId::new(5)),
            PaneId::Terminal(crate::TerminalId::new(2)),
            PaneId::Terminal(crate::TerminalId::new(4)),
        ],
        "modals above non-modals, add order within each class"
    );
    assert_eq!(
        panes.topmost_modal_pane_id(),
        Some(PaneId::Terminal(crate::TerminalId::new(4)))
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
        .get(&PaneId::Terminal(crate::TerminalId::new(id)))
        .unwrap()
        .position_and_size();
    Position::new(geom.y as i32, geom.x as u16)
}

fn xy(panes: &OverlayPanes, id: u32) -> (usize, usize) {
    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId::new(id)))
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
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
        .placement_of(&PaneId::Terminal(crate::TerminalId::new(1)))
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
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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

    panes.remove_pane(PaneId::Terminal(crate::TerminalId::new(1)));
    assert!(!panes.pane_is_being_moved_with_mouse());
}

// ---- a dragged position is the user's ---------------------------------------------------------

/// What a plugin re-asserting its placement sends: a size, a `y`, and no `x` at all - the shape
/// `new-agent` sends on every `TabUpdate`.
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
            PaneId::Terminal(crate::TerminalId::new(1)),
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
            PaneId::Terminal(crate::TerminalId::new(1)),
            plugin_placement("20%", "40", "12"),
            false,
        )
        .unwrap();

    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
            PaneId::Terminal(crate::TerminalId::new(1)),
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
            PaneId::Terminal(crate::TerminalId::new(1)),
            &[grow(Direction::Right)],
            (5, 2),
        )
        .unwrap();
    let resized_to = size(&panes, 1);

    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId::new(1)),
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
            PaneId::Terminal(crate::TerminalId::new(1)),
            placed_coordinates("10", "5", "40", "10"),
            true,
        )
        .unwrap();

    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId::new(1)),
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
            PaneId::Terminal(crate::TerminalId::new(1)),
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

    let replacement = PaneId::Terminal(crate::TerminalId::new(9));
    panes
        .replace_pane(
            PaneId::Terminal(crate::TerminalId::new(1)),
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

fn size(panes: &OverlayPanes, id: u32) -> (usize, usize) {
    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId::new(id)))
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
            PaneId::Terminal(crate::TerminalId::new(1)),
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
            PaneId::Terminal(crate::TerminalId::new(1)),
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
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
        .unwrap()
        .position_and_size();

    assert!(!panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId::new(1)),
            &[grow(Direction::Right)],
            (5, 2)
        )
        .unwrap());
    assert_eq!(
        panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size(),
        before
    );
    assert!(!panes.is_resizable(&PaneId::Terminal(crate::TerminalId::new(1))));
    assert!(
        !panes.is_resizable(&PaneId::Terminal(crate::TerminalId::new(7))),
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
            PaneId::Terminal(crate::TerminalId::new(1)),
            &[grow(Direction::Right)],
            (5, 2),
        )
        .unwrap();
    let resized_to = size(&panes, 1);

    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId::new(1)),
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
            PaneId::Terminal(crate::TerminalId::new(1)),
            &[grow(Direction::Right)],
            (5, 2),
        )
        .unwrap();
    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId::new(1)),
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
            PaneId::Terminal(crate::TerminalId::new(1)),
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
            PaneId::Terminal(crate::TerminalId::new(1)),
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
            PaneId::Terminal(crate::TerminalId::new(1)),
            &[shrink(Direction::Right)],
            (400, 0),
        )
        .unwrap();
    assert!(!panes
        .resize_pane_with_strategies(
            PaneId::Terminal(crate::TerminalId::new(1)),
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
            PaneId::Terminal(crate::TerminalId::new(1)),
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
            PaneId::Terminal(crate::TerminalId::new(1)),
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
            .resize_active_pane(crate::ClientId::new(1), &grow(Direction::Right))
            .unwrap(),
        "the client is not on an overlay"
    );

    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        crate::ClientId::new(1),
    );
    assert!(panes
        .resize_active_pane(crate::ClientId::new(1), &grow(Direction::Right))
        .unwrap());
    assert_eq!(size(&panes, 1), (53, 10));
}

// ---- per-pane suppression --------------------------------------------------------------------
//
// `hide_pane` sets one overlay aside without unloading it, so agent's Esc can put the sidebar away
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
    assert!(panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1))));

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

    assert!(panes.panes_contain(&PaneId::Terminal(crate::TerminalId::new(1))));
    assert!(panes
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
        .is_some());
    assert!(
        panes
            .placement_of(&PaneId::Terminal(crate::TerminalId::new(1)))
            .is_some(),
        "still an overlay, and still able to say how it is placed"
    );
    assert!(panes.is_hidden(&PaneId::Terminal(crate::TerminalId::new(1))));
}

#[test]
fn hiding_an_overlay_moves_its_clients_to_the_one_below_it() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId::new(1));
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
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        crate::ClientId::new(1),
    );

    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(2)));
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(1))),
        "focus walks down to the overlay still on screen"
    );

    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1)));
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
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
        .insert(crate::ClientId::new(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1)));

    assert!(!panes.may_be_focused(&PaneId::Terminal(crate::TerminalId::new(1))));
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        crate::ClientId::new(1),
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        None,
        "the focus was refused"
    );

    assert!(panes.show_pane(PaneId::Terminal(crate::TerminalId::new(1))));
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        crate::ClientId::new(1),
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(1)))
    );
}

#[test]
fn focusing_a_hidden_overlay_by_id_reveals_it() {
    // the reveal path: `LaunchOrFocusPlugin` naming a hidden agent is the user asking for it back,
    // and `Tab::focus_pane_with_id` routes through here
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId::new(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1)));

    panes
        .focus_pane_if_exists(
            PaneId::Terminal(crate::TerminalId::new(1)),
            crate::ClientId::new(1),
        )
        .expect("the overlay is ours");
    assert!(
        !panes.is_hidden(&PaneId::Terminal(crate::TerminalId::new(1))),
        "shown"
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(1))),
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
    panes.set_pane_being_moved_with_mouse(PaneId::Terminal(crate::TerminalId::new(1)), corner);
    assert!(panes.pane_is_being_moved_with_mouse());

    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1)));
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
        Some(PaneId::Terminal(crate::TerminalId::new(1)))
    );
    assert!(panes.has_panes_outside_viewport());

    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1)));
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
        .insert(crate::ClientId::new(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        crate::ClientId::new(1),
    );
    let visible = panes.pane_info(&HashMap::new());
    assert!(!visible[0].is_suppressed);

    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1)));
    let hidden = panes.pane_info(&HashMap::new());
    assert_eq!(
        hidden.len(),
        1,
        "still reported, so a plugin can see the state"
    );
    assert!(
        hidden[0].is_suppressed,
        "the one bit a plugin can read to tell it was hidden - agent keys its reveal off this"
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
    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1)));

    *display_area.borrow_mut() = Size {
        rows: 40,
        cols: 160,
    };
    panes.reapply_coordinates();
    panes.show_pane(PaneId::Terminal(crate::TerminalId::new(1)));

    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1)));
    panes
        .replace_pane(
            PaneId::Terminal(crate::TerminalId::new(1)),
            mock_pane(PaneId::Terminal(crate::TerminalId::new(2))),
        )
        .expect("replaced");

    assert!(
        panes.is_hidden(&PaneId::Terminal(crate::TerminalId::new(2))),
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
        .insert(crate::ClientId::new(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1)));
    add(&mut panes, 2, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        crate::ClientId::new(1),
    );

    assert_eq!(
        panes.topmost_modal_pane_id(),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "a hidden overlay is not in the running for the top of the stack either way"
    );
    panes.show_pane(PaneId::Terminal(crate::TerminalId::new(1)));
    assert_eq!(
        *panes.stack_layer_ids().last().unwrap(),
        PaneId::Terminal(crate::TerminalId::new(2)),
        "coming back puts it at the top of its own class, which is still below the modal"
    );
    assert!(
        !panes.may_be_focused(&PaneId::Terminal(crate::TerminalId::new(1))),
        "and the modal keeps the keyboard"
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2)))
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
        .insert(crate::ClientId::new(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(&mut panes, 2, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        crate::ClientId::new(1),
    );

    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(2)));
    panes.show_pane(PaneId::Terminal(crate::TerminalId::new(2)));

    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
        "the modal is back, and a modal that is back is a modal that is focused"
    );
    assert_eq!(
        panes.stack_layer_ids(),
        vec![
            PaneId::Terminal(crate::TerminalId::new(1)),
            PaneId::Terminal(crate::TerminalId::new(2))
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
        .insert(crate::ClientId::new(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(&mut panes, 2, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        crate::ClientId::new(1),
    );

    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(2)));
    panes.show_pane(PaneId::Terminal(crate::TerminalId::new(2)));

    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        crate::ClientId::new(1),
    );
    assert!(
        !panes.may_be_focused(&PaneId::Terminal(crate::TerminalId::new(1))),
        "the lock is armed again - the modal is back in the stack"
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(2))),
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
        .insert(crate::ClientId::new(1));
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    add(&mut panes, 2, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        crate::ClientId::new(1),
    );

    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(2)));

    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
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
        .insert(crate::ClientId::new(1));
    add(&mut panes, 1, OverlayKind::Modal, coordinates("50%", "50%"));
    add(&mut panes, 2, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        crate::ClientId::new(1),
    );

    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(2)));

    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(1)))
    );
}

#[test]
fn showing_a_modal_gives_it_to_every_connected_client() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId::new(1));
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId::new(2));
    add(&mut panes, 1, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.focus_pane_for_all_clients(PaneId::Terminal(crate::TerminalId::new(1)));

    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1)));
    panes.show_pane(PaneId::Terminal(crate::TerminalId::new(1)));

    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(1)))
    );
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(2)),
        Some(PaneId::Terminal(crate::TerminalId::new(1))),
        "a modal is focused for everybody, and coming back is no different from opening"
    );
}

/// The other half of the contract, and the one agent depends on: showing an *ordinary* overlay is
/// still only about drawing it. agent restores its agent list beside itself without wanting the
/// keyboard to leave the conversation.
#[test]
fn showing_a_hidden_dismissible_overlay_still_leaves_the_keyboard_alone() {
    let mut panes = overlay_panes(80, 20);
    panes
        .connected_clients
        .borrow_mut()
        .insert(crate::ClientId::new(1));
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
    panes.focus_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        crate::ClientId::new(1),
    );
    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(2)));

    panes.show_pane(PaneId::Terminal(crate::TerminalId::new(2)));

    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(1))),
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
        .insert(crate::ClientId::new(1));
    add(&mut panes, 1, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1)));

    assert!(panes
        .focus_pane_if_exists(
            PaneId::Terminal(crate::TerminalId::new(1)),
            crate::ClientId::new(1)
        )
        .is_ok());
    assert!(!panes.is_hidden(&PaneId::Terminal(crate::TerminalId::new(1))));
    assert_eq!(
        panes.active_pane_id(crate::ClientId::new(1)),
        Some(PaneId::Terminal(crate::TerminalId::new(1)))
    );
}

// ---- an overlay that is placed by its owner ---------------------------------------------------
//
// A launch that named no coordinates is born half the surface, *centered*. For agent's pair of
// corner panels that is squarely on top of each other, and only their owner moves them out of
// there - a round trip later, once the pane is on a tab for the placement to find. The flag says
// "do not paint me until somebody has placed me"; see `OverlayCarry::awaiting_placement`.

fn add_awaiting_placement(panes: &mut OverlayPanes, id: u32) {
    let pane_id = PaneId::Terminal(crate::TerminalId::new(id));
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
        panes.panes_contain(&PaneId::Terminal(crate::TerminalId::new(1))),
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

    assert!(!panes.is_hidden(&PaneId::Terminal(crate::TerminalId::new(1))));
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
            PaneId::Terminal(crate::TerminalId::new(1)),
            coordinates("60%", "50%"),
            false,
        )
        .unwrap();

    assert_eq!(
        panes.rendered_pane_ids(),
        vec![PaneId::Terminal(crate::TerminalId::new(1))]
    );
    assert_eq!(
        panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
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
        vec![PaneId::Terminal(crate::TerminalId::new(1))]
    );
}

/// It travels, so a pane that was still waiting when the user switched tabs is still waiting when
/// it arrives - and one that had been placed does not go blank on the way across.
#[test]
fn awaiting_placement_survives_a_move_between_collections() {
    let mut panes = overlay_panes(80, 20);
    add_awaiting_placement(&mut panes, 1);
    let carry = panes.carry_of(&PaneId::Terminal(crate::TerminalId::new(1)));
    assert!(carry.awaiting_placement);

    let pane = panes
        .remove_pane(PaneId::Terminal(crate::TerminalId::new(1)))
        .unwrap();
    let mut elsewhere = overlay_panes(80, 20);
    elsewhere.add_pane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        pane,
        OverlayKind::Dismissible,
        FloatingPaneCoordinates::default(),
        carry,
    );
    assert!(elsewhere.rendered_pane_ids().is_empty());

    elsewhere
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId::new(1)),
            coordinates("60%", "50%"),
            false,
        )
        .unwrap();
    let carry = elsewhere.carry_of(&PaneId::Terminal(crate::TerminalId::new(1)));
    assert!(
        !carry.awaiting_placement,
        "and a placed overlay carries that across rather than going blank on the next move"
    );
}

fn geom(panes: &OverlayPanes, id: u32) -> (usize, usize, usize, usize) {
    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId::new(id)))
        .unwrap()
        .position_and_size();
    (geom.x, geom.y, geom.cols.as_usize(), geom.rows.as_usize())
}

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

fn drawn(panes: &OverlayPanes, id: u32) -> (usize, usize, usize, usize) {
    let geom = panes
        .get(&PaneId::Terminal(crate::TerminalId::new(id)))
        .unwrap()
        .current_geom();
    (geom.x, geom.y, geom.cols.as_usize(), geom.rows.as_usize())
}

#[test]
fn expanding_an_overlay_tells_its_pty_the_viewport_size() {
    let (mut panes, pty) = overlay_panes_with_pty_capture(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        placed_coordinates("10", "5", "30", "8"),
    );
    let _ = resized_ptys(&pty);

    assert!(panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1))));
    assert_eq!(drawn(&panes, 1), (0, 0, 80, 20));
    assert_eq!(
        resized_ptys(&pty),
        vec![(TerminalId::new(1), 78, 18)],
        "the program inside is told the expanded content size"
    );

    assert!(panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1))));
    assert_eq!(
        resized_ptys(&pty),
        vec![(TerminalId::new(1), 28, 6)],
        "and its own content size, inside its frame again, when it is put back"
    );
}

#[test]
fn hiding_and_showing_an_overlay_repaints_the_others() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("60%", "50%"),
    );
    add(
        &mut panes,
        2,
        OverlayKind::Dismissible,
        coordinates("40%", "40%"),
    );
    let bystander = PaneId::Terminal(crate::TerminalId::new(2));

    panes.get_mut(&bystander).unwrap().set_should_render(false);
    assert!(panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1))));
    assert!(
        panes.get(&bystander).unwrap().should_render(),
        "the overlay that stayed is repainted when another is hidden"
    );

    panes.get_mut(&bystander).unwrap().set_should_render(false);
    assert!(panes.show_pane(PaneId::Terminal(crate::TerminalId::new(1))));
    assert!(
        panes.get(&bystander).unwrap().should_render(),
        "and again when it comes back"
    );
}

#[test]
fn a_frame_drag_repaints_the_overlays_it_did_not_move() {
    let mut panes = overlay_panes(80, 20);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        placed_coordinates("10", "5", "30", "8"),
    );
    add(
        &mut panes,
        2,
        OverlayKind::Dismissible,
        placed_coordinates("50", "12", "20", "6"),
    );
    let bystander = PaneId::Terminal(crate::TerminalId::new(2));
    panes.get_mut(&bystander).unwrap().set_should_render(false);

    drag_by(&mut panes, 1, 4, 3);

    assert_eq!(geom(&panes, 1), (14, 8, 30, 8), "the drag happened");
    assert!(
        panes.get(&bystander).unwrap().should_render(),
        "the overlay that did not move is repainted"
    );
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

    assert!(panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1))));

    assert_eq!(
        drawn(&panes, 1),
        (0, 1, 80, 18),
        "the tab bar and the status bar keep their rows, as they do for a tiled pane"
    );
    assert!(panes.pane_is_fullscreen(&PaneId::Terminal(crate::TerminalId::new(1))));
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

    assert!(panes.toggle_pane_no_ui_fullscreen(PaneId::Terminal(crate::TerminalId::new(1))));

    assert_eq!(drawn(&panes, 1), (0, 0, 80, 20));
    assert!(panes.fullscreen_covers_ui());
}

#[test]
fn the_two_keystrokes_switch_between_each_other() {
    let mut panes = overlay_panes_with_chrome(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));
    panes.toggle_pane_no_ui_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));
    assert!(panes.fullscreen_covers_ui());
    assert_eq!(drawn(&panes, 1), (0, 0, 80, 20));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));
    assert!(panes.fullscreen_is_active());
    assert!(!panes.fullscreen_covers_ui());
    assert_eq!(drawn(&panes, 1), (0, 1, 80, 18));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));
    assert!(!panes.fullscreen_is_active());
}

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
    panes.toggle_pane_no_ui_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));
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

    panes.toggle_pane_no_ui_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));

    assert_eq!(
        geom(&panes, 1),
        geom(&untouched, 1),
        "and what comes back is what a never-expanded twin resolved to on the same surface"
    );
    assert_eq!(drawn(&panes, 1), geom(&panes, 1), "with no override left");
}

#[test]
fn an_expansion_claims_nothing_and_a_plugin_keeps_placing_underneath_it() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));
    assert_eq!(
        claims(&panes, 1),
        (false, false),
        "an expansion claims nothing"
    );

    panes
        .change_pane_coordinates(
            PaneId::Terminal(crate::TerminalId::new(1)),
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

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));
    assert_eq!(
        geom(&panes, 1),
        (40, 6, 20, 6),
        "so the way back is where the plugin last put it, not where it was when it was expanded"
    );
}

#[test]
fn an_expanded_overlay_reports_the_rectangle_underneath_it() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    let group = HashMap::new();

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));

    let info = panes.pane_info(&group).into_iter().next().unwrap();
    assert!(info.is_fullscreen, "it says that it is expanded");
    assert_eq!(
        (
            info.pane_x.get(),
            info.pane_y.get(),
            info.pane_columns.get(),
            info.pane_rows.get()
        ),
        (2, 2, 30, 10),
        "and reports the rectangle it will come back to, not the one it is drawn with"
    );
    assert_eq!(
        (
            info.pane_content_x.get(),
            info.pane_content_y.get(),
            info.pane_content_columns.get(),
            info.pane_content_rows.get()
        ),
        (3, 3, 28, 8),
        "content area included - the frame it would wear there, not the one it wears now"
    );
}

#[test]
fn a_modal_may_not_be_expanded() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Modal, half(2));

    assert!(!panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1))));
    assert!(!panes.fullscreen_is_active());
    assert_eq!(drawn(&panes, 1), geom(&panes, 1));
}

#[test]
fn a_hidden_overlay_may_not_be_expanded() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    panes.hide_pane(PaneId::Terminal(crate::TerminalId::new(1)));

    assert!(!panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1))));
    assert!(!panes.fullscreen_is_active());
}

#[test]
fn closing_an_expanded_overlay_ends_the_expansion() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    add(&mut panes, 2, OverlayKind::Dismissible, half(40));
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));

    panes.remove_pane(PaneId::Terminal(crate::TerminalId::new(1)));

    assert!(!panes.fullscreen_is_active());
    assert_eq!(
        drawn(&panes, 2),
        geom(&panes, 2),
        "and the overlay it was covering is drawn as itself again"
    );
}

#[test]
fn a_pane_swapped_into_an_expanded_slot_is_expanded() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));

    panes
        .replace_pane(
            PaneId::Terminal(crate::TerminalId::new(1)),
            mock_pane(PaneId::Terminal(crate::TerminalId::new(9))),
        )
        .unwrap();

    assert!(panes.pane_is_fullscreen(&PaneId::Terminal(crate::TerminalId::new(9))));
    assert_eq!(drawn(&panes, 9), (0, 0, 80, 20));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(9)));
    assert_eq!(geom(&panes, 9), (2, 2, 30, 10));
}

#[test]
fn an_expanded_overlay_cannot_move_or_resize_its_outer_rectangle() {
    let (mut panes, children, _) = composed(MultipaneDirection::LeftRight);
    let (_, divider) = panes.multipane_info(&children[0]).unwrap().split.geometry(
        panes.outer_geom(children[0]).unwrap(),
        true,
        0,
    );
    let seam = Position::new(divider.y as i32, divider.x as u16);
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));
    let before = (geom(&panes, 1), geom(&panes, 2));

    assert!(
        !panes.move_pane_with_mouse(Position::new(0, 0)),
        "there is nowhere to move a pane that fills the screen"
    );
    assert!(!panes.is_resizable(&PaneId::Terminal(crate::TerminalId::new(1))));
    assert!(!panes
        .resize_pane_with_id(
            PaneId::Terminal(crate::TerminalId::new(1)),
            grow(Direction::Right)
        )
        .unwrap());
    assert_eq!(panes.seam_at(&seam), None, "the old seam is now content");

    assert_eq!(
        (geom(&panes, 1), geom(&panes, 2)),
        before,
        "so the rectangle underneath is exactly as it was"
    );
}

#[test]
fn an_expanded_overlay_does_not_cover_a_live_modal() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    add(&mut panes, 2, OverlayKind::Modal, half(40));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));

    assert_eq!(
        panes.stack_layer_ids(),
        vec![
            PaneId::Terminal(crate::TerminalId::new(1)),
            PaneId::Terminal(crate::TerminalId::new(2))
        ],
        "the modal is still the top of the stack"
    );
}

#[test]
fn an_expanded_overlay_stops_offering_a_resize() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    assert!(panes.is_resizable(&PaneId::Terminal(crate::TerminalId::new(1))));

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));

    assert!(!panes.is_resizable(&PaneId::Terminal(crate::TerminalId::new(1))));
}

#[test]
fn a_fullscreen_press_on_another_overlay_expands_that_one() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    add(&mut panes, 2, OverlayKind::Dismissible, half(40));
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));

    assert!(panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(2))));

    assert!(panes.pane_is_fullscreen(&PaneId::Terminal(crate::TerminalId::new(2))));
    assert_eq!(drawn(&panes, 2), (0, 0, 80, 20));
    assert!(!panes.pane_is_fullscreen(&PaneId::Terminal(crate::TerminalId::new(1))));
    assert_eq!(
        drawn(&panes, 1),
        geom(&panes, 1),
        "and the one that was expanded is back where its coordinates say"
    );

    assert!(panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(2))));
    assert!(
        !panes.fullscreen_is_active(),
        "a press naming the expanded panel still collapses it"
    );
}

#[test]
fn expanding_an_overlay_releases_a_drag_in_flight() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    assert!(panes.move_pane_with_mouse(frame_corner(&panes, 1)));
    assert!(panes.pane_is_being_moved_with_mouse());

    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));

    assert!(!panes.pane_is_being_moved_with_mouse());
}

#[test]
fn an_expanded_overlay_is_not_carried_by_a_drag_that_outlived_the_press() {
    let mut panes = overlay_panes(80, 20);
    add(&mut panes, 1, OverlayKind::Dismissible, half(2));
    panes.toggle_pane_fullscreen(PaneId::Terminal(crate::TerminalId::new(1)));
    let before = geom(&panes, 1);

    panes.set_pane_being_moved_with_mouse(
        PaneId::Terminal(crate::TerminalId::new(1)),
        Position::new(2, 2),
    );

    assert!(!panes.move_pane_to_position(&Position::new(9, 40)));
    assert_eq!(geom(&panes, 1), before);
}

fn composed(direction: MultipaneDirection) -> (OverlayPanes, [PaneId; 2], MultipaneId) {
    let mut panes = overlay_panes(120, 40);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50", "20"),
    );
    add(
        &mut panes,
        2,
        OverlayKind::Dismissible,
        coordinates("50", "20"),
    );
    let children = [
        PaneId::Terminal(TerminalId::new(1)),
        PaneId::Terminal(TerminalId::new(2)),
    ];
    let id = panes
        .create_multipane(
            children,
            MultipaneSplit {
                direction,
                ratio: 3700,
            },
            0,
            OverlayPlacement {
                background_opacity: None,
                kind: OverlayKind::Dismissible,
                coordinates: Some(placed_coordinates("10", "5", "80", "24")),
            },
        )
        .unwrap();
    (panes, children, id)
}

#[test]
fn multipane_composition_preserves_existing_terminal_contents() {
    assert_composition_preserves_contents(false);
}

#[test]
fn multipane_composition_preserves_existing_alternate_screen() {
    assert_composition_preserves_contents(true);
}

fn assert_composition_preserves_contents(alternate_screen: bool) {
    for direction in [MultipaneDirection::LeftRight, MultipaneDirection::TopBottom] {
        let mut panes = overlay_panes(120, 60);
        let children = [1, 2].map(|id| PaneId::Terminal(TerminalId::new(id)));
        for id in [1, 2] {
            add(
                &mut panes,
                id,
                OverlayKind::Dismissible,
                coordinates("40", "24"),
            );
        }
        let before = children.map(|child| {
            let pane = panes.get_mut(&child).unwrap();
            if alternate_screen {
                pane.handle_pty_bytes(b"\x1b[?1049h".to_vec());
            }
            // Fill the viewport and, on the normal screen, leave history above it. Composition
            // keeps each child's 38 x 22 content area, so neither reflow nor cursor motion is due.
            let content = (0..30)
                .map(|row| char::from(b'A' + row % 26).to_string().repeat(38))
                .collect::<Vec<_>>()
                .join("\r\n");
            pane.handle_pty_bytes(format!("{content}\x1b[H").into_bytes());
            assert_eq!(pane.cursor_coordinates(None), Some((1, 1, true)));
            (pane.dump_screen(false, None), pane.dump_screen(true, None))
        });
        let coordinates = match direction {
            MultipaneDirection::LeftRight => coordinates("79", "24"),
            MultipaneDirection::TopBottom => coordinates("40", "47"),
        };
        panes
            .create_multipane(
                children,
                MultipaneSplit {
                    direction,
                    ratio: 5000,
                },
                0,
                OverlayPlacement::dismissible(Some(coordinates)),
            )
            .unwrap();
        for (child, (viewport, history)) in children.into_iter().zip(before) {
            let pane = panes.get_mut(&child).unwrap();
            assert_eq!(pane.get_content_rows(), 22, "{direction:?}");
            assert_eq!(pane.get_content_columns(), 38, "{direction:?}");
            assert_eq!(pane.dump_screen(false, None), viewport, "{direction:?}");
            assert_eq!(pane.dump_screen(true, None), history, "{direction:?}");
            // The cursor's content-relative position is unchanged; only the old frame inset goes.
            assert_eq!(
                pane.cursor_coordinates(None),
                Some((0, 0, true)),
                "{direction:?}"
            );
            pane.handle_pty_bytes(b"!".to_vec());
            assert_eq!(
                pane.dump_screen(false, None),
                format!("!{}", &viewport[1..])
            );
            assert_eq!(pane.cursor_coordinates(None), Some((1, 0, true)));
        }
    }
}

#[test]
fn single_overlay_inherits_borderlessness_unless_coordinates_override_it() {
    for inherited in [false, true] {
        for requested in [None, Some(false), Some(true)] {
            let mut panes = overlay_panes(120, 40);
            let id = PaneId::Terminal(TerminalId::new(1));
            let mut pane = mock_pane(id);
            pane.set_borderless(inherited);
            let mut coordinates = coordinates("40", "24");
            coordinates.borderless = requested;
            panes.add_pane(
                id,
                pane,
                OverlayKind::Dismissible,
                coordinates,
                OverlayCarry::default(),
            );
            assert_single_overlay_border(&mut panes, id, requested.unwrap_or(inherited));
        }
    }
}

#[test]
fn single_overlay_replacement_inherits_borderlessness_unless_coordinates_override_it() {
    for inherited in [false, true] {
        for requested in [None, Some(false), Some(true)] {
            let mut panes = overlay_panes(120, 40);
            let mut coordinates = coordinates("40", "24");
            coordinates.borderless = requested;
            add(&mut panes, 1, OverlayKind::Dismissible, coordinates);
            let replacement_id = PaneId::Terminal(TerminalId::new(2));
            let mut replacement = mock_pane(replacement_id);
            replacement.set_borderless(inherited);
            panes
                .replace_pane(PaneId::Terminal(TerminalId::new(1)), replacement)
                .unwrap();
            assert_single_overlay_border(
                &mut panes,
                replacement_id,
                requested.unwrap_or(inherited),
            );
        }
    }
}

fn assert_single_overlay_border(panes: &mut OverlayPanes, id: PaneId, borderless: bool) {
    // Both fullscreen modes must restore the inherited/requested setting on exit.
    for covers_ui in [false, true] {
        let pane = panes.get(&id).unwrap();
        assert_eq!(pane.borderless(), borderless);
        assert_eq!(pane.get_content_columns(), if borderless { 40 } else { 38 });
        assert_eq!(pane.get_content_rows(), if borderless { 24 } else { 22 });
        let outer = panes.outer_geom(id).unwrap();
        assert_eq!(
            panes.position_is_on_frame(id, &Position::new(outer.y as i32, outer.x as u16)),
            !borderless,
        );
        assert!(panes.toggle_fullscreen_of(id, covers_ui));
        assert_eq!(panes.get(&id).unwrap().borderless(), borderless);
        assert!(panes.toggle_fullscreen_of(id, covers_ui));
    }
    let pane = panes.get(&id).unwrap();
    assert_eq!(pane.borderless(), borderless);
    assert_eq!(pane.get_content_columns(), if borderless { 40 } else { 38 });
    assert_eq!(pane.get_content_rows(), if borderless { 24 } else { 22 });
}

#[test]
fn multipane_frame_refresh_and_replacement_preserve_terminal_contents() {
    for direction in [MultipaneDirection::LeftRight, MultipaneDirection::TopBottom] {
        for operation in 0..4 {
            let (mut panes, children, _) = composed(direction);
            let before = children.map(|child| {
                let pane = panes.get_mut(&child).unwrap();
                let rows = pane.get_content_rows();
                let cols = pane.get_content_columns();
                let contents = (0..rows)
                    .map(|row| char::from(b'A' + row as u8).to_string().repeat(cols))
                    .collect::<Vec<_>>()
                    .join("\r\n");
                pane.handle_pty_bytes(format!("{contents}\x1b[H").into_bytes());
                assert_eq!(pane.cursor_coordinates(None), Some((0, 0, true)));
                (
                    pane.dump_screen(false, None),
                    pane.dump_screen(true, None),
                    pane.cursor_coordinates(None),
                    rows,
                    cols,
                    pane.current_geom(),
                )
            });
            for _ in 0..3 {
                match operation {
                    0 => panes.set_pane_frames().unwrap(),
                    1 => assert!(panes.set_borderless(&children[0], false)),
                    2 => {
                        let mut coordinates =
                            panes.overlay(&children[0]).unwrap().coordinates.clone();
                        coordinates.borderless = Some(false);
                        panes
                            .change_pane_coordinates(children[0], coordinates, false)
                            .unwrap();
                    },
                    _ => {
                        for child in children {
                            let replacement = PaneId::Terminal(TerminalId::new(3));
                            let original =
                                panes.replace_pane(child, mock_pane(replacement)).unwrap();
                            panes.replace_pane(replacement, original).unwrap();
                        }
                    },
                }
                for (child, expected) in children.iter().zip(&before) {
                    let pane = panes.get(child).unwrap();
                    assert_eq!(
                        (
                            pane.dump_screen(false, None),
                            pane.dump_screen(true, None),
                            pane.cursor_coordinates(None),
                            pane.get_content_rows(),
                            pane.get_content_columns(),
                            pane.current_geom(),
                        ),
                        *expected,
                        "{direction:?}, operation {operation}, child {child:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn multipane_owns_both_slots_and_structural_commands_resolve_either_member() {
    for direction in [MultipaneDirection::LeftRight, MultipaneDirection::TopBottom] {
        let (mut panes, children, id) = composed(direction);
        assert_eq!(panes.panes.len(), 1);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes.layers().len(), 1);
        assert_eq!(
            panes.get_pane_z_index(children[0]),
            panes.get_pane_z_index(children[1])
        );
        assert!(panes.remove_pane(children[1]).is_none());
        assert_eq!(panes.len(), 2);
        for child in children {
            panes.hide_pane(child);
            assert!(children.iter().all(|id| panes.is_hidden(id)));
            assert!(panes.layers().is_empty());
            panes.show_pane(child);
            assert!(children.iter().all(|id| !panes.is_hidden(id)));
            panes
                .resize_pane_with_id(child, grow(Direction::Right))
                .unwrap();
            assert_eq!(panes.multipane_info(&child).unwrap().split.ratio, 3700);
        }
        let before = panes.multipane_info(&children[0]).unwrap();
        assert!(panes
            .change_pane_coordinates_batch(
                vec![
                    (children[0], coordinates("30", "10")),
                    (children[1], coordinates("40", "10")),
                ],
                false
            )
            .is_err());
        assert_eq!(panes.multipane_info(&children[1]).unwrap(), before);
        assert!(panes
            .create_multipane(children, before.split, 0, OverlayPlacement::default())
            .is_err());
        assert_eq!(panes.multipane_info(&children[0]).unwrap().id, id);
    }
}

#[test]
fn multipane_divider_and_fullscreen_preserve_requested_ratio_through_tiny_surfaces() {
    for direction in [MultipaneDirection::LeftRight, MultipaneDirection::TopBottom] {
        let (mut panes, children, _) = composed(direction);
        let info = panes.multipane_info(&children[0]).unwrap();
        let outer = panes.outer_geom(children[0]).unwrap();
        let (_, divider) = info.split.geometry(outer, true, 0);
        let position = Position::new(divider.y as i32, divider.x as u16);
        assert_eq!(panes.seam_at(&position), Some((children[0], children[1])));
        panes.start_seam_drag(children[0], children[1], position);
        panes.drag_seam_to(&Position::new(
            position.line() as i32 + 2,
            position.column() as u16 + 5,
        ));
        let split = panes.multipane_info(&children[0]).unwrap().split;
        assert_ne!(split.ratio, info.split.ratio);
        assert_eq!(panes.outer_geom(children[0]), Some(outer));
        panes.toggle_pane_fullscreen(children[1]);
        for rows in 0..5 {
            for cols in 0..5 {
                *panes.display_area.borrow_mut() = Size { rows, cols };
                *panes.viewport.borrow_mut() = Viewport {
                    x: 0,
                    y: 0,
                    rows,
                    cols,
                };
                panes.reapply_coordinates();
                assert_eq!(panes.multipane_info(&children[0]).unwrap().split, split);
                for child in children {
                    let geom = panes.get(&child).unwrap().current_geom();
                    assert!(geom.x + geom.cols.as_usize() <= cols);
                    assert!(geom.y + geom.rows.as_usize() <= rows);
                }
            }
        }
        *panes.display_area.borrow_mut() = Size {
            rows: 40,
            cols: 120,
        };
        *panes.viewport.borrow_mut() = Viewport {
            x: 0,
            y: 0,
            rows: 40,
            cols: 120,
        };
        panes.reapply_coordinates();
        panes.unset_fullscreen();
        assert_eq!(panes.outer_geom(children[0]), Some(outer));
        assert_eq!(panes.multipane_info(&children[0]).unwrap().split, split);
    }
}

#[test]
fn multipane_transfer_and_replacement_keep_ownership_focus_and_placement() {
    let (mut panes, children, id) = composed(MultipaneDirection::LeftRight);
    let clients = [ClientId::new(1), ClientId::new(2)];
    panes.focus_pane(children[0], clients[0]);
    panes.focus_pane(children[1], clients[1]);
    panes
        .resize_pane_with_id(children[0], grow(Direction::Down))
        .unwrap();
    panes.toggle_pane_fullscreen(children[1]);
    let before = panes.multipane_info(&children[0]).unwrap();
    // both clients are still on the source tab, so both keep their half across the move
    panes.connected_clients.borrow_mut().extend(clients);
    let transfer = panes.take_container(children[1]).unwrap();
    assert!(panes.is_empty());
    let mut destination = overlay_panes(120, 40);
    destination.insert_container(transfer);
    for (client, child) in clients.into_iter().zip(children) {
        assert_eq!(destination.active_pane_id(client), Some(child));
    }
    assert_eq!(destination.multipane_info(&children[0]).unwrap(), before);
    assert!(destination.pane_is_fullscreen(&children[1]));
    let replacement = PaneId::Terminal(TerminalId::new(3));
    destination
        .replace_pane(children[1], mock_pane(replacement))
        .unwrap();
    assert_eq!(destination.active_pane_id(clients[1]), Some(replacement));
    assert_eq!(destination.multipane_info(&replacement).unwrap().id, id);
    assert_eq!(
        destination.multipane_info(&replacement).unwrap().children,
        [children[0], replacement]
    );
    destination.hide_pane(replacement);
    let transfer = destination.take_container(children[0]).unwrap();
    panes.insert_container(transfer);
    assert!(panes.is_hidden(&replacement));
    assert_eq!(
        panes.multipane_info(&replacement).unwrap().split,
        before.split
    );
}

/// A client that has already left the source tab does not take the keyboard back when the
/// container lands: that is agent carrying its panel after a tab switch, and the tab the client
/// went to keeps the keys. A client still on the source tab keeps its half.
#[test]
fn a_container_taken_after_its_client_left_arrives_unfocused() {
    let (mut panes, children, _) = composed(MultipaneDirection::LeftRight);
    let [stayed, left] = [ClientId::new(1), ClientId::new(2)];
    panes.connected_clients.borrow_mut().insert(stayed);
    panes.focus_pane(children[0], stayed);
    panes.focus_pane(children[0], left);

    let transfer = panes.take_container(children[0]).unwrap();
    let mut destination = overlay_panes(120, 40);
    destination.insert_container(transfer);

    assert_eq!(destination.active_pane_id(stayed), Some(children[0]));
    assert_eq!(
        destination.active_pane_id(left),
        None,
        "the client that switched away arrives on nothing"
    );
}

fn half(x: usize) -> FloatingPaneCoordinates {
    placed_coordinates(&x.to_string(), "2", "30", "10")
}

fn claims(panes: &OverlayPanes, id: u32) -> (bool, bool) {
    let carry = panes.carry_of(&PaneId::Terminal(TerminalId::new(id)));
    (carry.size_is_user_stated, carry.position_is_user_stated)
}

#[test]
fn multipane_renders_each_clients_focused_title_and_keeps_chrome_on_tiny_surfaces() {
    let clients = [ClientId::new(1), ClientId::new(2)];
    for direction in [MultipaneDirection::LeftRight, MultipaneDirection::TopBottom] {
        let (mut panes, children, _) = composed(direction);
        panes.connected_clients.borrow_mut().extend(clients);
        panes.connected_clients_in_app.borrow_mut().extend(clients);
        for ((client, child), title) in clients
            .into_iter()
            .zip(children)
            .zip(["LEFT CHILD", "RIGHT CHILD"])
        {
            panes.get_mut(&child).unwrap().update_name(title);
            panes.focus_pane(child, client);
        }
        let render = |panes: &mut OverlayPanes| {
            let mut output = Output::default();
            output.add_clients(
                &clients.into_iter().collect(),
                Rc::new(RefCell::new(LinkHandler::new())),
                None,
            );
            panes
                .render(
                    &mut output,
                    &HashMap::new(),
                    HashMap::new(),
                    None,
                    &HashMap::new(),
                    false,
                    0,
                    &HashMap::new(),
                )
                .unwrap();
            output.serialize().unwrap()
        };
        let rendered = render(&mut panes);
        assert!(rendered[&clients[0]].contains("LEFT CHILD"));
        assert!(!rendered[&clients[0]].contains("RIGHT CHILD"));
        assert!(rendered[&clients[1]].contains("RIGHT CHILD"));
        assert!(!rendered[&clients[1]].contains("LEFT CHILD"));
        panes.toggle_pane_fullscreen(children[0]);
        for rows in 0..5 {
            for cols in 0..5 {
                *panes.display_area.borrow_mut() = Size { rows, cols };
                *panes.viewport.borrow_mut() = Viewport {
                    x: 0,
                    y: 0,
                    rows,
                    cols,
                };
                panes.reapply_coordinates();
                let rendered = render(&mut panes);
                if rows > 0 && cols > 0 {
                    assert!(!rendered[&clients[0]].is_empty());
                }
                assert_eq!(panes.len(), 2);
            }
        }
    }
}

#[test]
fn multipane_modal_focus_is_trapped_at_the_container_boundary() {
    let mut panes = overlay_panes(120, 40);
    for id in [1, 2] {
        add(&mut panes, id, OverlayKind::Modal, coordinates("40", "20"));
    }
    add(
        &mut panes,
        3,
        OverlayKind::Dismissible,
        coordinates("20", "10"),
    );
    let children = [
        PaneId::Terminal(TerminalId::new(1)),
        PaneId::Terminal(TerminalId::new(2)),
    ];
    panes
        .create_multipane(
            children,
            MultipaneSplit {
                direction: MultipaneDirection::LeftRight,
                ratio: 5000,
            },
            0,
            OverlayPlacement {
                background_opacity: None,
                kind: OverlayKind::Modal,
                coordinates: None,
            },
        )
        .unwrap();
    let clients = [ClientId::new(1), ClientId::new(2)];
    panes.connected_clients.borrow_mut().extend(clients);
    for (client, child) in clients.into_iter().zip(children) {
        panes.focus_pane(child, client);
        panes.focus_pane(PaneId::Terminal(TerminalId::new(3)), client);
        assert_eq!(panes.active_pane_id(client), Some(child));
    }
    panes.hide_pane(children[0]);
    assert!(children.iter().all(|child| panes.is_hidden(child)));
    panes.show_pane(children[1]);
    for client in clients {
        assert_eq!(panes.active_pane_id(client), Some(children[1]));
    }
}

// ---- where a modal hands the keyboard back --------------------------------------------------

fn close(panes: &mut OverlayPanes, pane_id: PaneId) {
    panes.move_clients_out_of_pane(pane_id);
    panes.remove_pane(pane_id);
}

/// A modal opened from the first half of a multipane, as `new-agent` is from the agent
/// list, used to close onto whichever overlay was topmost - the panel's other half - so the next
/// key went into the conversation while the list still drew its selection.
#[test]
fn closing_a_modal_returns_the_keyboard_to_the_multipane_child_it_took_it_from() {
    let (mut panes, children, _) = composed(MultipaneDirection::LeftRight);
    let client = ClientId::new(1);
    panes.connected_clients.borrow_mut().insert(client);
    panes.focus_pane(children[0], client);
    add(&mut panes, 3, OverlayKind::Modal, coordinates("40", "10"));
    let modal = PaneId::Terminal(TerminalId::new(3));
    panes.focus_pane(modal, client);
    assert_eq!(panes.active_pane_id(client), Some(modal));

    close(&mut panes, modal);

    assert_eq!(
        panes.active_pane_id(client),
        Some(children[0]),
        "back on the half it came from, not the topmost one"
    );
}

/// The other side of the same rule: a client that was on the layers beneath goes back there,
/// rather than to an overlay that was on screen without having the keyboard.
#[test]
fn a_modal_opened_while_the_tiled_layer_had_the_keys_leaves_them_there_when_it_closes() {
    let (mut panes, _, _) = composed(MultipaneDirection::LeftRight);
    let client = ClientId::new(1);
    panes.connected_clients.borrow_mut().insert(client);
    add(&mut panes, 3, OverlayKind::Modal, coordinates("40", "10"));
    let modal = PaneId::Terminal(TerminalId::new(3));
    panes.focus_pane(modal, client);

    close(&mut panes, modal);

    assert_eq!(panes.active_pane_id(client), None);
}

/// Setting a modal aside hands the keyboard back the same way closing it does.
/// `hiding_a_modal_does_not_hand_the_keyboard_to_the_overlay_under_it` is the case where the
/// client came from the layers beneath.
#[test]
fn hiding_a_modal_hands_the_keyboard_back_to_the_overlay_it_took_it_from() {
    let mut panes = overlay_panes(80, 20);
    let client = ClientId::new(1);
    panes.connected_clients.borrow_mut().insert(client);
    for id in [1, 2] {
        add(
            &mut panes,
            id,
            OverlayKind::Dismissible,
            coordinates("50%", "50%"),
        );
    }
    panes.focus_pane(PaneId::Terminal(TerminalId::new(1)), client);
    add(&mut panes, 3, OverlayKind::Modal, coordinates("50%", "50%"));
    panes.focus_pane(PaneId::Terminal(TerminalId::new(3)), client);

    panes.hide_pane(PaneId::Terminal(TerminalId::new(3)));

    assert_eq!(
        panes.active_pane_id(client),
        Some(PaneId::Terminal(TerminalId::new(1)))
    );
}

/// A pane named while the modal holds the keyboard is where the keyboard goes when the modal
/// closes: `new-agent` names the agent it has just started, then closes itself. The modal
/// keeps the keyboard until then.
#[test]
fn a_pane_named_under_a_modal_takes_the_keyboard_when_the_modal_closes() {
    let beneath = PaneId::Terminal(TerminalId::new(9));
    for named_other_half in [true, false] {
        let (mut panes, children, _) = composed(MultipaneDirection::LeftRight);
        let client = ClientId::new(1);
        panes.connected_clients.borrow_mut().insert(client);
        panes.focus_pane(children[0], client);
        add(&mut panes, 3, OverlayKind::Modal, coordinates("40", "10"));
        let modal = PaneId::Terminal(TerminalId::new(3));
        panes.focus_pane(modal, client);

        let named = if named_other_half {
            children[1]
        } else {
            beneath
        };
        panes.return_from_modal_to(client, named);
        assert_eq!(panes.active_pane_id(client), Some(modal));

        close(&mut panes, modal);

        assert_eq!(
            panes.active_pane_id(client),
            named_other_half.then_some(children[1]),
            "a pane beneath the overlays is reached by leaving them"
        );
    }
}

/// While another modal is still up, the way back does not reach past it: the keyboard stays in
/// modal hands. Modal 2 came up without taking this client's keyboard, as one another client
/// opened would.
#[test]
fn a_way_back_does_not_reach_past_a_modal_still_on_screen() {
    let mut panes = overlay_panes(80, 20);
    let client = ClientId::new(1);
    panes.connected_clients.borrow_mut().insert(client);
    add(
        &mut panes,
        1,
        OverlayKind::Dismissible,
        coordinates("50%", "50%"),
    );
    panes.focus_pane(PaneId::Terminal(TerminalId::new(1)), client);
    add(&mut panes, 2, OverlayKind::Modal, coordinates("50%", "50%"));
    add(&mut panes, 3, OverlayKind::Modal, coordinates("40%", "40%"));
    panes.focus_pane(PaneId::Terminal(TerminalId::new(3)), client);

    close(&mut panes, PaneId::Terminal(TerminalId::new(3)));

    assert_eq!(
        panes.active_pane_id(client),
        Some(PaneId::Terminal(TerminalId::new(2)))
    );
}

#[test]
fn a_multipane_child_finds_its_sibling_only_along_the_split() {
    use Direction::*;
    for (direction, toward_second, toward_first, across) in [
        (MultipaneDirection::LeftRight, Right, Left, [Up, Down]),
        (MultipaneDirection::TopBottom, Down, Up, [Left, Right]),
    ] {
        let (mut panes, children, _) = composed(direction);
        assert_eq!(
            panes.sibling_toward(&children[0], toward_second),
            Some(children[1])
        );
        assert_eq!(
            panes.sibling_toward(&children[1], toward_first),
            Some(children[0])
        );
        assert_eq!(
            panes.sibling_toward(&children[0], toward_first),
            None,
            "out of the container, not round to the other half"
        );
        assert_eq!(panes.sibling_toward(&children[1], toward_second), None);
        for direction in across {
            for child in children {
                assert_eq!(panes.sibling_toward(&child, direction), None);
            }
        }
        add(
            &mut panes,
            3,
            OverlayKind::Dismissible,
            coordinates("20", "10"),
        );
        assert_eq!(
            panes.sibling_toward(&PaneId::Terminal(TerminalId::new(3)), toward_second),
            None,
            "a single overlay has no other half"
        );
    }
}

/// agent asks for 24 columns a half. A ratio outside that is drawn at the floor without being
/// rewritten, and a divider dragged past it stops there on either side.
#[test]
fn a_multipane_floor_holds_against_the_ratio_and_the_divider() {
    let mut panes = overlay_panes(120, 40);
    for id in [1, 2] {
        add(
            &mut panes,
            id,
            OverlayKind::Dismissible,
            coordinates("50", "20"),
        );
    }
    let children = [1, 2].map(|id| PaneId::Terminal(TerminalId::new(id)));
    panes
        .create_multipane(
            children,
            MultipaneSplit {
                direction: MultipaneDirection::LeftRight,
                ratio: 1,
            },
            24,
            OverlayPlacement::dismissible(Some(placed_coordinates("10", "5", "80", "24"))),
        )
        .unwrap();
    let widths = |panes: &OverlayPanes| {
        children.map(|child| panes.get(&child).unwrap().current_geom().cols.as_usize())
    };
    // 80 columns less the frame and the divider leave 77.
    assert_eq!(widths(&panes), [24, 53]);
    assert_eq!(panes.multipane_info(&children[0]).unwrap().split.ratio, 1);

    let outer = panes.outer_geom(children[0]).unwrap();
    let (_, divider) = panes
        .multipane_info(&children[0])
        .unwrap()
        .split
        .geometry(outer, true, 24);
    let seam = Position::new(divider.y as i32, divider.x as u16);
    assert_eq!(panes.seam_at(&seam), Some((children[0], children[1])));
    panes.start_seam_drag(children[0], children[1], seam);
    let right_edge = (outer.x + outer.cols.as_usize() - 1) as u16;
    panes.drag_seam_to(&Position::new(divider.y as i32, right_edge));
    assert_eq!(widths(&panes), [53, 24]);
    panes.stop_seam_drag(Position::new(divider.y as i32, outer.x as u16));
    assert_eq!(widths(&panes), [24, 53]);
}

/// A click on a divider the floor is squeezing - pressed and released where it is drawn -
/// moves nothing, so it must not store the floor's position as the ratio: the share it stands
/// in for comes back when the surface grows, and a click used to lose it for good.
#[test]
fn a_click_on_a_squeezed_divider_keeps_the_ratio() {
    let mut panes = overlay_panes(120, 40);
    for id in [1, 2] {
        add(
            &mut panes,
            id,
            OverlayKind::Dismissible,
            coordinates("50", "20"),
        );
    }
    let children = [1, 2].map(|id| PaneId::Terminal(TerminalId::new(id)));
    panes
        .create_multipane(
            children,
            MultipaneSplit {
                direction: MultipaneDirection::LeftRight,
                ratio: 1500,
            },
            24,
            OverlayPlacement::dismissible(Some(placed_coordinates("10", "5", "80", "24"))),
        )
        .unwrap();
    let outer = panes.outer_geom(children[0]).unwrap();
    let (_, divider) = panes
        .multipane_info(&children[0])
        .unwrap()
        .split
        .geometry(outer, true, 24);
    let seam = Position::new(divider.y as i32, divider.x as u16);
    assert_eq!(panes.seam_at(&seam), Some((children[0], children[1])));

    panes.start_seam_drag(children[0], children[1], seam);
    assert!(!panes.drag_seam_to(&seam), "nothing moved");
    panes.stop_seam_drag(seam);
    assert_eq!(
        panes.multipane_info(&children[0]).unwrap().split.ratio,
        1500
    );

    // A real move still sets it.
    panes.start_seam_drag(children[0], children[1], seam);
    panes.stop_seam_drag(Position::new(divider.y as i32, divider.x as u16 + 10));
    assert_ne!(
        panes.multipane_info(&children[0]).unwrap().split.ratio,
        1500
    );
}

#[test]
fn translucent_container_keeps_opacity_through_transfer_replacement_and_visibility() {
    let (mut panes, children, _) = composed(MultipaneDirection::LeftRight);
    assert!(panes.translucent_layers(3, 0).is_empty());
    panes.set_background_opacity(children[0], 75).unwrap();
    assert!(panes.set_background_opacity(children[0], 101).is_err());
    assert_eq!(
        panes.placement_of(&children[1]).unwrap().background_opacity,
        Some(75)
    );
    let container = panes.take_container(children[0]).unwrap();
    let mut destination = overlay_panes(120, 40);
    destination.insert_container(container);
    assert_eq!(destination.translucent_layers(3, 85)[0].0, 4);
    destination.hide_pane(children[0]);
    assert!(destination.translucent_layers(3, 85).is_empty());
    destination.show_pane(children[0]);
    assert_eq!(destination.translucent_layers(3, 85)[0].2, 85);
    assert!(destination.translucent_layers(3, 100).is_empty());
    assert_eq!(destination.translucent_layers(3, 0)[0].2, 0);
    assert_eq!(destination.translucent_layers(3, 85)[0].2, 85);
    let replacement = PaneId::Terminal(TerminalId::new(9));
    destination
        .replace_pane(children[1], mock_pane(replacement))
        .unwrap();
    assert_eq!(
        destination
            .placement_of(&replacement)
            .unwrap()
            .background_opacity,
        Some(75)
    );
    destination
        .set_background_opacity(replacement, 100)
        .unwrap();
    assert!(destination.translucent_layers(3, 85).is_empty());
    assert!(destination.translucent_layers(3, 0).is_empty());
}

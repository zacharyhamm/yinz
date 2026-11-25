//! Production round trips of the native scene: `Screen` renders, `serialize_native` encodes, and
//! a viewer here decodes each frame into a persistent cell grid, geometry and cursor, as the
//! native terminal does. Every step compares the result with an ANSI client of the same tab, so
//! stale cells, rects or cursors left by a transition fail the comparison.
use super::*;
use crate::screen::Notification;
use crate::tab::Tab;
use std::time::{Duration, Instant};
use yinz_types::animation::Tween;
use yinz_types::input::actions::Action;
use yinz_types::input::context_menu::ContextMenu;
use yinz_types::internal_ids::{NotificationId, TabId};
use yinz_types::pane_size::Viewport;
use yinz_types::render_negotiation::{Offer, Request};
use yinz_types::structured_render::{
    decode, GeometryRecord, PaneRect, WireCell, PANE_FLOATING, PANE_FOCUSED, PANE_POPUP,
    PANE_UNDER_FLOATING,
};
use {yinz_types::data::ContextMenuContext, yinz_types::data::ContextMenuItem};

const SIZE: Size = Size { cols: 80, rows: 20 };
const NATIVE: ClientId = ClientId::new(1);
const ANSI: ClientId = ClientId::new(2);
const SECOND_NATIVE: ClientId = ClientId::new(3);

/// What a native terminal keeps between frames.
struct Viewer {
    client: ClientId,
    cols: usize,
    rows: usize,
    cells: Vec<WireCell>,
    geometry: Option<GeometryRecord>,
    cursor: Option<(usize, usize)>,
    seq: Option<u64>,
    frames: usize,
}

impl Viewer {
    fn new(client: ClientId) -> Self {
        Viewer {
            client,
            cols: 0,
            rows: 0,
            cells: vec![],
            geometry: None,
            cursor: None,
            seq: None,
            frames: 0,
        }
    }

    fn apply(&mut self, bytes: &[u8]) {
        let view = decode(bytes).expect("a valid native frame");
        let header = *view.header();
        match self.seq {
            None => assert!(header.full_repaint(), "the first frame is a full repaint"),
            Some(seq) => assert_eq!(header.seq, seq + 1, "frames arrive in sequence"),
        }
        self.seq = Some(header.seq);
        let size = (header.cols as usize, header.rows as usize);
        if size != (self.cols, self.rows) {
            assert!(header.full_repaint(), "a new grid size is a full repaint");
            (self.cols, self.rows) = size;
        }
        if header.full_repaint() || header.clear() {
            self.cells = vec![WireCell::BLANK; self.cols * self.rows];
        }
        view.apply(&mut self.cells, self.cols, self.rows);
        if let Some(geometry) = view.geometry() {
            self.geometry = Some(geometry);
        }
        let cursor = view.cursor();
        self.cursor = cursor
            .visible
            .then_some((cursor.x as usize, cursor.y as usize));
        self.frames += 1;
    }

    fn row(&self, y: usize) -> String {
        let mut text = String::new();
        let mut skip = 0;
        for cell in &self.cells[y * self.cols..(y + 1) * self.cols] {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            skip = cell.width.saturating_sub(1) as usize;
            text.push(cell.character());
        }
        text.trim_end().to_owned()
    }

    fn rows(&self) -> Vec<String> {
        (0..self.rows).map(|y| self.row(y)).collect()
    }

    /// The rows of `size` at `offset`: what the tab painted, without presentation gutters.
    fn content_rows(&self, (dx, dy): (usize, usize), size: Size) -> Vec<String> {
        (dy..dy + size.rows)
            .map(|y| {
                self.row(y)
                    .chars()
                    .skip(dx)
                    .take(size.cols)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn text(&self) -> String {
        self.rows().join("\n")
    }

    fn geometry(&self) -> &[PaneRect] {
        &self
            .geometry
            .as_ref()
            .expect("the viewer has geometry")
            .panes
    }

    fn popups(&self) -> Vec<PaneRect> {
        self.geometry()
            .iter()
            .filter(|rect| rect.popup())
            .copied()
            .collect()
    }

    fn focused_pane(&self) -> PaneRect {
        let focused: Vec<_> = self
            .geometry()
            .iter()
            .filter(|rect| rect.focused() && !rect.popup())
            .collect();
        assert_eq!(focused.len(), 1, "one focused pane: {:?}", self.geometry());
        *focused[0]
    }

    /// A visible cursor inside the focused pane's content, never left behind by a transition.
    fn assert_cursor_in_focused_pane(&self) {
        let (x, y) = self.cursor.expect("the cursor is visible");
        let pane = self.focused_pane();
        assert!(
            pane.content_contains(x as u16, y as u16),
            "cursor {:?} is outside the focused pane {pane:?}",
            (x, y)
        );
    }
}

/// The ANSI client's view, rendered from the very same frames.
struct AnsiView {
    client: ClientId,
    grid: Grid,
}

impl AnsiView {
    fn new(client: ClientId, size: Size) -> Self {
        AnsiView {
            client,
            grid: grid_for_render_snapshot(size),
        }
    }

    fn rows(&self) -> Vec<String> {
        self.grid
            .as_character_lines()
            .iter()
            .map(|line| {
                line.iter()
                    .map(|character| character.character)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }
}

fn render(
    screen: &mut Screen,
    receiver: &channels::Receiver<(ServerInstruction, ErrorContext)>,
    viewers: &mut [&mut Viewer],
    ansi: Option<&mut AnsiView>,
) {
    screen.render_to_clients().expect("TEST");
    drain(receiver, viewers, ansi);
}

fn drain(
    receiver: &channels::Receiver<(ServerInstruction, ErrorContext)>,
    viewers: &mut [&mut Viewer],
    mut ansi: Option<&mut AnsiView>,
) {
    while let Ok((instruction, _)) = receiver.try_recv() {
        match instruction {
            ServerInstruction::RenderFrames(frames) => {
                for viewer in viewers.iter_mut() {
                    if let Some(bytes) = frames.get(&viewer.client) {
                        viewer.apply(bytes);
                    }
                }
            },
            ServerInstruction::Render(Some(mut rendered)) => {
                if let Some(ansi) = ansi.as_deref_mut()
                    && let Some(output) = rendered.remove(&ansi.client)
                {
                    take_snapshot_and_cursor_coordinates(&output, &mut ansi.grid);
                }
            },
            _ => {},
        }
    }
}

fn negotiate(screen: &mut Screen, client: ClientId) {
    screen
        .negotiate_render(client, Request::Offer(Offer::reader(None)))
        .expect("TEST");
    assert!(screen.core.native_render_clients.contains_key(&client));
}

fn print(screen: &mut Screen, pane: u32, text: &str) {
    screen
        .get_active_tab_mut(NATIVE)
        .expect("TEST")
        .handle_pty_bytes(crate::TerminalId::new(pane), text.as_bytes().to_vec())
        .expect("TEST");
}

fn tab(screen: &mut Screen) -> &mut Tab {
    screen.get_active_tab_mut(NATIVE).expect("TEST")
}

fn terminal(pane: u32) -> PaneId {
    PaneId::Terminal(crate::TerminalId::new(pane))
}

/// A tab laid out by `layout` with terminals `1..=panes`, each printing `MARKER-<n>`.
fn screen_with_layout(
    size: Size,
    layout: TiledPaneLayout,
    panes: u32,
) -> (
    Screen,
    channels::Receiver<(ServerInstruction, ErrorContext)>,
) {
    let mut screen = create_non_mirrored_screen(size);
    let receiver = screen_wired_to_server(&mut screen);
    screen
        .new_tab(TabId::new(0), (vec![], vec![]), None, Some(NATIVE))
        .expect("TEST");
    screen
        .apply_layout(
            layout,
            vec![],
            (1..=panes)
                .map(|pane| (crate::TerminalId::new(pane), None))
                .collect(),
            vec![],
            HashMap::new(),
            TabId::new(0),
            true,
            NATIVE,
            None,
        )
        .expect("TEST");
    for pane in 1..=panes {
        print(&mut screen, pane, &format!("MARKER-{pane}"));
    }
    (screen, receiver)
}

fn side_by_side() -> TiledPaneLayout {
    TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    }
}

/// Two side-by-side panes, a native client and an ANSI client of the same size, rendered once.
fn two_pane_scene() -> (
    Screen,
    channels::Receiver<(ServerInstruction, ErrorContext)>,
    Viewer,
    AnsiView,
) {
    let (mut screen, receiver) = screen_with_layout(SIZE, side_by_side(), 2);
    screen.add_client(ANSI).expect("TEST");
    screen.set_client_size(ANSI, SIZE).expect("TEST");
    negotiate(&mut screen, NATIVE);
    let mut viewer = Viewer::new(NATIVE);
    let mut ansi = AnsiView::new(ANSI, SIZE);
    drain(&receiver, &mut [&mut viewer], Some(&mut ansi));
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    (screen, receiver, viewer, ansi)
}

fn bounds(rect: &PaneRect) -> (u16, u16, u16, u16) {
    (rect.x, rect.y, rect.cols, rect.rows)
}

fn pane_bounds(screen: &mut Screen, pane: PaneId) -> (u16, u16, u16, u16) {
    let pane = tab(screen).get_pane_with_id(pane).expect("TEST");
    (
        pane.x() as u16,
        pane.y() as u16,
        pane.cols() as u16,
        pane.rows() as u16,
    )
}

fn marker_column(viewer: &Viewer, marker: &str) -> Option<usize> {
    viewer
        .rows()
        .iter()
        .find_map(|row| row.find(marker).map(|byte| row[..byte].chars().count()))
}

#[test]
fn native_scene_floating_layer_shows_and_hides_without_stale_cells() {
    let (mut screen, receiver, mut viewer, mut ansi) = two_pane_scene();
    assert_eq!(viewer.geometry().len(), 2);
    assert!(viewer
        .geometry()
        .iter()
        .all(|rect| rect.flags & (PANE_UNDER_FLOATING | PANE_FLOATING) == 0));
    assert_eq!(viewer.rows(), ansi.rows());
    viewer.assert_cursor_in_focused_pane();

    tab(&mut screen)
        .new_floating_pane(terminal(3), None, None, false, true, None, None)
        .expect("TEST");
    print(&mut screen, 3, "FLOAT-MARKER");
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    let geometry = viewer.geometry().to_vec();
    assert_eq!(geometry.len(), 3);
    assert_eq!(bounds(&geometry[2]), pane_bounds(&mut screen, terminal(3)));
    assert_ne!(geometry[2].flags & PANE_FLOATING, 0);
    assert_eq!(geometry[2].flags & PANE_UNDER_FLOATING, 0);
    for tiled in &geometry[..2] {
        assert_ne!(tiled.flags & PANE_UNDER_FLOATING, 0, "{tiled:?}");
    }
    assert!(viewer.text().contains("FLOAT-MARKER"));
    assert_eq!(viewer.rows(), ansi.rows());
    viewer.assert_cursor_in_focused_pane();
    assert_eq!(viewer.focused_pane(), geometry[2]);

    tab(&mut screen)
        .toggle_floating_panes(Some(NATIVE), None, None)
        .expect("TEST");
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    assert_eq!(viewer.geometry().len(), 2);
    assert!(viewer
        .geometry()
        .iter()
        .all(|rect| rect.flags & (PANE_UNDER_FLOATING | PANE_FLOATING) == 0));
    assert!(!viewer.text().contains("FLOAT-MARKER"), "{}", viewer.text());
    assert_eq!(viewer.rows(), ansi.rows());
    viewer.assert_cursor_in_focused_pane();
}

#[test]
fn native_scene_fullscreen_and_pane_moves_leave_no_stale_cells() {
    let (mut screen, receiver, mut viewer, mut ansi) = two_pane_scene();
    tab(&mut screen)
        .focus_pane_with_id(terminal(1), false, false, NATIVE)
        .expect("TEST");
    let before = viewer.geometry().to_vec();

    tab(&mut screen).toggle_active_pane_fullscreen(NATIVE);
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    assert_eq!(viewer.geometry().len(), 1);
    assert_eq!(
        bounds(&viewer.geometry()[0]),
        pane_bounds(&mut screen, terminal(1))
    );
    assert!(viewer.text().contains("MARKER-1"));
    assert!(!viewer.text().contains("MARKER-2"), "{}", viewer.text());
    assert_eq!(viewer.rows(), ansi.rows());
    viewer.assert_cursor_in_focused_pane();

    tab(&mut screen).toggle_active_pane_fullscreen(NATIVE);
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    assert_eq!(
        viewer.geometry().iter().map(bounds).collect::<Vec<_>>(),
        before.iter().map(bounds).collect::<Vec<_>>()
    );
    assert_eq!(viewer.rows(), ansi.rows());
    viewer.assert_cursor_in_focused_pane();

    let left = marker_column(&viewer, "MARKER-1").expect("TEST");
    let right = marker_column(&viewer, "MARKER-2").expect("TEST");
    assert!(left < right);
    tab(&mut screen).move_active_pane(NATIVE);
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    assert_eq!(marker_column(&viewer, "MARKER-1"), Some(right));
    assert_eq!(marker_column(&viewer, "MARKER-2"), Some(left));
    assert_eq!(viewer.text().matches("MARKER-1").count(), 1);
    assert_eq!(viewer.text().matches("MARKER-2").count(), 1);
    assert_eq!(
        bounds(&viewer.focused_pane()),
        pane_bounds(&mut screen, terminal(1)),
        "the focused rect moved with its pane"
    );
    assert_eq!(viewer.rows(), ansi.rows());
    viewer.assert_cursor_in_focused_pane();
}

#[test]
fn native_scene_collapsed_stack_titles_follow_focus() {
    let (mut screen, receiver) = screen_with_layout(SIZE, three_pane_stack(), 3);
    screen.add_client(ANSI).expect("TEST");
    screen.set_client_size(ANSI, SIZE).expect("TEST");
    negotiate(&mut screen, NATIVE);
    let mut viewer = Viewer::new(NATIVE);
    let mut ansi = AnsiView::new(ANSI, SIZE);
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));

    let titles = |viewer: &Viewer| viewer.geometry().iter().filter(|r| r.rows == 1).count();
    assert_eq!(viewer.geometry().len(), 3, "collapsed members have rects");
    assert_eq!(titles(&viewer), 2);
    assert_eq!(viewer.rows(), ansi.rows());
    let expanded = viewer.focused_pane();
    assert!(expanded.rows > 1);
    viewer.assert_cursor_in_focused_pane();

    let moved = tab(&mut screen).move_focus_down(NATIVE).expect("TEST")
        || tab(&mut screen).move_focus_up(NATIVE).expect("TEST");
    assert!(moved, "focus moves within the stack");
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    assert_eq!(viewer.geometry().len(), 3);
    assert_eq!(titles(&viewer), 2);
    let now_expanded = viewer.focused_pane();
    assert_ne!(bounds(&now_expanded), bounds(&expanded));
    assert!(now_expanded.rows > 1);
    assert_eq!(viewer.rows(), ansi.rows());
    viewer.assert_cursor_in_focused_pane();
}

#[test]
fn native_scene_palette_owns_the_cursor_and_closes_cleanly() {
    let (mut screen, receiver, mut viewer, mut ansi) = two_pane_scene();
    let pane_cursor_before = viewer.cursor;

    screen.toggle_command_palette(NATIVE);
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    let popups = viewer.popups();
    assert_eq!(popups.len(), 1, "{:?}", viewer.geometry());
    let palette = popups[0];
    assert_eq!(
        *viewer.geometry().last().unwrap(),
        palette,
        "the palette is on top"
    );
    assert_eq!(palette.flags, PANE_POPUP | PANE_FOCUSED);
    // The pane cursor gives way to the palette's own input caret, inside the palette.
    let (x, y) = viewer.cursor.expect("the palette caret is visible");
    assert!(palette.contains(x as u16, y as u16), "{:?}", (x, y));
    // The palette is this client's alone.
    assert_ne!(viewer.rows(), ansi.rows());

    screen.toggle_command_palette(NATIVE);
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    assert!(viewer.popups().is_empty());
    assert_eq!(viewer.rows(), ansi.rows(), "no palette cell survives");
    assert_eq!(viewer.cursor, pane_cursor_before);
    viewer.assert_cursor_in_focused_pane();
}

#[test]
fn native_scene_context_menu_is_a_popup_only_for_its_client() {
    let (mut screen, receiver, mut viewer, mut ansi) = two_pane_scene();
    screen.add_client(SECOND_NATIVE).expect("TEST");
    screen.set_client_size(SECOND_NATIVE, SIZE).expect("TEST");
    negotiate(&mut screen, SECOND_NATIVE);
    let mut other = Viewer::new(SECOND_NATIVE);
    render(
        &mut screen,
        &receiver,
        &mut [&mut viewer, &mut other],
        Some(&mut ansi),
    );
    screen.menus.context_menu = ContextMenu::from_items(vec![
        ContextMenuItem::new("Copy", vec![Action::Copy]),
        ContextMenuItem::new("Close pane", vec![Action::CloseFocus]),
    ]);
    let request = screen.open_context_menu(
        ContextMenuContext {
            pane_id: terminal(1),
            click: (5, 3),
            ..Default::default()
        },
        NATIVE,
    );
    let items = screen.menus.context_menu.items().to_vec();
    screen.set_context_menu_items(request, items, Some(crate::PluginId::new(7)), NATIVE);
    render(
        &mut screen,
        &receiver,
        &mut [&mut viewer, &mut other],
        Some(&mut ansi),
    );
    let surface = Viewport {
        x: 0,
        y: 0,
        cols: SIZE.cols,
        rows: SIZE.rows,
    };
    let (_, menu) = screen
        .menus
        .context_menu_cell_rect(NATIVE, surface)
        .expect("the menu is laid out");
    let popups = viewer.popups();
    assert_eq!(popups.len(), 1);
    assert_eq!(
        bounds(&popups[0]),
        (
            menu[0].x as u16,
            menu[0].y as u16,
            menu[0].cols as u16,
            menu[0].rows as u16
        )
    );
    assert_eq!(popups[0].flags, PANE_POPUP | PANE_FOCUSED);
    assert!(other.popups().is_empty(), "the other viewer has no menu");
    assert_eq!(other.rows(), ansi.rows());
    assert!(viewer.text().contains("Close pane"));

    screen.close_context_menu(NATIVE);
    render(
        &mut screen,
        &receiver,
        &mut [&mut viewer, &mut other],
        Some(&mut ansi),
    );
    assert!(viewer.popups().is_empty());
    assert_eq!(viewer.rows(), ansi.rows());
    viewer.assert_cursor_in_focused_pane();
}

#[test]
fn native_scene_toasts_are_popups_at_their_revealed_cells_until_reaped() {
    let (mut screen, receiver, mut viewer, mut ansi) = two_pane_scene();
    let start = Instant::now();
    screen.core.pinned_now = Some(start);
    let mut notification = Notification::new(
        NotificationId::new(0),
        "build finished".to_owned(),
        None,
        None,
        start,
    );
    // Half slid in: the box has not painted its leading columns yet.
    notification.reveal = Tween::settled(0.5);
    notification.y_offset = Tween::settled(0.0);
    screen.notices.notifications.push(notification);
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    let surface = Viewport {
        x: 0,
        y: 0,
        cols: SIZE.cols,
        rows: SIZE.rows,
    };
    let layout = screen.notification_layout(surface, start, NATIVE);
    assert_eq!(layout.len(), 1);
    assert!(
        layout[0].visible_cols < layout[0].width,
        "the toast is mid-slide"
    );
    let revealed = screen.notification_cell_rects(surface, start, NATIVE);
    let popups = viewer.popups();
    assert_eq!(
        popups.iter().map(bounds).collect::<Vec<_>>(),
        revealed
            .iter()
            .map(|rect| (
                rect.x as u16,
                rect.y as u16,
                rect.cols as u16,
                rect.rows as u16
            ))
            .collect::<Vec<_>>(),
        "only the revealed cells belong to the toast"
    );
    assert_eq!(popups[0].flags, PANE_POPUP, "a toast takes no input");
    // Toasts are session-wide: the ANSI client sees the very same cells.
    assert_eq!(viewer.rows(), ansi.rows());

    screen
        .dismiss_notification(NotificationId::new(0), start)
        .expect("TEST");
    let gone = start + Duration::from_secs(1);
    screen.core.pinned_now = Some(gone);
    screen.reap_settled_notifications(gone);
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    assert!(viewer.popups().is_empty());
    assert!(!viewer.text().contains("build finished"));
    assert_eq!(viewer.rows(), ansi.rows());
    viewer.assert_cursor_in_focused_pane();
}

#[test]
fn native_scene_permission_modal_is_a_focused_popup_over_a_hidden_cursor() {
    let (mut screen, receiver, mut viewer, mut ansi) = two_pane_scene();
    let before = viewer.cursor;
    screen.open_permission_modal(
        crate::PluginId::new(7),
        yinz_types::data::PluginPermission::new(
            "agents".into(),
            vec![yinz_types::data::PermissionType::ReadApplicationState],
        ),
    );
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    assert!(screen.permissions.permission_modal_open(NATIVE));
    let popups = viewer.popups();
    assert_eq!(popups.len(), 1);
    assert_eq!(popups[0].flags, PANE_POPUP | PANE_FOCUSED);
    assert_eq!(*viewer.geometry().last().unwrap(), popups[0]);
    assert!(viewer.text().contains("Plugin permissions"));
    assert_eq!(viewer.cursor, None, "the modal hides the cursor");

    screen.cancel_permission_modal(0);
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    assert!(viewer.popups().is_empty());
    assert!(!viewer.text().contains("Plugin permissions"));
    assert_eq!(viewer.cursor, before);
    viewer.assert_cursor_in_focused_pane();
}

#[test]
fn native_scene_simultaneous_native_clients_keep_their_own_focus_offsets_and_cursors() {
    let large = Size {
        cols: 100,
        rows: 26,
    };
    let (mut screen, receiver) = screen_with_layout(large, side_by_side(), 2);
    screen.add_client(ANSI).expect("TEST");
    screen.set_client_size(ANSI, SIZE).expect("TEST");
    screen.add_client(SECOND_NATIVE).expect("TEST");
    screen.set_client_size(SECOND_NATIVE, SIZE).expect("TEST");
    screen.resize_to_clients().expect("TEST");
    negotiate(&mut screen, NATIVE);
    negotiate(&mut screen, SECOND_NATIVE);
    tab(&mut screen)
        .focus_pane_with_id(terminal(1), false, false, NATIVE)
        .expect("TEST");
    screen
        .get_active_tab_mut(SECOND_NATIVE)
        .expect("TEST")
        .focus_pane_with_id(terminal(2), false, false, SECOND_NATIVE)
        .expect("TEST");
    let mut large_viewer = Viewer::new(NATIVE);
    let mut small_viewer = Viewer::new(SECOND_NATIVE);
    let mut ansi = AnsiView::new(ANSI, SIZE);
    render(
        &mut screen,
        &receiver,
        &mut [&mut large_viewer, &mut small_viewer],
        Some(&mut ansi),
    );

    let offset = screen.core.client_presentations[&NATIVE].1.offset();
    assert_eq!(offset, (10, 3));
    assert_eq!(
        (large_viewer.cols, large_viewer.rows),
        (large.cols, large.rows)
    );
    assert_eq!(
        (small_viewer.cols, small_viewer.rows),
        (SIZE.cols, SIZE.rows)
    );
    // Both native grids carry the ANSI client's cells, the larger one centered.
    assert_eq!(small_viewer.rows(), ansi.rows());
    assert_eq!(large_viewer.content_rows(offset, SIZE), ansi.rows());
    // The same panes, offset for the larger viewer, each focused as its own client chose.
    let small = small_viewer.geometry().to_vec();
    let large_rects = large_viewer.geometry().to_vec();
    assert_eq!(small.len(), 2);
    for (small, large) in small.iter().zip(&large_rects) {
        assert_eq!(
            (large.x, large.y),
            (small.x + offset.0 as u16, small.y + offset.1 as u16)
        );
        assert_eq!((large.cols, large.rows), (small.cols, small.rows));
    }
    assert_eq!(
        bounds(&small_viewer.focused_pane()),
        pane_bounds(&mut screen, terminal(2))
    );
    let (x, y, cols, rows) = pane_bounds(&mut screen, terminal(1));
    assert_eq!(
        bounds(&large_viewer.focused_pane()),
        (x + offset.0 as u16, y + offset.1 as u16, cols, rows)
    );
    small_viewer.assert_cursor_in_focused_pane();
    large_viewer.assert_cursor_in_focused_pane();

    // One client's palette is a popup for that client alone.
    screen.toggle_command_palette(SECOND_NATIVE);
    render(
        &mut screen,
        &receiver,
        &mut [&mut large_viewer, &mut small_viewer],
        Some(&mut ansi),
    );
    assert_eq!(small_viewer.popups().len(), 1);
    assert!(large_viewer.popups().is_empty());
    assert_eq!(large_viewer.content_rows(offset, SIZE), ansi.rows());
    large_viewer.assert_cursor_in_focused_pane();
}

fn three_pane_stack() -> TiledPaneLayout {
    TiledPaneLayout {
        children_are_stacked: true,
        children: vec![
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
        ],
        ..Default::default()
    }
}

#[test]
fn native_scene_cropped_recipient_hides_its_cursor_until_it_fits() {
    let (mut screen, receiver) = screen_with_layout(SIZE, three_pane_stack(), 3);
    screen.add_client(ANSI).expect("TEST");
    screen.set_client_size(ANSI, SIZE).expect("TEST");
    negotiate(&mut screen, NATIVE);
    let mut viewer = Viewer::new(NATIVE);
    let mut ansi = AnsiView::new(ANSI, SIZE);
    // Expand the bottom member, so the cursor sits low in the tab.
    while tab(&mut screen).move_focus_down(NATIVE).expect("TEST") {}
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    let (_, cursor_y) = viewer.cursor.expect("TEST");
    viewer.assert_cursor_in_focused_pane();
    let scene = viewer.geometry().to_vec();
    let uncropped = viewer.rows();
    assert_eq!(uncropped, ansi.rows());

    // A three-pane stack cannot be laid out in two rows, so its panes keep their places and
    // the recipient is smaller than the scene it is sent: a refused reflow.
    let cropped = Size {
        cols: SIZE.cols,
        rows: 2,
    };
    assert!(
        cursor_y >= cropped.rows,
        "the cursor is below the cropped grid"
    );
    screen.core.client_sizes.insert(NATIVE, cropped);
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    assert_eq!((viewer.cols, viewer.rows), (cropped.cols, cropped.rows));
    assert_eq!(
        viewer.cursor, None,
        "the cursor is past the recipient's grid"
    );
    assert_eq!(
        viewer.rows(),
        uncropped[..cropped.rows].to_vec(),
        "the same cells, cropped to the grid"
    );
    // The panes could not move, and their rects stay unclipped: image anchors are
    // validated against them. (The expanded member's title inset follows the viewport.)
    assert_eq!(
        viewer.geometry().iter().map(bounds).collect::<Vec<_>>(),
        scene.iter().map(bounds).collect::<Vec<_>>()
    );

    screen.core.client_sizes.insert(NATIVE, SIZE);
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    assert_eq!(viewer.rows(), ansi.rows());
    assert_eq!(viewer.cursor.map(|(_, y)| y), Some(cursor_y));
    viewer.assert_cursor_in_focused_pane();
}

#[test]
fn native_scene_reattach_and_late_negotiation_rebuild_geometry_and_cursor() {
    let (mut screen, receiver, mut viewer, mut ansi) = two_pane_scene();
    tab(&mut screen).toggle_active_pane_fullscreen(NATIVE);
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    tab(&mut screen).toggle_active_pane_fullscreen(NATIVE);
    render(&mut screen, &receiver, &mut [&mut viewer], Some(&mut ansi));
    let scene = viewer.geometry().to_vec();
    let cursor = viewer.cursor;

    // Reattach: the new attachment starts a fresh sequence with the complete scene.
    screen.remove_client(NATIVE).expect("TEST");
    screen.add_client(NATIVE).expect("TEST");
    screen.set_client_size(NATIVE, SIZE).expect("TEST");
    negotiate(&mut screen, NATIVE);
    let mut reattached = Viewer::new(NATIVE);
    render(
        &mut screen,
        &receiver,
        &mut [&mut reattached],
        Some(&mut ansi),
    );
    assert_eq!(reattached.geometry(), scene.as_slice());
    assert_eq!(reattached.rows(), ansi.rows());
    assert_eq!(reattached.cursor, cursor);
    reattached.assert_cursor_in_focused_pane();

    // A client that rendered ANSI frames first and negotiates later gets the cursor back.
    screen.add_client(SECOND_NATIVE).expect("TEST");
    screen.set_client_size(SECOND_NATIVE, SIZE).expect("TEST");
    render(
        &mut screen,
        &receiver,
        &mut [&mut reattached],
        Some(&mut ansi),
    );
    render(
        &mut screen,
        &receiver,
        &mut [&mut reattached],
        Some(&mut ansi),
    );
    negotiate(&mut screen, SECOND_NATIVE);
    let mut late = Viewer::new(SECOND_NATIVE);
    render(
        &mut screen,
        &receiver,
        &mut [&mut reattached, &mut late],
        Some(&mut ansi),
    );
    assert_eq!(late.geometry().len(), 2);
    assert_eq!(late.rows(), ansi.rows());
    late.assert_cursor_in_focused_pane();
}

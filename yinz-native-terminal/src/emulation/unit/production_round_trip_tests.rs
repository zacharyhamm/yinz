//! Production round trips: real PTY bytes (ANSI SGR, OSC 8, Unicode) go through the server's
//! `Grid`, its render, `Output::serialize_native`, the wire decoder and this crate's
//! `ScreenBuffer`, over several frames. Nothing here builds wire cells by hand, so the producer
//! and the consumer are checked against each other rather than against fixtures of either.
use crate::emulation::screen_buffer::ScreenBuffer;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use yinz_terminal::output::{CharacterChunk, NativeRenderState, Output, Presentation};
use yinz_terminal::panes::sixel::SixelImageStore;
use yinz_terminal::panes::{kitty_graphics::KittyImageStore, Grid, LinkHandler, TerminalCharacter};
use yinz_terminal::ClientId;
use yinz_types::data::PaletteColor;
use yinz_types::pane_size::Size;
use yinz_types::position::Position;
use yinz_types::structured_render::{decode, ATTR_BOLD, MAX_LINK_ID};
use yinz_types::style::{Palette, Style};

const FAMILY: &str = "\u{1F469}\u{200D}\u{1F467}";
const CLIENT: ClientId = ClientId::new(1);

struct RoundTrip {
    grid: Grid,
    links: Rc<RefCell<LinkHandler>>,
    output: Output,
    states: HashMap<ClientId, NativeRenderState>,
    size: Size,
    presentation: Option<Presentation>,
    screen: ScreenBuffer,
    /// Link entries the last frame carried.
    carried: usize,
    full_repaint: bool,
}

impl RoundTrip {
    /// A pane of `rows` x `cols` shown on a client terminal of `size`.
    fn new(rows: usize, cols: usize, size: Size) -> Self {
        let links = Rc::new(RefCell::new(LinkHandler::new()));
        let grid = Grid::new(
            rows,
            cols,
            Rc::new(RefCell::new(Palette::default())),
            Rc::new(RefCell::new(HashMap::new())),
            links.clone(),
            Rc::new(RefCell::new(None)),
            Rc::new(RefCell::new(SixelImageStore::default())),
            Rc::new(RefCell::new(KittyImageStore::default())),
            Style::default(),
            false,
            true,
            true,
            true,
            false,
        );
        RoundTrip {
            grid,
            links,
            output: Output::default(),
            states: HashMap::from([(CLIENT, NativeRenderState::default())]),
            size,
            presentation: None,
            screen: ScreenBuffer::new(size.rows, size.cols),
            carried: 0,
            full_repaint: false,
        }
    }

    fn feed(&mut self, bytes: &str) -> &mut Self {
        vte::Parser::new().advance(&mut self.grid, bytes.as_bytes());
        self
    }

    /// Render the grid's changed rows and send them to the client as one frame.
    fn frame(&mut self) -> &mut Self {
        let chunks = self
            .grid
            .render(0, 0, &Style::default())
            .unwrap()
            .map(|frame| frame.0)
            .unwrap_or_default();
        self.send(chunks)
    }

    fn send(&mut self, chunks: Vec<CharacterChunk>) -> &mut Self {
        self.output
            .add_clients(&HashSet::from([CLIENT]), self.links.clone(), None);
        self.output
            .add_character_chunks_to_client(CLIENT, chunks, None)
            .unwrap();
        let sizes = HashMap::from([(CLIENT, self.size)]);
        let presentations: HashMap<_, _> = self
            .presentation
            .map(|presentation| (CLIENT, presentation))
            .into_iter()
            .collect();
        let mut frames = self
            .output
            .serialize_native(&mut self.states, &sizes, &presentations);
        let bytes = frames.remove(&CLIENT).unwrap();
        let view = decode(&bytes).unwrap();
        self.carried = view.links().entries.len();
        self.full_repaint = view.header().full_repaint();
        self.screen.apply(&view).unwrap();
        self
    }

    fn text(&self, row: usize, col: usize) -> String {
        self.screen.text_at(row, col)
    }

    /// The whole unit (cell or group) at a cell.
    fn unit(&self, row: usize, col: usize) -> (usize, usize, String) {
        let span = self.screen.unit_span(row, col);
        (span.start, span.end, self.screen.unit_text(row, span))
    }

    fn link(&self, row: usize, col: usize) -> Option<&str> {
        self.screen.link_at(row, col)
    }
}

fn wide(cols: usize, rows: usize) -> Size {
    Size { cols, rows }
}

fn osc8(id: Option<&str>, uri: &str, text: &str) -> String {
    let params = id.map(|id| format!("id={id}")).unwrap_or_default();
    format!("\u{1b}]8;{params};{uri}\u{1b}\\{text}\u{1b}]8;;\u{1b}\\")
}

#[test]
fn combining_marks_arrive_as_one_cluster_cell() {
    let mut trip = RoundTrip::new(3, 20, wide(20, 3));
    trip.feed("ae\u{301}\u{302}b").frame();
    assert_eq!(trip.text(0, 0), "a");
    assert_eq!(trip.text(0, 1), "e\u{301}\u{302}");
    assert_eq!(trip.text(0, 2), "b");
    assert_eq!(trip.unit(0, 1), (1, 2, "e\u{301}\u{302}".into()));
}

#[test]
fn zwj_families_flags_and_skin_tones_arrive_as_groups() {
    let mut trip = RoundTrip::new(3, 30, wide(30, 3));
    trip.feed(&format!("{FAMILY}x\u{1F1E9}\u{1F1EA}y\u{1F44D}\u{1F3FD}z"))
        .frame();
    assert_eq!(trip.unit(0, 0), (0, 4, FAMILY.into()));
    assert_eq!(trip.unit(0, 3), (0, 4, FAMILY.into()));
    assert!(trip.screen.is_group_continuation(0, 2));
    assert!(!trip.screen.is_group_continuation(0, 1), "a wide tail");
    assert_eq!(trip.text(0, 4), "x");
    assert_eq!(trip.unit(0, 5), (5, 7, "\u{1F1E9}\u{1F1EA}".into()));
    assert_eq!(trip.text(0, 7), "y");
    assert_eq!(trip.unit(0, 8), (8, 12, "\u{1F44D}\u{1F3FD}".into()));
    assert_eq!(trip.text(0, 12), "z");
}

#[test]
fn a_narrow_warning_sign_drops_its_vs16_and_cjk_keeps_its_columns() {
    let mut trip = RoundTrip::new(3, 30, wide(30, 3));
    trip.feed("\u{26A0}\u{FE0F}\u{754C}e\u{301}\u{754C}!")
        .frame();
    assert_eq!(trip.text(0, 0), "\u{26A0}");
    assert_eq!(trip.text(0, 1), "\u{754C}");
    assert_eq!(trip.text(0, 3), "e\u{301}");
    assert_eq!(trip.text(0, 4), "\u{754C}");
    assert_eq!(trip.text(0, 6), "!");
    assert_eq!(trip.screen.cell(0, 1).width, 2);
}

#[test]
fn a_style_change_inside_a_family_splits_the_group_but_keeps_every_cell() {
    let mut trip = RoundTrip::new(3, 20, wide(20, 3));
    trip.feed("\u{1F469}\u{200D}\u{1b}[1m\u{1F467}\u{1b}[0mq")
        .frame();
    assert_eq!(trip.unit(0, 0), (0, 2, "\u{1F469}\u{200D}".into()));
    assert_eq!(trip.unit(0, 2), (2, 4, "\u{1F467}".into()));
    assert!(trip.screen.cell(0, 2).has(ATTR_BOLD));
    assert_eq!(trip.text(0, 4), "q");
}

#[test]
fn a_selection_across_a_cluster_styles_every_member() {
    let mut trip = RoundTrip::new(3, 20, wide(20, 3));
    trip.feed(&format!("ab{FAMILY}e\u{301}cd")).frame();
    let unselected = trip.screen.cell(0, 2).bg;
    trip.grid.start_selection(&Position::new(0, 1));
    trip.grid.end_selection(&Position::new(0, 7));
    trip.frame();
    assert_eq!(trip.unit(0, 2), (2, 6, FAMILY.into()));
    let selected: Vec<u32> = (1..7).map(|col| trip.screen.cell(0, col).bg).collect();
    assert!(selected
        .iter()
        .all(|bg| *bg == selected[0] && *bg != unselected));
    assert_eq!(trip.text(0, 6), "e\u{301}");
    assert_eq!(trip.screen.cell(0, 8).bg, unselected);
}

/// The previous frame filled a row with linked text; the next puts a family across the
/// client's right edge (`edge`, in client columns; the pane is drawn `dx`, `dy` in). The edge
/// columns must become blanks with no link and no stale cluster, and the IDs they released may
/// be reused without retargeting any visible cell.
fn edge_fill_case(mut trip: RoundTrip, edge: usize, dx: usize, dy: usize) {
    let mut linked = String::new();
    for index in 0..10 {
        linked.push_str(&osc8(None, &format!("https://row/{index}"), "e\u{301}b"));
    }
    trip.feed(&format!("{}\r\n", osc8(None, "https://kept", "keep")))
        .feed(&linked)
        .frame();
    let row: Vec<String> = (0..edge).map(|col| trip.text(1 + dy, col)).collect();
    assert!(
        trip.link(1 + dy, edge - 1).is_some(),
        "linked before the cut: {row:?}"
    );
    assert!(
        trip.screen.cluster_at(1 + dy, edge - 2).is_some(),
        "{row:?}"
    );
    // The family starts two client columns before the edge and needs four.
    let start = edge - 2 - dx;
    trip.feed(&format!("\u{1b}[2;{}H{FAMILY}", start + 1))
        .frame();
    for col in edge - 2..edge {
        assert_eq!(trip.text(1 + dy, col), " ", "column {col}");
        assert_eq!(trip.link(1 + dy, col), None, "column {col}");
        assert!(trip.screen.cluster_at(1 + dy, col).is_none());
    }
    assert!(
        trip.link(1 + dy, edge - 3).is_some(),
        "left of the unit is untouched"
    );
    for index in 0..300 {
        trip.feed(&format!(
            "\u{1b}[3;1H{}",
            osc8(None, &format!("https://new/{index}"), "n")
        ))
        .frame();
        assert_eq!(
            trip.link(2 + dy, dx),
            Some(format!("https://new/{index}").as_str())
        );
        assert_eq!(trip.link(dy, dx), Some("https://kept"));
        assert!(trip.link(1 + dy, edge - 3).is_some());
    }
}

#[test]
fn a_family_across_the_viewport_edge_blanks_the_visible_columns() {
    edge_fill_case(RoundTrip::new(4, 30, wide(12, 4)), 12, 0, 0);
}

// The tab is wider than the presented content here, so its rows overlap the right margin's
// row records in the same frame; the last writer must keep its cluster text.
#[test]
fn a_family_across_a_centered_clients_edge_blanks_the_visible_columns() {
    let mut trip = RoundTrip::new(4, 30, wide(16, 6));
    let presentation = Presentation {
        terminal: wide(16, 6),
        content: wide(12, 4),
        background: PaletteColor::EightBit(0),
    };
    trip.presentation = Some(presentation);
    let (dx, dy) = presentation.offset();
    assert_eq!((dx, dy), (2, 1));
    edge_fill_case(trip, 16, dx, dy);
}

#[test]
fn a_family_across_the_edge_of_a_vertically_centered_client_blanks_the_visible_columns() {
    // A client narrower than the tab but taller: no horizontal offset, rows moved down.
    let mut trip = RoundTrip::new(4, 30, wide(12, 6));
    let presentation = Presentation {
        terminal: wide(12, 6),
        content: wide(30, 4),
        background: PaletteColor::EightBit(0),
    };
    trip.presentation = Some(presentation);
    let (dx, dy) = presentation.offset();
    assert_eq!((dx, dy), (0, 1));
    edge_fill_case(trip, 12, dx, dy);
}

#[test]
fn link_ids_are_reused_past_255_uris_while_visible_cells_keep_theirs() {
    let mut trip = RoundTrip::new(4, 20, wide(20, 4));
    trip.feed(&osc8(None, "https://retained", "R")).frame();
    for index in 0..300 {
        trip.feed(&format!(
            "\u{1b}[2;1H{}",
            osc8(None, &format!("https://example/{index}"), "x")
        ))
        .frame();
        assert_eq!(
            trip.link(1, 0),
            Some(format!("https://example/{index}").as_str())
        );
        assert_eq!(trip.link(0, 0), Some("https://retained"));
        assert!(trip.carried <= 1, "deltas carry only fresh entries");
    }
}

#[test]
fn reuse_never_retargets_a_cell_emitted_earlier_in_the_same_frame() {
    let mut trip = RoundTrip::new(20, 16, wide(16, 20));
    let mut bytes = String::new();
    for index in 0..=MAX_LINK_ID as usize {
        bytes.push_str(&osc8(None, &format!("https://cell/{index}"), "x"));
    }
    trip.feed(&bytes).frame();
    for index in 0..MAX_LINK_ID as usize {
        let (row, col) = (index / 16, index % 16);
        assert_eq!(
            trip.link(row, col),
            Some(format!("https://cell/{index}").as_str()),
            "cell {index}"
        );
    }
    // The table is full when the 256th cell is reached, so it degrades to a plain cell.
    assert_eq!(trip.link(15, 15), None);
    assert_eq!(trip.text(15, 15), "x");
}

#[test]
fn a_full_repaint_reestablishes_the_complete_mapping() {
    let mut trip = RoundTrip::new(4, 20, wide(20, 4));
    trip.feed(&osc8(None, "https://a", "a")).frame();
    trip.feed(&osc8(None, "https://b", "b")).frame();
    trip.feed(&osc8(Some("x"), "https://a", "c")).frame();
    assert!(!trip.full_repaint);
    trip.states.get_mut(&CLIENT).unwrap().force_repaint();
    trip.grid.render_full_viewport();
    trip.frame();
    assert!(trip.full_repaint);
    assert_eq!(trip.carried, 3, "every visible identity is in the repaint");
    assert_eq!(trip.link(0, 0), Some("https://a"));
    assert_eq!(trip.link(0, 1), Some("https://b"));
    assert_eq!(trip.link(0, 2), Some("https://a"));
}

#[test]
fn explicit_ids_keep_links_to_one_uri_apart() {
    let mut trip = RoundTrip::new(3, 20, wide(20, 3));
    trip.feed(&osc8(Some("one"), "https://same", "ab"))
        .feed(&osc8(Some("two"), "https://same", "cd"))
        .feed(&osc8(None, "https://same", "ef"))
        .feed(&osc8(Some("one"), "https://same", "gh"))
        .frame();
    let ids: Vec<u8> = (0..8).map(|col| trip.screen.link_id_at(0, col)).collect();
    assert_eq!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
    assert_ne!(ids[2], ids[4]);
    assert_ne!(ids[0], ids[4]);
    assert_eq!(ids[6], ids[0], "the same explicit id is the same link");
    assert!((0..8).all(|col| trip.link(0, col) == Some("https://same")));
}

#[test]
fn ui_chunks_with_width_zero_entries_arrive_as_clusters_and_groups() {
    let mut trip = RoundTrip::new(3, 20, wide(20, 3));
    let label = "\u{754C}e\u{301}\u{1F469}\u{200D}\u{1F4BB}";
    let cells: Vec<TerminalCharacter> = label.chars().map(TerminalCharacter::new).collect();
    trip.send(vec![CharacterChunk::new(cells, 1, 0)]);
    assert_eq!(trip.text(0, 1), "\u{754C}");
    assert_eq!(trip.text(0, 3), "e\u{301}");
    assert_eq!(trip.unit(0, 4), (4, 8, "\u{1F469}\u{200D}\u{1F4BB}".into()));
}

#[test]
fn a_full_cluster_record_falls_back_to_first_code_points_without_breaking_the_frame() {
    let (rows, cols) = (120, 300);
    let mut trip = RoundTrip::new(rows, cols, wide(cols, rows));
    let mut bytes = String::new();
    let mut base = 0x4E00u32;
    for row in 0..rows {
        bytes.push_str(&format!("\u{1b}[{};1H", row + 1));
        for _ in 0..cols / 2 {
            bytes.push(char::from_u32(base).unwrap());
            base += 1;
            for mark in 0..29 {
                bytes.push(char::from_u32(0x300 + mark).unwrap());
            }
        }
    }
    trip.feed(&bytes).frame();
    let first = trip.text(0, 0);
    assert_eq!(first.chars().count(), 30);
    let last = trip.text(rows - 1, cols - 2);
    assert_eq!(last, char::from_u32(base - 1).unwrap().to_string());
    assert_eq!(trip.text(rows - 1, cols - 1), " ");
}

/// A scene of two side-by-side pane grids composed through `Output` at their offsets, with
/// popups and the cursor instructions tabs emit, applied to a live `TerminalState`.
mod scene {
    use super::*;
    use crate::emulation::screen_buffer::CursorShape;
    use crate::emulation::terminal::TerminalState;
    use yinz_terminal::panes::kitty_graphics::{InterceptorResult, KittyApcInterceptor};
    use yinz_terminal::panes::PaneId;
    use yinz_terminal::ui::damage::CellRect;
    use yinz_types::internal_ids::TerminalId;
    use yinz_types::pane_size::SizeInPixels;
    use yinz_types::render_negotiation::Agreement;
    use yinz_types::structured_render::{
        GeometryRecord, PaneRect, PANE_FOCUSED, PANE_POPUP, PANE_SELECTABLE,
    };

    const ROWS: usize = 6;
    const PANE_COLS: usize = 10;

    struct Scene {
        grids: [Grid; 2],
        kitty: Rc<RefCell<KittyImageStore>>,
        links: Rc<RefCell<LinkHandler>>,
        states: HashMap<ClientId, NativeRenderState>,
        size: Size,
        presentation: Option<Presentation>,
        viewer: TerminalState,
    }

    fn pane(index: usize) -> PaneRect {
        PaneRect {
            x: (index * PANE_COLS) as u16,
            y: 0,
            cols: PANE_COLS as u16,
            rows: ROWS as u16,
            flags: PANE_SELECTABLE | if index == 0 { PANE_FOCUSED } else { 0 },
            ..Default::default()
        }
    }

    fn popup_rect(rect: CellRect, focused: bool) -> PaneRect {
        PaneRect {
            x: rect.x as u16,
            y: rect.y as u16,
            cols: rect.cols as u16,
            rows: rect.rows as u16,
            flags: PANE_POPUP | if focused { PANE_FOCUSED } else { 0 },
            ..Default::default()
        }
    }

    fn offset(rect: PaneRect, (dx, dy): (usize, usize)) -> PaneRect {
        PaneRect {
            x: rect.x + dx as u16,
            y: rect.y + dy as u16,
            ..rect
        }
    }

    impl Scene {
        fn new() -> Self {
            let kitty = Rc::new(RefCell::new(KittyImageStore::default()));
            let links = Rc::new(RefCell::new(LinkHandler::new()));
            let grid = || {
                Grid::new(
                    ROWS,
                    PANE_COLS,
                    Rc::new(RefCell::new(Palette::default())),
                    Rc::new(RefCell::new(HashMap::new())),
                    links.clone(),
                    Rc::new(RefCell::new(Some(SizeInPixels {
                        width: 10,
                        height: 20,
                    }))),
                    Rc::new(RefCell::new(SixelImageStore::default())),
                    kitty.clone(),
                    Style::default(),
                    false,
                    true,
                    true,
                    true,
                    false,
                )
            };
            let size = Size {
                cols: 2 * PANE_COLS,
                rows: ROWS,
            };
            let mut viewer = TerminalState::new(size.rows, size.cols);
            viewer.set_cell_size(10, 20);
            viewer.negotiate(Agreement::default(), None);
            Scene {
                grids: [grid(), grid()],
                kitty,
                links,
                states: HashMap::from([(CLIENT, NativeRenderState::default())]),
                size,
                presentation: None,
                viewer,
            }
        }

        fn feed(&mut self, pane: usize, bytes: &[u8]) {
            let mut parser = vte::Parser::new();
            let mut interceptor = KittyApcInterceptor::new();
            for byte in bytes {
                match interceptor.advance(*byte) {
                    InterceptorResult::Forward(forward) => {
                        parser.advance(&mut self.grids[pane], forward.as_slice())
                    },
                    InterceptorResult::Captured(command) => {
                        self.grids[pane].handle_kitty_apc(&command);
                    },
                    InterceptorResult::Swallow | InterceptorResult::CaptureStarted => {},
                }
            }
        }

        /// Render both panes and send one frame with this scene's popups and cursor bytes.
        fn frame(&mut self, popups: &[(CellRect, bool)], cursor: &str) {
            let mut output = Output::new(
                Rc::new(RefCell::new(SixelImageStore::default())),
                Rc::new(RefCell::new(Some(SizeInPixels {
                    width: 10,
                    height: 20,
                }))),
                true,
                true,
                self.kitty.clone(),
                Rc::new(RefCell::new(HashMap::from([(CLIENT, true)]))),
                Rc::new(RefCell::new(HashMap::new())),
                Rc::new(RefCell::new(HashMap::new())),
                Rc::new(RefCell::new(HashMap::new())),
            );
            output.add_clients(&HashSet::from([CLIENT]), self.links.clone(), None);
            for (index, grid) in self.grids.iter_mut().enumerate() {
                let Some((chunks, _, _, kitty)) = grid
                    .render(index * PANE_COLS, 0, &Style::default())
                    .unwrap()
                else {
                    continue;
                };
                output
                    .add_character_chunks_to_client(CLIENT, chunks, None)
                    .unwrap();
                output.add_kitty_image_chunks_to_client(
                    CLIENT,
                    PaneId::Terminal(TerminalId::new(index as u32)),
                    kitty,
                    None,
                );
            }
            // Tabs and overlays always leave the frame non-empty for a native client.
            output.add_post_vte_instruction_to_client(CLIENT, cursor);
            output.set_native_geometry(
                CLIENT,
                GeometryRecord {
                    panes: vec![pane(0), pane(1)],
                },
            );
            for (rect, focused) in popups {
                output.add_native_popup(CLIENT, *rect, *focused);
            }
            let presentations: HashMap<_, _> = self
                .presentation
                .map(|presentation| (CLIENT, presentation))
                .into_iter()
                .collect();
            let mut frames = output.serialize_native(
                &mut self.states,
                &HashMap::from([(CLIENT, self.presentation.map_or(self.size, |p| p.terminal))]),
                &presentations,
            );
            self.viewer
                .apply_frame(&frames.remove(&CLIENT).unwrap())
                .expect("the live viewer accepts every production frame");
        }

        fn row(&self, row: usize) -> String {
            (0..self.viewer.size().cols)
                .map(|col| self.viewer.screen().cell(row, col).character())
                .collect()
        }
    }

    fn kitty_apc(control: &str, payload: &[u8]) -> Vec<u8> {
        use base64::Engine as _;
        let mut out = b"\x1b_G".to_vec();
        out.extend_from_slice(control.as_bytes());
        out.push(b';');
        out.extend_from_slice(
            base64::engine::general_purpose::STANDARD
                .encode(payload)
                .as_bytes(),
        );
        out.extend_from_slice(b"\x1b\\");
        out
    }

    #[test]
    fn a_popup_over_a_kitty_image_keeps_the_pane_as_its_owner_and_cursor_rules_hold() {
        let mut scene = Scene::new();
        scene.feed(0, b"LEFT\r\n");
        // A 2x2-cell image at the left pane's second row; the cursor stays put.
        let raster: Vec<u8> = (0..20 * 40 * 3).map(|i| (i % 251) as u8).collect();
        scene.feed(0, &kitty_apc("a=T,f=24,s=20,v=40,i=1,C=1", &raster));
        scene.feed(1, b"RIGHT");
        let palette = CellRect {
            x: 0,
            y: 0,
            cols: 8,
            rows: 3,
        };
        let toast = CellRect {
            x: 12,
            y: 4,
            cols: 6,
            rows: 2,
        };
        scene.frame(
            &[(toast, false), (palette, true)],
            "\x1b[2;5H\x1b[6 q\x1b[?25h",
        );
        assert_eq!(
            scene.viewer.geometry().panes,
            vec![
                pane(0),
                pane(1),
                popup_rect(toast, false),
                popup_rect(palette, true)
            ],
            "the scene arrives with popups last, in paint order"
        );
        let placements: Vec<_> = scene.viewer.graphics().placements().collect();
        assert_eq!(
            placements.len(),
            1,
            "the image under the palette is accepted"
        );
        let (x, y) = (placements[0].cell_x as u16, placements[0].cell_y as u16);
        assert_eq!((x, y), (0, 1));
        assert_eq!(
            scene.viewer.geometry().content_pane_at(x, y).unwrap().flags & PANE_POPUP,
            PANE_POPUP
        );
        assert_eq!(scene.viewer.geometry().image_owner_at(x, y), Some(&pane(0)));
        assert!(scene.row(0).starts_with("LEFT"));
        assert_eq!(&scene.row(0)[10..15], "RIGHT");
        assert!(scene.viewer.cursor_is_visible());
        assert_eq!(scene.viewer.cursor_position(), (1, 4));
        assert_eq!(scene.viewer.cursor_shape(), CursorShape::Beam);

        // An input overlay hides the cursor; position and shape survive the hide.
        scene.feed(1, b"!");
        scene.frame(&[(palette, true)], "\x1b[?25l");
        assert!(!scene.viewer.cursor_is_visible());
        assert_eq!(scene.viewer.cursor_position(), (1, 4));
        assert_eq!(scene.viewer.cursor_shape(), CursorShape::Beam);
        assert_eq!(
            scene.viewer.geometry().panes,
            vec![pane(0), pane(1), popup_rect(palette, true)],
            "the toast's rect left with it"
        );

        // The overlay closes: the cursor returns where it was, the scene is the panes alone.
        scene.feed(1, b"?");
        scene.frame(&[], "\x1b[?25h");
        assert!(scene.viewer.cursor_is_visible());
        assert_eq!(scene.viewer.cursor_position(), (1, 4));
        assert_eq!(scene.viewer.cursor_shape(), CursorShape::Beam);
        assert_eq!(scene.viewer.geometry().panes, vec![pane(0), pane(1)]);
        assert_eq!(scene.viewer.graphics().placements().count(), 1);
    }

    #[test]
    fn a_presentation_change_rebuilds_geometry_cursor_and_cells_and_a_cropped_cursor_hides() {
        let mut scene = Scene::new();
        scene.feed(0, b"LEFT");
        scene.feed(1, b"RIGHT");
        let menu = CellRect {
            x: 11,
            y: 1,
            cols: 6,
            rows: 3,
        };
        scene.frame(&[(menu, true)], "\x1b[1;5H\x1b[?25h");
        assert_eq!(scene.viewer.cursor_position(), (0, 4));

        // The client grows past the tab: the frame is a full repaint, everything offset.
        let presentation = Presentation {
            terminal: Size { cols: 24, rows: 8 },
            content: scene.size,
            background: PaletteColor::EightBit(0),
        };
        let (dx, dy) = presentation.offset();
        assert_eq!((dx, dy), (2, 1));
        scene.presentation = Some(presentation);
        for grid in &mut scene.grids {
            grid.render_full_viewport();
        }
        scene.frame(&[(menu, true)], "");
        assert_eq!(scene.viewer.size().cols, 24);
        assert_eq!(
            scene.viewer.geometry().panes,
            vec![
                offset(pane(0), (dx, dy)),
                offset(pane(1), (dx, dy)),
                offset(popup_rect(menu, true), (dx, dy))
            ]
        );
        assert!(scene.viewer.cursor_is_visible());
        assert_eq!(scene.viewer.cursor_position(), (dy, 4 + dx));
        assert_eq!(&scene.row(dy)[dx..dx + 4], "LEFT");
        assert_eq!(&scene.row(dy)[dx + 10..dx + 15], "RIGHT");

        // Mid-reflow the client is narrower than the tab; the cursor in the right pane is
        // past its grid and hidden, cells are cropped, and the cursor returns once it fits.
        scene.presentation = None;
        scene.size = Size { cols: 12, rows: 6 };
        for grid in &mut scene.grids {
            grid.render_full_viewport();
        }
        scene.frame(&[], "\x1b[1;16H");
        assert_eq!(scene.viewer.size().cols, 12);
        assert!(!scene.viewer.cursor_is_visible());
        assert_eq!(scene.row(0), "LEFT      RI");
        scene.size = Size {
            cols: 2 * PANE_COLS,
            rows: ROWS,
        };
        for grid in &mut scene.grids {
            grid.render_full_viewport();
        }
        scene.frame(&[], "");
        assert!(scene.viewer.cursor_is_visible());
        assert_eq!(scene.viewer.cursor_position(), (0, 15));
    }
}

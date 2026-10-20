//! How fast can yinz turn pty bytes into a frame when many panes are all producing output at
//! once?
//!
//! This drives the server's render pipeline in-process — no ptys, no threads, no client, and
//! none of the 10ms render debounce (`REPAINT_DELAY_MS` in `yinz-bus/src/background_jobs.rs`).
//! What is measured is everything between "bytes arrived from the pty" and "string handed to the
//! client":
//!
//! 1. `TerminalPane::handle_pty_bytes` - the kitty-graphics interceptor and the vte parse that
//!    mutates the `Grid`, marking lines dirty in the pane's `OutputBuffer`.
//! 2. `Pane::render` - `Grid::render` reads the dirty lines back out as `CharacterChunk`s.
//! 3. `Output::serialize` - the chunks for every pane become one ANSI string per client.
//!
//! `Screen` is private to yinz-screen (its only export is `screen_thread_main`), and yinz-server
//! does not depend on yinz-tab, so this builds the same fleet-of-panes-into-one-`Output` shape
//! that `Tab::render_with_inactive_clients` does, mirroring the chunk plumbing in
//! `yinz-panes/src/ui/pane_contents_and_ui.rs`. Pane frames are left out on purpose, matching a
//! `pane_frames false` session: frames are chrome redrawn only when a pane's boundaries change, so
//! they are not part of the steady-state cost this measures.
//!
//! Run with `cargo x bench`. See AGENTS.md for the baseline-comparison workflow and
//! `docs/render-profiling.md` for the single-pane graphics workload and live profiling.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hint::black_box;
use std::rc::Rc;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use yinz_server::output::{CharacterChunk, FloatingPanesStack, HostKittyState, Output};
use yinz_server::pane::Pane;
use yinz_server::panes::{KittyImageStore, LinkHandler, SixelImageStore, TerminalPane};
use yinz_terminal::{panes::TerminalCharacter, ui::damage::CellRect};

use yinz_types::pane_size::{Dimension, PaneGeom, Size, SizeInPixels};
use yinz_types::style::{Palette, Style};

/// The host terminal the pane fleet is tiled over. Large enough that a 16-way split still leaves
/// each pane a usable 50x12.
const SURFACE: Size = Size {
    rows: 50,
    cols: 200,
};

/// Bytes handed to *each* pane per iteration. A real pty read is up to 64KiB
/// (`yinz-pty/src/terminal_bytes.rs`), and a render covers whatever arrived inside one 10ms
/// debounce window, so this is a plausible per-pane-per-frame budget for a chatty program.
const BYTES_PER_PANE: usize = 8 * 1024;

const PANE_COUNTS: &[usize] = &[1, 2, 4, 8, 16];

/// `CursorMoves` only writes to every `ROW_STRIDE`th row, so the rest of the viewport stays clean.
const ROW_STRIDE: usize = 4;

const CLIENT_ID: yinz_server::ClientId = yinz_server::ClientId::new(1);

// ---------------------------------------------------------------------------------------------
// deterministic byte generation
// ---------------------------------------------------------------------------------------------

/// xorshift64*, so the workloads are reproducible run to run without pulling in a dependency.
/// Baseline comparisons are meaningless if the input drifts between runs.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // any non-zero state will do; fold the seed in so pane 0 and pane 7 get different text
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Workload {
    /// Plain ASCII that wraps and scrolls: `Grid::print`, scroll handling, whole-line dirtying.
    PlainText,
    /// Sparse output must not allocate a full screen row for each line.
    BlankLines,
    ShortLines,
    /// Text with dense SGR changes: `csi_dispatch`, per-character styling, and the SGR emission
    /// half of `serialize_chunks`.
    SgrChurn,
    /// A Kitty upload in the same pty read as dense text styling.
    MixedKittySgr,
    /// Absolute cursor positioning plus short in-place rewrites, the shape a `top`-like TUI has.
    /// Confined to every fourth row, so most of the viewport stays clean between frames - the
    /// best case for the per-line diff, and the one workload where it can pay off.
    CursorMoves,
    /// Alt-screen full-viewport repaint every frame. Every line dirty, every chunk re-serialized:
    /// the worst case for both the diff and the serializer.
    AltScreenRedraw,
    /// CJK, emoji (including ZWJ sequences and variation selectors) and combining marks. Stresses
    /// width calculation, wide-cell handling and UTF-8 throughout.
    UnicodeWide,
}

impl Workload {
    const ALL: &'static [Workload] = &[
        Workload::PlainText,
        Workload::BlankLines,
        Workload::ShortLines,
        Workload::SgrChurn,
        Workload::MixedKittySgr,
        Workload::CursorMoves,
        Workload::AltScreenRedraw,
        Workload::UnicodeWide,
    ];

    fn name(&self) -> &'static str {
        match self {
            Workload::PlainText => "plain_text",
            Workload::BlankLines => "blank_lines",
            Workload::ShortLines => "short_lines",
            Workload::SgrChurn => "sgr_churn",
            Workload::MixedKittySgr => "mixed_kitty_sgr",
            Workload::CursorMoves => "cursor_moves",
            Workload::AltScreenRedraw => "alt_screen_redraw",
            Workload::UnicodeWide => "unicode_wide",
        }
    }

    /// Fed once, before measurement, to put the pane in the mode the workload assumes.
    fn prelude(&self) -> &'static [u8] {
        match self {
            Workload::AltScreenRedraw => b"\x1b[?1049h",
            _ => b"",
        }
    }

    /// One iteration's worth of bytes for a single pane. Not exactly `target` bytes - generation
    /// stops at the first complete unit past it, so an escape sequence is never cut in half.
    fn chunk(&self, rows: usize, cols: usize, seed: u64, target: usize) -> Vec<u8> {
        let mut rng = Rng::new(seed);
        let mut out = Vec::with_capacity(target + 64);
        match self {
            Workload::PlainText => {
                while out.len() < target {
                    push_words(&mut out, &mut rng, cols * 9 / 10);
                    out.extend_from_slice(b"\r\n");
                }
            },
            Workload::BlankLines | Workload::ShortLines => {
                while out.len() < target {
                    if *self == Workload::ShortLines {
                        out.extend_from_slice(b"short log");
                    }
                    out.extend_from_slice(b"\r\n");
                }
            },
            Workload::SgrChurn | Workload::MixedKittySgr => {
                if *self == Workload::MixedKittySgr {
                    out.extend_from_slice(b"\x1b_Ga=t,f=32,s=1,v=1,i=1,q=2;AAAA/w==\x1b\\");
                }
                while out.len() < target {
                    let width = cols * 9 / 10;
                    let mut written = 0;
                    while written < width {
                        push_sgr(&mut out, &mut rng);
                        let run = 3 + rng.below(7);
                        for _ in 0..run.min(width - written) {
                            out.push(WORD_ALPHABET[rng.below(WORD_ALPHABET.len())]);
                            written += 1;
                        }
                    }
                    out.extend_from_slice(b"\x1b[m\r\n");
                }
            },
            Workload::CursorMoves => {
                // Rewrite short runs in place on a strided subset of the rows, like a
                // status/monitor TUI whose headers and gutters never change. Confining the writes
                // this way is the point of the workload: spread over every row, the byte budget
                // dirties the whole viewport and the per-line diff has nothing left to skip.
                let touchable_rows = rows.div_ceil(ROW_STRIDE).max(1);
                while out.len() < target {
                    let row = 1 + (rng.below(touchable_rows) * ROW_STRIDE).min(rows - 1);
                    let col = 1 + rng.below((cols / 2).max(1));
                    out.extend_from_slice(format!("\x1b[{};{}H", row, col).as_bytes());
                    push_words(&mut out, &mut rng, (cols / 3).max(1));
                }
            },
            Workload::AltScreenRedraw => {
                // full repaints until the budget is spent; each one dirties every line
                while out.len() < target {
                    out.extend_from_slice(b"\x1b[H");
                    for row in 1..=rows {
                        out.extend_from_slice(format!("\x1b[{};1H\x1b[K", row).as_bytes());
                        push_words(&mut out, &mut rng, cols.saturating_sub(1));
                    }
                }
            },
            Workload::UnicodeWide => {
                while out.len() < target {
                    let width = cols * 9 / 10;
                    let mut written = 0;
                    while written < width {
                        let glyph = &UNICODE_GLYPHS[rng.below(UNICODE_GLYPHS.len())];
                        out.extend_from_slice(glyph.text.as_bytes());
                        written += glyph.width;
                    }
                    out.extend_from_slice(b"\r\n");
                }
            },
        }
        out
    }
}

const WORD_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789/.-_";

fn push_words(out: &mut Vec<u8>, rng: &mut Rng, width: usize) {
    let mut written = 0;
    while written < width {
        let word_len = (2 + rng.below(10)).min(width - written);
        for _ in 0..word_len {
            out.push(WORD_ALPHABET[rng.below(WORD_ALPHABET.len())]);
        }
        written += word_len;
        if written < width {
            out.push(b' ');
            written += 1;
        }
    }
}

fn push_sgr(out: &mut Vec<u8>, rng: &mut Rng) {
    match rng.below(4) {
        0 => out.extend_from_slice(format!("\x1b[38;5;{}m", rng.below(256)).as_bytes()),
        1 => out.extend_from_slice(
            format!(
                "\x1b[38;2;{};{};{}m",
                rng.below(256),
                rng.below(256),
                rng.below(256)
            )
            .as_bytes(),
        ),
        2 => out.extend_from_slice(format!("\x1b[48;5;{}m", rng.below(256)).as_bytes()),
        _ => {
            // bold / dim / italic / underline
            let attr = [1, 2, 3, 4][rng.below(4)];
            out.extend_from_slice(format!("\x1b[{}m", attr).as_bytes())
        },
    }
}

struct Glyph {
    text: &'static str,
    /// display width in terminal cells
    width: usize,
}

const UNICODE_GLYPHS: &[Glyph] = &[
    // CJK - two cells each
    Glyph {
        text: "日",
        width: 2,
    },
    Glyph {
        text: "本",
        width: 2,
    },
    Glyph {
        text: "語",
        width: 2,
    },
    Glyph {
        text: "한",
        width: 2,
    },
    Glyph {
        text: "字",
        width: 2,
    },
    // emoji, including a ZWJ sequence and a variation selector
    Glyph {
        text: "🦀",
        width: 2,
    },
    Glyph {
        text: "🎉",
        width: 2,
    },
    Glyph {
        text: "👩‍💻",
        width: 2,
    },
    Glyph {
        text: "❤️",
        width: 2,
    },
    // combining marks over an ascii base - one cell, several codepoints
    Glyph {
        text: "e\u{0301}",
        width: 1,
    },
    Glyph {
        text: "a\u{0308}",
        width: 1,
    },
    // plain ascii, so the run is not uniformly wide
    Glyph {
        text: "x",
        width: 1,
    },
    Glyph {
        text: " ",
        width: 1,
    },
];

// ---------------------------------------------------------------------------------------------
// the pane fleet
// ---------------------------------------------------------------------------------------------

/// `n` terminal panes tiled over [`SURFACE`], sharing the image stores, link handler and palette
/// the way a real `Screen` shares them across a tab.
struct PaneFleet {
    panes: Vec<TerminalPane>,
    sixel_image_store: Rc<RefCell<SixelImageStore>>,
    kitty_image_store: Rc<RefCell<KittyImageStore>>,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    link_handler: Rc<RefCell<LinkHandler>>,
    kitty_host_capabilities: Rc<RefCell<HashMap<yinz_server::ClientId, bool>>>,
    kitty_host_state: Rc<RefCell<HashMap<yinz_server::ClientId, HostKittyState>>>,
    sixel_host_capabilities: Rc<RefCell<HashMap<yinz_server::ClientId, bool>>>,
    clients: HashSet<yinz_server::ClientId>,
}

impl PaneFleet {
    fn new(pane_count: usize) -> Self {
        let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
        let kitty_image_store = Rc::new(RefCell::new(KittyImageStore::default()));
        let character_cell_size = Rc::new(RefCell::new(None));
        let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
        let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
        let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));

        let panes = tile(pane_count)
            .into_iter()
            .enumerate()
            .map(|(index, geom)| {
                TerminalPane::new(
                    yinz_server::TerminalId::new(index as u32),
                    geom,
                    Style::default(),
                    index,
                    String::new(),
                    link_handler.clone(),
                    character_cell_size.clone(),
                    sixel_image_store.clone(),
                    kitty_image_store.clone(),
                    terminal_emulator_colors.clone(),
                    terminal_emulator_color_codes.clone(),
                    None,  // initial_pane_title
                    None,  // invoked_with
                    false, // debug
                    true,  // arrow_fonts
                    true,  // styled_underlines
                    true,  // osc8_hyperlinks
                    false, // explicitly_disable_keyboard_protocol
                    None,  // notification_end
                )
            })
            .collect();

        PaneFleet {
            panes,
            sixel_image_store,
            kitty_image_store,
            character_cell_size,
            link_handler,
            kitty_host_capabilities: Rc::new(RefCell::new(HashMap::new())),
            kitty_host_state: Rc::new(RefCell::new(HashMap::new())),
            sixel_host_capabilities: Rc::new(RefCell::new(HashMap::new())),
            clients: HashSet::from([CLIENT_ID]),
        }
    }

    /// The content area of the smallest pane, which every workload is generated against so no
    /// pane is fed lines wider than it can hold.
    fn smallest_content_size(&self) -> (usize, usize) {
        self.panes
            .iter()
            .map(|pane| (pane.get_content_rows(), pane.get_content_columns()))
            .min()
            .unwrap_or((SURFACE.rows, SURFACE.cols))
    }

    fn ingest(&mut self, chunks: &[Vec<u8>]) {
        // The clone is deliberate and belongs inside the timed region: `handle_pty_bytes` takes
        // its bytes by value, and the real pipeline pays exactly one `Vec` allocation per pty read
        // to hand them over (`bytes.to_vec()` in `yinz-pty/src/terminal_bytes.rs`).
        for (pane, chunk) in self.panes.iter_mut().zip(chunks) {
            pane.handle_pty_bytes(chunk.clone());
        }
    }

    /// One frame: collect every dirty pane's chunks into a single `Output` and serialize it, the
    /// way `Screen::render_to_clients` does.
    fn frame(&mut self) -> HashMap<yinz_server::ClientId, String> {
        self.frame_with_panel(None)
    }

    fn frame_with_panel(&mut self, opacity: Option<u8>) -> HashMap<yinz_server::ClientId, String> {
        let mut output = Output::new(
            self.sixel_image_store.clone(),
            self.character_cell_size.clone(),
            true, // styled_underlines
            true, // osc8_hyperlinks
            self.kitty_image_store.clone(),
            self.kitty_host_capabilities.clone(),
            self.kitty_host_state.clone(),
            self.sixel_host_capabilities.clone(),
            Rc::new(RefCell::new(HashMap::new())),
        );
        let rect = CellRect {
            x: 3,
            y: 14,
            cols: 120,
            rows: 33,
        };
        let stack = opacity.map(|_| FloatingPanesStack {
            layers: vec![PaneGeom {
                x: rect.x,
                y: rect.y,
                cols: Dimension::fixed(rect.cols),
                rows: Dimension::fixed(rect.rows),
                ..Default::default()
            }],
        });
        output.add_clients(&self.clients, self.link_handler.clone(), stack);
        if let Some(opacity) = opacity.filter(|opacity| *opacity < 100) {
            let palette = yinz_types::style::default_palette();
            output.begin_translucent_composition(
                &self.clients,
                vec![(1, rect, opacity)],
                palette.fg,
                palette.bg,
                Style::default().colors.text_unselected.background,
                &HashMap::new(),
            );
            // Same deliberate repaint as Tab::force_render_panes_intersecting, including clean rows.
            for pane in &mut self.panes {
                if rect.intersects_geom(&pane.current_geom()) {
                    pane.set_should_render(true);
                    pane.render_full_viewport();
                }
            }
        }

        for pane in self.panes.iter_mut() {
            // mirrors PaneContentsAndUi::render_pane_contents_for_client
            if let Some((
                character_chunks,
                raw_vte_output,
                sixel_image_chunks,
                kitty_image_chunks,
            )) = pane.render(Some(CLIENT_ID)).unwrap()
            {
                output
                    .add_character_chunks_to_client(CLIENT_ID, character_chunks, None)
                    .unwrap();
                output.add_sixel_image_chunks_to_client(CLIENT_ID, sixel_image_chunks, None);
                output.add_kitty_image_chunks_to_client(
                    CLIENT_ID,
                    pane.pid(),
                    kitty_image_chunks,
                    None,
                );
                if let Some(raw_vte_output) = raw_vte_output
                    && !raw_vte_output.is_empty()
                {
                    output.add_post_vte_instruction_to_client(
                        CLIENT_ID,
                        &format!(
                            "\u{1b}[{};{}H\u{1b}[m{}",
                            pane.y() + 1,
                            pane.x() + 1,
                            raw_vte_output
                        ),
                    );
                }
            }
        }

        // An opaque idle panel is already on the host and contributes no dirty rows.
        if opacity.is_some_and(|opacity| opacity < 100) {
            // A visible idle panel: mostly blank cells, with a title and a solid input row.
            for y in rect.y..rect.y + rect.rows {
                let mut row =
                    CharacterChunk::new(vec![TerminalCharacter::new(' '); rect.cols], rect.x, y);
                if y == rect.y {
                    row.terminal_characters[..5].clone_from_slice(
                        &"Agent"
                            .chars()
                            .map(TerminalCharacter::new)
                            .collect::<Vec<_>>(),
                    );
                }
                if y == rect.y + rect.rows - 2 {
                    row.pane_default_bg =
                        Some(Style::default().colors.text_unselected.background.into());
                }
                output
                    .add_character_chunks_to_client(CLIENT_ID, vec![row], Some(1))
                    .unwrap();
            }
            output.finish_translucent_composition();
        }
        output.serialize().unwrap()
    }

    /// A fresh `OutputBuffer` has `should_update_all_lines = true`, so an unwarmed first frame
    /// measures a full-viewport repaint no matter what the workload is. Burn one frame first so
    /// measurement sees the steady state.
    fn warm(&mut self, workload: Workload, chunks: &[Vec<u8>]) {
        let prelude = workload.prelude();
        if !prelude.is_empty() {
            for pane in self.panes.iter_mut() {
                pane.handle_pty_bytes(prelude.to_vec());
            }
        }
        self.ingest(chunks);
        black_box(self.frame());
    }
}

/// Split [`SURFACE`] into `n` roughly-square panes, distributing the remainder over the leading
/// rows/columns so the tiling covers the surface exactly.
fn tile(n: usize) -> Vec<PaneGeom> {
    assert!(n > 0, "a fleet needs at least one pane");
    let cols_count = (n as f64).sqrt().ceil() as usize;
    let rows_count = n.div_ceil(cols_count);

    let mut geoms = Vec::with_capacity(n);
    let mut placed = 0;
    let mut y = 0;
    for row in 0..rows_count {
        let height = SURFACE.rows / rows_count + usize::from(row < SURFACE.rows % rows_count);
        // the last row takes whatever panes are left rather than a full stripe
        let in_this_row = cols_count.min(n - placed);
        let mut x = 0;
        for col in 0..in_this_row {
            let width = SURFACE.cols / in_this_row + usize::from(col < SURFACE.cols % in_this_row);
            geoms.push(PaneGeom {
                x,
                y,
                rows: Dimension::fixed(height),
                cols: Dimension::fixed(width),
                ..Default::default()
            });
            x += width;
        }
        placed += in_this_row;
        y += height;
        if placed == n {
            break;
        }
    }
    geoms
}

/// One chunk per pane, each pane seeded differently so they are not all rendering identical text.
fn chunks_for(workload: Workload, fleet: &PaneFleet) -> Vec<Vec<u8>> {
    let (rows, cols) = fleet.smallest_content_size();
    (0..fleet.panes.len())
        .map(|index| {
            let seed = workload.name().len() as u64 * 1_000_003 + index as u64;
            workload.chunk(rows, cols, seed, BYTES_PER_PANE)
        })
        .collect()
}

fn setup(workload: Workload, pane_count: usize) -> (PaneFleet, Vec<Vec<u8>>) {
    let mut fleet = PaneFleet::new(pane_count);
    let chunks = chunks_for(workload, &fleet);
    fleet.warm(workload, &chunks);
    (fleet, chunks)
}

/// Generation stops at the first complete unit past [`BYTES_PER_PANE`], so the chunks overshoot
/// the nominal budget a little. Report throughput against what is actually fed, not the budget.
fn byte_count(chunks: &[Vec<u8>]) -> u64 {
    chunks.iter().map(|chunk| chunk.len() as u64).sum()
}

// ---------------------------------------------------------------------------------------------
// benchmarks
// ---------------------------------------------------------------------------------------------

/// Parse only: what `handle_pty_bytes` costs across the fleet. Isolates the kitty interceptor and
/// the vte-into-`Grid` half of the pipeline.
fn bench_ingest(c: &mut Criterion) {
    let mut group = c.benchmark_group("ingest");
    for workload in Workload::ALL {
        for &pane_count in PANE_COUNTS {
            let (mut fleet, chunks) = setup(*workload, pane_count);
            group.throughput(Throughput::Bytes(byte_count(&chunks)));
            group.bench_function(BenchmarkId::new(workload.name(), pane_count), |b| {
                b.iter(|| fleet.ingest(black_box(&chunks)));
            });
        }
    }
    group.finish();
}

/// The headline number: one debounced frame's worth of work with every pane dirty - ingest plus
/// diff plus chunk plus serialize.
fn bench_frame(c: &mut Criterion) {
    let mut group = c.benchmark_group("frame");
    for workload in Workload::ALL {
        for &pane_count in PANE_COUNTS {
            let (mut fleet, chunks) = setup(*workload, pane_count);
            group.throughput(Throughput::Bytes(byte_count(&chunks)));
            group.bench_function(BenchmarkId::new(workload.name(), pane_count), |b| {
                b.iter(|| {
                    fleet.ingest(black_box(&chunks));
                    black_box(fleet.frame())
                });
            });
        }
    }
    group.finish();
}

/// Render and serialize only, with ingest kept outside the timed region. Isolates the
/// `OutputBuffer` diff, chunk extraction and `Output::serialize`.
fn bench_render_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_only");
    for workload in Workload::ALL {
        for &pane_count in PANE_COUNTS {
            let (mut fleet, chunks) = setup(*workload, pane_count);
            group.throughput(Throughput::Bytes(byte_count(&chunks)));
            group.bench_function(BenchmarkId::new(workload.name(), pane_count), |b| {
                b.iter_custom(|iters| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iters {
                        fleet.ingest(&chunks);
                        let start = Instant::now();
                        let serialized = fleet.frame();
                        elapsed += start.elapsed();
                        black_box(serialized);
                    }
                    elapsed
                });
            });
        }
    }
    group.finish();
}

/// One pane repeatedly replaces a direct-transmission Kitty image, as a graphics-heavy TUI
/// does. Two precomputed images alternate so ingest and host encoding cannot reuse unchanged
/// pixels. Delete the previous image so even the ingest-only case has bounded storage.
fn bench_kitty_graphics(c: &mut Criterion) {
    use yinz_server::panes::kitty_graphics::test_support::{apc, apc_b64};

    const WIDTH: usize = 800;
    const HEIGHT: usize = 480;
    // 3072 raw bytes become Kitty's maximum 4096-byte base64 payload per APC.
    const CHUNK_BYTES: usize = 3072;

    let frames: Vec<Vec<Vec<u8>>> = (1..=2)
        .map(|seed| {
            let mut rng = Rng::new(seed);
            let pixels: Vec<u8> = (0..WIDTH * HEIGHT * 3).map(|_| rng.next() as u8).collect();
            let mut frame = apc("a=d,d=I,i=1,q=2");
            frame.extend_from_slice(b"\x1b[H");
            for (index, chunk) in pixels.chunks(CHUNK_BYTES).enumerate() {
                let more = usize::from((index + 1) * CHUNK_BYTES < pixels.len());
                let control = if index == 0 {
                    format!("a=T,f=24,t=d,i=1,s={WIDTH},v={HEIGHT},C=1,q=2,m={more}")
                } else {
                    format!("m={more},q=2")
                };
                frame.extend(apc_b64(&control, chunk));
            }
            vec![frame]
        })
        .collect();

    let mut group = c.benchmark_group("kitty_graphics");
    for stage in ["ingest", "frame", "render_only"] {
        let mut fleet = PaneFleet::new(1);
        *fleet.character_cell_size.borrow_mut() = Some(SizeInPixels {
            width: 8,
            height: 16,
        });
        fleet
            .kitty_host_capabilities
            .borrow_mut()
            .insert(CLIENT_ID, true);

        // Exercise both variants before timing. A missing capability or invalid transmission
        // must fail the benchmark instead of silently measuring text-only output.
        let mut serialized_bytes = 0;
        for frame in &frames {
            fleet.ingest(frame);
            assert_eq!(fleet.kitty_image_store.borrow().image_count(), 1);
            let output = fleet.frame();
            let content = &output[&CLIENT_ID];
            assert!(content.contains("\x1b_Ga=t,"), "no Kitty image transmitted");
            assert!(content.len() > WIDTH * HEIGHT * 4, "missing image payload");
            serialized_bytes = content.len();
        }
        eprintln!(
            "kitty_graphics/{stage}: one pane, {WIDTH}x{HEIGHT} RGB, input_bytes={}, serialized_bytes={serialized_bytes}",
            byte_count(&frames[0]),
        );
        group.throughput(Throughput::Bytes(if stage == "render_only" {
            serialized_bytes as u64
        } else {
            byte_count(&frames[0])
        }));
        let mut iteration = 0;
        group.bench_function(stage, |b| {
            b.iter_custom(|iters| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iters {
                    let frame = &frames[iteration % frames.len()];
                    iteration += 1;
                    if stage == "render_only" {
                        fleet.ingest(black_box(frame));
                    }
                    let start = Instant::now();
                    if stage != "render_only" {
                        fleet.ingest(black_box(frame));
                    }
                    let output = (stage != "ingest").then(|| fleet.frame());
                    elapsed += start.elapsed();
                    black_box(output);
                }
                elapsed
            });
        });
    }
    group.finish();
}

fn bench_panel(c: &mut Criterion) {
    let mut group = c.benchmark_group("panel");
    for &panes in &[1, 4, 16] {
        for opacity in [100, 75, 0] {
            let (mut fleet, chunks) = setup(Workload::CursorMoves, panes);
            group.bench_function(BenchmarkId::new(format!("opacity_{opacity}"), panes), |b| {
                b.iter(|| {
                    fleet.ingest(black_box(&chunks));
                    black_box(fleet.frame_with_panel(Some(opacity)))
                });
            });
        }
    }
    group.finish();
}

// Isolate composition and serialization with a full text background, including large terminals.
fn bench_panel_text(c: &mut Criterion) {
    use yinz_terminal::panes::terminal_character::{AnsiCode, DEFAULT_STYLES};

    let mut group = c.benchmark_group("panel_text");
    for (cols, rows) in [(200, 50), (400, 120)] {
        for background in ["blank", "plain", "colored"] {
            let clients = HashSet::from([CLIENT_ID]);
            let links = Rc::new(RefCell::new(LinkHandler::new()));
            let rect = CellRect {
                x: 0,
                y: 0,
                cols,
                rows,
            };
            let under: Vec<_> = (0..rows)
                .map(|y| {
                    CharacterChunk::new(
                        (0..cols)
                            .map(|x| {
                                let mut character =
                                    TerminalCharacter::new(if background == "blank" {
                                        ' '
                                    } else {
                                        char::from(b'a' + (x % 26) as u8)
                                    });
                                if background == "colored" {
                                    character.styles = DEFAULT_STYLES
                                        .foreground(Some(AnsiCode::ColorIndex((x / 8 % 16) as u8)))
                                        .into();
                                }
                                character
                            })
                            .collect(),
                        0,
                        y,
                    )
                })
                .collect();
            let over: Vec<_> = (0..rows)
                .map(|y| CharacterChunk::new(vec![TerminalCharacter::new(' '); cols], 0, y))
                .collect();
            group.bench_function(
                BenchmarkId::new(background, format!("{cols}x{rows}")),
                |b| {
                    b.iter(|| {
                        let mut output = Output::default();
                        output.add_clients(&clients, links.clone(), None);
                        let palette = yinz_types::style::default_palette();
                        output.begin_translucent_composition(
                            &clients,
                            vec![(1, rect, 75)],
                            palette.fg,
                            palette.bg,
                            Style::default().colors.text_unselected.background,
                            &HashMap::new(),
                        );
                        output
                            .add_character_chunks_to_client(CLIENT_ID, under.clone(), None)
                            .unwrap();
                        output
                            .add_character_chunks_to_client(CLIENT_ID, over.clone(), Some(1))
                            .unwrap();
                        output.finish_translucent_composition();
                        black_box(output.serialize().unwrap());
                    })
                },
            );
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_ingest,
    bench_frame,
    bench_render_only,
    bench_kitty_graphics,
    bench_panel,
    bench_panel_text
);
criterion_main!(benches);

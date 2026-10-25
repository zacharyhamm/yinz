//! Semantic checks across PTY ingestion, pane diffs, and host serialization.
use super::TerminalPane;
use crate::pane::{Pane, TerminalPaneIo};
use crate::panes::{KittyImageStore, LinkHandler, SixelImageStore};
use hegel::generators;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};
use yinz_terminal::{
    output::{HostKittyState, Output},
    panes::{terminal_character::AnsiCode, TerminalCharacter},
};
use yinz_types::{
    internal_ids::{ClientId, TerminalId},
    pane_size::{Dimension, PaneGeom, SizeInPixels},
    style::{Palette, Style},
};

const CLIENT: ClientId = ClientId::new(1);

fn geom(x: usize, y: usize, cols: usize, rows: usize) -> PaneGeom {
    PaneGeom {
        x,
        y,
        cols: Dimension::fixed(cols),
        rows: Dimension::fixed(rows),
        ..Default::default()
    }
}

struct Surface {
    panes: Vec<TerminalPane>,
    sixel: Rc<RefCell<SixelImageStore>>,
    kitty: Rc<RefCell<KittyImageStore>>,
    links: Rc<RefCell<LinkHandler>>,
    cell: Rc<RefCell<Option<SizeInPixels>>>,
    host_state: Rc<RefCell<HashMap<ClientId, HostKittyState>>>,
}

impl Surface {
    fn new(geometries: &[PaneGeom]) -> Self {
        let mut surface = Self {
            panes: vec![],
            sixel: Rc::new(RefCell::new(SixelImageStore::default())),
            kitty: Rc::new(RefCell::new(KittyImageStore::default())),
            links: Rc::new(RefCell::new(LinkHandler::new())),
            cell: Rc::new(RefCell::new(Some(SizeInPixels {
                width: 8,
                height: 16,
            }))),
            host_state: Rc::new(RefCell::new(HashMap::new())),
        };
        for (index, geometry) in geometries.iter().enumerate() {
            surface.panes.push(TerminalPane::new(
                TerminalId::new(index as u32 + 1),
                *geometry,
                Style::default(),
                index,
                String::new(),
                surface.links.clone(),
                surface.cell.clone(),
                surface.sixel.clone(),
                surface.kitty.clone(),
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
            ));
        }
        surface
    }

    fn frame(&mut self, full: bool, graphics: bool) -> String {
        let mut output = Output::new(
            self.sixel.clone(),
            self.cell.clone(),
            true,
            true,
            self.kitty.clone(),
            Rc::new(RefCell::new(HashMap::from([(CLIENT, graphics)]))),
            self.host_state.clone(),
            Rc::new(RefCell::new(HashMap::new())),
            Rc::new(RefCell::new(HashMap::new())),
        );
        output.add_clients(&HashSet::from([CLIENT]), self.links.clone(), None);
        for pane in &mut self.panes {
            if full {
                pane.render_full_viewport();
                pane.set_should_render(true);
            }
            if let Some((text, raw, sixel, kitty)) = pane.render(Some(CLIENT)).unwrap() {
                output
                    .add_character_chunks_to_client(CLIENT, text, None)
                    .unwrap();
                output.add_sixel_image_chunks_to_client(CLIENT, sixel, None);
                output.add_kitty_image_chunks_to_client(CLIENT, pane.pid(), kitty, None);
                if let Some(raw) = raw.filter(|raw| !raw.is_empty()) {
                    output.add_post_vte_instruction_to_client(
                        CLIENT,
                        &format!("\x1b[{};{}H\x1b[m{raw}", pane.y() + 1, pane.x() + 1),
                    );
                }
            }
        }
        output
            .serialize()
            .unwrap()
            .remove(&CLIENT)
            .unwrap_or_default()
    }
}

fn cells(pane: &mut TerminalPane) -> Vec<Vec<TerminalCharacter>> {
    pane.grid.render_full_viewport();
    pane.grid
        .read_changes(0, 0)
        .0
        .into_iter()
        .map(|chunk| chunk.terminal_characters)
        .collect()
}

fn number(tc: &hegel::TestCase, min: usize, max: usize) -> usize {
    tc.draw(
        generators::integers::<usize>()
            .min_value(min)
            .max_value(max),
    )
}

// Each draw exercises the five benchmark workloads, plus erasure and scroll boundaries.
fn operation(tc: &hegel::TestCase, kind: usize, cols: usize, rows: usize) -> String {
    match kind {
        0 => format!("text {}\r\n", number(tc, 0, 99)),
        1 => format!(
            "\x1b[38;5;{};48;2;{};20;30mstyled \x1b[m",
            number(tc, 0, 255),
            number(tc, 0, 255)
        ),
        2 => format!(
            "\x1b[{};{}Hx\x1b[K",
            number(tc, 1, rows),
            number(tc, 1, cols)
        ),
        3 => match number(tc, 0, 2) {
            0 => "\x1b[?1049h\x1b[H\x1b[2Jalternate",
            1 => "\x1b[H\x1b[2Jredrawn alternate screen",
            _ => "\x1b[?1049l",
        }
        .into(),
        4 => "界🙂👩\u{200d}💻e\u{301}✈\u{fe0f}".into(),
        5 => "\r\x1b[44m\x1b[2K \x1b[m".into(),
        _ => format!("\x1b[1;{}r\x1b[{};1H\n\n\x1b[r", rows, rows),
    }
}

fn feed_partitioned(pane: &mut TerminalPane, bytes: &[u8], sizes: &[usize]) {
    let mut offset = 0;
    for size in sizes.iter().cycle() {
        pane.handle_pty_bytes(vec![]);
        let end = (offset + size).min(bytes.len());
        pane.handle_pty_bytes(bytes[offset..end].to_vec());
        offset = end;
        if offset == bytes.len() {
            break;
        }
    }
}

#[hegel::test]
fn pty_partitions_preserve_styled_state_and_replies(tc: hegel::TestCase) {
    let (cols, rows) = (number(&tc, 1, 16), number(&tc, 1, 6));
    let mut whole = Surface::new(&[geom(0, 0, cols, rows)]);
    let mut split = Surface::new(&[geom(0, 0, cols, rows)]);
    let sizes: Vec<_> = (0..8).map(|_| number(&tc, 1, 13)).collect();
    let mut stream = Vec::new();
    for step in 0..number(&tc, 7, 21) {
        stream.extend(
            operation(
                &tc,
                if step < 7 { step } else { number(&tc, 0, 6) },
                cols,
                rows,
            )
            .into_bytes(),
        );
        stream.extend_from_slice(b"\x1b[6n");
    }
    whole.panes[0].handle_pty_bytes(stream.clone());
    feed_partitioned(&mut split.panes[0], &stream, &sizes);
    let (a, b) = (&mut whole.panes[0], &mut split.panes[0]);
    assert_eq!(
        cells(a),
        cells(b),
        "stream {stream:?}, partitions {sizes:?}"
    );
    assert_eq!(a.cursor_coordinates(), b.cursor_coordinates());
    let replies = a.drain_messages_to_pty();
    assert!(!replies.is_empty());
    assert_eq!(replies, b.drain_messages_to_pty());
    assert_eq!(a.pending_pty_input, b.pending_pty_input);
    // A partial parser token must also survive the partitioning into the next read.
    for bytes in [
        b"\x1b[38;2;1;".as_slice(),
        b"2;3m",
        "界".as_bytes(),
        b"\x1b[6n",
    ] {
        whole.panes[0].handle_pty_bytes(bytes.to_vec());
        feed_partitioned(&mut split.panes[0], bytes, &[1]);
    }
    // The host-query pause must retain exactly the bytes that will be replayed later.
    whole.panes[0].arm_forward_pause();
    split.panes[0].arm_forward_pause();
    whole.panes[0].handle_pty_bytes(stream.clone());
    feed_partitioned(&mut split.panes[0], &stream, &sizes);
    assert_eq!(whole.panes[0].drain_pending_pty_input(), stream);
    assert_eq!(split.panes[0].drain_pending_pty_input(), stream);
    assert_eq!(cells(&mut whole.panes[0]), cells(&mut split.panes[0]));
    assert_eq!(
        whole.panes[0].drain_messages_to_pty(),
        split.panes[0].drain_messages_to_pty()
    );
}

/// Parser batches end at Kitty capture starts and read boundaries. Splitting a stream with APCs,
/// other escapes and doubled ESCs changes those batches without changing the resulting pane, and
/// the pane is the one byte-at-a-time ingest builds.
#[hegel::test]
fn bulk_and_intercepted_ingest_leave_identical_panes(tc: hegel::TestCase) {
    let (cols, rows) = (number(&tc, 1, 16), number(&tc, 1, 6));
    let mut whole = Surface::new(&[geom(0, 0, cols, rows)]);
    let mut split = Surface::new(&[geom(0, 0, cols, rows)]);
    let mut per_byte = Surface::new(&[geom(0, 0, cols, rows)]);
    let sizes: Vec<_> = (0..8).map(|_| number(&tc, 1, 13)).collect();
    let mut stream = Vec::new();
    for _ in 0..number(&tc, 1, 20) {
        let piece = match number(&tc, 0, 10) {
            7 => format!(
                "\x1b_Ga=T,f=24,s=1,v=1,i={},q=2;AAAA\x1b\\",
                number(&tc, 1, 3)
            ),
            // An APC that is not Kitty graphics, and a doubled ESC.
            8 => "\x1b_other\x1b\\".into(),
            9 => "\x1b\x1b[1mbold\x1b[m".into(),
            10 => "\x1b_\x1b[2mdim\x1b[m".into(),
            kind => operation(&tc, kind, cols, rows),
        };
        stream.extend(piece.into_bytes());
    }
    whole.panes[0].handle_pty_bytes(stream.clone());
    feed_partitioned(&mut split.panes[0], &stream, &sizes);
    per_byte.panes[0].ingest_per_byte(&stream);
    let (a, b, c) = (
        &mut whole.panes[0],
        &mut split.panes[0],
        &mut per_byte.panes[0],
    );
    assert_eq!(
        cells(a),
        cells(b),
        "stream {stream:?}, partitions {sizes:?}"
    );
    assert_eq!(cells(a), cells(c), "stream {stream:?}");
    assert_eq!(a.cursor_coordinates(), b.cursor_coordinates());
    assert_eq!(a.cursor_coordinates(), c.cursor_coordinates());
    assert_eq!(a.pending_pty_input, b.pending_pty_input);
    assert!(
        a.pending_pty_input.is_empty(),
        "the stream asks the host nothing"
    );
    let replies = a.drain_messages_to_pty();
    assert_eq!(replies, b.drain_messages_to_pty());
    assert_eq!(replies, c.drain_messages_to_pty());
    let frame = whole.frame(true, true);
    assert_eq!(frame, split.frame(true, true));
    assert_eq!(frame, per_byte.frame(true, true));
}

fn check_frame(surface: &mut Surface, host: &mut Surface, cols: usize, rows: usize) {
    host.panes[0].handle_pty_bytes(surface.frame(false, false).into_bytes());
    let mut fresh = Surface::new(&[geom(0, 0, cols, rows)]);
    fresh.panes[0].handle_pty_bytes(surface.frame(true, false).into_bytes());
    assert_eq!(cells(&mut host.panes[0]), cells(&mut fresh.panes[0]));
}

#[hegel::test]
fn incremental_frames_match_full_repaint(tc: hegel::TestCase) {
    let (cols, rows) = (number(&tc, 1, 18), number(&tc, 1, 6));
    let mut surface = Surface::new(&[geom(0, 0, cols, rows)]);
    let mut host = Surface::new(&[geom(0, 0, cols, rows)]);
    check_frame(&mut surface, &mut host, cols, rows);
    let interval = number(&tc, 1, 4);
    for step in 0..number(&tc, 14, 28) {
        let bytes = operation(
            &tc,
            if step < 7 { step } else { number(&tc, 0, 6) },
            cols,
            rows,
        );
        surface.panes[0].handle_pty_bytes(bytes.into_bytes());
        if step % interval == 0 {
            check_frame(&mut surface, &mut host, cols, rows);
        }
    }
    check_frame(&mut surface, &mut host, cols, rows);
    check_frame(&mut surface, &mut host, cols, rows);
}

#[test]
fn fixed_frames_preserve_short_lines_styled_blanks_and_alternate_screen() {
    let mut surface = Surface::new(&[geom(0, 0, 6, 3)]);
    let mut host = Surface::new(&[geom(0, 0, 6, 3)]);
    for bytes in [
        "abcdef",
        "\rxy\x1b[K",
        "\x1b[2;1H\x1b[48;2;1;2;3m\x1b[2K\x1b[m",
        "\x1b[?1049h\x1b[Hother",
        "\x1b[H\x1b[2Jnew",
        "\x1b[?1049l",
    ] {
        surface.panes[0].handle_pty_bytes(bytes.as_bytes().to_vec());
        check_frame(&mut surface, &mut host, 6, 3);
    }
    let actual = cells(&mut host.panes[0]);
    assert_eq!(
        actual[0].iter().map(|c| c.character).collect::<String>(),
        "xy    "
    );
    for cell in &actual[1] {
        assert_eq!(cell.character, ' ');
        assert_eq!(cell.styles.background, Some(AnsiCode::RgbCode((1, 2, 3))));
    }
    for (cols, rows) in [(1, 1), (6, 3)] {
        surface.panes[0].set_geom(geom(0, 0, cols, rows));
        host.panes[0].set_geom(geom(0, 0, cols, rows));
        check_frame(&mut surface, &mut host, cols, rows);
    }
}

#[test]
fn uneven_pane_fleets_keep_offsets_styles_and_neighbors() {
    for count in [1, 2, 4, 8, 16] {
        let (cols, rows) = (37, 17);
        let across = if count >= 4 { 4 } else { count };
        let down = count / across;
        let geometries: Vec<_> = (0..count)
            .map(|i| {
                let (x, y) = (i % across * cols / across, i / across * rows / down);
                geom(
                    x,
                    y,
                    (i % across + 1) * cols / across - x,
                    (i / across + 1) * rows / down - y,
                )
            })
            .collect();
        let mut surface = Surface::new(&geometries);
        let mut host = Surface::new(&[geom(0, 0, cols, rows)]);
        for (i, pane) in surface.panes.iter_mut().enumerate() {
            pane.handle_pty_bytes(
                format!("\x1b[38;5;{}m{}\x1b[m", i + 16, (b'A' + i as u8) as char).into_bytes(),
            );
        }
        check_frame(&mut surface, &mut host, cols, rows);
        let before = cells(&mut host.panes[0]);
        for (i, g) in geometries.iter().enumerate() {
            assert_eq!(before[g.y][g.x].character, (b'A' + i as u8) as char);
            assert_eq!(
                before[g.y][g.x].styles.foreground,
                Some(AnsiCode::ColorIndex(i as u8 + 16))
            );
            assert_eq!(before[g.y][g.x + 1].character, ' ');
        }
        surface.panes[0].handle_pty_bytes(b"\x1b[H\x1b[48;2;9;8;7mZ\x1b[K".to_vec());
        check_frame(&mut surface, &mut host, cols, rows);
        let after = cells(&mut host.panes[0]);
        for g in &geometries[1..] {
            for y in g.y..g.y + g.rows.as_usize() {
                assert_eq!(
                    after[y][g.x..g.x + g.cols.as_usize()],
                    before[y][g.x..g.x + g.cols.as_usize()]
                );
            }
        }
        check_frame(&mut surface, &mut host, cols, rows);
        assert_eq!(cells(&mut host.panes[0]), after);
        for bytes in [
            "\r\nplain text that wraps and scrolls\r\n",
            "\x1b[38;5;43mindexed\x1b[48;2;7;8;9m RGB \x1b[m",
            "\x1b[2;2Hx\x1b[K",
            "\x1b[?1049h\x1b[2Jalt",
            "\x1b[H\x1b[2Jredraw",
            "\x1b[?1049l",
            "界🙂👩\u{200d}💻e\u{301}✈\u{fe0f}",
        ] {
            for pane in &mut surface.panes {
                feed_partitioned(pane, bytes.as_bytes(), &[1, 3, 2]);
            }
            check_frame(&mut surface, &mut host, cols, rows);
            check_frame(&mut surface, &mut host, cols, rows);
        }
    }
}

fn kitty_commands(frame: &str) -> Vec<(HashMap<&str, &str>, &str)> {
    frame
        .split("\x1b_G")
        .skip(1)
        .map(|tail| {
            let body = tail.split_once("\x1b\\").expect("terminated APC").0;
            let (control, payload) = body.split_once(';').unwrap_or((body, ""));
            (
                control
                    .split(',')
                    .map(|pair| pair.split_once('=').unwrap())
                    .collect(),
                payload,
            )
        })
        .collect()
}

fn kitty_replacements(
    dimensions: &[(usize, usize)],
    seed: u8,
    partitions: &[usize],
    upload_bytes: usize,
) {
    use crate::panes::kitty_graphics::test_support::{apc, apc_b64};
    let mut surface = Surface::new(&[geom(2, 1, 132, 4)]);
    let mut unsupported = Surface::new(&[geom(2, 1, 132, 4)]);
    let mut host = Surface::new(&[geom(0, 0, 134, 5)]);
    for (step, &(width, height)) in dimensions.iter().enumerate() {
        let mut noise = 0x9e3779b9u32 ^ ((seed as u32) << 16) ^ step as u32;
        let rgb: Vec<_> = (0..width * height * 3)
            .map(|_| {
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                noise as u8
            })
            .collect();
        let expected: Vec<_> = rgb
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect();
        // Same delete-and-retransmit lifecycle as the benchmark, with a moving placement.
        let mut bytes = apc("a=d,d=I,i=1,q=2");
        let column = step % 2 + 1;
        bytes.extend_from_slice(format!("\x1b[2;{column}H").as_bytes());
        let chunks: Vec<_> = rgb.chunks(upload_bytes).collect();
        for (index, chunk) in chunks.iter().enumerate() {
            let more = usize::from(index + 1 < chunks.len());
            let control = if index == 0 {
                format!("a=T,f=24,t=d,i=1,s={width},v={height},C=1,q=2,m={more}")
            } else {
                format!("m={more},q=2")
            };
            bytes.extend(apc_b64(&control, chunk));
        }
        feed_partitioned(&mut surface.panes[0], &bytes, partitions);
        feed_partitioned(&mut unsupported.panes[0], &bytes, partitions);
        assert!(kitty_commands(&unsupported.frame(false, false)).is_empty());
        assert_eq!(surface.kitty.borrow().image_count(), 1);
        assert_eq!(surface.kitty.borrow().total_bytes(), expected.len());
        assert_eq!(surface.panes[0].grid.kitty_placement_count(), 1);
        assert!(surface.panes[0].drain_messages_to_pty().is_empty());
        assert!(surface.panes[0].pending_pty_input.is_empty());
        let frame = surface.frame(false, true);
        let commands = kitty_commands(&frame);
        let transmits: Vec<_> = commands
            .iter()
            .filter(|(c, _)| c.get("a").copied().unwrap_or("t") == "t")
            .collect();
        assert!(!transmits.is_empty());
        if width >= 1024 {
            assert!(
                transmits.len() > 1,
                "noisy fixture must still chunk after compression"
            );
        }
        assert_eq!(transmits[0].0["s"], width.to_string());
        assert_eq!(transmits[0].0["v"], height.to_string());
        assert_eq!(transmits[0].0["f"], "32");
        assert_eq!(transmits[0].0["o"], "z");
        for (index, (control, payload)) in transmits.iter().enumerate() {
            // Spec: data is "chunked up into chunks no larger than 4096 bytes".
            assert!(payload.len() <= 4096);
            assert_eq!(
                control["m"],
                if index + 1 < transmits.len() {
                    "1"
                } else {
                    "0"
                }
            );
        }
        host.panes[0].handle_pty_bytes(frame.into_bytes());
        let placements = host.panes[0].grid.kitty_placements();
        assert_eq!(
            placements.len(),
            1,
            "replacement must withdraw the previous placement"
        );
        let placement = &placements[0];
        assert_eq!(
            (placement.display_rect.x, placement.display_rect.y),
            ((2 + column - 1) * 8, 2 * 16)
        );
        assert_eq!(
            (placement.display_rect.width, placement.display_rect.height),
            (width, height)
        );
        let store = host.kitty.borrow();
        assert_eq!(store.image_count(), 1, "old host image must be freed");
        let image = store.get(placement.internal_id).unwrap();
        assert_eq!(
            (image.width as usize, image.height as usize),
            (width, height)
        );
        assert_eq!(image.rgba, expected);
        assert_eq!(surface.host_state.borrow()[&CLIENT].transmitted.len(), 1);
        assert_eq!(
            surface.host_state.borrow()[&CLIENT].live_placements.len(),
            1
        );
        assert!(kitty_commands(&surface.frame(false, true)).is_empty());
        // Even a forced redraw reuses the upload and placement on an unchanged host.
        assert!(kitty_commands(&surface.frame(true, true)).is_empty());
    }
    surface.panes[0].handle_pty_bytes(apc("a=d,d=I,i=1,q=2"));
    host.panes[0].handle_pty_bytes(surface.frame(false, true).into_bytes());
    assert_eq!(surface.kitty.borrow().image_count(), 0);
    assert_eq!(host.kitty.borrow().image_count(), 0);
    assert_eq!(host.panes[0].grid.kitty_placement_count(), 0);
}

#[hegel::test]
fn kitty_replacement_survives_upload_and_pty_partitions(tc: hegel::TestCase) {
    let dimensions: Vec<_> = (0..number(&tc, 2, 5))
        .map(|_| (number(&tc, 1, 40), number(&tc, 1, 16)))
        .collect();
    let partitions: Vec<_> = (0..6).map(|_| number(&tc, 1, 31)).collect();
    kitty_replacements(
        &dimensions,
        number(&tc, 0, 255) as u8,
        &partitions,
        3 * number(&tc, 1, 64),
    );
}

#[test]
fn kitty_payloads_straddle_input_and_output_base64_chunk_boundaries() {
    // RGB input reaches 4096 base64 bytes at 1024 pixels. Noise keeps the larger
    // compressed RGBA fixtures above the host chunk limit too.
    kitty_replacements(
        &[
            (767, 1),
            (768, 1),
            (769, 1),
            (1023, 1),
            (1024, 1),
            (1025, 1),
            (1, 1),
        ],
        19,
        &[1, 7, 4095, 2],
        3072,
    );
}

#[test]
fn a_wide_character_in_a_one_column_pane_erases_the_previous_frame() {
    let mut surface = Surface::new(&[geom(0, 0, 1, 3)]);
    let mut host = Surface::new(&[geom(0, 0, 1, 3)]);
    surface.panes[0].handle_pty_bytes(b"x\r\nx\r\nx".to_vec());
    check_frame(&mut surface, &mut host, 1, 3);
    surface.panes[0].handle_pty_bytes("\x1b[H界".as_bytes().to_vec());
    check_frame(&mut surface, &mut host, 1, 3);
    assert_eq!(cells(&mut host.panes[0])[1][0].character, ' ');
}

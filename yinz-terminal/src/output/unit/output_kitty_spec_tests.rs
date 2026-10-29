//! Conformance of the graphics commands yinz *emits*.
//!
//! Yinz does not pass an application's kitty escape codes through to the terminal it is running
//! inside. It decodes them, stores the raster, and re-encodes fresh commands per client with its
//! own image and placement ids. That makes yinz a graphics *client* here, and the same
//! specification applies to what it sends: the chunking rules, the quiet level, and the cursor
//! policy are all obligations rather than choices.
//!
//! Derived from `graphics-protocol.rst` ("Transferring larger amounts of data", "Suppressing
//! responses", "Controlling displayed image layout"). What the existing tests in `output_tests.rs`
//! already cover — transmit-once diffing, occlusion cropping, host id ranges, capability gating —
//! is not repeated here.

use super::output_tests::{
    create_test_kitty_parts, extract_kitty_commands, kitty_chunk, run_kitty_frame,
    run_kitty_frame_with_cell_size, store_test_kitty_image,
};

/// The control data of a command, as key/value pairs.
fn control_of(command: &[u8]) -> Vec<(String, String)> {
    let control = match command.iter().position(|byte| *byte == b';') {
        Some(position) => &command[..position],
        None => command,
    };
    String::from_utf8_lossy(control)
        .split(',')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let (key, value) = segment.split_once('=').unwrap_or((segment, ""));
            (key.to_owned(), value.to_owned())
        })
        .collect()
}

fn value_of<'a>(control: &'a [(String, String)], key: &str) -> Option<&'a str> {
    control
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// The base64 payload of a command, if it has one.
fn payload_of(command: &[u8]) -> &[u8] {
    match command.iter().position(|byte| *byte == b';') {
        Some(position) => &command[position + 1..],
        None => &[],
    }
}

fn action_of(command: &[u8]) -> String {
    value_of(&control_of(command), "a")
        .unwrap_or("t")
        .to_owned()
}

/// One frame's worth of emitted commands, for an image large enough to need several chunks.
fn commands_for_a_large_image() -> Vec<Vec<u8>> {
    let parts = create_test_kitty_parts();
    // 200x200 RGBA is 160,000 bytes, well over 4096 base64 characters once encoded.
    let internal = store_test_kitty_image(&parts.0, 200, 200);
    let frame = run_kitty_frame(&parts, vec![kitty_chunk(internal, 1, 0, 0)], None);
    extract_kitty_commands(&frame)
}

/// The transmission commands of [`commands_for_a_large_image`], which every chunking rule below
/// indexes from both ends. Asserting the shape here means a change in emission behaviour fails
/// with this message rather than with an index panic inside whichever test noticed first.
fn transmits_for_a_large_image() -> Vec<Vec<u8>> {
    let transmits: Vec<Vec<u8>> = commands_for_a_large_image()
        .into_iter()
        .filter(|command| action_of(command) == "t")
        .collect();
    assert!(
        transmits.len() > 1,
        "this fixture is meant to be large enough to need chunking, got {} transmit command(s)",
        transmits.len()
    );
    transmits
}

#[test]
fn no_transmitted_chunk_exceeds_the_documented_maximum() {
    // Spec: the data is "chunked up into chunks no larger than 4096 bytes". A host terminal is
    // entitled to drop anything longer.
    let transmits = transmits_for_a_large_image();

    for command in &transmits {
        assert!(
            payload_of(command).len() <= 4096,
            "a chunk carried {} payload bytes",
            payload_of(command).len()
        );
    }
}

#[test]
fn every_chunk_but_the_last_is_a_multiple_of_four_base64_characters() {
    // Spec: "All chunks, except the last, must have a size that is a multiple of 4". Splitting
    // base64 anywhere else would corrupt the reassembled image.
    let transmits = transmits_for_a_large_image();

    for command in &transmits[..transmits.len() - 1] {
        let payload = payload_of(command);
        assert_eq!(
            payload.len() % 4,
            0,
            "a non-final chunk was {} bytes, not a multiple of 4",
            payload.len()
        );
    }
}

#[test]
fn the_chunk_sequence_is_marked_with_m_and_terminated_with_m_zero() {
    // Spec: `m=1` on every chunk that has a successor, `m=0` on the last one. Without the
    // terminator the host waits forever for more data.
    let transmits = transmits_for_a_large_image();

    for command in &transmits[..transmits.len() - 1] {
        assert_eq!(
            value_of(&control_of(command), "m"),
            Some("1"),
            "a non-final chunk did not announce more data"
        );
    }
    assert_eq!(
        value_of(&control_of(&transmits[transmits.len() - 1]), "m"),
        Some("0"),
        "the final chunk must close the sequence"
    );
}

#[test]
fn only_the_first_chunk_carries_the_image_metadata() {
    // Spec: "only the first escape code needs to have the full set of control codes such as width,
    // height, format, etc. Subsequent chunks must have only the m and optionally q keys". A host
    // that sees `s=` again mid-transfer is entitled to treat it as a new image.
    let transmits = transmits_for_a_large_image();

    let first = control_of(&transmits[0]);
    assert!(
        value_of(&first, "s").is_some(),
        "the first chunk sizes the image"
    );
    assert!(value_of(&first, "v").is_some());
    assert!(value_of(&first, "i").is_some());

    for command in &transmits[1..] {
        let control = control_of(command);
        for key in ["s", "v", "f", "t", "a"] {
            assert_eq!(
                value_of(&control, key),
                None,
                "a continuation chunk repeated the {:?} key",
                key
            );
        }
    }
}

#[test]
fn every_emitted_command_suppresses_the_hosts_replies() {
    // Spec, `q=2`: suppress both success and error responses. The host terminal writes its
    // replies to yinz's own stdin, where they would otherwise be indistinguishable from the
    // user typing.
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 30, 40);
    let frame = run_kitty_frame(&parts, vec![kitty_chunk(internal, 1, 0, 0)], None);

    let commands = extract_kitty_commands(&frame);
    assert!(!commands.is_empty());
    for command in &commands {
        assert_eq!(
            value_of(&control_of(command), "q"),
            Some("2"),
            "a command did not suppress replies: {:?}",
            String::from_utf8_lossy(command)
        );
    }
}

#[test]
fn placements_tell_the_host_not_to_move_its_cursor() {
    // Yinz positions every image itself and then keeps drawing text; if the host moved its own
    // cursor after a placement, everything drawn afterwards would land in the wrong cell.
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 30, 40);
    let frame = run_kitty_frame(&parts, vec![kitty_chunk(internal, 1, 0, 0)], None);

    let placements: Vec<Vec<u8>> = extract_kitty_commands(&frame)
        .into_iter()
        .filter(|command| action_of(command) == "p")
        .collect();
    assert!(!placements.is_empty());
    for command in &placements {
        assert_eq!(value_of(&control_of(command), "C"), Some("1"));
    }
}

#[test]
fn a_placement_names_both_an_image_and_a_placement_id() {
    // Spec: a placement is identified by the pair, and re-sending the pair replaces it rather
    // than stacking a second copy. Yinz relies on that to move images between frames.
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 30, 40);
    let frame = run_kitty_frame(&parts, vec![kitty_chunk(internal, 1, 0, 0)], None);

    let placements: Vec<Vec<u8>> = extract_kitty_commands(&frame)
        .into_iter()
        .filter(|command| action_of(command) == "p")
        .collect();
    assert!(!placements.is_empty());
    for command in &placements {
        let control = control_of(command);
        let image_id: u32 = value_of(&control, "i")
            .expect("a placement names its image")
            .parse()
            .expect("a numeric image id");
        assert_ne!(image_id, 0, "id 0 means 'no id', which cannot be replaced");
        assert!(value_of(&control, "p").is_some(), "and its placement id");
    }
}

#[test]
fn transmitted_images_are_sent_as_rgba_with_their_dimensions() {
    // The store normalises everything to RGBA, so the host is always told f=32 and given exact
    // pixel dimensions rather than being asked to decode anything.
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 30, 40);
    let frame = run_kitty_frame(&parts, vec![kitty_chunk(internal, 1, 0, 0)], None);

    let first_transmit = extract_kitty_commands(&frame)
        .into_iter()
        .find(|command| action_of(command) == "t")
        .expect("the image is transmitted");
    let control = control_of(&first_transmit);
    assert_eq!(value_of(&control, "f"), Some("32"));
    assert_eq!(
        value_of(&control, "t"),
        Some("d"),
        "sent inline, not via a file"
    );
    assert_eq!(value_of(&control, "s"), Some("30"));
    assert_eq!(value_of(&control, "v"), Some("40"));
}

#[test]
fn a_deletion_names_what_it_is_deleting() {
    // A bare `a=d` would delete every image on the host's screen, including those belonging to
    // other panes and to whatever else shares that terminal.
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 30, 40);
    run_kitty_frame(&parts, vec![kitty_chunk(internal, 1, 0, 0)], None);
    // A frame with no chunks: the placement from the previous frame has to be withdrawn.
    let frame = run_kitty_frame(&parts, vec![], None);

    for command in extract_kitty_commands(&frame)
        .into_iter()
        .filter(|command| action_of(command) == "d")
    {
        let control = control_of(&command);
        let specifier = value_of(&control, "d").expect("a delete names its specifier");
        assert!(
            matches!(specifier, "i" | "I" | "n" | "N" | "r" | "R"),
            "delete used the untargeted specifier {:?}",
            specifier
        );
        assert!(
            value_of(&control, "i").is_some(),
            "and the image it applies to"
        );
    }
}

#[test]
fn the_payload_of_every_transmitted_chunk_is_valid_base64() {
    let commands = commands_for_a_large_image();
    for command in commands.iter().filter(|command| action_of(command) == "t") {
        let payload = payload_of(command);
        assert!(
            payload
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')),
            "a chunk payload contained a character outside the base64 alphabet"
        );
    }
}

#[test]
fn kitty_placeholder_crops_share_a_raster_across_render_frames_and_drop_stale_resized_strips() {
    use crate::panes::grid::Grid;
    use crate::panes::kitty_graphics::KittyApcInterceptor;
    use crate::panes::kitty_test_support::{
        apc_b64, feed_bytes, placeholder_cells, PlaceholderCell, PlaceholderColor,
    };
    use crate::panes::sixel::SixelImageStore;
    use crate::panes::LinkHandler;
    use std::{cell::RefCell, collections::HashMap, rc::Rc};
    use yinz_types::{
        data::{Palette, Style},
        pane_size::SizeInPixels,
    };

    // Spec: "The image will eventually be fit to the specified rectangle, its aspect
    // ratio preserved." Crops of that fitted raster must meet at terminal-cell boundaries.
    // https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders
    let parts = create_test_kitty_parts();
    let cell = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 10,
        height: 19,
    })));
    let mut grid = Grid::new(
        10,
        20,
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(LinkHandler::new())),
        cell.clone(),
        Rc::new(RefCell::new(SixelImageStore::default())),
        parts.0.clone(),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    );
    let run_frame =
        |chunks| run_kitty_frame_with_cell_size(&parts, chunks, None, cell.borrow().unwrap());
    let mut parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    let mut bytes = apc_b64(
        "a=T,f=24,s=20,v=20,i=1,U=1,x=4,y=2,w=10,h=10,c=10,r=6,q=2",
        &[255; 1200],
    );
    let placeholder_start = bytes.len();
    for row in 0..6 {
        bytes.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
        bytes.extend_from_slice(
            placeholder_cells(
                PlaceholderColor::Indexed(1),
                None,
                &(0..10)
                    .map(|column| PlaceholderCell::at(row, column))
                    .collect::<Vec<_>>(),
            )
            .as_bytes(),
        );
    }
    feed_bytes(&mut grid, &mut parser, &mut interceptor, &bytes);
    let chunks = grid.read_changes(0, 0).2;
    assert_eq!(chunks.len(), 6);
    for chunk in &chunks {
        assert_eq!(
            chunk.variant_key.source,
            crate::panes::sixel::PixelRect::new(4, 2, 10, 10)
        );
        assert_eq!(chunk.variant_key.size, (100, 100));
    }
    let uids: Vec<_> = chunks.iter().map(|chunk| chunk.placement_uid).collect();
    for (row, chunk) in chunks.iter().enumerate() {
        assert_eq!((chunk.cell_x, chunk.cell_y), (0, row));
        assert_eq!((chunk.source_px_x, chunk.source_px_y), (0, row * 19));
        assert_eq!(
            (chunk.source_px_width, chunk.source_px_height),
            (100, if row == 5 { 5 } else { 19 })
        );
    }
    let first = extract_kitty_commands(&run_frame(chunks));
    let transmissions: Vec<_> = first
        .iter()
        .filter(|command| action_of(command) == "t")
        .map(|command| control_of(command))
        .filter(|control| value_of(control, "s").is_some())
        .collect();
    assert_eq!(
        transmissions.len(),
        1,
        "all strips share one fitted raster upload"
    );
    assert_eq!(value_of(&transmissions[0], "s"), Some("100"));
    assert_eq!(value_of(&transmissions[0], "v"), Some("100"));
    let placements: Vec<_> = first
        .iter()
        .filter(|command| action_of(command) == "p")
        .map(|command| control_of(command))
        .collect();
    assert_eq!(placements.len(), 6);
    for (row, control) in placements.iter().enumerate() {
        for (key, expected) in [
            ("x", 0),
            ("y", row * 19),
            ("w", 100),
            ("h", if row == 5 { 5 } else { 19 }),
        ] {
            assert_eq!(value_of(control, key), Some(expected.to_string().as_str()));
        }
        assert_eq!(value_of(control, "i"), value_of(&placements[0], "i"));
    }

    // Plugin render frames rebuild their viewport. Redrawing continuing runs must reuse
    // their placement identities and the single fitted raster already sent to the host.
    grid.delete_viewport_and_scroll();
    feed_bytes(
        &mut grid,
        &mut parser,
        &mut interceptor,
        &bytes[placeholder_start..],
    );
    let chunks = grid.read_changes(0, 0).2;
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| chunk.placement_uid)
            .collect::<Vec<_>>(),
        uids
    );
    assert!(extract_kitty_commands(&run_frame(chunks)).is_empty());

    // Taller cells leave the last two placeholder rows entirely outside the 100x100 raster.
    *cell.borrow_mut() = Some(SizeInPixels {
        width: 10,
        height: 25,
    });
    grid.character_cell_size_possibly_changed();
    let chunks = grid.read_changes(0, 0).2;
    assert_eq!(chunks.len(), 4);
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| chunk.placement_uid)
            .collect::<Vec<_>>(),
        uids[..4]
    );
    for (row, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.source_px_y, row * 25);
        assert_eq!(chunk.source_px_height, 25);
    }
    let resized = extract_kitty_commands(&run_frame(chunks));
    for obsolete in &placements[4..] {
        assert!(
            resized.iter().any(|command| {
                let control = control_of(command);
                action_of(command) == "d"
                    && value_of(&control, "p") == value_of(obsolete, "p")
                    && value_of(&control, "i") == value_of(obsolete, "i")
            }),
            "obsolete strip must be withdrawn from the host"
        );
    }

    // A scrolled/fragmented run can start inside the image, at a different screen row.
    // Its source offsets must survive serialization rather than being reset to zero.
    let mut offset_run = b"\x1b[1;1H\x1b[2K".to_vec();
    offset_run.extend_from_slice(
        placeholder_cells(
            PlaceholderColor::Indexed(1),
            None,
            &(2..10)
                .map(|column| PlaceholderCell::at(1, column))
                .collect::<Vec<_>>(),
        )
        .as_bytes(),
    );
    feed_bytes(&mut grid, &mut parser, &mut interceptor, &offset_run);
    let shifted = extract_kitty_commands(&run_frame(grid.read_changes(0, 0).2));
    let shifted_placements: Vec<_> = shifted
        .iter()
        .filter(|command| action_of(command) == "p")
        .map(|command| control_of(command))
        .collect();
    assert_eq!(shifted_placements.len(), 1);
    for (key, expected) in [("x", "20"), ("y", "25"), ("w", "80"), ("h", "25")] {
        assert_eq!(value_of(&shifted_placements[0], key), Some(expected));
    }

    // A wider cell changes the fitted raster itself and must replace the host's old pixels.
    *cell.borrow_mut() = Some(SizeInPixels {
        width: 12,
        height: 25,
    });
    grid.character_cell_size_possibly_changed();
    let resized = extract_kitty_commands(&run_frame(grid.read_changes(0, 0).2));
    assert!(resized.iter().any(
        |command| action_of(command) == "d" && value_of(&control_of(command), "d") == Some("I")
    ));
    let mut decoder = crate::panes::kitty_graphics::KittyCommandParser::new();
    let mut decoded = None;
    for command in resized.iter().filter(|command| action_of(command) == "t") {
        if let Some(command) = decoder.parse(command) {
            decoded = command.unwrap().image;
        }
    }
    let image = decoded.expect("resize uploads the replacement raster");
    assert_eq!((image.width, image.height), (120, 120));
    assert_eq!(image.bytes, vec![255; 120 * 120 * 4]);

    // Spec: c selects "the 1-based frame number of the frame that should be made current".
    let frame_pixels: Vec<u8> = (0..400).flat_map(|_| [12, 34, 56, 255]).collect();
    feed_bytes(
        &mut grid,
        &mut parser,
        &mut interceptor,
        &apc_b64("a=f,f=32,s=20,v=20,i=1,q=2", &frame_pixels),
    );
    feed_bytes(
        &mut grid,
        &mut parser,
        &mut interceptor,
        &apc_b64("a=a,i=1,c=2,q=2", &[]),
    );
    let animated = extract_kitty_commands(&run_frame(grid.read_changes(0, 0).2));
    let mut decoded = None;
    for command in animated.iter().filter(|command| action_of(command) == "t") {
        if let Some(command) = decoder.parse(command) {
            decoded = command.unwrap().image;
        }
    }
    let image = decoded.expect("frame selection rebuilds and uploads the fitted crop");
    assert_eq!((image.width, image.height), (120, 120));
    assert_eq!(image.bytes, [12, 34, 56, 255].repeat(120 * 120));
}

#[test]
fn editing_current_frames_repaints_explicit_and_placeholder_images_without_text() {
    use crate::panes::grid::Grid;
    use crate::panes::kitty_graphics::{KittyApcInterceptor, KittyCommandParser};
    use crate::panes::kitty_test_support::{
        apc_b64, feed_bytes, placeholder_cells, PlaceholderCell, PlaceholderColor,
    };
    use crate::panes::sixel::SixelImageStore;
    use crate::panes::LinkHandler;
    use std::{cell::RefCell, collections::HashMap, rc::Rc};
    use yinz_types::{
        data::{Palette, Style},
        pane_size::SizeInPixels,
    };

    // Spec, r: "an existing frame is edited"; the current raster must reach the host immediately.
    for virtual_placement in [false, true] {
        for current in [1, 2] {
            let parts = create_test_kitty_parts();
            let cell = SizeInPixels {
                width: 10,
                height: 20,
            };
            let mut grid = Grid::new(
                10,
                20,
                Rc::new(RefCell::new(Palette::default())),
                Rc::new(RefCell::new(HashMap::new())),
                Rc::new(RefCell::new(LinkHandler::new())),
                Rc::new(RefCell::new(Some(cell))),
                Rc::new(RefCell::new(SixelImageStore::default())),
                parts.0.clone(),
                Style::default(),
                false,
                true,
                true,
                true,
                false,
            );
            let mut parser = vte::Parser::new();
            let mut interceptor = KittyApcInterceptor::new();
            let mut bytes = apc_b64(
                &format!(
                    "a=T,f=32,s=1,v=1,i=1,c=1,r=1,C=1,q=2,U={}",
                    u8::from(virtual_placement)
                ),
                &[255; 4],
            );
            if virtual_placement {
                bytes.extend_from_slice(
                    placeholder_cells(
                        PlaceholderColor::Indexed(1),
                        None,
                        &[PlaceholderCell::at(0, 0)],
                    )
                    .as_bytes(),
                );
            }
            feed_bytes(&mut grid, &mut parser, &mut interceptor, &bytes);
            let mut send = |grid: &mut Grid, control: &str, pixels: &[u8]| {
                feed_bytes(
                    grid,
                    &mut parser,
                    &mut interceptor,
                    &apc_b64(control, pixels),
                );
            };
            send(&mut grid, "a=f,f=32,s=1,v=1,i=1,q=2", &[255; 4]);
            send(&mut grid, &format!("a=a,i=1,c={current},q=2"), &[]);
            let render = |grid: &mut Grid| {
                extract_kitty_commands(&run_kitty_frame_with_cell_size(
                    &parts,
                    grid.read_changes(0, 0).2,
                    None,
                    cell,
                ))
            };
            let first = render(&mut grid);
            assert!(first.iter().any(|command| action_of(command) == "t"));
            grid.should_render = false;

            send(
                &mut grid,
                &format!("a=f,f=32,s=1,v=1,i=1,r={},q=2", 3 - current),
                &[10, 20, 30, 255],
            );
            assert!(
                render(&mut grid).is_empty(),
                "editing another frame keeps the host raster"
            );
            grid.should_render = false;
            send(&mut grid, "a=f,f=32,s=1,v=1,i=1,r=7,q=2", &[0; 4]);
            assert!(!grid.should_render, "a rejected edit cannot dirty the grid");
            assert!(render(&mut grid).is_empty());

            send(
                &mut grid,
                &format!("a=f,f=32,s=1,v=1,i=1,r={current},q=2"),
                &[12, 34, 56, 255],
            );
            assert!(
                grid.should_render,
                "editing the current frame must schedule a render"
            );
            let commands = render(&mut grid);
            assert_eq!(
                commands
                    .iter()
                    .map(|command| action_of(command))
                    .collect::<Vec<_>>(),
                ["d", "t", "p"]
            );
            let mut decoder = KittyCommandParser::new();
            let mut transmitted = None;
            for command in commands.iter().filter(|command| action_of(command) == "t") {
                if let Some(command) = decoder.parse(command) {
                    transmitted = command.unwrap().image;
                }
            }
            let image = transmitted.expect("the edited raster is transmitted");
            let height = if virtual_placement { 10 } else { 20 };
            assert_eq!((image.width, image.height), (10, height));
            assert_eq!(image.bytes, [12, 34, 56, 255].repeat(10 * height as usize));
            assert!(
                render(&mut grid).is_empty(),
                "the unchanged next render reuses the new raster"
            );
        }
    }
}

#[test]
fn compressed_transmissions_decode_to_exact_original_and_scaled_pixels() {
    use crate::panes::kitty_graphics::KittyCommandParser;
    for scaled in [false, true] {
        let parts = create_test_kitty_parts();
        let internal = store_test_kitty_image(&parts.0, 200, 200);
        let mut chunk = kitty_chunk(internal, 1, 0, 0);
        let expected = if scaled {
            let pixels = vec![42; 30 * 40 * 4];
            assert!(parts.0.borrow_mut().add_scaled_variant(
                internal,
                chunk.variant_key,
                pixels.clone()
            ));
            chunk.scaled_px = Some((30, 40));
            pixels
        } else {
            parts.0.borrow().get(internal).unwrap().rgba.clone()
        };
        let frame = run_kitty_frame(&parts, vec![chunk], None);
        let mut parser = KittyCommandParser::new();
        let mut decoded = None;
        for command in extract_kitty_commands(&frame) {
            if action_of(&command) != "t" {
                continue;
            }
            if value_of(&control_of(&command), "a") == Some("t") {
                assert_eq!(value_of(&control_of(&command), "o"), Some("z"));
            }
            if let Some(result) = parser.parse(&command) {
                decoded = result.unwrap().image;
            }
        }
        let decoded = decoded.expect("complete image");
        assert_eq!(decoded.bytes, expected);
        assert_eq!(
            (decoded.width, decoded.height),
            if scaled { (30, 40) } else { (200, 200) }
        );
    }
}

#[test]
fn crop_replacement_withdraws_obsolete_host_rasters_and_emits_the_new_pixels() {
    use crate::panes::kitty_graphics::{KittyCommandParser, KittyGrid, KittyVerticalAnchor};
    use crate::panes::kitty_test_support::{apc_b64, unwrap_apc};
    use std::{cell::RefCell, rc::Rc};
    use yinz_types::pane_size::SizeInPixels;

    // Spec: "If an image with the same id and placement id already exists, it must be replaced."
    let parts = create_test_kitty_parts();
    let cell = SizeInPixels {
        width: 10,
        height: 20,
    };
    let mut grid = KittyGrid::new(Rc::new(RefCell::new(Some(cell))), parts.0.clone());
    let mut parser = KittyCommandParser::new();
    let pixels: Vec<u8> = (0..8).flat_map(|i| [i, 0, 0, 255]).collect();
    let command = parser
        .parse(unwrap_apc(&apc_b64("a=t,f=32,s=4,v=2,i=1", &pixels)))
        .unwrap()
        .unwrap();
    grid.transmit(&command, command.image.clone().unwrap())
        .unwrap();
    let internal = grid.pane_image_id_map()[&1];
    let mut first_frame = None;
    let mut old_host_id: Option<String> = None;
    for x in [0, 1, 2, 0] {
        let command = parser
            .parse(unwrap_apc(&apc_b64(
                &format!("a=p,i=1,p=1,x={x},w=1,h=2,c=1,r=1"),
                &[],
            )))
            .unwrap()
            .unwrap();
        grid.place(
            1,
            internal,
            &command,
            (0, 0),
            Some(cell),
            KittyVerticalAnchor {
                canonical_line: 0,
                offset_px_from_line_start: 0,
            },
        )
        .unwrap();
        let frame = run_kitty_frame(&parts, grid.viewport_kitty_chunks(10, 0, 20, 0, 0), None);
        let commands = extract_kitty_commands(&frame);
        if let Some(host_id) = &old_host_id {
            assert!(commands.iter().any(|command| {
                let control = control_of(command);
                value_of(&control, "a") == Some("d")
                    && value_of(&control, "d") == Some("I")
                    && value_of(&control, "i") == Some(host_id.as_str())
            }));
        }
        let mut decoder = KittyCommandParser::new();
        let mut decoded = None;
        for command in commands.iter().filter(|command| action_of(command) == "t") {
            old_host_id = value_of(&control_of(command), "i").map(str::to_owned);
            if let Some(command) = decoder.parse(command) {
                decoded = command.unwrap().image;
            }
        }
        let image = decoded.expect("replacement raster is transmitted");
        assert_eq!((image.width, image.height), (10, 20));
        assert_eq!(&image.bytes[..4], &[x, 0, 0, 255]);
        if first_frame.is_none() {
            first_frame = Some(frame);
        }
    }
    let mut decoder = KittyCommandParser::new();
    let mut decoded = None;
    for command in extract_kitty_commands(&first_frame.unwrap()) {
        if action_of(&command) == "t"
            && let Some(command) = decoder.parse(&command)
        {
            decoded = command.unwrap().image;
        }
    }
    assert_eq!(&decoded.unwrap().bytes[..4], &[0, 0, 0, 255]);
}

#[test]
fn changing_one_shared_crop_deletes_only_its_old_host_placement() {
    use crate::panes::sixel::PixelRect;
    // Spec: "If an image with the same id and placement id already exists, it must be replaced."
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 2, 1);
    let mut changed = kitty_chunk(internal, 1, 0, 0);
    changed.scaled_px = Some((10, 20));
    changed.variant_key.source = PixelRect::new(0, 0, 1, 1);
    changed.variant_key.size = (10, 20);
    changed.source_px_width = 10;
    changed.source_px_height = 20;
    changed.dest_cells = (1, 1);
    let sibling = crate::panes::kitty_graphics::KittyImageChunk {
        placement_uid: 2,
        cell_x: 4,
        ..changed
    };
    let first = extract_kitty_commands(&run_kitty_frame(&parts, vec![changed, sibling], None));
    let placements: Vec<_> = first
        .iter()
        .filter(|c| action_of(c) == "p")
        .map(|c| control_of(c))
        .collect();
    assert_eq!(placements.len(), 2);
    assert_eq!(value_of(&placements[0], "i"), value_of(&placements[1], "i"));
    let old_image = value_of(&placements[0], "i").unwrap();
    let placement = value_of(&placements[0], "p").unwrap();
    assert_ne!(Some(placement), value_of(&placements[1], "p"));

    changed.variant_key.source.x = 1;
    let frame = extract_kitty_commands(&run_kitty_frame(&parts, vec![changed, sibling], None));
    let actions: Vec<_> = frame.iter().map(|c| action_of(c)).collect();
    assert_eq!(actions, ["t", "d", "p"]);
    let deletion = control_of(&frame[1]);
    assert_eq!(value_of(&deletion, "d"), Some("i"));
    assert_eq!(value_of(&deletion, "i"), Some(old_image));
    assert_eq!(value_of(&deletion, "p"), Some(placement));
    let replacement = control_of(&frame[2]);
    assert_ne!(value_of(&replacement, "i"), Some(old_image));
    assert_eq!(value_of(&replacement, "p"), Some(placement));

    changed.cell_x = 2;
    let moved = extract_kitty_commands(&run_kitty_frame(&parts, vec![changed, sibling], None));
    assert_eq!(moved.len(), 1);
    let moved = control_of(&moved[0]);
    assert_eq!(value_of(&moved, "a"), Some("p"));
    assert_eq!(value_of(&moved, "i"), value_of(&replacement, "i"));
    assert_eq!(value_of(&moved, "p"), Some(placement));
    let deleted = extract_kitty_commands(&run_kitty_frame(&parts, vec![sibling], None));
    assert_eq!(deleted.len(), 1);
    let deleted = control_of(&deleted[0]);
    assert_eq!(value_of(&deleted, "a"), Some("d"));
    assert_eq!(value_of(&deleted, "d"), Some("i"));
    assert_eq!(value_of(&deleted, "i"), value_of(&replacement, "i"));
    assert_eq!(value_of(&deleted, "p"), Some(placement));
    assert!(extract_kitty_commands(&run_kitty_frame(&parts, vec![sibling], None)).is_empty());
    assert!(parts
        .0
        .borrow()
        .scaled_variant(internal, sibling.variant_key)
        .is_some());
}

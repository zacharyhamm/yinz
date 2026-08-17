//! Conformance of the graphics commands zellij *emits*.
//!
//! Zellij does not pass an application's kitty escape codes through to the terminal it is running
//! inside. It decodes them, stores the raster, and re-encodes fresh commands per client with its
//! own image and placement ids. That makes zellij a graphics *client* here, and the same
//! specification applies to what it sends: the chunking rules, the quiet level, and the cursor
//! policy are all obligations rather than choices.
//!
//! Derived from `graphics-protocol.rst` ("Transferring larger amounts of data", "Suppressing
//! responses", "Controlling displayed image layout"). What the existing tests in `output_tests.rs`
//! already cover — transmit-once diffing, occlusion cropping, host id ranges, capability gating —
//! is not repeated here.

use super::output_tests::{
    create_test_kitty_parts, extract_kitty_commands, kitty_chunk, run_kitty_frame,
    store_test_kitty_image,
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
    // replies to zellij's own stdin, where they would otherwise be indistinguishable from the
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
    // Zellij positions every image itself and then keeps drawing text; if the host moved its own
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
    // than stacking a second copy. Zellij relies on that to move images between frames.
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

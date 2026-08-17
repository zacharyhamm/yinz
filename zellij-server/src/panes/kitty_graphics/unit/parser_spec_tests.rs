//! Control-data grammar, chunking, compression and payload decoding, asserted against the
//! protocol specification rather than against the current implementation.
//!
//! Derived from `graphics-protocol.rst` (the control key reference table, "Transferring larger
//! amounts of data", "Compression", "The transmission medium", and the format rules) and from
//! kitty's `test_load_images`, `test_load_png`, `test_load_png_simple` and
//! `test_suppressing_gr_command_responses`.
//!
//! Every parser-level deviation has been closed; the outstanding ones are all further up the
//! stack, indexed in the module doc of `kitty_graphics/mod.rs`.

use super::super::parser::*;
use super::super::test_support::*;

fn command_bytes(control: &str, payload: &[u8]) -> Vec<u8> {
    unwrap_apc(&apc_b64(control, payload)).to_vec()
}

fn parse(control: &str, payload: &[u8]) -> Result<KittyCommand, KittyError> {
    let mut parser = KittyCommandParser::new();
    parser
        .parse(&command_bytes(control, payload))
        .expect("a complete command produces a result")
}

fn parse_ok(control: &str, payload: &[u8]) -> KittyCommand {
    parse(control, payload).expect("expected the command to be accepted")
}

fn parse_err(control: &str, payload: &[u8]) -> KittyError {
    parse(control, payload).expect_err("expected the command to be rejected")
}

/// A 2x2 RGB raster: the smallest payload that satisfies `f=24,s=2,v=2`.
fn rgb_2x2() -> Vec<u8> {
    rgb_block(2, 2)
}

// ---- G1: control-data grammar ----

#[test]
fn every_key_is_parsed_into_its_field() {
    let command = parse_ok(
        "a=T,f=24,t=d,s=2,v=2,i=7,p=9,x=1,y=2,w=1,h=1,c=3,r=4,X=5,Y=6,z=-3,q=1,C=1",
        &rgb_2x2(),
    );
    assert_eq!(command.action, KittyAction::TransmitAndDisplay);
    assert_eq!(command.format, KittyFormat::Rgb24);
    assert_eq!(command.medium, KittyMedium::Direct);
    assert_eq!(command.pixel_width, Some(2));
    assert_eq!(command.pixel_height, Some(2));
    assert_eq!(command.image_id, Some(7));
    assert_eq!(command.placement_id, Some(9));
    assert_eq!((command.source_x, command.source_y), (1, 2));
    assert_eq!((command.source_w, command.source_h), (1, 1));
    assert_eq!((command.columns, command.rows), (3, 4));
    assert_eq!((command.cell_offset_x, command.cell_offset_y), (5, 6));
    assert_eq!(command.z_index, -3);
    assert_eq!(command.quiet, 1);
    assert!(command.suppress_cursor_movement);
}

#[test]
fn omitted_keys_take_their_documented_defaults() {
    // Spec: a defaults to t, f to 32, t to d, and every geometry key to 0.
    let command = parse_ok("s=1,v=1", &rgba_block(1, 1));
    assert_eq!(command.action, KittyAction::Transmit);
    assert_eq!(command.format, KittyFormat::Rgba32);
    assert_eq!(command.medium, KittyMedium::Direct);
    assert_eq!(command.image_id, None);
    assert_eq!(command.image_number, None);
    assert_eq!(command.placement_id, None);
    assert_eq!(
        (
            command.source_x,
            command.source_y,
            command.source_w,
            command.source_h
        ),
        (0, 0, 0, 0)
    );
    assert_eq!((command.columns, command.rows), (0, 0));
    assert_eq!(command.z_index, 0);
    assert_eq!(command.quiet, 0);
    assert!(!command.compressed);
    assert!(!command.more_chunks);
    assert!(!command.suppress_cursor_movement);
}

#[test]
fn each_documented_action_letter_is_accepted() {
    assert_eq!(
        parse_ok("a=t,s=1,v=1", &rgba_block(1, 1)).action,
        KittyAction::Transmit
    );
    assert_eq!(
        parse_ok("a=T,s=1,v=1", &rgba_block(1, 1)).action,
        KittyAction::TransmitAndDisplay
    );
    assert_eq!(parse_ok("a=p,i=1", &[]).action, KittyAction::Display);
    assert_eq!(parse_ok("a=d", &[]).action, KittyAction::Delete);
    assert_eq!(
        parse_ok("a=q,s=1,v=1", &rgba_block(1, 1)).action,
        KittyAction::Query
    );
}

#[test]
fn an_unrecognised_action_is_rejected() {
    assert_eq!(parse_err("a=Z", &[]).code, KittyErrorCode::Einval);
    // A multi-character value is not a valid action either.
    assert_eq!(parse_err("a=tt", &[]).code, KittyErrorCode::Einval);
}

#[test]
fn the_three_documented_formats_are_accepted_and_others_are_not() {
    assert_eq!(
        parse_ok("f=24,s=1,v=1", &rgb_block(1, 1)).format,
        KittyFormat::Rgb24
    );
    assert_eq!(
        parse_ok("f=32,s=1,v=1", &rgba_block(1, 1)).format,
        KittyFormat::Rgba32
    );
    assert_eq!(
        parse_ok("f=100", &png(1, 1, PngKind::Rgba8)).format,
        KittyFormat::Png
    );
    assert_eq!(
        parse_err("f=8,s=1,v=1", &rgb_block(1, 1)).code,
        KittyErrorCode::Einval
    );
}

#[test]
fn the_z_index_is_a_signed_32_bit_value() {
    // Spec: "z-index ... a 32-bit signed integer", used to place images under text.
    assert_eq!(parse_ok("a=p,i=1,z=2147483647", &[]).z_index, i32::MAX);
    assert_eq!(parse_ok("a=p,i=1,z=-2147483648", &[]).z_index, i32::MIN);
    // The spec singles out this threshold: below it, images draw under even non-default
    // background cells.
    assert_eq!(parse_ok("a=p,i=1,z=-1073741825", &[]).z_index, -1073741825);
}

#[test]
fn out_of_range_numeric_values_are_rejected() {
    assert_eq!(
        parse_err("a=p,i=4294967296", &[]).code,
        KittyErrorCode::Einval
    );
    assert_eq!(
        parse_err("a=p,i=1,z=2147483648", &[]).code,
        KittyErrorCode::Einval
    );
    assert_eq!(parse_err("a=p,i=-1", &[]).code, KittyErrorCode::Einval);
}

#[test]
fn non_numeric_values_are_rejected() {
    for control in ["a=p,i=abc", "s=x,v=1", "a=p,i=1,c=3.5", "a=p,i=1,p= 4"] {
        assert_eq!(
            parse_err(control, &[]).code,
            KittyErrorCode::Einval,
            "expected {:?} to be rejected",
            control
        );
    }
}

#[test]
fn malformed_control_data_is_rejected() {
    // A segment with no `=` is not a key/value pair.
    assert_eq!(parse_err("a=p,i", &[]).code, KittyErrorCode::Einval);
    assert_eq!(parse_err("=5", &[]).code, KittyErrorCode::Einval);
}

#[test]
fn keys_this_terminal_does_not_implement_are_ignored_rather_than_failing() {
    // A judgement call, not a spec rule: the specification says nothing about unrecognised keys,
    // and kitty's own parser is stricter — it rejects an unknown key character with EINVAL. A
    // multiplexer sits between clients and a host terminal that may be newer than it is, so
    // ignoring is the safer default: a client using a later extension still gets its image drawn.
    // Pinned here so a change of mind is a visible decision; see the master index in
    // `kitty_graphics/mod.rs`.
    let command = parse_ok("a=T,f=24,s=2,v=2,i=1,ZZ=5,N=1", &rgb_2x2());
    assert_eq!(command.image_id, Some(1));
    assert!(command.image.is_some());
}

#[test]
fn the_transient_usage_hint_is_accepted_and_changes_nothing_observable() {
    // Spec: N=1 marks an image as transient, a storage hint the terminal may act on or ignore.
    let command = parse_ok("a=T,f=24,s=2,v=2,i=1,N=1", &rgb_2x2());
    assert_eq!(command.image.unwrap().bytes, rgb_2x2());
}

#[test]
fn specifying_both_an_image_id_and_an_image_number_is_rejected() {
    // Spec: "Specifying both `i` and `I` keys in any command is an error. The terminal must reply
    // with an EINVAL error message, unless silenced."
    let error = parse_err("a=T,f=24,s=2,v=2,i=3,I=4", &rgb_2x2());
    assert_eq!(error.code, KittyErrorCode::Einval);
    assert_eq!(error.image_id, Some(3));
    assert_eq!(error.image_number, Some(4));
}

#[test]
fn an_explicit_zero_image_id_does_not_count_as_specifying_one() {
    // A judgement call, not a spec rule; see the master index in `kitty_graphics/mod.rs`. Read
    // strictly, `i=0,I=5` specifies both keys and is an error. But `i=0` is the protocol's own
    // spelling of "no image id" — it is the documented default, and a placement id is documented
    // as ignored "for an image that does not have an id (i.e. has id=0)" — so a client that writes
    // its id field out unconditionally gets the image number honoured rather than a refusal.
    let command = parse_ok("a=T,f=24,s=2,v=2,i=0,I=5", &rgb_2x2());
    assert_eq!(command.image_number, Some(5));
    assert!(command.image.is_some());
}

#[test]
fn an_error_echoes_the_identifiers_from_the_command_that_caused_it() {
    // Without this a client cannot tell which of its in-flight images failed.
    let error = parse_err("a=T,f=24,s=1,v=1,i=42,p=7", &[]);
    assert_eq!(error.image_id, Some(42));
    assert_eq!(error.placement_id, Some(7));
}

#[test]
fn identifiers_are_echoed_even_when_a_later_key_is_the_malformed_one() {
    // The identifiers appear before the bad key; the client still needs them back.
    let error = parse_err("i=9,p=2,f=77", &[]);
    assert_eq!(error.code, KittyErrorCode::Einval);
    assert_eq!(error.image_id, Some(9));
    assert_eq!(error.placement_id, Some(2));
}

#[test]
fn every_documented_delete_specifier_is_accepted() {
    // Spec table: a i n c p q x y z r, each in both cases, with the extra keys that specifier
    // takes. Lowercase deletes placements only; uppercase also frees the image data.
    //
    // The table's eleventh entry, `f`/`F` — delete animation frames — is deliberately missing
    // here: `d=f` is EINVAL today because there are no frames to delete. It belongs to RED-5, and
    // is asserted by `kitty_animation_red::red_anim_deleting_a_frame_leaves_the_image_displayable`.
    for (letter, extra) in [
        ('a', ""),
        ('i', ",i=1,p=2"),
        ('n', ",I=3,p=2"),
        ('c', ""),
        ('p', ",x=4,y=5"),
        ('q', ",x=4,y=5,z=6"),
        ('x', ",x=4"),
        ('y', ",y=5"),
        ('z', ",z=6"),
        ('r', ",x=1,y=9"),
    ] {
        for specifier in [letter, letter.to_ascii_uppercase()] {
            let control = format!("a=d,d={}{}", specifier, extra);
            let command = parse_ok(&control, &[]);
            assert_eq!(command.delete_specifier, Some(specifier));
        }
    }
}

#[test]
fn an_unknown_delete_specifier_is_rejected() {
    assert_eq!(parse_err("a=d,d=k", &[]).code, KittyErrorCode::Einval);
}

// ---- G2: chunked transmission ----

#[test]
fn a_chunked_upload_yields_nothing_until_the_final_chunk() {
    // Spec: "terminals must not display anything, until the entire sequence is received and
    // validated".
    let raster = rgb_block(4, 4);
    let encoded = base64(&raster);
    let (first, rest) = encoded.split_at(16);
    let (second, third) = rest.split_at(16);

    let mut parser = KittyCommandParser::new();
    assert!(parser
        .parse(&chunk("a=T,f=24,s=4,v=4,i=1,m=1", first))
        .is_none());
    assert!(parser.parse(&chunk("m=1", second)).is_none());
    let command = parser
        .parse(&chunk("m=0", third))
        .expect("the final chunk completes the upload")
        .expect("the reassembled payload is valid");
    assert_eq!(command.image.unwrap().bytes, raster);
}

#[test]
fn only_the_first_chunk_needs_to_carry_the_control_data() {
    // Spec: "only the first escape code needs to have the full set of control codes such as width,
    // height, format, etc. Subsequent chunks must have only the `m` and optionally `q` keys".
    let raster = rgb_block(2, 2);
    let encoded = base64(&raster);
    let (first, second) = encoded.split_at(8);

    let mut parser = KittyCommandParser::new();
    parser.parse(&chunk("a=T,f=24,s=2,v=2,i=1,p=5,z=-1,m=1", first));
    let command = parser
        .parse(&chunk("m=0", second))
        .unwrap()
        .expect("valid upload");
    // The control data from the first chunk survives reassembly.
    assert_eq!(command.image_id, Some(1));
    assert_eq!(command.placement_id, Some(5));
    assert_eq!(command.z_index, -1);
    assert_eq!(command.action, KittyAction::TransmitAndDisplay);
}

#[test]
fn a_final_chunk_may_carry_no_payload_at_all() {
    let raster = rgb_block(2, 2);
    let mut parser = KittyCommandParser::new();
    parser.parse(&chunk("a=T,f=24,s=2,v=2,i=1,m=1", &base64(&raster)));
    let command = parser
        .parse(&chunk("m=0", ""))
        .unwrap()
        .expect("valid upload");
    assert_eq!(command.image.unwrap().bytes, raster);
}

#[test]
fn a_final_chunk_may_be_the_bare_terminator_the_spec_itself_emits() {
    // The spec's minimal example closes a chunked upload with `printf "\033_Gm=0;\033\\"`: control
    // data, a semicolon, and nothing after it. `apc_raw` drops the semicolon when there is no
    // payload, so this is the one shape the rest of the suite never produces even though it is the
    // one real clients copy out of the documentation.
    let raster = rgb_block(2, 2);
    let mut parser = KittyCommandParser::new();
    parser.parse(&chunk("a=T,f=24,s=2,v=2,i=1,m=1", &base64(&raster)));

    let command = parser
        .parse(b"m=0;")
        .expect("the final chunk completes the upload")
        .expect("the reassembled payload is valid");
    assert_eq!(command.image.unwrap().bytes, raster);
}

#[test]
fn a_delete_command_arriving_mid_upload_aborts_it() {
    // Spec: "If an image is being loaded in chunks and the upload is not complete when any delete
    // command is received, the partial upload must be aborted."
    let mut parser = KittyCommandParser::new();
    parser.parse(&chunk("a=T,f=24,s=2,v=2,i=1,m=1", "AAAA"));
    assert!(parser.has_pending());

    let delete = parser
        .parse(&command_bytes("a=d,d=a", &[]))
        .unwrap()
        .expect("the delete is honoured");
    assert_eq!(delete.action, KittyAction::Delete);
    assert!(
        !parser.has_pending(),
        "the partial upload must be discarded"
    );
}

#[test]
fn an_upload_can_be_retried_after_being_aborted() {
    let raster = rgb_block(2, 2);
    let mut parser = KittyCommandParser::new();
    parser.parse(&chunk("a=T,f=24,s=2,v=2,i=1,m=1", "AAAA"));
    parser.parse(&command_bytes("a=d,d=a", &[]));

    let command = parser
        .parse(&command_bytes("a=T,f=24,s=2,v=2,i=1", &raster))
        .unwrap()
        .expect("the retry succeeds");
    assert_eq!(command.image.unwrap().bytes, raster);
}

#[test]
fn the_quiet_level_may_be_raised_on_a_later_chunk() {
    // Spec: subsequent chunks "must have only the `m` and optionally `q` keys", so a client can go
    // quiet partway through a long upload.
    let raster = rgb_block(2, 2);
    let encoded = base64(&raster);
    let (first, second) = encoded.split_at(8);

    let mut parser = KittyCommandParser::new();
    parser.parse(&chunk("a=T,f=24,s=2,v=2,i=1,m=1", first));
    let command = parser
        .parse(&chunk("m=0,q=2", second))
        .unwrap()
        .expect("valid upload");
    assert_eq!(command.quiet, 2);
}

#[test]
fn a_chunked_upload_carries_its_image_number_through_to_completion() {
    let raster = rgb_block(2, 2);
    let encoded = base64(&raster);
    let (first, second) = encoded.split_at(8);

    let mut parser = KittyCommandParser::new();
    parser.parse(&chunk("a=T,f=24,s=2,v=2,I=13,m=1", first));
    let command = parser
        .parse(&chunk("m=0", second))
        .unwrap()
        .expect("valid upload");
    assert_eq!(command.image_number, Some(13));
    assert_eq!(command.image_id, None);
}

#[test]
fn a_chunked_upload_of_a_query_is_reassembled_like_any_other() {
    let raster = rgb_block(2, 2);
    let encoded = base64(&raster);
    let (first, second) = encoded.split_at(8);

    let mut parser = KittyCommandParser::new();
    parser.parse(&chunk("a=q,f=24,s=2,v=2,i=1,m=1", first));
    let command = parser
        .parse(&chunk("m=0", second))
        .unwrap()
        .expect("valid upload");
    assert_eq!(command.action, KittyAction::Query);
    assert_eq!(command.image.unwrap().bytes, raster);
}

#[test]
fn a_chunked_upload_whose_payload_is_short_fails_only_at_the_end() {
    // The data is only validated once reassembled, so the error names the whole transfer.
    let mut parser = KittyCommandParser::new();
    parser.parse(&chunk("a=T,f=24,s=8,v=8,i=1,m=1", "AAAA"));
    let error = parser
        .parse(&chunk("m=0", ""))
        .unwrap()
        .expect_err("the reassembled payload is too short");
    assert_eq!(error.code, KittyErrorCode::Enodata);
    assert_eq!(error.image_id, Some(1));
}

#[test]
fn a_malformed_continuation_chunk_fails_the_whole_upload() {
    let mut parser = KittyCommandParser::new();
    parser.parse(&chunk("a=T,f=24,s=2,v=2,i=1,m=1", "AAAA"));
    let error = parser
        .parse(&chunk("m=7", "AAAA"))
        .unwrap()
        .expect_err("m=7 is not a valid continuation");
    assert_eq!(error.code, KittyErrorCode::Einval);
    assert_eq!(error.image_id, Some(1));
    assert!(!parser.has_pending());
}

// ---- G3: compression ----

#[test]
fn a_zlib_compressed_payload_is_inflated_before_use() {
    // Spec: "only RFC 1950 ZLIB based deflate compression is supported, which is specified using
    // `o=z`", and the compression "occurs prior to base64 encoding".
    let raster = rgb_block(8, 8);
    let command = parse_ok("a=T,f=24,s=8,v=8,i=1,o=z", &deflate(&raster));
    assert_eq!(command.image.unwrap().bytes, raster);
}

#[test]
fn compression_and_chunking_compose() {
    let raster = rgb_block(8, 8);
    let encoded = base64(&deflate(&raster));
    let (first, second) = encoded.split_at(encoded.len() / 2 / 4 * 4);

    let mut parser = KittyCommandParser::new();
    parser.parse(&chunk("a=T,f=24,s=8,v=8,i=1,o=z,m=1", first));
    let command = parser
        .parse(&chunk("m=0", second))
        .unwrap()
        .expect("valid upload");
    assert_eq!(command.image.unwrap().bytes, raster);
}

#[test]
fn a_compressed_png_is_inflated_then_decoded() {
    let image = png(4, 4, PngKind::Rgba8);
    let command = parse_ok("a=T,f=100,i=1,o=z", &deflate(&image));
    let decoded = command.image.unwrap();
    assert_eq!((decoded.width, decoded.height), (4, 4));
}

#[test]
fn an_unknown_compression_scheme_is_rejected() {
    assert_eq!(
        parse_err("a=T,f=24,s=2,v=2,o=q", &rgb_2x2()).code,
        KittyErrorCode::Einval
    );
}

#[test]
fn a_payload_that_does_not_inflate_is_rejected() {
    assert_eq!(
        parse_err("a=T,f=24,s=2,v=2,i=1,o=z", b"not actually deflate data").code,
        KittyErrorCode::Einval
    );
}

// ---- G4: transmission media ----

#[test]
fn a_regular_file_transfer_reads_the_file_and_leaves_it_alone() {
    // Spec, "the transmission medium": deletion is described only for `t=t`, "a temporary file,
    // the terminal emulator will delete the file after reading the pixel data". `t=f` is "a
    // simple file", so it is the client's, not the terminal's, to remove.
    let raster = rgb_block(2, 2);
    let file = regular_file(&raster);
    let command = parse_ok("a=T,f=24,s=2,v=2,i=1,t=f", &file.payload());
    assert_eq!(command.image.unwrap().bytes, raster);
    assert!(file.exists(), "a t=f transfer must never delete the file");
}

#[test]
fn a_size_and_offset_select_a_window_of_the_file() {
    // Spec: S is "the size of data to read from the file" and O "the offset from which to read".
    let raster = rgb_block(2, 2);
    let mut padded = vec![0xffu8; 7];
    padded.extend_from_slice(&raster);
    padded.extend_from_slice(&[0xee; 5]);
    let file = regular_file(&padded);

    let command = parse_ok(
        &format!("a=T,f=24,s=2,v=2,i=1,t=f,O=7,S={}", raster.len()),
        &file.payload(),
    );
    assert_eq!(command.image.unwrap().bytes, raster);
}

#[test]
fn an_offset_past_the_end_of_the_file_is_an_io_error() {
    let file = regular_file(&rgb_block(2, 2));
    let error = parse_err("a=T,f=24,s=2,v=2,i=1,t=f,O=9999", &file.payload());
    assert_eq!(error.code, KittyErrorCode::Ebadf);
}

#[test]
fn a_missing_file_is_an_io_error() {
    let error = parse_err(
        "a=T,f=24,s=2,v=2,i=1,t=f",
        b"/nonexistent/zellij/kitty/image.bin",
    );
    assert_eq!(error.code, KittyErrorCode::Ebadf);
}

#[test]
fn a_directory_is_not_a_readable_image_source() {
    // Spec: "terminal emulators must refuse to read any device/socket/etc. special files. Only
    // regular files are allowed."
    let error = parse_err("a=T,f=24,s=2,v=2,i=1,t=f", b"/tmp");
    assert_eq!(error.code, KittyErrorCode::Ebadf);
}

#[test]
fn a_symlink_to_an_image_file_is_followed() {
    // Spec: "the terminal emulator must follow symlinks".
    let raster = rgb_block(2, 2);
    let file = regular_file(&raster);
    let link = file.path.with_extension("link");
    std::os::unix::fs::symlink(&file.path, &link).expect("create symlink");

    let command = parse_ok(
        "a=T,f=24,s=2,v=2,i=1,t=f",
        link.to_string_lossy().as_bytes(),
    );
    assert_eq!(command.image.unwrap().bytes, raster);
    let _ = std::fs::remove_file(&link);
}

#[test]
fn a_temp_file_carrying_the_marker_inside_a_temp_directory_is_deleted() {
    // Spec: for t=t the terminal deletes the file, but only when it is in a known temporary
    // directory *and* the path contains `tty-graphics-protocol`.
    let raster = rgb_block(2, 2);
    let file = marked_temp_file(&raster);
    let command = parse_ok("a=T,f=24,s=2,v=2,i=1,t=t", &file.payload());
    assert_eq!(command.image.unwrap().bytes, raster);
    assert!(
        !file.exists(),
        "both conditions held, so the file is deleted"
    );
}

#[test]
fn a_temp_file_without_the_marker_survives_even_inside_a_temp_directory() {
    // The other half of the same rule: the marker is what distinguishes a file the client created
    // for this transfer from any other file the client happened to name, so deleting without it
    // destroys unrelated user data.
    let raster = rgb_block(2, 2);
    let file = unmarked_temp_file(&raster);

    let command = parse_ok("a=T,f=24,s=2,v=2,i=1,t=t", &file.payload());
    assert_eq!(command.image.unwrap().bytes, raster);
    assert!(
        file.exists(),
        "a temp-directory file without the tty-graphics-protocol marker must not be deleted"
    );
}

#[test]
fn a_temp_file_with_the_marker_outside_a_temp_directory_survives() {
    // And the mirror image: the marker alone does not license deletion, because a client can put
    // that string in any path it likes.
    let raster = rgb_block(2, 2);
    let file = marked_non_temp_file(&raster);

    let command = parse_ok("a=T,f=24,s=2,v=2,i=1,t=t", &file.payload());
    assert_eq!(command.image.unwrap().bytes, raster);
    assert!(
        file.exists(),
        "a file outside any temporary directory must not be deleted"
    );
}

#[test]
fn an_unknown_transmission_medium_is_rejected() {
    assert_eq!(
        parse_err("a=T,f=24,s=2,v=2,t=q", &rgb_2x2()).code,
        KittyErrorCode::Einval
    );
}

// ---- G4b: shared memory ----
//
// Linux only: `/dev/shm` *is* the POSIX shm namespace there, so an object can be created and
// inspected with ordinary file operations. The implementation is portable POSIX; a macOS fixture
// needs `shm_open` through `libc`.

#[cfg(target_os = "linux")]
mod shared_memory {
    use super::*;

    /// A POSIX shared memory object, named the way a client would name one.
    struct ShmObject {
        name: String,
    }

    impl ShmObject {
        fn new(bytes: &[u8]) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let name = format!(
                "/zellij-kitty-shm-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            std::fs::write(format!("/dev/shm{}", name), bytes).expect("create shm object");
            ShmObject { name }
        }
        fn payload(&self) -> Vec<u8> {
            self.name.as_bytes().to_vec()
        }
        fn exists(&self) -> bool {
            std::path::Path::new(&format!("/dev/shm{}", self.name)).exists()
        }
    }

    impl Drop for ShmObject {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(format!("/dev/shm{}", self.name));
        }
    }

    #[test]
    fn a_shared_memory_transfer_reads_the_object_contents() {
        // Spec, "the transmission medium": `t=s` names "a *shared memory object*, which on POSIX
        // systems is a POSIX shared memory object". It exists so a client can hand over a large
        // image without pushing it through the tty.
        let raster = rgb_block(4, 4);
        let object = ShmObject::new(&raster);

        let command = parse_ok("a=T,f=24,s=4,v=4,i=1,t=s", &object.payload());
        assert_eq!(command.image.unwrap().bytes, raster);
    }

    #[test]
    fn the_object_is_unlinked_after_being_read() {
        // Spec: "The terminal emulator must read the data from the memory object and then unlink
        // and close it on POSIX" — otherwise every image a client sends leaks memory until reboot.
        let object = ShmObject::new(&rgb_block(4, 4));
        let _ = parse("a=T,f=24,s=4,v=4,i=1,t=s", &object.payload());
        assert!(
            !object.exists(),
            "the object must be unlinked after the read"
        );
    }

    #[test]
    fn a_shared_memory_transfer_honours_the_size_and_offset_keys() {
        let raster = rgb_block(2, 2);
        let mut padded = vec![0xffu8; 5];
        padded.extend_from_slice(&raster);
        let object = ShmObject::new(&padded);

        let command = parse_ok(
            &format!("a=T,f=24,s=2,v=2,i=1,t=s,O=5,S={}", raster.len()),
            &object.payload(),
        );
        assert_eq!(command.image.unwrap().bytes, raster);
    }

    #[test]
    fn a_missing_object_is_an_io_error() {
        // A client racing against its own cleanup should get EBADF, not a blanket "unsupported"
        // that tells it nothing about what went wrong.
        let error = parse_err(
            "a=T,f=24,s=2,v=2,i=1,t=s",
            b"/zellij-kitty-shm-does-not-exist",
        );
        assert_eq!(error.code, KittyErrorCode::Ebadf);
    }
}

// ---- G5: PNG ----

#[test]
fn every_png_colour_type_is_normalised_to_rgba() {
    // Spec: "The terminal emulator must understand pixel data in three formats, 24-bit RGB,
    // 32-bit RGBA and PNG", and "any PNG image can be transmitted directly". The protocol has no
    // way to report a colour type back, so all of them must be handled.
    for kind in [
        PngKind::Rgba8,
        PngKind::Rgb8,
        PngKind::Gray8,
        PngKind::GrayAlpha8,
        PngKind::Palette8,
    ] {
        let command = parse_ok("a=T,f=100,i=1", &png(3, 2, kind));
        let image = command
            .image
            .unwrap_or_else(|| panic!("{:?} decoded", kind));
        assert_eq!((image.width, image.height), (3, 2), "{:?}", kind);
        assert_eq!(
            image.bytes.len(),
            3 * 2 * 4,
            "{:?} must normalise to 4 bytes per pixel",
            kind
        );
    }
}

#[test]
fn a_png_carries_its_own_dimensions_and_ignores_s_and_v() {
    // Spec, "PNG data": "the width and height of the image will be read from the PNG data
    // itself", so an `s`/`v` that disagrees with the header cannot be what wins.
    let command = parse_ok("a=T,f=100,i=1,s=99,v=99", &png(4, 3, PngKind::Rgb8));
    let image = command.image.unwrap();
    assert_eq!((image.width, image.height), (4, 3));
}

#[test]
fn a_one_by_one_transparent_png_decodes_to_a_single_pixel() {
    // The degenerate case kitty regression-tests: a client probing support with the smallest
    // possible image.
    let command = parse_ok("a=T,f=100,i=1", &png(1, 1, PngKind::Rgba8));
    let image = command.image.unwrap();
    assert_eq!((image.width, image.height), (1, 1));
    assert_eq!(image.bytes.len(), 4);
}

#[test]
fn malformed_png_payloads_are_all_refused_as_bad_pngs() {
    // Every way a PNG can be broken has to come back as an error the client can see, and none of
    // them may read past the end of the payload. The last case is a kitty regression
    // (`test_load_png_simple`): a payload shorter than the decoder's initial buffer used to be an
    // out-of-bounds read.
    //
    // The protocol reserves no code for this, but kitty has one and clients that already speak to
    // kitty will recognise it. It is worth distinguishing: EINVAL says "your control data was
    // malformed", which a client answers by fixing its command, where EBADPNG says "your control
    // data was fine and your image was not", which it answers by re-encoding the image.
    let mut corrupt_pixels = png(4, 4, PngKind::Rgba8);
    let len = corrupt_pixels.len();
    // Scribble over the compressed image data, leaving the header intact.
    for byte in corrupt_pixels[len / 2..len - 12].iter_mut() {
        *byte = 0;
    }
    let truncated = {
        let image = png(4, 4, PngKind::Rgba8);
        image[..image.len() / 2].to_vec()
    };

    for (description, payload) in [
        ("not a png at all", b"definitely not a png".to_vec()),
        ("truncated", truncated),
        ("a corrupt pixel stream", corrupt_pixels),
        (
            "a chunk shorter than the decoder's buffer",
            vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a],
        ),
    ] {
        let error = parse_err("a=T,f=100,i=1", &payload);
        assert_eq!(
            error.image_id,
            Some(1),
            "{} must be reported against the image it names",
            description
        );
        assert_eq!(
            error.code,
            KittyErrorCode::Ebadpng,
            "{} is a bad png, not a malformed command",
            description
        );
    }
}

// ---- G6: raw image dimensions ----

#[test]
fn rgb_data_is_three_bytes_per_pixel_and_rgba_is_four() {
    let rgb = parse_ok("a=T,f=24,s=3,v=2,i=1", &rgb_block(3, 2));
    assert_eq!(rgb.image.unwrap().bytes.len(), 3 * 2 * 3);
    let rgba = parse_ok("a=T,f=32,s=3,v=2,i=1", &rgba_block(3, 2));
    assert_eq!(rgba.image.unwrap().bytes.len(), 3 * 2 * 4);
}

#[test]
fn raw_data_without_dimensions_is_rejected() {
    // Spec: s and v are required for f=24 and f=32; without them the payload cannot be laid out.
    assert_eq!(
        parse_err("a=T,f=24,i=1", &rgb_2x2()).code,
        KittyErrorCode::Einval
    );
    assert_eq!(
        parse_err("a=T,f=24,s=2,i=1", &rgb_2x2()).code,
        KittyErrorCode::Einval
    );
    assert_eq!(
        parse_err("a=T,f=24,v=2,i=1", &rgb_2x2()).code,
        KittyErrorCode::Einval
    );
    assert_eq!(
        parse_err("a=T,f=24,s=0,v=2,i=1", &rgb_2x2()).code,
        KittyErrorCode::Einval
    );
}

#[test]
fn too_little_data_for_the_declared_dimensions_is_reported_as_missing_data() {
    // kitty answers ENODATA here, distinguishing "you sent a broken image" from "your command
    // was malformed".
    let error = parse_err("a=T,f=24,s=10,v=10,i=1", &rgb_block(2, 2));
    assert_eq!(error.code, KittyErrorCode::Enodata);
}

#[test]
fn excess_data_beyond_the_declared_dimensions_is_discarded() {
    let mut payload = rgb_block(2, 2);
    payload.extend_from_slice(&[0xff; 32]);
    let command = parse_ok("a=T,f=24,s=2,v=2,i=1", &payload);
    assert_eq!(command.image.unwrap().bytes, rgb_block(2, 2));
}

#[test]
fn an_implausibly_large_image_is_refused_before_allocating_for_it() {
    let error = parse_err("a=T,f=32,s=4294967295,v=4294967295,i=1", &[]);
    assert_eq!(error.code, KittyErrorCode::Einval);
}

#[test]
fn a_payload_that_is_not_valid_base64_is_rejected() {
    let mut parser = KittyCommandParser::new();
    let raw = unwrap_apc(&apc_raw("a=T,f=24,s=2,v=2,i=1", b"!!!not base64!!!")).to_vec();
    let error = parser
        .parse(&raw)
        .unwrap()
        .expect_err("invalid base64 is rejected");
    assert_eq!(error.code, KittyErrorCode::Einval);
}

// ---- local helpers ----

fn base64(data: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    STANDARD.encode(data)
}

fn deflate(data: &[u8]) -> Vec<u8> {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// One chunk of a multi-chunk upload: control data plus an already-base64 payload slice.
fn chunk(control: &str, payload_b64: &str) -> Vec<u8> {
    unwrap_apc(&apc_raw(control, payload_b64.as_bytes())).to_vec()
}

use super::*;
use base64::engine::general_purpose::STANDARD as BASE64_ENCODER;
use base64::Engine as _;
use std::io::{Seek, Write};

const PNG_2X2_RGBA: [u8; 75] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72, 0xb6, 0x0d,
    0x24, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x0c, 0x81, 0x34, 0x18, 0x00, 0x00, 0x49, 0xc8, 0x09, 0xf7, 0x03, 0xd9, 0x64, 0xf1, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];
const PNG_2X2_EXPECTED_RGBA: [u8; 16] = [
    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
];

fn b64(data: &[u8]) -> String {
    BASE64_ENCODER.encode(data)
}

fn raw_cmd(control: &str, payload: &[u8]) -> Vec<u8> {
    format!("{};{}", control, b64(payload)).into_bytes()
}

fn parse_one(parser: &mut KittyCommandParser, raw: &[u8]) -> Result<KittyCommand, KittyError> {
    parser
        .parse(raw)
        .expect("expected a completed parse result")
}

fn raster_12_bytes() -> Vec<u8> {
    (1..=12).collect()
}

#[test]
fn yazi_first_chunk_control_string_parses_to_expected_fields() {
    let command = parse_control_data(b"q=2,a=T,z=-1,C=1,f=24,s=2,v=2,m=1").unwrap();
    assert_eq!(command.quiet, 2);
    assert_eq!(command.action, KittyAction::TransmitAndDisplay);
    assert_eq!(command.z_index, -1);
    assert!(command.suppress_cursor_movement);
    assert_eq!(command.format, KittyFormat::Rgb24);
    assert_eq!(command.pixel_width, Some(2));
    assert_eq!(command.pixel_height, Some(2));
    assert!(command.more_chunks);
    assert_eq!(command.medium, KittyMedium::Direct);
    assert_eq!(command.image_id, None);
    assert_eq!(command.image_number, None);
    assert_eq!(command.placement_id, None);
    assert!(!command.compressed);
    assert_eq!(command.delete_specifier, None);
    assert_eq!(command.image, None);
    assert_eq!(command.source_x, 0);
    assert_eq!(command.source_y, 0);
    assert_eq!(command.source_w, 0);
    assert_eq!(command.source_h, 0);
    assert_eq!(command.columns, 0);
    assert_eq!(command.rows, 0);
    assert_eq!(command.cell_offset_x, 0);
    assert_eq!(command.cell_offset_y, 0);
}

#[test]
fn three_chunk_upload_concatenates_payloads_and_completes_on_m0() {
    let raster = raster_12_bytes();
    let full_b64 = b64(&raster);
    assert_eq!(full_b64.len(), 16);
    let chunk_one = &full_b64[..8];
    let chunk_two = &full_b64[8..12];
    let chunk_three = &full_b64[12..];
    let mut parser = KittyCommandParser::new();
    assert!(parser
        .parse(format!("a=T,f=24,s=2,v=2,i=7,m=1;{}", chunk_one).as_bytes())
        .is_none());
    assert!(parser
        .parse(format!("m=1;{}", chunk_two).as_bytes())
        .is_none());
    let command = parse_one(&mut parser, format!("m=0;{}", chunk_three).as_bytes()).unwrap();
    let image = command.image.unwrap();
    assert_eq!(image.bytes, raster);
    assert_eq!(image.width, 2);
    assert_eq!(image.height, 2);
    assert!(!command.more_chunks);
    assert_eq!(command.image_id, Some(7));
    assert!(!parser.has_pending());
}

#[test]
fn quality_pass_chunked_upload_is_bounded_and_recovers_after_overflow() {
    let encoded_limit = MAX_DECODED_BYTES.div_ceil(3) * 4;
    let chunk = format!("m=1;{}", "A".repeat(4096)).into_bytes();
    for final_chunk in [b"m=1;AAAA".as_slice(), b"m=0;AAAA".as_slice()] {
        let mut parser = KittyCommandParser::new();
        assert!(parser.parse(b"a=t,f=24,s=1,v=1,i=17,m=1;").is_none());
        // Reserve the boundary once so the regression does not double a large Vec allocation.
        parser
            .pending
            .as_mut()
            .unwrap()
            .payload_b64
            .reserve_exact(encoded_limit + 4);
        for _ in 0..encoded_limit / 4096 {
            assert!(parser.parse(&chunk).is_none());
        }
        let remainder = encoded_limit % 4096;
        assert!(parser.parse(&chunk[..4 + remainder]).is_none());

        let error = parse_one(&mut parser, final_chunk).unwrap_err();
        assert_eq!(error.code, KittyErrorCode::Einval);
        assert_eq!(error.image_id, Some(17));
        assert!(!parser.has_pending());
        let next = parse_one(&mut parser, b"a=t,f=24,s=1,v=1,i=18;AAAA").unwrap();
        assert_eq!(next.image_id, Some(18));
        assert_eq!(next.image.unwrap().bytes, vec![0; 3]);
    }
}

#[test]
fn delete_between_chunks_aborts_upload_and_returns_the_delete() {
    let raster = raster_12_bytes();
    let full_b64 = b64(&raster);
    let chunk_one = &full_b64[..8];
    let chunk_rest = &full_b64[8..];
    let mut parser = KittyCommandParser::new();
    assert!(parser
        .parse(format!("a=T,f=24,s=2,v=2,m=1;{}", chunk_one).as_bytes())
        .is_none());
    assert!(parser.has_pending());
    let delete = parse_one(&mut parser, b"a=d,d=A").unwrap();
    assert_eq!(delete.action, KittyAction::Delete);
    assert_eq!(delete.delete_specifier, Some('A'));
    assert!(!parser.has_pending());
    let err = parse_one(&mut parser, format!("m=0;{}", chunk_rest).as_bytes()).unwrap_err();
    assert_eq!(err.code, KittyErrorCode::Einval);
}

#[test]
fn zlib_compressed_raster_inflates_to_original_bytes() {
    let raster = raster_12_bytes();
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&raster).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut parser = KittyCommandParser::new();
    let command = parse_one(&mut parser, &raw_cmd("a=T,f=24,s=2,v=2,o=z", &compressed)).unwrap();
    let image = command.image.unwrap();
    assert_eq!(image.bytes, raster);
    assert_eq!(image.format, KittyFormat::Rgb24);
}

#[test]
fn embedded_png_fixture_decodes_to_expected_rgba() {
    let mut parser = KittyCommandParser::new();
    let command = parse_one(&mut parser, &raw_cmd("a=T,f=100", &PNG_2X2_RGBA)).unwrap();
    assert_eq!(
        command.image,
        Some(DecodedImage {
            bytes: PNG_2X2_EXPECTED_RGBA.to_vec(),
            width: 2,
            height: 2,
            format: KittyFormat::Rgba32,
        })
    );
}

#[test]
fn non_png_payload_with_png_format_is_ebadpng() {
    let mut parser = KittyCommandParser::new();
    let err = parse_one(&mut parser, &raw_cmd("a=T,f=100", b"this is not a png")).unwrap_err();
    assert_eq!(err.code, KittyErrorCode::Ebadpng);
}

#[test]
fn raw_payload_with_insufficient_bytes_is_enodata() {
    let mut parser = KittyCommandParser::new();
    let payload: Vec<u8> = (1..=11).collect();
    let err = parse_one(&mut parser, &raw_cmd("a=T,f=24,s=2,v=2", &payload)).unwrap_err();
    assert_eq!(err.code, KittyErrorCode::Enodata);
}

#[test]
fn raw_payload_with_excess_bytes_is_truncated() {
    let mut parser = KittyCommandParser::new();
    let payload: Vec<u8> = (1..=20).collect();
    let command = parse_one(&mut parser, &raw_cmd("a=T,f=24,s=2,v=2", &payload)).unwrap();
    let image = command.image.unwrap();
    assert_eq!(image.bytes.len(), 12);
    assert_eq!(image.bytes, (1..=12).collect::<Vec<u8>>());
}

#[test]
fn raw_format_without_dimensions_is_einval() {
    let mut parser = KittyCommandParser::new();
    let err = parse_one(&mut parser, &raw_cmd("a=T,f=24", &raster_12_bytes())).unwrap_err();
    assert_eq!(err.code, KittyErrorCode::Einval);
}

#[test]
fn icat_direct_probe_with_data_size_parses_ok() {
    let mut parser = KittyCommandParser::new();
    let command = parse_one(
        &mut parser,
        &raw_cmd("t=d,a=q,i=1,s=1,v=1,f=24,S=3", b"123"),
    )
    .unwrap();
    assert_eq!(command.action, KittyAction::Query);
    assert_eq!(command.image_id, Some(1));
    assert_eq!(command.data_size, Some(3));
    assert_eq!(command.image.unwrap().bytes, b"123".to_vec());
}

#[test]
fn data_offset_and_size_window_file_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("windowed.bin");
    std::fs::write(&path, (1..=10).collect::<Vec<u8>>()).unwrap();
    let mut parser = KittyCommandParser::new();
    let command = parse_one(
        &mut parser,
        &raw_cmd(
            "a=T,t=f,f=24,s=1,v=1,O=4,S=3",
            path.to_str().unwrap().as_bytes(),
        ),
    )
    .unwrap();
    assert_eq!(command.data_offset, Some(4));
    assert_eq!(command.data_size, Some(3));
    assert_eq!(command.image.unwrap().bytes, vec![5, 6, 7]);
}

#[test]
fn unknown_key_is_silently_ignored() {
    let mut parser = KittyCommandParser::new();
    let command = parse_one(&mut parser, b"a=T,i=5,q=1,Z=9,f=24,s=1,v=1;AAAA").unwrap();
    assert_eq!(command.image_id, Some(5));
    assert_eq!(command.quiet, 1);
    assert_eq!(command.image.unwrap().width, 1);
}

#[test]
fn known_but_unsupported_keys_do_not_break_valid_command() {
    let mut parser = KittyCommandParser::new();
    let command = parse_one(
        &mut parser,
        b"a=T,i=5,N=1,P=2,Q=3,H=4,V=5,f=24,s=1,v=1;AAAA",
    )
    .unwrap();
    assert_eq!(command.image_id, Some(5));
    assert!(command.image.is_some());
}

#[test]
fn the_unicode_placeholder_key_marks_the_placement_virtual() {
    let mut parser = KittyCommandParser::new();
    let command = parse_one(&mut parser, b"a=T,U=1,f=24,s=1,v=1;AAAA").unwrap();
    assert!(command.virtual_placement);
    let command = parse_one(&mut parser, b"a=T,U=0,f=24,s=1,v=1;AAAA").unwrap();
    assert!(!command.virtual_placement);
    let command = parse_one(&mut parser, b"a=T,f=24,s=1,v=1;AAAA").unwrap();
    assert!(!command.virtual_placement);
}

#[test]
fn animation_actions_parse() {
    for (control, action) in [
        (b"a=f".as_slice(), KittyAction::Frame),
        (b"a=a".as_slice(), KittyAction::Animate),
        (b"a=c".as_slice(), KittyAction::Compose),
    ] {
        assert_eq!(parse_control_data(control).unwrap().action, action);
    }
}

#[test]
fn oversized_dimensions_are_einval() {
    let mut parser = KittyCommandParser::new();
    let err = parse_one(&mut parser, b"a=T,f=24,s=10001,v=1;AAAA").unwrap_err();
    assert_eq!(err.code, KittyErrorCode::Einval);
    let err = parse_one(&mut parser, b"a=T,f=32,s=10000,v=10000;AAAA").unwrap_err();
    assert_eq!(err.code, KittyErrorCode::Einval);
}

#[test]
fn file_medium_reads_file_and_leaves_it_in_place() {
    let raster = raster_12_bytes();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("img.bin");
    std::fs::write(&path, &raster).unwrap();
    let mut parser = KittyCommandParser::new();
    let command = parse_one(
        &mut parser,
        &raw_cmd("a=T,f=24,s=2,v=2,t=f", path.to_str().unwrap().as_bytes()),
    )
    .unwrap();
    assert_eq!(command.image.unwrap().bytes, raster);
    assert!(path.exists());
}

#[test]
fn file_medium_nonexistent_path_is_ebadf() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("does-not-exist.bin");
    let mut parser = KittyCommandParser::new();
    let err = parse_one(
        &mut parser,
        &raw_cmd("a=T,f=24,s=2,v=2,t=f", path.to_str().unwrap().as_bytes()),
    )
    .unwrap_err();
    assert_eq!(err.code, KittyErrorCode::Ebadf);
}

// A temp-directory file with no `tty-graphics-protocol` marker used to be asserted here as
// "deleted". The spec requires it to survive — see
// `parser_spec_tests::a_temp_file_without_the_marker_survives_even_inside_a_temp_directory`.
// The conforming case (marker *and* temp directory) is covered by
// `parser_spec_tests::a_temp_file_carrying_the_marker_inside_a_temp_directory_is_deleted`.

#[test]
fn temp_file_medium_outside_temp_dir_is_not_deleted() {
    let raster = raster_12_bytes();
    let dir = tempfile::Builder::new()
        .prefix("kitty-parser-test")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap();
    let path = dir.path().join("img.bin");
    std::fs::write(&path, &raster).unwrap();
    let mut parser = KittyCommandParser::new();
    let command = parse_one(
        &mut parser,
        &raw_cmd("a=T,f=24,s=2,v=2,t=t", path.to_str().unwrap().as_bytes()),
    )
    .unwrap();
    assert_eq!(command.image.unwrap().bytes, raster);
    assert!(path.exists());
}

#[test]
fn file_medium_larger_than_decode_cap_is_einval() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.bin");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_DECODED_BYTES as u64 + 1).unwrap();
    drop(file);
    let mut parser = KittyCommandParser::new();
    let err = parse_one(
        &mut parser,
        &raw_cmd("a=T,f=24,s=2,v=2,t=f", path.to_str().unwrap().as_bytes()),
    )
    .unwrap_err();
    assert_eq!(err.code, KittyErrorCode::Einval);
    assert!(path.exists());
}

#[test]
fn file_medium_window_within_oversized_file_reads_only_requested_range() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge_windowed.bin");
    let mut file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_DECODED_BYTES as u64 + 1024).unwrap();
    file.seek(std::io::SeekFrom::Start(64)).unwrap();
    file.write_all(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12])
        .unwrap();
    drop(file);
    let mut parser = KittyCommandParser::new();
    let command = parse_one(
        &mut parser,
        &raw_cmd(
            "a=T,t=f,f=24,s=2,v=2,O=64,S=12",
            path.to_str().unwrap().as_bytes(),
        ),
    )
    .unwrap();
    assert_eq!(command.image.unwrap().bytes, raster_12_bytes());
}

#[test]
fn file_medium_offset_beyond_eof_is_ebadf() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("small.bin");
    std::fs::write(&path, raster_12_bytes()).unwrap();
    let mut parser = KittyCommandParser::new();
    let err = parse_one(
        &mut parser,
        &raw_cmd(
            "a=T,t=f,f=24,s=2,v=2,O=64,S=12",
            path.to_str().unwrap().as_bytes(),
        ),
    )
    .unwrap_err();
    assert_eq!(err.code, KittyErrorCode::Ebadf);
}

// A marked file outside any temp directory used to be asserted here as "deleted". The spec
// requires it to survive — see
// `parser_spec_tests::a_temp_file_with_the_marker_outside_a_temp_directory_survives`.

// ---- property-based tests (hegel) ----

use hegel::generators;

/// A plausible-looking kitty control string, so the generated chunks actually reach the parser's
/// interesting paths instead of bouncing off `parse_control_data`.
///
/// Deliberately never emits `f=100` (PNG) or `o=z` (zlib): those hand the payload to a real
/// decoder, which is orders of magnitude slower than the parsing this property is about. Random
/// binary chunks below could in principle spell one out, but not in any realistic number of runs.
#[hegel::composite]
fn any_control_chunk(tc: &hegel::TestCase) -> Vec<u8> {
    let pair_count = tc.draw(generators::integers::<usize>().max_value(8));
    let keys = vec![
        "a", "f", "s", "v", "m", "q", "i", "I", "p", "d", "x", "y", "w", "h", "z", "C", "c", "r",
        "X", "Y", "t", "o", "S", "O", "??",
    ];
    let values = vec![
        "0",
        "1",
        "2",
        "24",
        "32",
        "T",
        "t",
        "d",
        "q",
        "-1",
        "a",
        "",
        "4294967296",
        "-99999999999",
        "1.5",
        "\u{ff}",
    ];
    let mut control = String::new();
    for index in 0..pair_count {
        if index > 0 {
            control.push(',');
        }
        control.push_str(tc.draw(generators::sampled_from(keys.clone())));
        control.push('=');
        control.push_str(tc.draw(generators::sampled_from(values.clone())));
    }
    let mut raw = control.into_bytes();
    if tc.draw(generators::booleans()) {
        raw.push(b';');
        // payload is base64 in a well formed command, garbage is on purpose here too
        raw.extend_from_slice(&tc.draw(generators::binary().max_size(256)));
    }
    raw
}

#[hegel::test]
fn parse_never_panics_on_any_chunk_sequence(tc: hegel::TestCase) {
    let chunk_count = tc.draw(generators::integers::<usize>().max_value(8));
    let chunks: Vec<Vec<u8>> = (0..chunk_count)
        .map(|_| {
            if tc.draw(generators::booleans()) {
                tc.draw(any_control_chunk())
            } else {
                // capped at a few KiB purely for runtime
                tc.draw(generators::binary().max_size(4096))
            }
        })
        .collect();

    let mut parser = KittyCommandParser::new();
    // the property is "does not panic on any chunk sequence"
    for chunk in &chunks {
        let _ = parser.parse(chunk);
    }
    parser.abort_pending();
    assert!(
        !parser.has_pending(),
        "abort_pending left an upload pending after {:?}",
        chunks
    );
}

mod decode_base64_props {
    use super::*;
    use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
    use base64::engine::DecodePaddingMode;
    use hegel::generators;

    /// The decoder the payload pipeline used before `base64-simd`.
    const ORACLE: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );

    /// Trailing padding is trimmed first, as `run_payload_pipeline` does.
    fn trimmed(payload: &[u8]) -> &[u8] {
        let end = payload
            .iter()
            .rposition(|b| *b != b'=')
            .map(|pos| pos + 1)
            .unwrap_or(0);
        &payload[..end]
    }

    #[test]
    fn decoder_validates_every_byte_at_simd_and_chunk_boundaries() {
        for len in [0, 1, 2, 23, 24, 25, 47, 48, 49, 3071, 3072, 3073] {
            let bytes: Vec<_> = (0..len).map(|i| i as u8).collect();
            let encoded = BASE64_ENCODER.encode(&bytes);
            let mut input = trimmed(encoded.as_bytes()).to_vec();
            assert_eq!(decode_base64(&input).unwrap(), bytes);
            // Probe both the vector body and scalar tail, including non-zero trailing bits.
            let input_len = input.len();
            let positions = [0, 1, 31, 32, 63, 64, 4095, input_len.saturating_sub(1)];
            for at in positions.into_iter().filter(|at| *at < input_len) {
                let original = input[at];
                for byte in 0..=u8::MAX {
                    input[at] = byte;
                    let unpadded = trimmed(&input);
                    assert_eq!(
                        decode_base64(unpadded).ok(),
                        ORACLE.decode(unpadded).ok(),
                        "decoded length {len}, position {at}, byte {byte}"
                    );
                }
                input[at] = original;
            }
        }
    }

    #[hegel::test]
    fn decode_base64_matches_the_base64_crate(tc: hegel::TestCase) {
        let mut payload = if tc.draw(generators::booleans()) {
            // Usually-valid input: an encoding, then a few corruptions.
            let len = tc.draw(generators::integers::<usize>().max_value(64));
            let bytes: Vec<u8> = (0..len)
                .map(|_| tc.draw(generators::integers::<u8>()))
                .collect();
            BASE64_ENCODER.encode(bytes).into_bytes()
        } else {
            vec![]
        };
        let alphabet = [
            b'A', b'Q', b'g', b'/', b'+', b'=', b' ', b'\n', b'-', b'_', 0xff,
        ];
        for _ in 0..tc.draw(generators::integers::<usize>().max_value(4)) {
            let byte =
                alphabet[tc.draw(generators::integers::<usize>().max_value(alphabet.len() - 1))];
            let at = tc.draw(generators::integers::<usize>().max_value(payload.len()));
            if tc.draw(generators::booleans()) || at == payload.len() {
                payload.insert(at, byte);
            } else {
                payload[at] = byte;
            }
        }
        let input = trimmed(&payload);
        assert_eq!(
            decode_base64(input).ok(),
            ORACLE.decode(input).ok(),
            "{:?}",
            String::from_utf8_lossy(input)
        );
    }
}

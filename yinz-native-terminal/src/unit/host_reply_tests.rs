use super::*;
use std::sync::{Arc, Mutex};

fn geometry() -> Geometry {
    Geometry {
        rows: 40,
        cols: 120,
        cell_width: 10,
        cell_height: 20,
    }
}

fn answered(query: &[u8]) -> Vec<u8> {
    answer(query, geometry(), &Paints::default(), None)
}

#[test]
fn the_text_area_size_is_answered_in_pixels() {
    assert_eq!(answered(b"\x1b[14t"), b"\x1b[4;800;1200t");
}

#[test]
fn the_cell_size_is_answered_from_the_measured_font_metrics() {
    assert_eq!(answered(b"\x1b[16t"), b"\x1b[6;20;10t");
}

#[test]
fn the_pixel_replies_track_the_current_geometry() {
    let geometry = Geometry {
        rows: 24,
        cols: 80,
        cell_width: 9,
        cell_height: 18,
    };
    assert_eq!(
        answer(b"\x1b[14t", geometry, &Paints::default(), None),
        b"\x1b[4;432;720t"
    );
    assert_eq!(
        answer(b"\x1b[16t", geometry, &Paints::default(), None),
        b"\x1b[6;18;9t"
    );
}

#[test]
fn the_default_colors_are_answered_in_the_xparse_shape() {
    assert_eq!(
        answered(b"\x1b]10;?\x1b\\"),
        b"\x1b]10;rgb:e5e5/e5e5/e5e5\x1b\\"
    );
    assert_eq!(
        answered(b"\x1b]11;?\x1b\\"),
        b"\x1b]11;rgb:0000/0000/0000\x1b\\"
    );
}

#[test]
fn a_palette_register_is_answered_with_the_color_the_renderer_draws() {
    assert_eq!(
        answered(b"\x1b]4;1;?\x1b\\"),
        b"\x1b]4;1;rgb:cdcd/0000/0000\x1b\\"
    );
    assert_eq!(
        answered(b"\x1b]4;196;?\x1b\\"),
        b"\x1b]4;196;rgb:ffff/0000/0000\x1b\\"
    );
}

#[test]
fn the_terminator_of_the_query_is_mirrored_back() {
    assert_eq!(
        answered(b"\x1b]11;?\x07"),
        b"\x1b]11;rgb:0000/0000/0000\x07"
    );
    assert_eq!(
        answered(b"\x1b]4;0;?\x07"),
        b"\x1b]4;0;rgb:0000/0000/0000\x07"
    );
}

#[test]
fn the_answers_agree_with_the_palette_seeded_at_attach() {
    for index in 0u8..=255 {
        let query = format!("\x1b]4;{};?\x1b\\", index).into_bytes();
        let expected = format!(
            "\x1b]4;{};{}\x1b\\",
            index,
            xparse_color(Paints::default().indexed(index))
        );
        assert_eq!(answered(&query), expected.into_bytes());
    }
}

#[test]
fn the_configured_colors_are_what_a_pane_is_told() {
    let paints = Paints {
        foreground: [0x11, 0x22, 0x33],
        background: [0x44, 0x55, 0x66],
        ansi: {
            let mut ansi = crate::emulation::color::ANSI_16;
            ansi[1] = [0x77, 0x88, 0x99];
            ansi
        },
        ..Paints::default()
    };
    assert_eq!(
        answer(b"\x1b]10;?\x1b\\", geometry(), &paints, None),
        b"\x1b]10;rgb:1111/2222/3333\x1b\\"
    );
    assert_eq!(
        answer(b"\x1b]11;?\x1b\\", geometry(), &paints, None),
        b"\x1b]11;rgb:4444/5555/6666\x1b\\"
    );
    assert_eq!(
        answer(b"\x1b]4;1;?\x1b\\", geometry(), &paints, None),
        b"\x1b]4;1;rgb:7777/8888/9999\x1b\\"
    );
}

#[test]
fn a_clipboard_read_is_still_answered_from_the_clipboard() {
    let mut clipboard = clipboard::Clipboard::in_memory();
    clipboard.set("copied");
    let handle = Arc::new(Mutex::new(clipboard));
    assert_eq!(
        answer(
            b"\x1b]52;c;?\x1b\\",
            geometry(),
            &Paints::default(),
            Some(&handle)
        ),
        b"\x1b]52;c;Y29waWVk\x1b\\"
    );
}

#[test]
fn a_clipboard_read_without_a_clipboard_falls_through_to_an_empty_reply() {
    assert!(answered(b"\x1b]52;c;?\x1b\\").is_empty());
}

#[test]
fn anything_outside_the_whitelist_is_left_to_the_server_cache() {
    for query in [
        &b""[..],
        &b"\x1b[18t"[..],
        &b"\x1b[14"[..],
        &b"\x1b]10;?"[..],
        &b"\x1b]11;?\x1b"[..],
        &b"\x1b]12;?\x1b\\"[..],
        &b"\x1b]4;256;?\x1b\\"[..],
        &b"\x1b]4;;?\x1b\\"[..],
        &b"\x1b]4;1;\x1b\\"[..],
        &b"\x1b]4;-1;?\x1b\\"[..],
    ] {
        assert!(answered(query).is_empty(), "{:?}", query);
    }
}

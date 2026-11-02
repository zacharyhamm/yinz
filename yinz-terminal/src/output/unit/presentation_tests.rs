use super::*;

fn presentation(cols: usize, rows: usize) -> Presentation {
    Presentation {
        terminal: Size { cols, rows },
        content: Size { cols: 80, rows: 24 },
        background: PaletteColor::Rgb((12, 34, 56)),
    }
}

#[test]
fn centers_each_axis_and_leaves_odd_cell_on_far_edge() {
    for (cols, rows, offset) in [
        (80, 24, (0, 0)),
        (101, 24, (10, 0)),
        (80, 31, (0, 3)),
        (101, 31, (10, 3)),
        (60, 20, (0, 0)),
    ] {
        assert_eq!(presentation(cols, rows).offset(), offset);
    }
    let bytes = presentation(101, 31).present("\x1b[2J\x1b[1;1Hhello\x1b[24;80H".into());
    assert!(bytes.ends_with("\x1b[4;11Hhello\x1b[27;90H"));
    assert!(bytes.contains("\x1b[48;2;12;34;56m"));
    assert!(bytes.contains(&format!("\x1b[31;1H{}", " ".repeat(101))));
}

#[test]
fn preserves_graphics_and_osc_payloads_but_moves_their_cursor() {
    let payload = "\x1b]8;;https://example.test/\x1b[1;1H\x1b\\\x1bPqdata\x1b[1;1H\x1b\\\x1b_Gdata\x1b[1;1H\x1b\\";
    let bytes = presentation(100, 30).present(format!("\x1b[2;3H{payload}"));
    assert!(bytes.ends_with(&format!("\x1b[5;13H{payload}")));
}

#[test]
fn restores_margin_color_after_clear_and_preserves_equal_size_output() {
    let source = "\x1b[m\x1b[2J\x1b[1;1Hhello";
    assert_eq!(presentation(80, 24).present(source.into()), source);
    let bytes = presentation(100, 30).present(source.into());
    assert_eq!(bytes.matches("\x1b[48;2;12;34;56m").count(), 1);
    assert!(bytes.ends_with("\x1b[4;11Hhello"));
}

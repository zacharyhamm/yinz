use super::*;
use std::io::Cursor;

#[test]
fn quota_png_dimensions_color_antialiasing_and_digits() {
    let terminal = TerminalTheme {
        ansi: [(17, 43, 91); 16],
        foreground: (255, 255, 255),
        background: (0, 0, 0),
    };
    for value in (0..=9).chain([68, 100, 102, 999]) {
        let png = render(value, PaletteColor::EightBit(2), Some(terminal), 144, 36).unwrap();
        let mut reader = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buffer).unwrap();
        assert_eq!(
            (info.width, info.height, info.color_type),
            (144, 36, png::ColorType::Rgba)
        );
        assert!(buffer
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[..3] == [17, 43, 91]));
        assert!(buffer
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[3] > 0 && p[3] < 255));
        assert!(buffer.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
        assert!(buffer.as_chunks::<4>().0.iter().any(|p| p[3] == 0));
    }
    for (color, rgb) in [
        (PaletteColor::Rgb((0, 0, 0)), [0, 0, 0]),
        (PaletteColor::Rgb((255, 255, 255)), [255, 255, 255]),
        (PaletteColor::EightBit(196), [255, 0, 0]),
    ] {
        for (width, height) in [(108, 24), (18, 2), (1, 1)] {
            let png = render(102, color, None, width, height).unwrap();
            let mut reader = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
            let mut buffer = vec![0; reader.output_buffer_size()];
            let info = reader.next_frame(&mut buffer).unwrap();
            assert_eq!((info.width, info.height), (width, height));
            assert!(buffer.as_chunks::<4>().0.iter().all(|p| p[..3] == rgb));
        }
    }
}

#[test]
fn compact_digits_are_distinct_continuous_strokes() {
    let mut glyphs = Vec::new();
    for digit in 0..=9 {
        let png = render(digit, PaletteColor::Rgb((255, 255, 255)), None, 144, 36).unwrap();
        let mut reader = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut buffer).unwrap();
        // The numeral sits in these columns, before the smaller percent sign.
        let mut ink = std::collections::BTreeSet::new();
        for y in 0..36 {
            for x in 106..129 {
                if buffer[(y * 144 + x) * 4 + 3] >= 128 {
                    ink.insert((x as i32, y as i32));
                }
            }
        }
        assert!(!ink.is_empty(), "missing digit {digit}");
        assert!(!glyphs.contains(&ink), "duplicate digit {digit}");
        glyphs.push(ink.clone());
        let mut pending = vec![ink.pop_first().unwrap()];
        while let Some((x, y)) = pending.pop() {
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let neighbor = (x + dx, y + dy);
                    if ink.remove(&neighbor) {
                        pending.push(neighbor);
                    }
                }
            }
        }
        assert!(ink.is_empty(), "disconnected digit {digit}");
    }
}

#[test]
fn quota_png_rejects_unbounded_requests_before_allocation() {
    for (value, width, height) in [
        (68, 0, 60),
        (68, 1025, 60),
        (68, 100, 257),
        (68, 1024, 256),
        (u64::MAX, 160, 60),
        (68, u32::MAX, u32::MAX),
    ] {
        assert!(render(value, PaletteColor::EightBit(2), None, width, height).is_err());
    }
}

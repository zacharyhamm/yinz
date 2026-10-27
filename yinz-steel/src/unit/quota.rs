use super::*;
use std::io::Cursor;

#[test]
fn quota_png_dimensions_color_antialiasing_and_digits() {
    let terminal = TerminalTheme {
        ansi: [(17, 43, 91); 16],
        foreground: (255, 255, 255),
        background: (0, 0, 0),
    };
    for value in [0, 1, 8, 68, 100, 102] {
        let png = render(value, PaletteColor::EightBit(2), Some(terminal), 160, 60).unwrap();
        let mut reader = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buffer).unwrap();
        assert_eq!(
            (info.width, info.height, info.color_type),
            (160, 60, png::ColorType::Rgba)
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
    }
    assert!(digit_distance(8, 0.29, 0.5) < 0.0);
    assert!(digit_distance(0, 0.29, 0.5) > 0.0);
    assert_ne!(
        render(68, PaletteColor::Rgb((0, 0, 0)), None, 160, 60).unwrap(),
        render(68, PaletteColor::Rgb((255, 255, 255)), None, 160, 60).unwrap()
    );
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

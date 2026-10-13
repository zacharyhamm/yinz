use super::*;

#[test]
fn pane_surround_fallback_and_overrides() {
    for (background, expected) in [
        (PaletteColor::Rgb((0, 0, 0)), PaletteColor::Rgb((0, 0, 0))),
        (
            PaletteColor::Rgb((10, 12, 13)),
            PaletteColor::Rgb((0, 0, 1)),
        ),
        (
            PaletteColor::Rgb((40, 42, 54)),
            PaletteColor::Rgb((28, 30, 42)),
        ),
        (
            PaletteColor::Rgb((248, 249, 250)),
            PaletteColor::Rgb((236, 237, 238)),
        ),
        (PaletteColor::EightBit(16), PaletteColor::Rgb((0, 0, 0))),
        (PaletteColor::EightBit(196), PaletteColor::Rgb((243, 0, 0))),
        (
            PaletteColor::EightBit(255),
            PaletteColor::Rgb((226, 226, 226)),
        ),
    ] {
        let mut style = Style::default();
        style.colors.text_unselected.background = background;
        assert_eq!(style.pane_surround(), expected);
        if let PaletteColor::Rgb(rgb) = background {
            style.colors.text_unselected.background = PaletteColor::EightBit(1);
            style.terminal = Some(TerminalTheme {
                foreground: (255, 255, 255),
                background: rgb,
                ansi: [(0, 0, 0); 16],
            });
            assert_eq!(style.pane_surround(), expected);
        }
        style.colors.pane_surround = Some(PaletteColor::EightBit(9));
        assert_eq!(style.pane_surround(), PaletteColor::EightBit(9));
        let json = serde_json::to_value(style).unwrap();
        assert_eq!(serde_json::from_value::<Style>(json).unwrap(), style);
    }
    for index in 0..16 {
        let mut style = Style::default();
        style.colors.text_unselected.background = PaletteColor::EightBit(index);
        assert_eq!(style.pane_surround(), PaletteColor::EightBit(index));
    }
    let mut old = serde_json::to_value(Styling::default()).unwrap();
    old.as_object_mut().unwrap().remove("pane_surround");
    assert_eq!(
        serde_json::from_value::<Styling>(old)
            .unwrap()
            .pane_surround,
        None
    );
}

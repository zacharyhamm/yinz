use super::*;
use yinz_types::style::StyleDeclaration;

#[test]
fn shared_boundaries_repaint_when_only_ui_defaults_change() {
    let colors = StyleDeclaration {
        base: PaletteColor::Rgb((30, 40, 50)),
        background: PaletteColor::Rgb((240, 245, 250)),
        ..Default::default()
    };
    let make = |colors| {
        let mut boundaries = Boundaries::new(Viewport::default(), colors);
        boundaries.boundary_characters.insert(
            Coordinates::new(1, 1),
            BoundarySymbol::new(boundary_type::CROSS).color(None),
        );
        boundaries
    };
    let previous = make(colors);
    assert!(previous.render(Some(&previous)).unwrap().is_empty());
    for colors in [
        StyleDeclaration {
            background: PaletteColor::Rgb((20, 25, 30)),
            ..colors
        },
        StyleDeclaration {
            base: PaletteColor::Rgb((60, 70, 80)),
            ..colors
        },
    ] {
        let boundaries = make(colors);
        let chunks = boundaries.render(Some(&previous)).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].pane_default_bg, Some(colors.background.into()));
        assert_eq!(chunks[0].pane_default_fg, Some(colors.base.into()));
        assert!(boundaries.render(Some(&boundaries)).unwrap().is_empty());
    }
}

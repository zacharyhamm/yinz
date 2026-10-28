use super::*;

#[test]
fn gutter_edges_preserve_existing_symbols_and_colors() {
    use crate::panes::terminal_pane::TerminalPane;
    use yinz_types::internal_ids::TerminalId;
    use yinz_types::pane_size::{Dimension, PaneGeom};

    let outer = Viewport {
        x: 0,
        y: 0,
        cols: 100,
        rows: 10,
    };
    let mut pane = TerminalPane::new(
        TerminalId::new(0),
        PaneGeom {
            cols: Dimension::fixed(100),
            rows: Dimension::fixed(10),
            ..Default::default()
        },
        Default::default(),
        0,
        String::new(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        None,
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    );
    pane.set_max_content_width(80);
    let geom = pane.render_geom();
    let left = geom.x - 1;
    let right = geom.x + geom.cols.as_usize();
    let existing = BoundarySymbol {
        boundary_type: boundary_type::BOTTOM_LEFT,
        invisible: true,
        color: Some((PaletteColor::Rgb((100, 150, 200)), 8)),
        background: Some(PaletteColor::Rgb((20, 25, 30))),
    };
    let chrome = Some((PaletteColor::EightBit(1), PaletteColor::EightBit(2)));
    let gutter = BoundarySymbol::new(boundary_type::VERTICAL).color(None, chrome);
    let mut boundaries = Boundaries::new(outer);
    for x in [left, right] {
        boundaries
            .boundary_characters
            .insert(Coordinates::new(x, 9), existing);
    }
    boundaries.add_gutter_edges(&pane, outer, None, chrome);
    for x in [left, right] {
        assert_eq!(
            boundaries.boundary_characters[&Coordinates::new(x, 9)],
            existing
        );
        assert_eq!(
            boundaries.boundary_characters[&Coordinates::new(x, 0)],
            gutter
        );
    }
}

#[test]
fn shared_boundaries_repaint_when_either_chrome_color_changes() {
    let foreground = PaletteColor::Rgb((30, 40, 50));
    let background = PaletteColor::Rgb((240, 245, 250));
    let make = |colors| {
        let mut boundaries = Boundaries::new(Viewport::default());
        boundaries.boundary_characters.insert(
            Coordinates::new(1, 1),
            BoundarySymbol::new(boundary_type::CROSS).color(None, colors),
        );
        boundaries
    };
    let previous = make(Some((foreground, background)));
    assert!(previous.render(Some(&previous)).unwrap().is_empty());
    assert_eq!(previous.changed_color_cells(&previous).count(), 0);
    for colors in [
        Some((foreground, PaletteColor::Rgb((20, 25, 30)))),
        Some((PaletteColor::Rgb((60, 70, 80)), background)),
        None,
    ] {
        let boundaries = make(colors);
        let chunks = boundaries.render(Some(&previous)).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            boundaries
                .changed_color_cells(&previous)
                .collect::<Vec<_>>(),
            vec![(1, 1)]
        );
        let character = &chunks[0].terminal_characters[0];
        assert_eq!(
            character.styles.background,
            colors.map(|(_, bg)| bg.into()).or(RESET_STYLES.background)
        );
        assert_eq!(character.styles.foreground, colors.map(|(fg, _)| fg.into()));
        assert!(boundaries.render(Some(&boundaries)).unwrap().is_empty());
    }
}

#[test]
fn intersections_keep_foreground_precedence_and_chrome_background() {
    let chrome = Some((
        PaletteColor::Rgb((30, 40, 50)),
        PaletteColor::Rgb((240, 245, 250)),
    ));
    let selected = Some((PaletteColor::Rgb((100, 150, 200)), 8));
    for colors in [(selected, None), (None, selected)] {
        let first = BoundarySymbol::new(boundary_type::HORIZONTAL).color(colors.0, chrome);
        let second = BoundarySymbol::new(boundary_type::VERTICAL).color(colors.1, chrome);
        let combined = combine_symbols(first, second).unwrap();
        assert_eq!(combined.boundary_type, boundary_type::CROSS);
        assert_eq!(combined.color, selected);
        assert_eq!(combined.background, chrome.map(|(_, bg)| bg));
    }
}

use super::*;

fn key(palette: &mut CommandPalette, key: BareKey) {
    assert!(!palette.handle_key(KeyWithModifier::new(key)));
}

#[test]
fn filtering_selection_and_feedback() {
    let mut palette = CommandPalette::default();
    assert_eq!(palette.matches(), OPTIONS);
    key(&mut palette, BareKey::Up);
    assert_eq!(palette.selected, 0);
    for _ in 0..10 {
        key(&mut palette, BareKey::Down);
    }
    assert_eq!(palette.selected, 5);
    key(&mut palette, BareKey::Enter);
    assert_eq!(palette.feedback, Some("Help"));
    palette.insert_text("NEW");
    assert_eq!(palette.selected, 0);
    assert_eq!(palette.feedback, None);
    assert_eq!(palette.matches(), ["New pane", "New tab"]);
    key(&mut palette, BareKey::Down);
    key(&mut palette, BareKey::Enter);
    assert_eq!(palette.feedback, Some("New tab"));
    palette.insert_text("z");
    assert!(palette.matches().is_empty());
    key(&mut palette, BareKey::Down);
    key(&mut palette, BareKey::Enter);
    assert_eq!(palette.selected, 0);
    assert_eq!(palette.feedback, None);
    let layout = palette.layout(Viewport {
        cols: 80,
        rows: 24,
        ..Default::default()
    });
    let text: String = palette
        .chunks(&layout, &Style::default())
        .iter()
        .flat_map(|chunk| chunk.terminal_characters.iter().map(|c| c.character))
        .collect();
    assert!(text.contains("No matches"));
    assert!(palette.handle_key(KeyWithModifier::new(BareKey::Esc)));
}

#[test]
fn editor_keys_unicode_and_paste() {
    let mut palette = CommandPalette::default();
    palette.insert_text("日本\n\ttext\u{2003}more\x1b");
    assert_eq!(palette.input.get_text(), "日本  text more");
    palette.handle_key(KeyWithModifier::new(BareKey::Char('w')).with_ctrl_modifier());
    assert_eq!(palette.input.get_text(), "日本  text ");
    key(&mut palette, BareKey::Home);
    key(&mut palette, BareKey::Right);
    key(&mut palette, BareKey::Delete);
    assert_eq!(palette.input.get_text(), "日  text ");
    key(&mut palette, BareKey::Char('界'));
    assert_eq!(palette.input.get_text(), "日界  text ");
    let layout = palette.layout(Viewport {
        cols: 80,
        rows: 24,
        ..Default::default()
    });
    assert_eq!(palette.cursor(&layout), Some((15, 4)));
    palette.handle_key(KeyWithModifier::new(BareKey::Char('n')).with_alt_modifier());
    assert_eq!(palette.input.get_text(), "日界  text ");
}

#[test]
fn layout_and_cursor_fit_even_the_smallest_surfaces() {
    let mut palette = CommandPalette::default();
    palette.insert_text("界e\u{301}".repeat(80).as_str());
    for cols in 0..90 {
        for rows in 0..35 {
            let surface = Viewport {
                x: 2,
                y: 3,
                cols,
                rows,
            };
            let layout = palette.layout(surface);
            assert!(layout.rect.x >= surface.x && layout.rect.y >= surface.y);
            assert!(layout.rect.x + layout.rect.cols <= surface.x + cols);
            assert!(layout.rect.y + layout.rect.rows <= surface.y + rows);
            for chunk in palette.chunks(&layout, &Style::default()) {
                assert_eq!(chunk.width(), layout.rect.cols);
                assert!(chunk.y < surface.y + rows);
            }
            if let Some((x, y)) = palette.cursor(&layout) {
                assert!(layout.rect.contains(x, y));
            }
        }
    }
    let layout = palette.layout(Viewport {
        cols: 100,
        rows: 30,
        ..Default::default()
    });
    assert_eq!(
        layout.rect,
        CellRect {
            x: 18,
            y: 5,
            cols: 64,
            rows: 11
        }
    );
}

#[test]
fn clicking_and_short_list_window_share_the_rendered_geometry() {
    let mut palette = CommandPalette::default();
    let layout = palette.layout(Viewport {
        cols: 80,
        rows: 24,
        ..Default::default()
    });
    palette.click(
        &layout,
        Position::new(layout.options_y as i32 + 2, layout.rect.x as u16 + 2),
    );
    key(&mut palette, BareKey::Enter);
    assert_eq!(palette.feedback, Some("Find file"));
    let surface = Viewport {
        cols: 30,
        rows: 6,
        ..Default::default()
    };
    for _ in 0..5 {
        key(&mut palette, BareKey::Down);
    }
    let layout = palette.layout(surface);
    assert_eq!(layout.first_option, 5);
    palette.click(
        &layout,
        Position::new(layout.options_y as i32, layout.rect.x as u16 + 2),
    );
    key(&mut palette, BareKey::Enter);
    assert_eq!(palette.feedback, Some("Help"));
}

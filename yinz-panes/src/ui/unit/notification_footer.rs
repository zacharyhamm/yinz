use super::*;
use yinz_types::colors::PaletteColor;

#[test]
fn footer_is_one_dim_theme_row_with_fading_and_column_safe_truncation() {
    let mut style = Style::default();
    style.colors.text_unselected.base = PaletteColor::Rgb((120, 160, 200));
    style.colors.text_unselected.background = PaletteColor::Rgb((10, 20, 30));
    for (width, expected) in [(5, "…"), (12, "Alt+a t…"), (30, "Alt+a to activate")] {
        for fade in [0.0, 0.5, 1.0] {
            let chunks = notification_chunks(
                &["message".into()],
                Some("Alt+a to activate"),
                2,
                3,
                width,
                &style,
                fade,
            );
            assert_eq!(chunks.len(), 4);
            assert_eq!(chunks[2].y, 5);
            assert_eq!(chunks[3].y, 6);
            let footer = &chunks[2].terminal_characters;
            let text: String = footer.iter().map(|c| c.character).collect();
            assert!(text.contains(expected), "{text}");
            assert_eq!(footer.iter().map(|c| c.width()).sum::<usize>(), width);
            for c in &footer[1..footer.len() - 1] {
                assert_eq!(c.styles.dim, Some(AnsiCode::On));
                assert_eq!(
                    c.styles.foreground,
                    Some(
                        blend(
                            style.colors.text_unselected.background,
                            style.colors.text_unselected.base,
                            fade
                        )
                        .into()
                    )
                );
                assert_eq!(
                    c.styles.background,
                    Some(style.colors.text_unselected.background.into())
                );
            }
            assert_ne!(
                chunks[1].terminal_characters[1].styles.dim,
                Some(AnsiCode::On)
            );
        }
    }
    let chunks = notification_chunks(&["".into()], Some("界界界"), 0, 0, 8, &style, 1.0);
    let text: String = chunks[2]
        .terminal_characters
        .iter()
        .map(|c| c.character)
        .collect();
    assert_eq!(text, "│ 界…  │");
}

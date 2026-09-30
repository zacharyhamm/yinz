use super::super::theme::*;
use insta::assert_snapshot;
use std::path::{Path, PathBuf};
use yinz_types::colors::PaletteColor::Rgb;
use yinz_types::style::MultiplayerColors;

#[test]
fn swamp_builtin_themes() {
    // The builtin loader returns no themes under cfg(test), so parse the asset directly.
    let themes = Themes::from_string(include_str!("../../assets/themes/swamp.kdl"), false).unwrap();
    assert_eq!(themes.inner().len(), 2);
    for (name, green, cyan, magenta) in [
        (
            "swamp",
            Rgb((57, 255, 20)),
            Rgb((0, 255, 255)),
            Rgb((255, 0, 255)),
        ),
        (
            "swamp-hud",
            Rgb((74, 222, 128)),
            Rgb((34, 211, 238)),
            Rgb((232, 121, 249)),
        ),
    ] {
        let palette = themes.get_theme(name).unwrap().palette;
        for (unselected, selected) in [
            (palette.text_unselected, palette.text_selected),
            (palette.list_unselected, palette.list_selected),
            (palette.table_cell_unselected, palette.table_cell_selected),
        ] {
            assert_eq!(unselected.base, Rgb((209, 213, 219)));
            assert_eq!(unselected.background, Rgb((10, 10, 10)));
            assert_eq!(selected.base, Rgb((255, 255, 255)));
            assert_eq!(selected.background, Rgb((26, 26, 26)));
            assert_ne!(selected.background, unselected.background);
            for style in [unselected, selected] {
                assert_eq!(style.emphasis_0, Rgb((251, 146, 60)));
                assert_eq!(style.emphasis_1, cyan);
                assert_eq!(style.emphasis_2, green);
                assert_eq!(style.emphasis_3, magenta);
            }
        }
        assert_eq!(palette.frame_unselected.unwrap().base, Rgb((156, 163, 175)));
        assert_eq!(palette.frame_selected.base, green);
        assert_eq!(palette.frame_highlight.base, cyan);
        assert_eq!(palette.table_title.base, green);
        assert_eq!(palette.exit_code_success.base, green);
        assert_eq!(palette.exit_code_error.base, Rgb((248, 113, 113)));
        assert_eq!(palette.exit_code_error.emphasis_0, Rgb((251, 191, 36)));
        assert_eq!(palette.ribbon_selected.background, green);
        assert_eq!(
            palette.ribbon_unselected.background,
            if name == "swamp-hud" {
                cyan
            } else {
                Rgb((209, 213, 219))
            }
        );
        assert_eq!(
            palette.ribbon_selected.emphasis_1,
            palette.ribbon_selected.base
        );
        // Hovered tabs and buttons use emphasis_1 as their background.
        assert_eq!(palette.ribbon_unselected.emphasis_1, cyan);
        assert_ne!(
            palette.ribbon_unselected.emphasis_1,
            palette.ribbon_unselected.base
        );
        for ribbon in [palette.ribbon_selected, palette.ribbon_unselected] {
            assert_eq!(ribbon.base, Rgb((10, 10, 10)));
            assert_eq!(ribbon.emphasis_0, ribbon.base);
            assert_eq!(ribbon.emphasis_2, ribbon.base);
            assert_eq!(ribbon.emphasis_3, ribbon.base);
        }
        assert_eq!(
            palette.multiplayer_user_colors,
            MultiplayerColors {
                player_1: magenta,
                player_2: Rgb((96, 165, 250)),
                player_3: Rgb((167, 139, 250)),
                player_4: Rgb((251, 191, 36)),
                player_5: cyan,
                player_6: Rgb((251, 146, 60)),
                player_7: Rgb((248, 113, 113)),
                player_8: Rgb((209, 213, 219)),
                player_9: Rgb((244, 114, 182)),
                player_10: green,
            }
        );
    }
}

fn theme_test_dir(theme: String) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let theme_dir = root.join("src/unit/fixtures/themes");
    theme_dir.join(theme)
}

#[test]
fn dracula_theme_from_file() {
    let path = theme_test_dir("dracula.kdl".into());
    let theme = Themes::from_path(path).unwrap();
    assert_snapshot!(format!("{:#?}", theme));
}

#[test]
fn no_theme_is_err() {
    let path = theme_test_dir("nonexistent.kdl".into());
    let theme = Themes::from_path(path);
    assert!(theme.is_err());
}

/// A `#…` color with a multi-byte char or the wrong digit count must be a serde
/// error, not a panic. The visitor gated on byte length then sliced at fixed byte
/// offsets, so `#é1` (4 bytes, 2 chars) split the `é` mid-char. This is the serde
/// path; the KDL config path carries the same guard.
#[test]
fn a_malformed_hex_color_is_a_serde_error_not_a_panic() {
    for bad in [
        r##""#é1""##,
        r##""#abcé1""##,
        r##""#12""##,
        r##""#1234567""##,
        r##""#ggg""##,
        r##""#12g""##,
        r##""#""##,
        r##""é1""##,
    ] {
        let result = serde_json::from_str::<HexColor>(bad);
        assert!(
            result.is_err(),
            "expected {bad} to be rejected, got {result:?}"
        );
    }
}

#[test]
fn valid_hex_colors_deserialize() {
    let cases = [
        (r##""#fff""##, (0xff, 0xff, 0xff)),
        (r##""#1a2""##, (0x11, 0xaa, 0x22)),
        (r##""#1a2B3c""##, (0x1a, 0x2b, 0x3c)),
        (r##""1a2B3c""##, (0x1a, 0x2b, 0x3c)),
    ];
    for (input, expected) in cases {
        let color: HexColor = serde_json::from_str(input).unwrap();
        let rgb: (u8, u8, u8) = color.into();
        assert_eq!(rgb, expected, "for {input}");
    }
}

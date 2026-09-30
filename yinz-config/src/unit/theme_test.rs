use super::super::theme::*;
use crate::consts::YINZ_DEFAULT_THEMES;
use std::path::{Path, PathBuf};

#[test]
fn builtin_themes_round_trip() {
    // External themes are skipped by to_kdl, so parse as inline themes here.
    for file in YINZ_DEFAULT_THEMES.files() {
        let path = Path::new("assets/themes").join(file.path());
        let path = path.display();
        let content = file
            .contents_utf8()
            .unwrap_or_else(|| panic!("{path}: theme asset is not UTF-8"));
        let themes = Themes::from_string(content, false)
            .unwrap_or_else(|error| panic!("{path}: failed to parse themes: {error}"));
        assert!(!themes.inner().is_empty(), "{path}: no themes found");
        let serialized = themes
            .to_kdl()
            .unwrap_or_else(|| panic!("{path}: no themes serialized"));
        let reparsed = Themes::from_string(&serialized.to_string(), false)
            .unwrap_or_else(|error| panic!("{path}: failed to parse serialized themes: {error}"));
        assert_eq!(
            themes, reparsed,
            "{path}: themes changed after serialization"
        );
    }
}

#[test]
fn swamp_builtin_themes() {
    // The builtin loader returns no themes under cfg(test), so parse the asset directly.
    let themes = Themes::from_string(include_str!("../../assets/themes/swamp.kdl"), false).unwrap();
    for name in ["swamp", "swamp-hud"] {
        assert!(
            themes.get_theme(name).is_some(),
            "missing builtin theme {name}"
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
    let expected = Themes::from_string(include_str!("fixtures/themes/dracula.kdl"), true).unwrap();
    assert_eq!(theme, expected);
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

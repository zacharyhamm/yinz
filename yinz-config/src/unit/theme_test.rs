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

#[test]
fn terminal_palettes_are_complete_and_optional() {
    for file in YINZ_DEFAULT_THEMES.files() {
        let themes = Themes::from_string(file.contents_utf8().unwrap(), false).unwrap();
        for (name, theme) in themes.inner() {
            assert_eq!(theme.terminal.is_some(), name != "ansi", "{name}");
        }
    }
    let legacy = include_str!("fixtures/themes/dracula.kdl");
    let themes = Themes::from_string(legacy, false).unwrap();
    assert!(themes
        .inner()
        .values()
        .all(|theme| theme.terminal.is_none()));
}

#[test]
fn terminal_palette_validation_and_legacy_ui() {
    let mut colors = String::from("foreground \"#123\"; background 4 5 6;\n");
    for name in yinz_types::style::TerminalTheme::ANSI_NAMES {
        colors.push_str(&format!("{name} 10 20 30;\n"));
    }
    let source = format!("themes {{ test {{ terminal {{ {colors} }}; }}; }}");
    let parsed = Themes::from_string(&source, false).unwrap();
    let terminal = parsed.get_theme("test").unwrap().terminal.unwrap();
    assert_eq!(terminal.foreground, (17, 34, 51));
    assert_eq!(terminal.background, (4, 5, 6));
    assert_eq!(terminal.ansi, [(10, 20, 30); 16]);
    assert_eq!(
        parsed,
        Themes::from_string(&parsed.to_kdl().unwrap().to_string(), false).unwrap()
    );
    for invalid in [
        source.replace("bright_white 10 20 30;", ""),
        source.replace("foreground \"#123\"", "foreground 7"),
        source.replace("background 4 5 6", "background 256 5 6"),
        source.replace("background 4 5 6", "background -1 5 6"),
        source.replace("background 4 5 6", "background \"#xyz\""),
        source.replace("background 4 5 6;", "background 4 5 6; background 7 8 9;"),
        source.replace("bright_white", "unknown"),
    ] {
        assert!(Themes::from_string(&invalid, false).is_err(), "{invalid}");
    }
    let legacy = include_str!("fixtures/themes/dracula.kdl");
    let original = Themes::from_string(legacy, false).unwrap();
    let with_terminal = legacy.replacen(
        "dracula {",
        &format!("dracula {{ terminal {{ {colors} }}\n"),
        1,
    );
    let parsed = Themes::from_string(&with_terminal, false).unwrap();
    assert_eq!(
        original.get_theme("dracula").unwrap().palette,
        parsed.get_theme("dracula").unwrap().palette
    );
    assert_eq!(
        parsed.get_theme("dracula").unwrap().terminal,
        Some(terminal)
    );
}

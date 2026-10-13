use super::super::theme::*;
use crate::consts::YINZ_DEFAULT_THEMES;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use yinz_types::{colors::eightbit_to_rgb, data::PaletteColor};

const DARK_HOST: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (128, 0, 0),
    (0, 128, 0),
    (128, 128, 0),
    (0, 0, 128),
    (128, 0, 128),
    (0, 128, 128),
    (192, 192, 192),
    (128, 128, 128),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (0, 0, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];
const LIGHT_HOST: [(u8, u8, u8); 16] = [
    (250, 250, 250),
    (160, 0, 0),
    (0, 100, 0),
    (100, 80, 0),
    (0, 0, 160),
    (100, 0, 100),
    (0, 100, 100),
    (60, 60, 60),
    (180, 180, 180),
    (180, 30, 30),
    (30, 120, 30),
    (120, 100, 30),
    (30, 30, 180),
    (120, 30, 120),
    (30, 120, 120),
    (20, 20, 20),
];

fn color_rgb(color: PaletteColor, host: &[(u8, u8, u8); 16]) -> (u8, u8, u8) {
    match color {
        PaletteColor::Rgb(rgb) => rgb,
        PaletteColor::EightBit(index) if index < 16 => host[usize::from(index)],
        PaletteColor::EightBit(index) => eightbit_to_rgb(index),
    }
}

fn contrast_ratio(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let luminance = |(r, g, b): (u8, u8, u8)| {
        let linear = |channel: u8| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn builtin_text_selection_is_readable_and_distinct_on_both_hosts() {
    for file in YINZ_DEFAULT_THEMES.files() {
        let source = file.contents_utf8().unwrap();
        let themes = Themes::from_string(source, false).unwrap();
        for (name, theme) in themes.inner() {
            let selected = theme.palette.text_selected;
            for host in [&DARK_HOST, &LIGHT_HOST] {
                let background = color_rgb(selected.background, host);
                // Mouse and keyboard pane selection use text_selected, also used by plugin UI.
                for (role, foreground) in [
                    ("base", selected.base),
                    ("emphasis_0", selected.emphasis_0),
                    ("emphasis_1", selected.emphasis_1),
                    ("emphasis_2", selected.emphasis_2),
                    ("emphasis_3", selected.emphasis_3),
                ] {
                    let ratio = contrast_ratio(color_rgb(foreground, host), background);
                    assert!(
                        ratio >= 4.5,
                        "{name}: text_selected.{role}: contrast {ratio:.2}:1 < 4.5:1"
                    );
                }
                let panel = color_rgb(theme.palette.text_unselected.background, host);
                let terminal = theme.terminal.map(|terminal| terminal.background);
                for unselected in [Some(panel), terminal].into_iter().flatten() {
                    let ratio = contrast_ratio(background, unselected);
                    assert!(
                        ratio >= 3.0,
                        "{name}: selection on {unselected:?}: contrast {ratio:.2}:1 < 3:1"
                    );
                }
            }
        }
    }
}

#[test]
fn builtin_light_themes_have_readable_ui_colors() {
    for name in [
        "ayu-light",
        "catppuccin-latte",
        "dayfox",
        "everforest-light",
        "gruvbox-light",
        "iceberg-light",
        "pencil-light",
        "solarized-light",
        "tokyo-night-light",
    ] {
        let file = YINZ_DEFAULT_THEMES
            .get_file(format!("{name}.kdl"))
            .expect("light theme asset");
        let themes = Themes::from_string(file.contents_utf8().unwrap(), false).unwrap();
        let theme = themes.get_theme(name).unwrap();
        for host in [&DARK_HOST, &LIGHT_HOST] {
            check_ui_theme_contrast(name, theme, host);
        }
    }
}

fn check_ui_theme_contrast(name: &str, theme: &Theme, host: &[(u8, u8, u8); 16]) {
    let colors = theme.palette;
    let rgb = |color| color_rgb(color, host);
    let panel = rgb(colors.text_unselected.background);
    let check = |role: &str, field: &str, foreground, background| {
        let ratio = contrast_ratio(rgb(foreground), background);
        assert!(
            ratio >= 4.5,
            "{name}: {role}.{field} on {background:?}: contrast {ratio:.2}:1 < 4.5:1"
        );
    };
    for (role, declaration, opaque, transparent) in [
        ("text_unselected", colors.text_unselected, true, true),
        ("text_selected", colors.text_selected, true, false),
        (
            "table_cell_unselected",
            colors.table_cell_unselected,
            true,
            true,
        ),
        (
            "table_cell_selected",
            colors.table_cell_selected,
            true,
            false,
        ),
        ("list_unselected", colors.list_unselected, true, true),
        ("list_selected", colors.list_selected, true, false),
        ("ribbon_unselected", colors.ribbon_unselected, true, false),
        ("ribbon_selected", colors.ribbon_selected, true, false),
        ("table_title", colors.table_title, false, true),
        ("frame_selected", colors.frame_selected, false, true),
        ("frame_highlight", colors.frame_highlight, false, true),
        ("exit_code_success", colors.exit_code_success, false, true),
        ("exit_code_error", colors.exit_code_error, false, true),
    ] {
        for (field, foreground) in [
            ("base", declaration.base),
            ("emphasis_0", declaration.emphasis_0),
            ("emphasis_1", declaration.emphasis_1),
            ("emphasis_2", declaration.emphasis_2),
            ("emphasis_3", declaration.emphasis_3),
        ] {
            // These roles leave unused emphasis slots set to indexed zero.
            if !opaque && field != "base" && foreground == PaletteColor::EightBit(0) {
                continue;
            }
            // Builtin bars use this slot as a background, and pane frames use it on hover.
            if role == "ribbon_unselected" && field == "emphasis_1" {
                check(role, field, foreground, panel);
                if let Some(terminal) = theme.terminal {
                    check(role, field, foreground, terminal.background);
                }
                continue;
            }
            if opaque {
                check(role, field, foreground, rgb(declaration.background));
            }
            if transparent {
                check(role, field, foreground, panel);
                if let Some(terminal) = theme.terminal {
                    check(role, field, foreground, terminal.background);
                }
            }
            if role == "ribbon_unselected" {
                check(role, field, foreground, rgb(declaration.emphasis_1));
            }
        }
    }
    let players = colors.multiplayer_user_colors;
    for (index, foreground) in [
        players.player_1,
        players.player_2,
        players.player_3,
        players.player_4,
        players.player_5,
        players.player_6,
        players.player_7,
        players.player_8,
        players.player_9,
        players.player_10,
    ]
    .into_iter()
    .enumerate()
    {
        let field = format!("player_{}", index + 1);
        check("multiplayer_user_colors", &field, foreground, panel);
        if let Some(terminal) = theme.terminal {
            check(
                "multiplayer_user_colors",
                &field,
                foreground,
                terminal.background,
            );
        }
    }
    for (role, selected, unselected) in [
        ("text", colors.text_selected, colors.text_unselected),
        (
            "table_cell",
            colors.table_cell_selected,
            colors.table_cell_unselected,
        ),
        ("list", colors.list_selected, colors.list_unselected),
        ("ribbon", colors.ribbon_selected, colors.ribbon_unselected),
    ] {
        assert_ne!(
            selected.background, unselected.background,
            "{name}: {role} selection is invisible"
        );
    }
}

#[test]
fn ghostty_inventory_covers_every_source_theme_and_preserves_imported_palettes() {
    let mut builtins = HashMap::new();
    for file in YINZ_DEFAULT_THEMES.files() {
        let themes = Themes::from_string(file.contents_utf8().unwrap(), false).unwrap();
        for (name, theme) in themes.inner() {
            assert!(
                builtins.insert(name.clone(), theme.clone()).is_none(),
                "duplicate builtin theme {name}"
            );
        }
    }
    let mut source_names = HashSet::new();
    let mut yinz_names = HashSet::new();
    let mut imported = 0;
    let mut existing = 0;
    for row in include_str!("fixtures/ghostty-themes.tsv")
        .lines()
        .filter(|row| !row.starts_with('#'))
    {
        let columns: Vec<_> = row.split('\t').collect();
        assert_eq!(columns.len(), 4, "{row}");
        let (source_name, name, disposition) = (columns[0], columns[1], columns[2]);
        assert!(source_names.insert(source_name), "{source_name}");
        assert!(yinz_names.insert(name), "{name}");
        let theme = builtins
            .get(name)
            .unwrap_or_else(|| panic!("missing Ghostty port or existing equivalent: {row}"));
        match disposition {
            "existing" => existing += 1,
            "imported" => {
                imported += 1;
                let file = YINZ_DEFAULT_THEMES
                    .get_file(format!("{name}.kdl"))
                    .unwrap_or_else(|| panic!("missing Ghostty asset: {name}"));
                assert!(file.contents_utf8().unwrap().contains(
                    "// Source: https://github.com/mbadolato/iTerm2-Color-Schemes/blob/31756e77934045c66b28f59463ddb2870606d7ca/ghostty/"
                ));
                let terminal = theme.terminal.unwrap();
                let actual: Vec<_> = [terminal.foreground, terminal.background]
                    .into_iter()
                    .chain(terminal.ansi)
                    .collect();
                let expected: Vec<_> = columns[3]
                    .split_whitespace()
                    .map(|color| yinz_types::colors::parse_hex_rgb(color).unwrap())
                    .collect();
                assert_eq!(actual, expected, "{source_name}: terminal palette changed");
                for host in [&DARK_HOST, &LIGHT_HOST] {
                    check_ui_theme_contrast(name, theme, host);
                }
            },
            _ => panic!("invalid Ghostty inventory disposition: {row}"),
        }
    }
    assert_eq!(source_names.len(), 725);
    assert_eq!(imported, 691);
    assert_eq!(existing, 34);
    assert_eq!(builtins.len(), 734);
}

#[test]
fn builtin_frame_colors_contrast_with_terminal_background_on_both_hosts() {
    for file in YINZ_DEFAULT_THEMES.files() {
        let themes = Themes::from_string(file.contents_utf8().unwrap(), false).unwrap();
        for (name, theme) in themes.inner() {
            for host in [&DARK_HOST, &LIGHT_HOST] {
                check_frame_contrast(name, theme, host);
            }
        }
    }
}

fn check_frame_contrast(name: &str, theme: &Theme, host: &[(u8, u8, u8); 16]) {
    // Without a terminal block the background is unknown; ansi deliberately inherits the host.
    let Some(terminal) = theme.terminal else {
        return;
    };
    let colors = theme.palette;
    let background = terminal.background;
    let players = colors.multiplayer_user_colors;
    let foregrounds = [
        (
            "unfocused frame",
            colors
                .frame_unselected
                .map(|frame| frame.base)
                .unwrap_or(PaletteColor::Rgb(terminal.foreground)),
        ),
        ("frame_selected.base", colors.frame_selected.base),
        ("frame_highlight.base", colors.frame_highlight.base),
        (
            "frame_highlight.emphasis_0",
            colors.frame_highlight.emphasis_0,
        ),
        (
            "frame_highlight.emphasis_1",
            colors.frame_highlight.emphasis_1,
        ),
        (
            "ribbon_unselected.emphasis_1",
            colors.ribbon_unselected.emphasis_1,
        ),
        ("player_1", players.player_1),
        ("player_2", players.player_2),
        ("player_3", players.player_3),
        ("player_4", players.player_4),
        ("player_5", players.player_5),
        ("player_6", players.player_6),
        ("player_7", players.player_7),
        ("player_8", players.player_8),
        ("player_9", players.player_9),
        ("player_10", players.player_10),
    ];
    for (role, color) in foregrounds {
        let ratio = contrast_ratio(color_rgb(color, host), background);
        assert!(
            ratio >= 4.5,
            "{name}: {role} on terminal background: {ratio:.2}:1 < 4.5:1"
        );
    }
}

#[test]
fn contrast_resolver_uses_host_indices_and_supports_the_full_indexed_palette() {
    for host in [&DARK_HOST, &LIGHT_HOST] {
        assert_eq!(color_rgb(PaletteColor::Rgb((1, 2, 3)), host), (1, 2, 3));
        for index in 0..16 {
            assert_eq!(
                color_rgb(PaletteColor::EightBit(index), host),
                host[usize::from(index)]
            );
        }
        for index in 16..=255 {
            assert_eq!(
                color_rgb(PaletteColor::EightBit(index), host),
                eightbit_to_rgb(index)
            );
        }
    }
}

#[test]
fn themes_without_terminal_blocks_keep_ui_contrast_checks() {
    let themes =
        Themes::from_string(include_str!("../../assets/themes/ayu-light.kdl"), false).unwrap();
    let mut theme = themes.get_theme("ayu-light").unwrap().clone();
    theme.terminal = None;
    for host in [&DARK_HOST, &LIGHT_HOST] {
        check_ui_theme_contrast("ayu-light without terminal", &theme, host);
        check_frame_contrast("ayu-light without terminal", &theme, host);
    }
}

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

#[test]
fn every_builtin_has_explicit_pane_surround() {
    let mut count = 0;
    for file in YINZ_DEFAULT_THEMES.files() {
        let themes = Themes::from_string(file.contents_utf8().unwrap(), false).unwrap();
        for (name, theme) in themes.inner() {
            count += 1;
            let expected = if name == "ansi" {
                PaletteColor::EightBit(0)
            } else {
                let (r, g, b) = theme.terminal.unwrap().background;
                PaletteColor::Rgb((
                    r.saturating_sub(12),
                    g.saturating_sub(12),
                    b.saturating_sub(12),
                ))
            };
            assert_eq!(theme.palette.pane_surround, Some(expected), "{name}");
        }
    }
    assert_eq!(count, 734);
}

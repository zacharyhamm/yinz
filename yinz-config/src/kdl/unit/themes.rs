use super::*;

/// A `#…` color whose body is not clean ASCII hex — a multi-byte char, or the
/// wrong number of digits — must reach the `ConfigError` the parser already
/// models, never a panic. The length gate used to be on bytes while the parse
/// sliced at fixed byte offsets, so `#é1` (4 bytes, 2 chars) split the `é`
/// mid-char and panicked. This path is reachable from a plugin `reconfigure()`
/// and from `yinz action`, where a panic takes the whole server down.
#[test]
fn a_malformed_hex_color_is_a_config_error_not_a_panic() {
    for bad in ["#é1", "#abcé1", "#12", "#1234567", "#ggg", "#12g", "#"] {
        let doc: KdlDocument = format!(r#"fg "{bad}""#).parse().unwrap();
        let result = crate::kdl::palette_color_from_kdl("fg", &doc);
        assert!(
            result.is_err(),
            "expected {bad:?} to be rejected as a config error, got {result:?}"
        );
    }
}

#[test]
fn valid_hex_colors_still_parse_to_rgb() {
    let cases = [
        ("#fff", (0xff, 0xff, 0xff)),
        ("#000", (0x00, 0x00, 0x00)),
        ("#1a2", (0x11, 0xaa, 0x22)),
        ("#ffffff", (0xff, 0xff, 0xff)),
        ("#1a2B3c", (0x1a, 0x2b, 0x3c)),
    ];
    for (input, (r, g, b)) in cases {
        let doc: KdlDocument = format!(r#"fg "{input}""#).parse().unwrap();
        let color = crate::kdl::palette_color_from_kdl("fg", &doc).unwrap();
        assert_eq!(color, PaletteColor::Rgb((r, g, b)), "for {input}");
    }
}

#[test]
fn themes_to_string() {
    let fake_config = r#"
        themes {
           dracula {
                fg 248 248 242
                bg 40 42 54
                black 0 0 0
                red 255 85 85
                green 80 250 123
                yellow 241 250 140
                blue 98 114 164
                magenta 255 121 198
                cyan 139 233 253
                white 255 255 255
                orange 255 184 108
            }
        }"#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let sourced_from_external_file = false;
    let deserialized =
        Themes::from_kdl(document.get("themes").unwrap(), sourced_from_external_file).unwrap();
    let serialized = Themes::to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = Themes::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("themes")
            .unwrap(),
        sourced_from_external_file,
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config",
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn themes_to_string_with_hex_definitions() {
    let fake_config = r##"
        themes {
            nord {
                fg "#D8DEE9"
                bg "#2E3440"
                black "#3B4252"
                red "#BF616A"
                green "#A3BE8C"
                yellow "#EBCB8B"
                blue "#81A1C1"
                magenta "#B48EAD"
                cyan "#88C0D0"
                white "#E5E9F0"
                orange "#D08770"
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let sourced_from_external_file = false;
    let deserialized =
        Themes::from_kdl(document.get("themes").unwrap(), sourced_from_external_file).unwrap();
    let serialized = Themes::to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = Themes::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("themes")
            .unwrap(),
        sourced_from_external_file,
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn themes_to_string_with_eight_bit_definitions() {
    let fake_config = r##"
        themes {
            default {
                fg 1
                bg 10
                black 20
                red 30
                green 40
                yellow 50
                blue 60
                magenta 70
                cyan 80
                white 90
                orange 254
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let sourced_from_external_file = false;
    let deserialized =
        Themes::from_kdl(document.get("themes").unwrap(), sourced_from_external_file).unwrap();
    let serialized = Themes::to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = Themes::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("themes")
            .unwrap(),
        sourced_from_external_file,
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn themes_to_string_with_combined_definitions() {
    let fake_config = r##"
        themes {
            default {
                fg 1
                bg 10
                black 20
                red 30
                green 40
                yellow 50
                blue 60
                magenta 70
                cyan 80
                white 255 255 255
                orange "#D08770"
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let sourced_from_external_file = false;
    let deserialized =
        Themes::from_kdl(document.get("themes").unwrap(), sourced_from_external_file).unwrap();
    let serialized = Themes::to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = Themes::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("themes")
            .unwrap(),
        sourced_from_external_file,
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn themes_to_string_with_multiple_theme_definitions() {
    let fake_config = r##"
        themes {
           nord {
               fg "#D8DEE9"
               bg "#2E3440"
               black "#3B4252"
               red "#BF616A"
               green "#A3BE8C"
               yellow "#EBCB8B"
               blue "#81A1C1"
               magenta "#B48EAD"
               cyan "#88C0D0"
               white "#E5E9F0"
               orange "#D08770"
           }
           dracula {
                fg 248 248 242
                bg 40 42 54
                black 0 0 0
                red 255 85 85
                green 80 250 123
                yellow 241 250 140
                blue 98 114 164
                magenta 255 121 198
                cyan 139 233 253
                white 255 255 255
                orange 255 184 108
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let sourced_from_external_file = false;
    let deserialized =
        Themes::from_kdl(document.get("themes").unwrap(), sourced_from_external_file).unwrap();
    let serialized = Themes::to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = Themes::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("themes")
            .unwrap(),
        sourced_from_external_file,
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn pane_surround_parses_and_roundtrips_modern_and_legacy_themes() {
    let legacy = "fg 1; bg 2; black 3; red 4; green 5; yellow 6; blue 7; magenta 8; cyan 9; white 10; orange 11;";
    for body in ["", legacy] {
        for (value, expected) in [
            ("", None),
            (
                "pane_surround \"#1c1e2a\";",
                Some(PaletteColor::Rgb((28, 30, 42))),
            ),
            (
                "pane_surround \"#abc\";",
                Some(PaletteColor::Rgb((170, 187, 204))),
            ),
            ("pane_surround 1 2 3;", Some(PaletteColor::Rgb((1, 2, 3)))),
            ("pane_surround 0;", Some(PaletteColor::EightBit(0))),
            ("pane_surround 255;", Some(PaletteColor::EightBit(255))),
        ] {
            let themes =
                Themes::from_string(&format!("themes {{ custom {{ {body} {value} }}; }}"), false)
                    .unwrap();
            assert_eq!(themes.inner()["custom"].palette.pane_surround, expected);
            let serialized = themes.to_kdl().unwrap().to_string();
            assert_eq!(Themes::from_string(&serialized, false).unwrap(), themes);
            if !body.is_empty() {
                assert_eq!(
                    themes.inner()["custom"].palette.text_unselected.background,
                    PaletteColor::EightBit(3)
                );
            }
        }
    }
}

#[test]
fn pane_surround_rejects_invalid_values() {
    for value in [
        "",
        "-1",
        "256",
        "0 0 256",
        "-1 0 0",
        "1 2",
        "1 2 3 4",
        "true",
        "\"#ggg\"",
        "red=1",
        "0 {}",
        "0; pane_surround 1",
    ] {
        assert!(
            Themes::from_string(
                &format!("themes {{ custom {{ pane_surround {value}; }}; }}"),
                false
            )
            .is_err(),
            "{value}"
        );
    }
}

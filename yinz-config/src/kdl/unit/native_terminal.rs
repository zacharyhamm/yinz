use super::*;

#[test]
fn native_startup_dimensions_kdl_contract() {
    for (fragment, columns, rows) in [
        ("", None, None),
        ("initial_columns 2;", Some(2), None),
        ("initial_rows 1000;", None, Some(1000)),
        ("initial_columns 1000; initial_rows 2;", Some(1000), Some(2)),
    ] {
        let config = Config::from_kdl(&format!("terminal {{ {fragment} }}"), None).unwrap();
        assert_eq!(
            (
                config.terminal.initial_columns,
                config.terminal.initial_rows
            ),
            (columns, rows)
        );
        let serialized = config.to_string(false).unwrap();
        assert_eq!(
            Config::from_kdl(&serialized, None).unwrap().terminal,
            config.terminal
        );
    }
    for key in ["initial_columns", "initial_rows"] {
        for value in [
            "-2", "0", "1", "1001", "65536", "2.0", "\"2\"", "(typed)2", "true",
        ] {
            let text = format!("terminal {{ {key} {value}; }}");
            let ConfigError::KdlError(error) = Config::from_kdl(&text, None).unwrap_err() else {
                panic!("expected a KDL diagnostic for {text}");
            };
            assert!(
                error.error_message.contains(&format!(
                    "{key} must be a plain integer between 2 and 1000 cells"
                )),
                "{error:?}"
            );
            let offset = error.offset.unwrap();
            let len = error.len.unwrap();
            assert!(text[offset..offset + len].contains(key), "{error:?}");
        }
        for fragment in [
            format!("({key}){key} 2;"),
            format!("{key} n=2;"),
            format!("{key};"),
            format!("{key} 2 3;"),
            format!("{key} 2 {{ x; }};"),
        ] {
            let error = Config::from_kdl(&format!("terminal {{ {fragment} }}"), None).unwrap_err();
            let detail = crate::editor::validation_error(error);
            assert!(
                detail.contains(key) && detail.contains("between 2 and 1000"),
                "{detail}"
            );
        }
        let text = format!("terminal {{ {key} 2; {key} 3; }}");
        let error = Config::from_kdl(&text, None).unwrap_err();
        assert!(crate::editor::validation_error(error)
            .contains(&format!("duplicate terminal setting {key}")));
    }
}

#[test]
fn native_terminal_settings_merge_roundtrip_and_validate() {
    let base = Config::from_kdl(
        r#"terminal { font_size 14.0; font_family "Iosevka"; opacity 0.9; }"#,
        None,
    )
    .unwrap();
    let merged = Config::from_kdl(r#"terminal { padding 4.0; }"#, Some(base)).unwrap();
    assert_eq!(merged.terminal.font_size, Some(14.0));
    assert_eq!(merged.terminal.padding, Some(4.0));
    let serialized = merged.to_string(false).unwrap();
    let roundtrip = Config::from_kdl(&serialized, None).unwrap();
    assert_eq!(roundtrip.terminal, merged.terminal);
    for text in [
        r#"terminal { font_size -1; }"#,
        r#"terminal { opacity 2; }"#,
        r#"terminal { padding "big"; }"#,
        r#"terminal { startup_mode "invalid"; }"#,
        r#"terminal { typo true; }"#,
        r#"terminal { font_size 12; font_size 14; }"#,
    ] {
        assert!(Config::from_kdl(text, None).is_err(), "{text}");
    }
}

#[test]
fn font_faces_aliases_blanks_roundtrip_and_weight_validation() {
    for name in [
        "font",
        "font_family",
        "font_bold",
        "font_italic",
        "font_bold_italic",
    ] {
        for value in ["Iosevka Term", "", "  "] {
            let config =
                Config::from_kdl(&format!("terminal {{ {name} {value:?}; }}"), None).unwrap();
            let text = config.to_string(false).unwrap();
            assert!(!text.contains("font_family"), "{text}");
            assert_eq!(
                Config::from_kdl(&text, None).unwrap().terminal,
                config.terminal
            );
        }
        for value in ["", "true", "400", "1.5", "\"a\" \"b\"", "value=\"a\""] {
            assert!(Config::from_kdl(&format!("terminal {{ {name} {value}; }}"), None).is_err());
        }
        assert!(
            Config::from_kdl(&format!("terminal {{ {name} \"a\"; {name} \"b\"; }}"), None).is_err()
        );
    }
    for (a, b) in [("font", "font_family"), ("font_family", "font")] {
        for value in ["a", "b"] {
            let error =
                Config::from_kdl(&format!("terminal {{ {a} \"a\"; {b} {value:?}; }}"), None)
                    .unwrap_err();
            assert!(format!("{error:?}").contains("font/font_family"));
        }
    }
    for weight in ["100", "900"] {
        assert!(Config::from_kdl(&format!("terminal {{ font_weight {weight}; }}"), None).is_ok());
    }
    for weight in ["-1", "99", "901", "400.5", "\"400\"", "true"] {
        assert!(Config::from_kdl(&format!("terminal {{ font_weight {weight}; }}"), None).is_err());
    }
}

#[test]
fn documented_native_terminal_example_loads() {
    let docs = include_str!("../../../../docs/native-terminal.md");
    let example = docs
        .split("```kdl\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    let config = Config::from_kdl(example, None).unwrap();
    assert_eq!(config.terminal.font_ligatures, Some(true));
    assert_eq!(config.terminal.cursor_trail, Some(true));
    assert_eq!(config.terminal.cell_width, Some(1.0));
    assert_eq!(config.terminal.line_height, Some(1.0));
    assert_eq!(config.terminal.baseline_offset, Some(2));
    assert_eq!(config.terminal.padding, Some(4.0));
    assert_eq!(config.terminal.padding_top, Some(2.0));
    assert_eq!(config.terminal.padding_left, Some(6.0));
    assert_eq!(config.terminal.padding_balance, Some(false));
    assert_eq!(
        config.terminal.padding_color,
        Some(yinz_types::input::native_terminal::PaddingColor::Background)
    );
    assert_eq!(config.terminal.minimum_contrast, Some(1.0));
    assert_eq!(config.terminal.font_family.as_deref(), Some("Iosevka Term"));
    assert_eq!(config.terminal.font_bold, config.terminal.font_family);
    assert_eq!(config.terminal.font_italic, config.terminal.font_family);
    assert_eq!(
        config.terminal.font_bold_italic,
        config.terminal.font_family
    );
    let features_example = docs
        .split("```kdl\n")
        .skip(1)
        .filter_map(|block| block.split("```").next())
        .find(|block| block.contains("font_features"))
        .unwrap();
    let config = Config::from_kdl(features_example, None).unwrap();
    assert_eq!(
        config.terminal.font_features,
        features(&["ss01", "zero", "-calt", "+dlig"])
    );
}

#[test]
fn ligature_aliases_validate_roundtrip_and_overlay() {
    assert_eq!(
        Config::from_kdl("terminal {}", None)
            .unwrap()
            .terminal
            .font_ligatures,
        None
    );
    for name in ["ligatures", "font_ligatures"] {
        for enabled in [false, true] {
            let config = Config::from_kdl(
                &format!("terminal {{ {name} {enabled}; padding 2; }}"),
                None,
            )
            .unwrap();
            assert_eq!(config.terminal.font_ligatures, Some(enabled));
            let text = config.to_string(false).unwrap();
            assert!(!text.contains("font_ligatures"), "{text}");
            assert_eq!(text.matches("ligatures ").count(), 1, "{text}");
            let roundtrip = Config::from_kdl(&text, None).unwrap();
            assert_eq!(roundtrip.terminal, config.terminal);
            assert_eq!(roundtrip.to_string(false).unwrap(), text);
            let inherited = Config::from_kdl("terminal { opacity 0.5; }", Some(config)).unwrap();
            assert_eq!(inherited.terminal.font_ligatures, Some(enabled));
            for other in ["ligatures", "font_ligatures"] {
                let overridden = Config::from_kdl(
                    &format!("terminal {{ {other} {}; }}", !enabled),
                    Some(inherited.clone()),
                )
                .unwrap();
                assert_eq!(overridden.terminal.font_ligatures, Some(!enabled));
                assert_eq!(overridden.terminal.padding, Some(2.0));
                for next in [false, true] {
                    let input = format!("terminal {{ {name} {enabled}; {other} {next}; }}");
                    let parsed_error = Config::from_kdl(&input, None).unwrap_err();
                    if let ConfigError::KdlError(error) = &parsed_error {
                        assert!(error.offset.is_some());
                        assert!(error.len.is_some_and(|len| len > 0));
                    } else {
                        panic!("expected a source-located KDL error: {parsed_error:?}");
                    }
                    let error = format!("{parsed_error:?}");
                    assert!(error.contains("ligatures/font_ligatures"), "{error}");
                    assert!(
                        error.contains(&format!("earlier value {enabled}")),
                        "{error}"
                    );
                    assert!(
                        error.contains(&format!("conflicting {other} {next}")),
                        "{error}"
                    );
                }
            }
        }
        for value in [
            "",
            "\"true\"",
            "1",
            "0.5",
            "null",
            "true false",
            "value=true",
            "(bool)true",
            "true {}",
            "{ child true; }",
        ] {
            let input = format!("terminal {{ {name} {value}; }}");
            let error = format!("{:?}", Config::from_kdl(&input, None).unwrap_err());
            assert!(error.contains(name), "{input}: {error}");
        }
    }
}

#[test]
fn typed_ligature_nodes_are_not_plain_booleans() {
    for name in ["ligatures", "font_ligatures"] {
        let input = format!("terminal {{ (setting){name} true; }}");
        let error = format!("{:?}", Config::from_kdl(&input, None).unwrap_err());
        assert!(error.contains(name), "{error}");
        assert!(error.contains("plain boolean"), "{error}");
    }
}

#[test]
fn cell_metrics_validate_plain_finite_multipliers_and_roundtrip() {
    assert_eq!(
        Config::from_kdl("terminal {}", None)
            .unwrap()
            .terminal
            .cell_width,
        None
    );
    for name in ["cell_width", "line_height"] {
        for value in ["0.5", "1", "1.25", "3"] {
            let config =
                Config::from_kdl(&format!("terminal {{ {name} {value}; }}"), None).unwrap();
            let serialized = config.to_string(false).unwrap();
            assert_eq!(
                Config::from_kdl(&serialized, None).unwrap().terminal,
                config.terminal
            );
        }
        for value in [
            "",
            "0.499",
            "3.001",
            "-1",
            "1e999",
            "\"1.5\"",
            "true",
            "null",
            "1 2",
            "value=1",
            "(float)1.5",
            "1 {}",
        ] {
            let input = format!("terminal {{ {name} {value}; }}");
            let error = format!("{:?}", Config::from_kdl(&input, None).unwrap_err());
            assert!(error.contains(name), "{input}: {error}");
        }
        let error = format!(
            "{:?}",
            Config::from_kdl(&format!("terminal {{ (metric){name} 1.5; }}"), None).unwrap_err()
        );
        assert!(error.contains("plain numeric"), "{error}");
        for next in [1, 2] {
            let error = format!(
                "{:?}",
                Config::from_kdl(&format!("terminal {{ {name} 1; {name} {next}; }}"), None)
                    .unwrap_err()
            );
            assert!(
                error.contains(&format!("duplicate terminal setting {name}")),
                "{error}"
            );
        }
    }
    // Exercise nonfinite values directly: KDL text versions differ in their spelling.
    for name in ["cell_width", "line_height"] {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut block = KdlNode::new("terminal");
            let mut children = KdlDocument::new();
            let mut metric = KdlNode::new(name);
            metric.push(KdlValue::Base10Float(value));
            children.nodes_mut().push(metric);
            block.set_children(children);
            let error = format!("{:?}", native_terminal::from_kdl(&block).unwrap_err());
            assert!(
                error.contains(name) && error.contains("finite multiplier"),
                "{error}"
            );
        }
    }
}

fn features(list: &[&str]) -> Option<Vec<String>> {
    Some(list.iter().map(|tag| tag.to_string()).collect())
}

#[test]
fn font_features_parse_in_order_roundtrip_and_preserve_explicit_empty() {
    let config = Config::from_kdl(
        r#"terminal { font_features "ss01" "zero" "-calt" "+dlig"; padding 2; }"#,
        None,
    )
    .unwrap();
    assert_eq!(
        config.terminal.font_features,
        features(&["ss01", "zero", "-calt", "+dlig"])
    );
    assert_eq!(
        Config::from_kdl("terminal { padding 2; }", None)
            .unwrap()
            .terminal
            .font_features,
        None
    );
    assert_eq!(
        Config::from_kdl("terminal { font_features; }", None)
            .unwrap()
            .terminal
            .font_features,
        features(&[])
    );
    for input in [
        r#"terminal { font_features "ss01" "zero" "-calt" "+dlig"; }"#,
        r#"terminal { font_features "-calt"; padding 2; }"#,
        "terminal { font_features; }",
        r#"terminal { font_features "calt" "-calt" "calt"; }"#,
    ] {
        let config = Config::from_kdl(input, None).unwrap();
        let text = config.to_string(false).unwrap();
        assert_eq!(text.matches("font_features").count(), 1, "{text}");
        let roundtrip = Config::from_kdl(&text, None).unwrap();
        assert_eq!(roundtrip.terminal, config.terminal, "{text}");
        assert_eq!(roundtrip.to_string(false).unwrap(), text);
    }
}

#[test]
fn malformed_font_features_point_at_the_offending_entry() {
    for (value, bad) in [
        (r#""ss1""#, r#""ss1""#),
        (r#""ss011""#, r#""ss011""#),
        (r#""--calt""#, r#""--calt""#),
        (r#""""#, r#""""#),
        ("4", "4"),
        (r#""ss 1""#, r#""ss 1""#),
        (r#""cv01=2""#, r#""cv01=2""#),
        ("true", "true"),
        ("null", "null"),
        (r#"name="ss01""#, r#"name="ss01""#),
        (r#"(tag)"ss01""#, r#"(tag)"ss01""#),
        (r#""ss01" "ss1""#, r#""ss1""#),
        (r#""ss01" 4 "zero""#, "4"),
    ] {
        let input = format!("terminal {{ font_features {value}; }}");
        let parsed_error = Config::from_kdl(&input, None).unwrap_err();
        let ConfigError::KdlError(error) = &parsed_error else {
            panic!("expected a source-located KDL error: {parsed_error:?}");
        };
        let offset = error.offset.unwrap();
        let len = error.len.unwrap();
        assert!(len > 0, "{input}");
        assert_eq!(
            input[offset..offset + len].trim(),
            bad,
            "{input}: {parsed_error:?}"
        );
        let message = format!("{parsed_error:?}");
        assert!(message.contains("font feature"), "{input}: {message}");
        assert!(message.contains("font_features"), "{input}: {message}");
    }
    let error = format!(
        "{:?}",
        Config::from_kdl(r#"terminal { font_features "ss1"; }"#, None).unwrap_err()
    );
    assert!(
        error.contains(r#"\"ss1\" is not a font feature"#),
        "{error}"
    );
    assert!(error.contains("ss01"), "{error}");
    for input in [
        r#"terminal { (list)font_features "ss01"; }"#,
        r#"terminal { font_features "ss01" { }; }"#,
        r#"terminal { font_features { child "ss01"; }; }"#,
        r#"terminal { font_features "ss01"; font_features "zero"; }"#,
        r#"terminal { font_features; font_features; }"#,
    ] {
        let error = format!("{:?}", Config::from_kdl(input, None).unwrap_err());
        assert!(error.contains("font_features"), "{input}: {error}");
    }
    let error = format!(
        "{:?}",
        Config::from_kdl(
            r#"terminal { font_features "ss01"; font_features "zero"; }"#,
            None
        )
        .unwrap_err()
    );
    assert!(
        error.contains("duplicate terminal setting font_features"),
        "{error}"
    );
}

#[test]
fn later_font_feature_layers_replace_wholesale_and_omission_inherits() {
    let base = Config::from_kdl(
        r#"terminal { font_features "ss01" "zero"; padding 2; }"#,
        None,
    )
    .unwrap();
    let inherited = Config::from_kdl("terminal { opacity 0.5; }", Some(base.clone())).unwrap();
    assert_eq!(
        inherited.terminal.font_features,
        features(&["ss01", "zero"])
    );
    let replaced =
        Config::from_kdl(r#"terminal { font_features "-calt"; }"#, Some(inherited)).unwrap();
    assert_eq!(replaced.terminal.font_features, features(&["-calt"]));
    assert_eq!(replaced.terminal.padding, Some(2.0));
    assert_eq!(replaced.terminal.opacity, Some(0.5));
    let cleared = Config::from_kdl("terminal { font_features; }", Some(replaced)).unwrap();
    assert_eq!(cleared.terminal.font_features, features(&[]));
    let restored =
        Config::from_kdl(r#"terminal { font_features "ss01"; }"#, Some(cleared)).unwrap();
    assert_eq!(restored.terminal.font_features, features(&["ss01"]));
}

#[test]
fn baseline_offset_validates_integer_pixels_and_roundtrips() {
    assert_eq!(
        Config::from_kdl("terminal {}", None)
            .unwrap()
            .terminal
            .baseline_offset,
        None
    );
    for offset in [-64, -2, 0, 2, 64] {
        let config =
            Config::from_kdl(&format!("terminal {{ baseline_offset {offset}; }}"), None).unwrap();
        assert_eq!(config.terminal.baseline_offset, Some(offset));
        assert_eq!(
            Config::from_kdl(&config.to_string(false).unwrap(), None)
                .unwrap()
                .terminal,
            config.terminal
        );
    }
    for bad in [
        "",
        "-65",
        "65",
        "2147483648",
        "-2147483649",
        "2.0",
        "1e999",
        "\"2\"",
        "true",
        "null",
        "2 3",
        "value=2",
        "(int)2",
        "2 {}",
    ] {
        let input = format!("terminal {{ baseline_offset {bad}; }}");
        let error = format!("{:?}", Config::from_kdl(&input, None).unwrap_err());
        assert!(error.contains("baseline_offset"), "{input}: {error}");
    }
    for input in [
        "terminal { (int)baseline_offset 2; }",
        "terminal { baseline_offset 2; baseline_offset 2; }",
    ] {
        assert!(Config::from_kdl(input, None).is_err(), "{input}");
    }
    // Integer lexing overflow is diagnosed by KDL before setting validation.
    assert!(Config::from_kdl(
        "terminal { baseline_offset 99999999999999999999999999999; }",
        None
    )
    .is_err());
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut block = KdlNode::new("terminal");
        let mut children = KdlDocument::new();
        let mut setting = KdlNode::new("baseline_offset");
        setting.push(KdlValue::Base10Float(value));
        children.nodes_mut().push(setting);
        block.set_children(children);
        let error = format!("{:?}", native_terminal::from_kdl(&block).unwrap_err());
        assert!(
            error.contains("plain integer") && error.contains("-64 and 64"),
            "{error}"
        );
    }
}

#[test]
fn baseline_offset_validation_points_at_the_setting_node() {
    for value in ["65", "-65", "2.0", "true", "null", "(int)2"] {
        let input = format!("terminal {{ padding 3; baseline_offset {value}; opacity 0.5; }}");
        let error = Config::from_kdl(&input, None).unwrap_err();
        let ConfigError::KdlError(error) = error else {
            panic!("expected a source-located KDL diagnostic");
        };
        let offset = error.offset.unwrap();
        let len = error.len.unwrap();
        let source = input[offset..offset + len].trim();
        assert!(source.starts_with("baseline_offset"), "{source}");
        assert!(
            !source.contains("padding") && !source.contains("opacity"),
            "{source}"
        );
    }
}

#[test]
fn underline_metrics_validate_and_roundtrip_as_independent_plain_integers() {
    assert_eq!(
        Config::terminal_settings_from_kdl("mouse_mode false").unwrap(),
        None
    );
    let empty = Config::terminal_settings_from_kdl("terminal {}")
        .unwrap()
        .unwrap();
    assert_eq!(
        (empty.underline_offset, empty.underline_thickness),
        (None, None)
    );
    for name in ["underline_offset", "underline_thickness"] {
        for value in [-64, -2, 0, 2, 64] {
            let input = format!("terminal {{ {name} {value}; }}");
            let supplied = Config::terminal_settings_from_kdl(&input).unwrap().unwrap();
            let expected = if name == "underline_offset" {
                (Some(value), None)
            } else {
                (None, Some(value))
            };
            assert_eq!(
                (supplied.underline_offset, supplied.underline_thickness),
                expected
            );
            let config = Config::from_kdl(&input, None).unwrap();
            assert_eq!(config.terminal, supplied);
            let serialized = config.to_string(false).unwrap();
            assert_eq!(
                Config::from_kdl(&serialized, None).unwrap().terminal,
                supplied
            );
        }
        for setting in [
            "",
            "-65",
            "65",
            "2147483648",
            "2.5",
            "2.0",
            "\"2\"",
            "true",
            "null",
            "2 3",
            "value=2",
            "(int)2",
            "2 {}",
            &format!("2; {name} 2"),
        ]
        .map(|value| format!("{name} {value}"))
        .into_iter()
        .chain([format!("(int){name} 2")])
        {
            let input = format!("terminal {{ padding 4; {setting}; opacity 0.5; }}");
            let error = Config::from_kdl(&input, None).unwrap_err();
            let ConfigError::KdlError(error) = error else {
                panic!("{input}: {error:?}");
            };
            assert!(error.error_message.contains(name), "{input}: {error:?}");
            let offset = error.offset.unwrap();
            let source = &input[offset..offset + error.len.unwrap()];
            assert!(
                source.contains(name) && !source.contains("padding") && !source.contains("opacity"),
                "{source:?}"
            );
            assert!(Config::terminal_settings_from_kdl(&input).is_err());
        }
    }
    let combined = Config::terminal_settings_from_kdl(
        "terminal { underline_offset -2; underline_thickness 1; }",
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        (combined.underline_offset, combined.underline_thickness),
        (Some(-2), Some(1))
    );
    assert!(Config::terminal_settings_from_kdl("terminal {}\nterminal {}").is_err());
    assert!(Config::terminal_settings_from_kdl("terminal { underline_offset").is_err());
}

#[test]
fn cursor_trail_validates_roundtrips_and_overlays() {
    assert_eq!(
        Config::from_kdl("terminal {}", None)
            .unwrap()
            .terminal
            .cursor_trail,
        None
    );
    for enabled in [false, true] {
        let config = Config::from_kdl(
            &format!("terminal {{ cursor_trail {enabled}; padding 2; }}"),
            None,
        )
        .unwrap();
        assert_eq!(config.terminal.cursor_trail, Some(enabled));
        let text = config.to_string(false).unwrap();
        let roundtrip = Config::from_kdl(&text, None).unwrap();
        assert_eq!(roundtrip.terminal, config.terminal);
        let inherited = Config::from_kdl("terminal { opacity 0.5; }", Some(config)).unwrap();
        assert_eq!(inherited.terminal.cursor_trail, Some(enabled));
        let overridden = Config::from_kdl(
            &format!("terminal {{ cursor_trail {}; }}", !enabled),
            Some(inherited),
        )
        .unwrap();
        assert_eq!(overridden.terminal.cursor_trail, Some(!enabled));
        assert_eq!(overridden.terminal.padding, Some(2.0));
    }
    for bad in [
        "cursor_trail",
        "cursor_trail 1",
        "cursor_trail \"true\"",
        "cursor_trail null",
        "cursor_trail (bool)true",
        "(bool)cursor_trail true",
        "cursor_trail true false",
        "cursor_trail value=true",
        "cursor_trail true {}",
        "cursor_trail true; cursor_trail false",
    ] {
        let input = format!("terminal {{ padding 4; {bad}; opacity 0.5; }}");
        let ConfigError::KdlError(error) = Config::from_kdl(&input, None).unwrap_err() else {
            panic!("{input}: expected KdlError");
        };
        assert!(
            error.error_message.contains("cursor_trail"),
            "{input}: {error:?}"
        );
    }
}

const PADDING_KEYS: [&str; 5] = [
    "padding",
    "padding_top",
    "padding_right",
    "padding_bottom",
    "padding_left",
];

#[test]
fn padding_sides_parse_resolve_and_roundtrip_independently() {
    use yinz_types::input::native_terminal::PaddingSides;
    for key in PADDING_KEYS {
        for (literal, expected) in [("0", 0.0), ("3", 3.0), ("2.5", 2.5), ("256", 256.0)] {
            let config =
                Config::from_kdl(&format!("terminal {{ {key} {literal}; }}"), None).unwrap();
            let value = match key {
                "padding" => config.terminal.padding,
                "padding_top" => config.terminal.padding_top,
                "padding_right" => config.terminal.padding_right,
                "padding_bottom" => config.terminal.padding_bottom,
                _ => config.terminal.padding_left,
            };
            assert_eq!(value, Some(expected), "{key} {literal}");
        }
    }
    let config = Config::from_kdl(
        "terminal { padding 4; padding_left 0; padding_top 1.5; padding_balance true; }",
        None,
    )
    .unwrap();
    assert_eq!(
        config.terminal.padding_sides(),
        PaddingSides {
            top: 1.5,
            right: 4.0,
            bottom: 4.0,
            left: 0.0
        }
    );
    assert!(config.terminal.padding_balance_enabled());
    let text = config.to_string(false).unwrap();
    for key in [
        "padding 4",
        "padding_left 0",
        "padding_top 1.5",
        "padding_balance true",
    ] {
        assert!(text.contains(key), "{key} missing from {text}");
    }
    for key in ["padding_right", "padding_bottom"] {
        assert!(!text.contains(key), "{key} should stay unset in {text}");
    }
    let roundtrip = Config::from_kdl(&text, None).unwrap();
    assert_eq!(roundtrip.terminal, config.terminal);
    for balance in ["true", "false"] {
        let config =
            Config::from_kdl(&format!("terminal {{ padding_balance {balance}; }}"), None).unwrap();
        assert_eq!(config.terminal.padding_balance, Some(balance == "true"));
        assert_eq!(
            Config::from_kdl(&config.to_string(false).unwrap(), None)
                .unwrap()
                .terminal,
            config.terminal
        );
    }
}

#[test]
fn padding_overlays_keep_sides_from_any_layer_over_shorthand() {
    use yinz_types::input::native_terminal::PaddingSides;
    // A later shorthand does not clear an earlier side.
    let base = Config::from_kdl("terminal { padding_left 2; }", None).unwrap();
    let merged = Config::from_kdl("terminal { padding 5; }", Some(base)).unwrap();
    assert_eq!(
        merged.terminal.padding_sides(),
        PaddingSides {
            top: 5.0,
            right: 5.0,
            bottom: 5.0,
            left: 2.0
        }
    );
    // A later side overrides only that side of an earlier shorthand.
    let base = Config::from_kdl("terminal { padding 5; padding_balance true; }", None).unwrap();
    let merged = Config::from_kdl("terminal { padding_bottom 1; }", Some(base)).unwrap();
    assert_eq!(
        merged.terminal.padding_sides(),
        PaddingSides {
            top: 5.0,
            right: 5.0,
            bottom: 1.0,
            left: 5.0
        }
    );
    assert!(merged.terminal.padding_balance_enabled());
    let toggled = Config::from_kdl("terminal { padding_balance false; }", Some(merged)).unwrap();
    assert!(!toggled.terminal.padding_balance_enabled());
    assert_eq!(toggled.terminal.padding_bottom, Some(1.0));

    // Drop-in fragments use the same per-field precedence; invalid fragments are skipped.
    let dir = tempfile::tempdir().unwrap();
    for (file, text) in [
        ("10-side.kdl", "terminal { padding_right 7; }"),
        (
            "20-shorthand.kdl",
            "terminal { padding 3; padding_balance true; }",
        ),
        ("30-invalid.kdl", "terminal { padding_top 300; padding 9; }"),
        (
            "40-invalid.kdl",
            "terminal { padding_balance 1; padding 9; }",
        ),
    ] {
        std::fs::write(dir.path().join(file), text).unwrap();
    }
    let base = Config::from_kdl("terminal { padding_top 1; }", None).unwrap();
    let config = base.with_overlay_dir(Some(dir.path()));
    assert_eq!(
        config.terminal.padding_sides(),
        PaddingSides {
            top: 1.0,
            right: 7.0,
            bottom: 3.0,
            left: 3.0
        }
    );
    assert!(config.terminal.padding_balance_enabled());
}

#[test]
fn invalid_padding_values_name_the_setting_at_its_node() {
    let mut cases: Vec<(String, &str, String)> = Vec::new();
    for key in PADDING_KEYS {
        for value in [
            "-1", "-0.5", "256.5", "257", "1000", "1e999", "\"4\"", "(px)4", "true", "n=4", "4 5",
            "4 { x; }",
        ] {
            cases.push((
                format!("terminal {{ opacity 0.5; {key} {value}; }}"),
                key,
                format!("terminal {key} must be a plain number between 0 and 256 logical pixels"),
            ));
        }
        cases.push((
            format!("terminal {{ ({key}){key} 4; }}"),
            key,
            format!("terminal {key} must be a plain number between 0 and 256 logical pixels"),
        ));
        cases.push((
            format!("terminal {{ {key} 4; {key} 4; }}"),
            key,
            format!("duplicate terminal setting {key}"),
        ));
    }
    for value in ["\"true\"", "1", "(flag)true", "n=true", "true false"] {
        cases.push((
            format!("terminal {{ opacity 0.5; padding_balance {value}; }}"),
            "padding_balance",
            "terminal padding_balance must be a plain boolean true or false".to_owned(),
        ));
    }
    cases.push((
        "terminal { padding_balance true; padding_balance true; }".to_owned(),
        "padding_balance",
        "duplicate terminal setting padding_balance".to_owned(),
    ));
    for (text, key, message) in cases {
        let ConfigError::KdlError(error) = Config::from_kdl(&text, None).unwrap_err() else {
            panic!("expected a KDL diagnostic for {text}");
        };
        assert_eq!(error.error_message, message, "{text}");
        let offset = error.offset.unwrap();
        let len = error.len.unwrap();
        let span = &text[offset..offset + len];
        assert!(
            span.contains(key) && !span.starts_with("terminal"),
            "{text}: span {span:?}"
        );
    }
    let error = Config::from_kdl("terminal { padding_x 4; }", None).unwrap_err();
    assert!(
        crate::editor::validation_error(error).contains("padding_x"),
        "unknown spellings stay rejected"
    );
}

#[test]
fn padding_color_validates_roundtrips_and_overlays() {
    use yinz_types::input::native_terminal::PaddingColor;
    let unset = Config::from_kdl("terminal { padding 2; }", None).unwrap();
    assert_eq!(unset.terminal.padding_color, None);
    assert!(!unset.to_string(false).unwrap().contains("padding_color"));
    for (literal, mode, canonical) in [
        ("\"background\"", PaddingColor::Background, "background"),
        ("\"extend\"", PaddingColor::Extend, "extend"),
        ("\"Extend\"", PaddingColor::Extend, "extend"),
        ("\"BACKGROUND\"", PaddingColor::Background, "background"),
    ] {
        let config = Config::from_kdl(
            &format!("terminal {{ padding_color {literal}; padding 2; }}"),
            None,
        )
        .unwrap();
        assert_eq!(config.terminal.padding_color, Some(mode), "{literal}");
        let text = config.to_string(false).unwrap();
        assert!(
            text.contains(&format!("padding_color \"{canonical}\"")),
            "{literal}: {text}"
        );
        let roundtrip = Config::from_kdl(&text, None).unwrap();
        assert_eq!(roundtrip.terminal, config.terminal);
        let inherited = Config::from_kdl("terminal { opacity 0.5; }", Some(config)).unwrap();
        assert_eq!(inherited.terminal.padding_color, Some(mode));
        let other = match mode {
            PaddingColor::Background => PaddingColor::Extend,
            PaddingColor::Extend => PaddingColor::Background,
        };
        let other_literal = match other {
            PaddingColor::Background => "background",
            PaddingColor::Extend => "extend",
        };
        let overridden = Config::from_kdl(
            &format!("terminal {{ padding_color \"{other_literal}\"; }}"),
            Some(inherited),
        )
        .unwrap();
        assert_eq!(overridden.terminal.padding_color, Some(other));
        assert_eq!(overridden.terminal.padding, Some(2.0));
        assert_eq!(overridden.terminal.opacity, Some(0.5));
    }
    for (bad, message) in [
        ("padding_color \"mirror\"", None),
        ("padding_color \"\"", None),
        ("padding_color true", None),
        ("padding_color 1", None),
        ("padding_color null", None),
        ("padding_color", None),
        ("padding_color (s)\"extend\"", None),
        ("(s)padding_color \"extend\"", None),
        ("padding_color \"extend\" \"background\"", None),
        ("padding_color value=\"extend\"", None),
        ("padding_color \"extend\" {}", None),
        (
            "padding_color \"extend\"; padding_color \"extend\"",
            Some("duplicate terminal setting padding_color"),
        ),
    ] {
        let input = format!("terminal {{ padding 4; {bad}; opacity 0.5; }}");
        let ConfigError::KdlError(error) = Config::from_kdl(&input, None).unwrap_err() else {
            panic!("{input}: expected KdlError");
        };
        assert_eq!(
            error.error_message,
            message.unwrap_or("terminal padding_color must be \"background\" or \"extend\""),
            "{input}"
        );
        let offset = error.offset.unwrap();
        let len = error.len.unwrap();
        let span = &input[offset..offset + len];
        assert!(
            span.contains("padding_color") && !span.starts_with("terminal"),
            "{input}: span {span:?}"
        );
    }
    // The duplicate is reported at the second node.
    let input = "terminal { padding_color \"extend\"; padding_color \"background\"; }";
    let ConfigError::KdlError(error) = Config::from_kdl(input, None).unwrap_err() else {
        panic!("expected KdlError");
    };
    assert!(input[error.offset.unwrap()..].starts_with("padding_color \"background\""));
}

#[test]
fn minimum_contrast_validates_roundtrips_and_overlays() {
    let unset = Config::from_kdl("terminal { padding 2; }", None).unwrap();
    assert_eq!(unset.terminal.minimum_contrast, None);
    assert_eq!(unset.terminal.minimum_contrast_ratio(), 1.0);
    assert!(!unset.to_string(false).unwrap().contains("minimum_contrast"));
    for (literal, ratio) in [
        ("1", 1.0),
        ("1.0", 1.0),
        ("1.5", 1.5),
        ("3.1", 3.1),
        ("4.5", 4.5),
        ("7", 7.0),
        ("21", 21.0),
        ("21.0", 21.0),
    ] {
        let config = Config::from_kdl(
            &format!("terminal {{ minimum_contrast {literal}; padding 2; }}"),
            None,
        )
        .unwrap();
        assert_eq!(config.terminal.minimum_contrast, Some(ratio), "{literal}");
        let text = config.to_string(false).unwrap();
        assert!(text.contains("minimum_contrast "), "{literal}: {text}");
        let roundtrip = Config::from_kdl(&text, None).unwrap();
        assert_eq!(roundtrip.terminal, config.terminal, "{literal}: {text}");
        let inherited = Config::from_kdl("terminal { opacity 0.5; }", Some(config)).unwrap();
        assert_eq!(inherited.terminal.minimum_contrast, Some(ratio));
        let overridden =
            Config::from_kdl("terminal { minimum_contrast 7; }", Some(inherited)).unwrap();
        assert_eq!(overridden.terminal.minimum_contrast, Some(7.0));
        assert_eq!(overridden.terminal.padding, Some(2.0));
        assert_eq!(overridden.terminal.opacity, Some(0.5));
    }
    // Exactly representable values serialize as written.
    let text = Config::from_kdl("terminal { minimum_contrast 4.5; }", None)
        .unwrap()
        .to_string(false)
        .unwrap();
    assert!(text.contains("minimum_contrast 4.5"), "{text}");
    let message = "terminal minimum_contrast must be a plain number from 1 to 21 (1 turns it off)";
    for (bad, expected) in [
        ("minimum_contrast 0.9", message),
        ("minimum_contrast 0", message),
        ("minimum_contrast -3", message),
        ("minimum_contrast 22", message),
        ("minimum_contrast 21.5", message),
        ("minimum_contrast 1e400", message),
        ("minimum_contrast \"high\"", message),
        ("minimum_contrast \"4.5\"", message),
        ("minimum_contrast true", message),
        ("minimum_contrast null", message),
        ("minimum_contrast", message),
        ("minimum_contrast (t)4.5", message),
        ("(t)minimum_contrast 4.5", message),
        ("minimum_contrast value=4.5", message),
        ("minimum_contrast 4.5 7", message),
        ("minimum_contrast 4.5 {}", message),
        (
            "minimum_contrast 4.5; minimum_contrast 4.5",
            "duplicate terminal setting minimum_contrast",
        ),
    ] {
        let input = format!("terminal {{ padding 4; {bad}; opacity 0.5; }}");
        let ConfigError::KdlError(error) = Config::from_kdl(&input, None).unwrap_err() else {
            panic!("{input}: expected KdlError");
        };
        assert_eq!(error.error_message, expected, "{input}");
        let offset = error.offset.unwrap();
        let len = error.len.unwrap();
        let span = &input[offset..offset + len];
        assert!(
            span.contains("minimum_contrast") && !span.starts_with("terminal"),
            "{input}: span {span:?}"
        );
    }
    // The duplicate is reported at the second node.
    let input = "terminal { minimum_contrast 4.5; minimum_contrast 7; }";
    let ConfigError::KdlError(error) = Config::from_kdl(input, None).unwrap_err() else {
        panic!("expected KdlError");
    };
    assert!(input[error.offset.unwrap()..].starts_with("minimum_contrast 7"));
    assert!(
        Config::from_kdl("terminal { min_contrast 4.5; }", None).is_err(),
        "unknown spellings stay rejected"
    );
}

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
    assert_eq!(config.terminal.cell_width, Some(1.0));
    assert_eq!(config.terminal.line_height, Some(1.0));
    assert_eq!(config.terminal.baseline_offset, Some(2));
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

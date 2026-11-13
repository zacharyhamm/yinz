use super::*;
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
    assert_eq!(config.terminal.font_family.as_deref(), Some("Iosevka Term"));
    assert_eq!(config.terminal.font_bold, config.terminal.font_family);
    assert_eq!(config.terminal.font_italic, config.terminal.font_family);
    assert_eq!(
        config.terminal.font_bold_italic,
        config.terminal.font_family
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

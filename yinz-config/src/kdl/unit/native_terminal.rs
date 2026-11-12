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
    assert_eq!(config.terminal.font_family.as_deref(), Some("Iosevka Term"));
    assert_eq!(config.terminal.font_bold, config.terminal.font_family);
    assert_eq!(config.terminal.font_italic, config.terminal.font_family);
    assert_eq!(
        config.terminal.font_bold_italic,
        config.terminal.font_family
    );
}

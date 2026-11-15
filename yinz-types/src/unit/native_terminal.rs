use super::*;

#[test]
fn font_spellings_serialize_canonically_and_preserve_blank_resets() {
    for spelling in ["font", "font_family"] {
        for family in ["Iosevka Term", "", "  "] {
            let value = serde_json::json!({spelling: family, "font_bold": family,
                "font_italic": family, "font_bold_italic": family});
            let config: TerminalConfig = serde_json::from_value(value).unwrap();
            assert_eq!(config.font_family.as_deref(), Some(family));
            let serialized = serde_json::to_value(&config).unwrap();
            assert_eq!(serialized["font"], family);
            assert!(serialized.get("font_family").is_none());
            assert_eq!(
                serde_json::from_value::<TerminalConfig>(serialized).unwrap(),
                config
            );
        }
    }
    assert_eq!(TerminalConfig::default().font_family, None);
}

#[test]
fn duplicate_aliases_are_rejected_in_either_order_even_when_equal() {
    for text in [
        r#"{"font":"a","font_family":"a"}"#,
        r#"{"font_family":"b","font":"a"}"#,
        r#"{"font":"a","font_family":"b"}"#,
        r#"{"font_family":"a","font":"a"}"#,
    ] {
        assert!(
            serde_json::from_str::<TerminalConfig>(text).is_err(),
            "{text}"
        );
    }
}

#[test]
fn ligature_spellings_share_one_optional_boolean_and_canonical_output() {
    assert_eq!(TerminalConfig::default().font_ligatures, None);
    for name in ["ligatures", "font_ligatures"] {
        for enabled in [false, true] {
            let config: TerminalConfig =
                serde_json::from_value(serde_json::json!({name: enabled})).unwrap();
            assert_eq!(config.font_ligatures, Some(enabled));
            let value = serde_json::to_value(&config).unwrap();
            assert_eq!(value["ligatures"], enabled);
            assert!(value.get("font_ligatures").is_none());
            assert_eq!(
                serde_json::from_value::<TerminalConfig>(value).unwrap(),
                config
            );
        }
        // Option accepts null as omission in the shared model; KDL requires a boolean.
        assert_eq!(
            serde_json::from_value::<TerminalConfig>(serde_json::json!({name: null}))
                .unwrap()
                .font_ligatures,
            None
        );
        for value in [
            serde_json::json!("true"),
            serde_json::json!(1),
            serde_json::json!([]),
        ] {
            assert!(
                serde_json::from_value::<TerminalConfig>(serde_json::json!({name: value})).is_err()
            );
        }
        for other in ["ligatures", "font_ligatures"] {
            for enabled in [false, true] {
                let text = format!("{{\"{name}\":true,\"{other}\":{enabled}}}");
                assert!(
                    serde_json::from_str::<TerminalConfig>(&text).is_err(),
                    "{text}"
                );
            }
        }
    }
}

#[test]
fn cell_metrics_preserve_omission_and_explicit_defaults_in_roundtrips() {
    let default = TerminalConfig::default();
    assert_eq!((default.cell_width, default.line_height), (None, None));
    for name in ["cell_width", "line_height"] {
        for multiplier in [0.5, 1.0, 1.25, 3.0] {
            let config: TerminalConfig =
                serde_json::from_value(serde_json::json!({name: multiplier})).unwrap();
            let value = serde_json::to_value(&config).unwrap();
            assert_eq!(value[name], multiplier);
            assert_eq!(
                serde_json::from_value::<TerminalConfig>(value).unwrap(),
                config
            );
        }
        for value in ["true", "\"1.25\"", "[]", "{}"] {
            assert!(
                serde_json::from_str::<TerminalConfig>(&format!("{{{name:?}:{value}}}")).is_err()
            );
        }
        for next in [1.0, 1.5] {
            assert!(serde_json::from_str::<TerminalConfig>(&format!(
                "{{{name:?}:1.0,{name:?}:{next}}}"
            ))
            .is_err());
        }
    }
}

#[test]
fn font_features_preserve_omission_empty_and_ordered_lists() {
    assert_eq!(TerminalConfig::default().font_features, None);
    let omitted: TerminalConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(omitted.font_features, None);
    assert_eq!(
        serde_json::from_value::<TerminalConfig>(serde_json::json!({"font_features": null}))
            .unwrap()
            .font_features,
        None
    );
    for list in [
        vec![],
        vec!["ss01"],
        vec!["ss01", "zero", "-calt", "+dlig"],
        vec!["calt", "-calt"],
    ] {
        let config: TerminalConfig =
            serde_json::from_value(serde_json::json!({ "font_features": list })).unwrap();
        let expected: Vec<String> = list.iter().map(|tag| tag.to_string()).collect();
        assert_eq!(config.font_features.as_ref(), Some(&expected));
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["font_features"], serde_json::json!(list));
        assert_eq!(
            serde_json::from_value::<TerminalConfig>(value).unwrap(),
            config
        );
    }
    for value in [
        serde_json::json!("ss01"),
        serde_json::json!(true),
        serde_json::json!([1]),
        serde_json::json!([["ss01"]]),
        serde_json::json!({"ss01": true}),
    ] {
        assert!(serde_json::from_value::<TerminalConfig>(
            serde_json::json!({"font_features": value})
        )
        .is_err());
    }
    assert!(serde_json::from_value::<TerminalConfig>(
        serde_json::json!({"font_feature": ["ss01"]})
    )
    .is_err());
}

#[test]
fn font_feature_entries_follow_the_pinned_syntax() {
    assert_eq!(parse_font_feature("ss01"), Some((*b"ss01", 1)));
    assert_eq!(parse_font_feature("+dlig"), Some((*b"dlig", 1)));
    assert_eq!(parse_font_feature("-calt"), Some((*b"calt", 0)));
    assert_eq!(parse_font_feature("!~#$"), Some((*b"!~#$", 1)));
    for text in [
        "",
        "+",
        "-",
        "ss1",
        "ss011",
        "--calt",
        "+-calt",
        "ss 1",
        "cv01=2",
        "c\u{e9}lt",
        "\u{e9}\u{e9}",
        "ss0\t",
    ] {
        assert_eq!(parse_font_feature(text), None, "{text:?}");
    }
}

#[test]
fn baseline_offset_optional_integer_roundtrips_without_losing_zero() {
    for text in ["{}", r#"{"baseline_offset":null}"#] {
        assert_eq!(
            serde_json::from_str::<TerminalConfig>(text)
                .unwrap()
                .baseline_offset,
            None
        );
    }
    for offset in [-64, -2, 0, 2, 64] {
        let config: TerminalConfig =
            serde_json::from_value(serde_json::json!({"baseline_offset":offset})).unwrap();
        assert_eq!(config.baseline_offset, Some(offset));
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["baseline_offset"], offset);
        assert_eq!(
            serde_json::from_value::<TerminalConfig>(value).unwrap(),
            config
        );
    }
    for bad in [
        "2.0",
        "true",
        "\"2\"",
        "[]",
        "{}",
        "2147483648",
        "-2147483649",
    ] {
        assert!(
            serde_json::from_str::<TerminalConfig>(&format!("{{\"baseline_offset\":{bad}}}"))
                .is_err(),
            "{bad}"
        );
    }
}

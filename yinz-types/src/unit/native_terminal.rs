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

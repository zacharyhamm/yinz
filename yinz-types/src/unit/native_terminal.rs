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

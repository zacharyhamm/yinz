use super::*;

#[test]
fn keys_preserve_json_and_keybind_strings() {
    for (key, json, text) in [
        (
            KeyWithModifier::new(BareKey::Char('\0')),
            r#"{"bare_key":{"Char":"\u0000"},"key_modifiers":[]}"#,
            "\0",
        ),
        (
            KeyWithModifier::new(BareKey::F(u8::MAX)),
            r#"{"bare_key":{"F":255},"key_modifiers":[]}"#,
            "F255",
        ),
        (
            KeyWithModifier::new(BareKey::Left)
                .with_ctrl_modifier()
                .with_shift_modifier(),
            r#"{"bare_key":"Left","key_modifiers":["Ctrl","Shift"]}"#,
            "Ctrl Shift left",
        ),
    ] {
        assert_eq!(serde_json::to_string(&key).unwrap(), json);
        assert_eq!(serde_json::from_str::<KeyWithModifier>(json).unwrap(), key);
        assert_eq!(key.to_keybind_string(), text);
        assert_eq!(text.parse::<KeyWithModifier>().unwrap(), key);
    }
    assert!(serde_json::from_str::<BareKey>(r#"{"F":256}"#).is_err());
    assert!(serde_json::from_str::<BareKey>(r#"{"F":-1}"#).is_err());
}

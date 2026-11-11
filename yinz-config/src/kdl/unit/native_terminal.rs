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

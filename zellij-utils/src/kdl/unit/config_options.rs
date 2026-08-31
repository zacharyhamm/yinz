use super::*;

#[test]
fn firecracker_provider_options_round_trip() {
    let document: KdlDocument = r#"
        firecracker_provider "/opt/bin/zellij-firecracker"
        firecracker_provider_timeout_secs 90
    "#
    .parse()
    .unwrap();
    let options = Options::from_kdl(&document).unwrap();
    assert_eq!(
        options.firecracker_provider,
        Some(PathBuf::from("/opt/bin/zellij-firecracker"))
    );
    assert_eq!(options.firecracker_provider_timeout_secs, Some(90));

    let mut serialized = KdlDocument::new();
    serialized
        .nodes_mut()
        .append(&mut Options::to_kdl(&options, false));
    assert_eq!(Options::from_kdl(&serialized).unwrap(), options);

    let invalid: KdlDocument = "firecracker_provider_timeout_secs 0".parse().unwrap();
    assert!(Options::from_kdl(&invalid).is_err());
    let relative: KdlDocument = "firecracker_provider \"bin/provider\"".parse().unwrap();
    assert!(Options::from_kdl(&relative).is_err());
}

#[test]
fn config_options_to_string() {
    let fake_config = r##"
        simplified_ui true
        theme "dracula"
        default_mode "locked"
        default_shell "fish"
        default_cwd "/tmp/foo"
        default_layout "compact"
        layout_dir "/tmp/layouts"
        builtin_plugins_dir "/tmp/builtin-plugins"
        theme_dir "/tmp/themes"
        user_plugin_dir "/tmp/plugins"
        mouse_mode false
        pane_frames false
        mirror_session true
        on_force_close "quit"
        scroll_buffer_size 100
        copy_command "pbcopy"
        copy_clipboard "system"
        copy_on_select false
        scrollback_editor "vim"
        session_name "my_cool_session"
        attach_to_session false
        auto_layout false
        session_serialization true
        serialize_pane_viewport false
        scrollback_lines_to_serialize 1000
        styled_underlines false
        serialization_interval 1
        disable_session_metadata true
        support_kitty_keyboard_protocol false
    "##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = Options::from_kdl(&document).unwrap();
    let mut serialized = Options::to_kdl(&deserialized, false);
    let mut fake_document = KdlDocument::new();
    fake_document.nodes_mut().append(&mut serialized);
    let deserialized_from_serialized =
        Options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap()).unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(fake_document.to_string());
}

#[test]
fn config_options_to_string_with_comments() {
    let fake_config = r##"
        simplified_ui true
        theme "dracula"
        default_mode "locked"
        default_shell "fish"
        default_cwd "/tmp/foo"
        default_layout "compact"
        layout_dir "/tmp/layouts"
        builtin_plugins_dir "/tmp/builtin-plugins"
        theme_dir "/tmp/themes"
        user_plugin_dir "/tmp/plugins"
        mouse_mode false
        pane_frames false
        mirror_session true
        on_force_close "quit"
        scroll_buffer_size 100
        copy_command "pbcopy"
        copy_clipboard "system"
        copy_on_select false
        scrollback_editor "vim"
        session_name "my_cool_session"
        attach_to_session false
        auto_layout false
        session_serialization true
        serialize_pane_viewport false
        scrollback_lines_to_serialize 1000
        styled_underlines false
        serialization_interval 1
        disable_session_metadata true
        support_kitty_keyboard_protocol false
    "##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = Options::from_kdl(&document).unwrap();
    let mut serialized = Options::to_kdl(&deserialized, true);
    let mut fake_document = KdlDocument::new();
    fake_document.nodes_mut().append(&mut serialized);
    let deserialized_from_serialized =
        Options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap()).unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(fake_document.to_string());
}

#[test]
fn config_options_to_string_without_options() {
    let fake_config = r##"
    "##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = Options::from_kdl(&document).unwrap();
    let mut serialized = Options::to_kdl(&deserialized, false);
    let mut fake_document = KdlDocument::new();
    fake_document.nodes_mut().append(&mut serialized);
    let deserialized_from_serialized =
        Options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap()).unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(fake_document.to_string());
}

#[test]
fn nested_session_handling_kdl_round_trip_for_every_variant() {
    use crate::input::options::NestedSessionHandling;
    let cases = [
        ("ask", NestedSessionHandling::Ask),
        ("fullscreen", NestedSessionHandling::Fullscreen),
        ("descend", NestedSessionHandling::Descend),
        ("never", NestedSessionHandling::Never),
    ];
    for (value, expected) in cases {
        let fake_config = format!(
            r##"
                nested_session_handling "{value}"
            "##
        );
        let document: KdlDocument = fake_config.parse().unwrap();
        let parsed = Options::from_kdl(&document).unwrap();
        assert_eq!(
            parsed.nested_session_handling,
            Some(expected),
            "case: {value}"
        );

        let mut serialized = Options::to_kdl(&parsed, false);
        let mut fake_document = KdlDocument::new();
        fake_document.nodes_mut().append(&mut serialized);
        let reparsed =
            Options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap()).unwrap();
        assert_eq!(parsed, reparsed, "round-trip mismatch for {value}");
    }
}

#[test]
fn host_notification_protocol_kdl_round_trip_for_every_variant() {
    use crate::input::options::HostNotificationProtocol;
    let cases = [
        ("auto", HostNotificationProtocol::Auto),
        ("osc9", HostNotificationProtocol::Osc9),
        ("osc99", HostNotificationProtocol::Osc99),
        ("bell", HostNotificationProtocol::Bell),
        ("off", HostNotificationProtocol::Off),
    ];
    for (value, expected) in cases {
        let fake_config = format!(
            r##"
                host_notification_protocol "{value}"
            "##
        );
        let document: KdlDocument = fake_config.parse().unwrap();
        let parsed = Options::from_kdl(&document).unwrap();
        assert_eq!(
            parsed.host_notification_protocol,
            Some(expected),
            "case: {value}"
        );

        let mut serialized = Options::to_kdl(&parsed, false);
        let mut fake_document = KdlDocument::new();
        fake_document.nodes_mut().append(&mut serialized);
        let reparsed =
            Options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap()).unwrap();
        assert_eq!(parsed, reparsed, "round-trip mismatch for {value}");
    }
}

#[test]
fn an_unknown_host_notification_protocol_is_a_config_error() {
    let fake_config = r##"
        host_notification_protocol "carrier-pigeon"
    "##;
    let document: KdlDocument = fake_config.parse().unwrap();
    assert!(Options::from_kdl(&document).is_err());
}

#[test]
fn an_unset_host_notification_protocol_parses_as_none() {
    let document: KdlDocument = r##"
        simplified_ui true
    "##
    .parse()
    .unwrap();
    let parsed = Options::from_kdl(&document).unwrap();
    assert_eq!(parsed.host_notification_protocol, None);
}

#[test]
fn config_options_to_string_with_some_options() {
    let fake_config = r##"
        default_layout "compact"
    "##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = Options::from_kdl(&document).unwrap();
    let mut serialized = Options::to_kdl(&deserialized, false);
    let mut fake_document = KdlDocument::new();
    fake_document.nodes_mut().append(&mut serialized);
    let deserialized_from_serialized =
        Options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap()).unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(fake_document.to_string());
}

#[test]
fn bare_config_from_default_assets_to_string() {
    let fake_config = Config::from_default_assets().unwrap();
    let fake_config_stringified = fake_config.to_string(false);
    let deserialized_from_serialized = Config::from_kdl(&fake_config_stringified, None).unwrap();
    assert_eq!(
        fake_config, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(fake_config_stringified);
}

#[test]
fn bare_config_from_default_assets_to_string_with_comments() {
    let fake_config = Config::from_default_assets().unwrap();
    let fake_config_stringified = fake_config.to_string(true);
    let deserialized_from_serialized = Config::from_kdl(&fake_config_stringified, None).unwrap();
    assert_eq!(
        fake_config, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(fake_config_stringified);
}

#[test]
fn osc8_hyperlinks_config_parsing() {
    let config_with_osc8_disabled = r#"
        osc8_hyperlinks false
    "#;
    let config = Config::from_kdl(config_with_osc8_disabled, None).unwrap();
    assert_eq!(config.options.osc8_hyperlinks, Some(false));

    let config_with_osc8_enabled = r#"
        osc8_hyperlinks true
    "#;
    let config = Config::from_kdl(config_with_osc8_enabled, None).unwrap();
    assert_eq!(config.options.osc8_hyperlinks, Some(true));

    // Test serialization roundtrip
    let serialized = config.to_string(false);
    let deserialized = Config::from_kdl(&serialized, None).unwrap();
    assert_eq!(deserialized.options.osc8_hyperlinks, Some(true));
}

#[test]
fn session_recording_options_kdl_roundtrip() {
    let fake_config = r#"
        session_recording true
        session_recording_input true
    "#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let options = Options::from_kdl(&document).unwrap();
    assert_eq!(options.session_recording, Some(true));
    assert_eq!(options.session_recording_input, Some(true));

    let serialized = Options::to_kdl(&options, false);
    let mut doc_for_string = KdlDocument::new();
    let mut serialized_clone = serialized.clone();
    doc_for_string.nodes_mut().append(&mut serialized_clone);
    let serialized_str = doc_for_string.to_string();
    assert!(serialized_str.contains("session_recording"));
    assert!(serialized_str.contains("session_recording_input"));

    let mut fake_document = KdlDocument::new();
    let mut serialized_clone2 = serialized.clone();
    fake_document.nodes_mut().append(&mut serialized_clone2);
    let reparsed =
        Options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap()).unwrap();
    assert_eq!(reparsed.session_recording, Some(true));
    assert_eq!(reparsed.session_recording_input, Some(true));
}

#[test]
fn explicit_theme_hue_from_kdl() {
    let fake_config = r##"
        explicit_theme_hue "light"
    "##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = Options::from_kdl(&document).unwrap();
    assert_eq!(deserialized.explicit_theme_hue, Some(ThemeHue::Light));

    let empty_document: KdlDocument = "".parse().unwrap();
    let deserialized_empty = Options::from_kdl(&empty_document).unwrap();
    assert_eq!(
        deserialized_empty.explicit_theme_hue, None,
        "an unspecified explicit_theme_hue leaves the host terminal in charge"
    );
}

#[test]
fn explicit_theme_hue_rejects_unknown_values() {
    let fake_config = r##"
        explicit_theme_hue "sepia"
    "##;
    let document: KdlDocument = fake_config.parse().unwrap();
    assert!(
        Options::from_kdl(&document).is_err(),
        "only 'dark' and 'light' are accepted"
    );
}

#[test]
fn explicit_theme_hue_round_trips_through_kdl() {
    let fake_config = r##"
        explicit_theme_hue "dark"
    "##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = Options::from_kdl(&document).unwrap();
    let mut serialized = Options::to_kdl(&deserialized, false);
    let mut fake_document = KdlDocument::new();
    fake_document.nodes_mut().append(&mut serialized);
    let deserialized_from_serialized =
        Options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap()).unwrap();
    assert_eq!(
        deserialized_from_serialized.explicit_theme_hue,
        Some(ThemeHue::Dark),
        "explicit_theme_hue survives a serialize/parse round trip"
    );
}

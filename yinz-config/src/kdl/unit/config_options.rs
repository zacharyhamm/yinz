use super::*;

#[test]
fn unsigned_options_reject_negative_numbers_and_validate_zero() {
    for option in [
        "scroll_buffer_size",
        "scrollback_lines_to_serialize",
        "notification_timeout",
        "serialization_interval",
        "firecracker_provider_timeout_secs",
        "locked_escape_timeout_ms",
        "agent_pane_min_columns",
        "agent_pane_min_rows",
    ] {
        for invalid in [-1, -i64::MAX] {
            let document = format!("{option} {invalid}").parse().unwrap();
            let error = crate::kdl::options::from_kdl(&document).unwrap_err();
            assert!(
                matches!(&error, ConfigError::KdlError(details) if details.error_message.contains(option)),
                "{error:?}"
            );
        }
        let document = format!("{option} 0").parse().unwrap();
        let requires_positive = matches!(
            option,
            "serialization_interval" | "firecracker_provider_timeout_secs"
        );
        assert_eq!(
            crate::kdl::options::from_kdl(&document).is_err(),
            requires_positive,
            "{option}"
        );
        let document = format!("{option} 1").parse().unwrap();
        crate::kdl::options::from_kdl(&document).unwrap();
    }
}

#[test]
fn firecracker_provider_options_round_trip() {
    let document: KdlDocument = r#"
        firecracker_provider "/opt/bin/yinz-firecracker"
        firecracker_provider_timeout_secs 90
    "#
    .parse()
    .unwrap();
    let options = crate::kdl::options::from_kdl(&document).unwrap();
    assert_eq!(
        options.firecracker_provider,
        Some(PathBuf::from("/opt/bin/yinz-firecracker"))
    );
    assert_eq!(
        options.firecracker_provider_timeout_secs,
        Some(yinz_types::units::ElapsedSeconds::new(90))
    );

    let mut serialized = KdlDocument::new();
    serialized
        .nodes_mut()
        .append(&mut crate::kdl::options::to_kdl(&options, false));
    assert_eq!(crate::kdl::options::from_kdl(&serialized).unwrap(), options);

    let invalid: KdlDocument = "firecracker_provider_timeout_secs 0".parse().unwrap();
    assert!(crate::kdl::options::from_kdl(&invalid).is_err());
    let relative: KdlDocument = "firecracker_provider \"bin/provider\"".parse().unwrap();
    assert!(crate::kdl::options::from_kdl(&relative).is_err());

    // `~` is expanded the way a layout's paths are, so the same spelling works
    // in both places.
    let tilde: KdlDocument = "firecracker_provider \"~/bin/yinz-firecracker\""
        .parse()
        .unwrap();
    let home = directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .expect("a home directory");
    assert_eq!(
        crate::kdl::options::from_kdl(&tilde)
            .unwrap()
            .firecracker_provider,
        Some(home.join("bin/yinz-firecracker"))
    );
}

#[test]
fn locked_escape_timeout_options_round_trip() {
    let document: KdlDocument = "locked_escape_timeout_ms 400".parse().unwrap();
    let options = crate::kdl::options::from_kdl(&document).unwrap();
    assert_eq!(
        options.locked_escape_timeout_ms,
        Some(yinz_types::units::ElapsedMillis::new(400))
    );

    let mut serialized = KdlDocument::new();
    serialized
        .nodes_mut()
        .append(&mut crate::kdl::options::to_kdl(&options, false));
    assert_eq!(crate::kdl::options::from_kdl(&serialized).unwrap(), options);

    // 0 is meaningful: it disables the Ctrl+g prefix rather than being an error.
    let disabled: KdlDocument = "locked_escape_timeout_ms 0".parse().unwrap();
    assert_eq!(
        crate::kdl::options::from_kdl(&disabled)
            .unwrap()
            .locked_escape_timeout_ms,
        Some(yinz_types::units::ElapsedMillis::new(0))
    );

    let negative: KdlDocument = "locked_escape_timeout_ms -1".parse().unwrap();
    assert!(crate::kdl::options::from_kdl(&negative).is_err());
}

#[test]
fn agent_pane_floor_options_round_trip() {
    let document: KdlDocument = r#"
        agent_pane_min_columns 100
        agent_pane_min_rows 0
    "#
    .parse()
    .unwrap();
    let options = crate::kdl::options::from_kdl(&document).unwrap();
    assert_eq!(options.agent_pane_min_columns, Some(100));
    assert_eq!(
        options.agent_pane_min_rows,
        Some(0),
        "zero is how the rule is turned off"
    );

    let mut serialized = KdlDocument::new();
    serialized
        .nodes_mut()
        .append(&mut crate::kdl::options::to_kdl(&options, false));
    assert_eq!(crate::kdl::options::from_kdl(&serialized).unwrap(), options);

    let unset = crate::kdl::options::from_kdl(&KdlDocument::new()).unwrap();
    assert_eq!(unset.agent_pane_min_columns, None);
    assert_eq!(unset.agent_pane_min_rows, None);

    for negative in ["agent_pane_min_columns -1", "agent_pane_min_rows -24"] {
        let document: KdlDocument = negative.parse().unwrap();
        assert!(
            crate::kdl::options::from_kdl(&document).is_err(),
            "{negative}"
        );
    }
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
        locked_escape_timeout_ms 400
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
    let deserialized = crate::kdl::options::from_kdl(&document).unwrap();
    let mut serialized = crate::kdl::options::to_kdl(&deserialized, false);
    let mut fake_document = KdlDocument::new();
    fake_document.nodes_mut().append(&mut serialized);
    let deserialized_from_serialized =
        crate::kdl::options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap())
            .unwrap();
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
        locked_escape_timeout_ms 400
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
    let deserialized = crate::kdl::options::from_kdl(&document).unwrap();
    let mut serialized = crate::kdl::options::to_kdl(&deserialized, true);
    let mut fake_document = KdlDocument::new();
    fake_document.nodes_mut().append(&mut serialized);
    let deserialized_from_serialized =
        crate::kdl::options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap())
            .unwrap();
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
    let deserialized = crate::kdl::options::from_kdl(&document).unwrap();
    let mut serialized = crate::kdl::options::to_kdl(&deserialized, false);
    let mut fake_document = KdlDocument::new();
    fake_document.nodes_mut().append(&mut serialized);
    let deserialized_from_serialized =
        crate::kdl::options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap())
            .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(fake_document.to_string());
}

#[test]
fn nested_session_handling_kdl_round_trip_for_every_variant() {
    use yinz_types::input::options::NestedSessionHandling;
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
        let parsed = crate::kdl::options::from_kdl(&document).unwrap();
        assert_eq!(
            parsed.nested_session_handling,
            Some(expected),
            "case: {value}"
        );

        let mut serialized = crate::kdl::options::to_kdl(&parsed, false);
        let mut fake_document = KdlDocument::new();
        fake_document.nodes_mut().append(&mut serialized);
        let reparsed = crate::kdl::options::from_kdl(
            &fake_document.to_string().parse::<KdlDocument>().unwrap(),
        )
        .unwrap();
        assert_eq!(parsed, reparsed, "round-trip mismatch for {value}");
    }
}

#[test]
fn host_notification_protocol_kdl_round_trip_for_every_variant() {
    use yinz_types::input::options::HostNotificationProtocol;
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
        let parsed = crate::kdl::options::from_kdl(&document).unwrap();
        assert_eq!(
            parsed.host_notification_protocol,
            Some(expected),
            "case: {value}"
        );

        let mut serialized = crate::kdl::options::to_kdl(&parsed, false);
        let mut fake_document = KdlDocument::new();
        fake_document.nodes_mut().append(&mut serialized);
        let reparsed = crate::kdl::options::from_kdl(
            &fake_document.to_string().parse::<KdlDocument>().unwrap(),
        )
        .unwrap();
        assert_eq!(parsed, reparsed, "round-trip mismatch for {value}");
    }
}

#[test]
fn an_unknown_host_notification_protocol_is_a_config_error() {
    let fake_config = r##"
        host_notification_protocol "carrier-pigeon"
    "##;
    let document: KdlDocument = fake_config.parse().unwrap();
    assert!(crate::kdl::options::from_kdl(&document).is_err());
}

#[test]
fn an_unset_host_notification_protocol_parses_as_none() {
    let document: KdlDocument = r##"
        simplified_ui true
    "##
    .parse()
    .unwrap();
    let parsed = crate::kdl::options::from_kdl(&document).unwrap();
    assert_eq!(parsed.host_notification_protocol, None);
}

#[test]
fn config_options_to_string_with_some_options() {
    let fake_config = r##"
        default_layout "compact"
    "##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = crate::kdl::options::from_kdl(&document).unwrap();
    let mut serialized = crate::kdl::options::to_kdl(&deserialized, false);
    let mut fake_document = KdlDocument::new();
    fake_document.nodes_mut().append(&mut serialized);
    let deserialized_from_serialized =
        crate::kdl::options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap())
            .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(fake_document.to_string());
}

#[test]
fn bare_config_from_default_assets_to_string() {
    let fake_config = Config::from_default_assets().unwrap();
    let fake_config_stringified = fake_config.to_string(false).unwrap();
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
    let fake_config_stringified = fake_config.to_string(true).unwrap();
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
    let serialized = config.to_string(false).unwrap();
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
    let options = crate::kdl::options::from_kdl(&document).unwrap();
    assert_eq!(options.session_recording, Some(true));
    assert_eq!(options.session_recording_input, Some(true));

    let serialized = crate::kdl::options::to_kdl(&options, false);
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
        crate::kdl::options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap())
            .unwrap();
    assert_eq!(reparsed.session_recording, Some(true));
    assert_eq!(reparsed.session_recording_input, Some(true));
}

#[test]
fn plugin_data_options_kdl_roundtrip() {
    let fake_config = r#"
        plugin_data_url "postgres://yinz:secret@127.0.0.1:5432/yinz"
        plugin_data_schema "yinz_test"
    "#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let options = crate::kdl::options::from_kdl(&document).unwrap();
    assert_eq!(
        options.plugin_data_url.as_deref(),
        Some("postgres://yinz:secret@127.0.0.1:5432/yinz")
    );
    assert_eq!(options.plugin_data_schema.as_deref(), Some("yinz_test"));

    let serialized = crate::kdl::options::to_kdl(&options, false);
    let mut fake_document = KdlDocument::new();
    let mut serialized_clone = serialized.clone();
    fake_document.nodes_mut().append(&mut serialized_clone);
    let reparsed =
        crate::kdl::options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap())
            .unwrap();
    assert_eq!(reparsed.plugin_data_url, options.plugin_data_url);
    assert_eq!(reparsed.plugin_data_schema, options.plugin_data_schema);

    // Unset, neither is written out - not even as a commented template - so a serialized config
    // never carries a database URL nobody put there.
    let unset = crate::kdl::options::to_kdl(&Options::default(), true);
    let mut doc = KdlDocument::new();
    let mut unset_clone = unset.clone();
    doc.nodes_mut().append(&mut unset_clone);
    assert!(!doc.to_string().contains("plugin_data_url"));
}

#[test]
fn host_id_kdl_roundtrip() {
    let document: KdlDocument = "host_id \"build-box-1\"".parse().unwrap();
    let options = crate::kdl::options::from_kdl(&document).unwrap();
    assert_eq!(
        options.host_id.as_ref().map(|id| id.as_str()),
        Some("build-box-1")
    );

    let serialized = crate::kdl::options::to_kdl(&options, false);
    let mut fake_document = KdlDocument::new();
    let mut serialized_clone = serialized.clone();
    fake_document.nodes_mut().append(&mut serialized_clone);
    let reparsed =
        crate::kdl::options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap())
            .unwrap();
    assert_eq!(reparsed.host_id, options.host_id);

    // Unset, nothing is written out: the generated id in the data dir is the normal case, and a
    // commented template would invite pinning one by hand for no reason.
    let unset = crate::kdl::options::to_kdl(&Options::default(), true);
    let mut doc = KdlDocument::new();
    let mut unset_clone = unset.clone();
    doc.nodes_mut().append(&mut unset_clone);
    assert!(!doc.to_string().contains("host_id"));
}

/// A host id is carried in uids that join their parts with `:`, and lands in an indexed column,
/// so the shapes that would make a uid ambiguous are refused where they are written rather than
/// where they break.
#[test]
fn host_id_rejects_shapes_that_would_break_a_uid() {
    for bad in [
        "host_id \"has:colon\"",
        "host_id \"has space\"",
        "host_id \"has/slash\"",
        "host_id \"\"",
    ] {
        let document: KdlDocument = bad.parse().unwrap();
        assert!(
            crate::kdl::options::from_kdl(&document).is_err(),
            "{bad} should have been rejected"
        );
    }
}

#[test]
fn plugin_data_options_reject_other_databases_and_bad_schema_names() {
    for bad in [
        "plugin_data_url \"sqlite:///tmp/x.db\"",
        "plugin_data_url \"mysql://x/y\"",
        "plugin_data_url \"/var/lib/yinz/plugin-data.db\"",
        "plugin_data_schema \"Public\"",
        "plugin_data_schema \"_private\"",
        "plugin_data_schema \"yinz-data\"",
    ] {
        let document: KdlDocument = bad.parse().unwrap();
        assert!(
            crate::kdl::options::from_kdl(&document).is_err(),
            "{bad} should have been rejected"
        );
    }
}

#[test]
fn explicit_theme_hue_from_kdl() {
    let fake_config = r##"
        explicit_theme_hue "light"
    "##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = crate::kdl::options::from_kdl(&document).unwrap();
    assert_eq!(deserialized.explicit_theme_hue, Some(ThemeHue::Light));

    let empty_document: KdlDocument = "".parse().unwrap();
    let deserialized_empty = crate::kdl::options::from_kdl(&empty_document).unwrap();
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
        crate::kdl::options::from_kdl(&document).is_err(),
        "only 'dark' and 'light' are accepted"
    );
}

#[test]
fn explicit_theme_hue_round_trips_through_kdl() {
    let fake_config = r##"
        explicit_theme_hue "dark"
    "##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = crate::kdl::options::from_kdl(&document).unwrap();
    let mut serialized = crate::kdl::options::to_kdl(&deserialized, false);
    let mut fake_document = KdlDocument::new();
    fake_document.nodes_mut().append(&mut serialized);
    let deserialized_from_serialized =
        crate::kdl::options::from_kdl(&fake_document.to_string().parse::<KdlDocument>().unwrap())
            .unwrap();
    assert_eq!(
        deserialized_from_serialized.explicit_theme_hue,
        Some(ThemeHue::Dark),
        "explicit_theme_hue survives a serialize/parse round trip"
    );
}

#[test]
fn agent_double_escape_options_round_trip() {
    assert_eq!(Options::default().agent_double_escape, None);
    for value in [false, true] {
        let document: KdlDocument = format!("agent_double_escape {value}").parse().unwrap();
        let options = crate::kdl::options::from_kdl(&document).unwrap();
        assert_eq!(options.agent_double_escape, Some(value));
        let mut serialized = KdlDocument::new();
        serialized
            .nodes_mut()
            .append(&mut crate::kdl::options::to_kdl(&options, true));
        let reparsed: KdlDocument = serialized.to_string().parse().unwrap();
        assert_eq!(crate::kdl::options::from_kdl(&reparsed).unwrap(), options);
        assert_eq!(
            options.merge(Options::default()).agent_double_escape,
            Some(value)
        );
        assert_eq!(
            options
                .merge(Options {
                    agent_double_escape: Some(!value),
                    ..Options::default()
                })
                .agent_double_escape,
            Some(!value)
        );
    }
    let invalid: KdlDocument = "agent_double_escape 1".parse().unwrap();
    assert!(crate::kdl::options::from_kdl(&invalid).is_err());
}

#[test]
fn pane_animations_round_trip_and_merge() {
    use crate::kdl::options::{from_kdl, to_kdl};
    let default = Options::default();
    assert!(default.pane_animations.unwrap_or(true));
    for enabled in [false, true] {
        let document: KdlDocument = format!("pane_animations {enabled}").parse().unwrap();
        let options = from_kdl(&document).unwrap();
        assert_eq!(options.pane_animations, Some(enabled));
        for comments in [false, true] {
            let mut serialized = KdlDocument::new();
            serialized
                .nodes_mut()
                .append(&mut to_kdl(&options, comments));
            assert_eq!(
                from_kdl(&serialized.to_string().parse().unwrap()).unwrap(),
                options
            );
        }
        assert_eq!(
            options.merge(default.clone()).pane_animations,
            Some(enabled)
        );
        assert_eq!(
            default.merge(options.clone()).pane_animations,
            Some(enabled)
        );
        let opposite = Options {
            pane_animations: Some(!enabled),
            ..Default::default()
        };
        assert_eq!(options.merge(opposite).pane_animations, Some(!enabled));
    }
    let invalid = "pane_animations 42".parse().unwrap();
    assert!(from_kdl(&invalid).is_err());
}

#[test]
fn legacy_startup_tip_option_is_ignored_like_other_unknown_options() {
    for text in [
        "show_startup_tips true",
        "show_startup_tips false",
        "unknown_option true",
    ] {
        let document: KdlDocument = text.parse().unwrap();
        assert_eq!(
            crate::kdl::options::from_kdl(&document).unwrap(),
            Options::default()
        );
    }
}

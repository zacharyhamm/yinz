use super::*;

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
fn legacy_startup_options_are_ignored_like_other_unknown_options() {
    for text in [
        "show_startup_tips true",
        "show_startup_tips false",
        "show_release_notes true",
        "show_release_notes false",
        "unknown_option true",
    ] {
        let document: KdlDocument = text.parse().unwrap();
        assert_eq!(
            crate::kdl::options::from_kdl(&document).unwrap(),
            Options::default()
        );
    }
}

#[test]
fn global_background_opacity_parsing_merge_and_serialization() {
    for opacity in [0, 85, 100] {
        for literal in [
            opacity.to_string(),
            format!("\"{opacity}\""),
            format!("\"{opacity}%\""),
        ] {
            let document = format!("background_opacity {literal}").parse().unwrap();
            let options = crate::kdl::options::from_kdl(&document).unwrap();
            assert_eq!(options.background_opacity, Some(opacity));
            let merged = Options {
                background_opacity: Some(50),
                ..Default::default()
            }
            .merge(options.clone());
            assert_eq!(merged.background_opacity, Some(opacity));
            let serialized = crate::kdl::options::to_kdl(&merged, false);
            let mut document = KdlDocument::new();
            document.nodes_mut().extend(serialized);
            assert_eq!(
                crate::kdl::options::from_kdl(&document)
                    .unwrap()
                    .background_opacity,
                Some(opacity)
            );
            assert_eq!(
                options.merge(Options::default()).background_opacity,
                Some(opacity)
            );
        }
    }
    for literal in [
        "",
        "-1",
        "101",
        "256",
        "85.5",
        "true",
        "\"\"",
        "\"85.5%\"",
        "\"+85\"",
        "\"101%\"",
        "\"clear\"",
    ] {
        let document = format!("background_opacity {literal}").parse().unwrap();
        assert!(
            crate::kdl::options::from_kdl(&document).is_err(),
            "{literal}"
        );
    }
}

#[test]
fn removed_recording_options_follow_unknown_key_behavior() {
    let document: KdlDocument = "session_recording true\nsession_recording_input true"
        .parse()
        .unwrap();
    let options = crate::kdl::options::from_kdl(&document).unwrap();
    assert_eq!(
        options,
        crate::kdl::options::from_kdl(&KdlDocument::new()).unwrap()
    );
}

#[test]
fn unsigned_options_reject_negative_numbers_and_validate_zero() {
    for option in [
        "scroll_buffer_size",
        "max_agent_width",
        "scrollback_lines_to_serialize",
        "notification_timeout",
        "serialization_interval",
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
        let requires_positive = matches!(option, "serialization_interval");
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
fn agent_theme_mapping_is_validated_and_round_trips() {
    use yinz_types::agent::theme::AgentThemeMapping;
    for (value, expected) in [
        ("semantic", AgentThemeMapping::Semantic),
        ("off", AgentThemeMapping::Off),
    ] {
        let document: KdlDocument = format!("agent_theme_mapping \"{value}\"").parse().unwrap();
        let options = crate::kdl::options::from_kdl(&document).unwrap();
        assert_eq!(options.agent_theme_mapping, Some(expected));
        let mut document = KdlDocument::new();
        document
            .nodes_mut()
            .extend(crate::kdl::options::to_kdl(&options, false));
        assert_eq!(
            crate::kdl::options::from_kdl(&document)
                .unwrap()
                .agent_theme_mapping,
            Some(expected)
        );
    }
    let invalid: KdlDocument = "agent_theme_mapping \"hue\"".parse().unwrap();
    assert!(crate::kdl::options::from_kdl(&invalid).is_err());
}

#[test]
fn max_agent_width_roundtrips_and_preserves_disabled_values_for_precedence() {
    for width in [0, 1, 79, 80, 81] {
        let document = format!("max_agent_width {width}").parse().unwrap();
        let options = crate::kdl::options::from_kdl(&document).unwrap();
        assert_eq!(options.max_agent_width, Some(width));
        let mut serialized = KdlDocument::new();
        serialized
            .nodes_mut()
            .append(&mut crate::kdl::options::to_kdl(&options, false));
        let roundtrip = crate::kdl::options::from_kdl(&serialized).unwrap();
        assert_eq!(roundtrip.max_agent_width, Some(width));
        let base = Options {
            max_agent_width: Some(100),
            ..Options::default()
        };
        assert_eq!(base.merge(options).max_agent_width, Some(width));
    }
}

#[test]
fn agent_mouse_override_round_trip_and_merge() {
    for enabled in [false, true] {
        let document: KdlDocument = format!("context_menu_override_agent_mouse {enabled}")
            .parse()
            .unwrap();
        let options = crate::kdl::options::from_kdl(&document).unwrap();
        assert_eq!(options.context_menu_override_agent_mouse, Some(enabled));
        let mut serialized = KdlDocument::new();
        serialized
            .nodes_mut()
            .append(&mut crate::kdl::options::to_kdl(&options, false));
        assert_eq!(crate::kdl::options::from_kdl(&serialized).unwrap(), options);
        let base = Options {
            context_menu_override_agent_mouse: Some(!enabled),
            ..Default::default()
        };
        assert_eq!(
            base.merge(options.clone())
                .context_menu_override_agent_mouse,
            Some(enabled)
        );
        assert_eq!(
            options
                .merge(Options::default())
                .context_menu_override_agent_mouse,
            Some(enabled)
        );
    }
}

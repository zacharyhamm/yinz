use super::*;

#[test]
fn max_agent_width_cli_round_trip_and_precedence() {
    let config = Options {
        max_agent_width: Some(100),
        ..Options::default()
    };
    for value in [None, Some(0), Some(1), Some(79), Some(80), Some(81)] {
        let options = Options {
            max_agent_width: value,
            ..Options::default()
        };
        assert_eq!(Options::from(CliOptions::from(options.clone())), options);
        assert_eq!(
            merge_from_cli(&config, options).max_agent_width,
            value.or(Some(100))
        );
    }
}

#[test]
fn agent_mouse_override_cli_round_trip_and_precedence() {
    let config = Options {
        context_menu_override_agent_mouse: Some(true),
        ..Default::default()
    };
    for value in [None, Some(false), Some(true)] {
        let options = Options {
            context_menu_override_agent_mouse: value,
            ..Default::default()
        };
        assert_eq!(Options::from(CliOptions::from(options.clone())), options);
        assert_eq!(
            merge_from_cli(&config, options).context_menu_override_agent_mouse,
            value.or(Some(true))
        );
    }
}

#[test]
fn agent_double_escape_cli_round_trip() {
    for value in [None, Some(false), Some(true)] {
        let options = Options {
            agent_double_escape: value,
            ..Options::default()
        };
        assert_eq!(Options::from(CliOptions::from(options.clone())), options);
        assert_eq!(
            merge_from_cli(&Options::default(), options).agent_double_escape,
            value
        );
    }
}
#[test]
fn a_host_notification_protocol_unset_everywhere_stays_unset() {
    assert_eq!(
        Options::default()
            .merge(Options::default())
            .host_notification_protocol,
        None
    );
    assert_eq!(
        merge_from_cli(&Options::default(), Options::default()).host_notification_protocol,
        None
    );
}
#[test]
fn a_host_notification_protocol_given_on_the_command_line_wins() {
    let config = Options {
        host_notification_protocol: Some(HostNotificationProtocol::Osc9),
        ..Default::default()
    };
    let cli = Options {
        host_notification_protocol: Some(HostNotificationProtocol::Off),
        ..Default::default()
    };
    assert_eq!(
        merge_from_cli(&config, cli).host_notification_protocol,
        Some(HostNotificationProtocol::Off)
    );
}
#[test]
fn a_host_notification_protocol_absent_from_the_command_line_keeps_the_configured_one() {
    let config = Options {
        host_notification_protocol: Some(HostNotificationProtocol::Osc99),
        ..Default::default()
    };
    assert_eq!(
        merge_from_cli(&config, Options::default()).host_notification_protocol,
        Some(HostNotificationProtocol::Osc99),
    );
}

#[test]
fn tab_groups_cli_value_overrides_instead_of_toggling_config() {
    let base = Options {
        tab_groups: Some(true),
        ..Default::default()
    };
    for enabled in [false, true] {
        let override_options = Options {
            tab_groups: Some(enabled),
            ..Default::default()
        };
        assert_eq!(
            merge_from_cli(&base, override_options).tab_groups,
            Some(enabled)
        );
    }
    assert_eq!(
        merge_from_cli(&base, Options::default()).tab_groups,
        Some(true)
    );
}

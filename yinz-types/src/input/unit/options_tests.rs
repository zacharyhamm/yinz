use super::*;

#[test]
fn pane_frame_style_from_str_accepts_all_variants() {
    assert_eq!(
        "full".parse::<PaneFrameStyle>().unwrap(),
        PaneFrameStyle::Full
    );
    assert_eq!(
        "titles".parse::<PaneFrameStyle>().unwrap(),
        PaneFrameStyle::Titles
    );
    assert_eq!(
        "none".parse::<PaneFrameStyle>().unwrap(),
        PaneFrameStyle::None
    );
    assert_eq!(
        "NONE".parse::<PaneFrameStyle>().unwrap(),
        PaneFrameStyle::None
    );
    assert!("bogus".parse::<PaneFrameStyle>().is_err());
}

#[test]
fn host_notification_protocol_from_str_accepts_all_variants() {
    assert_eq!(
        "auto".parse::<HostNotificationProtocol>().unwrap(),
        HostNotificationProtocol::Auto
    );
    assert_eq!(
        "osc9".parse::<HostNotificationProtocol>().unwrap(),
        HostNotificationProtocol::Osc9
    );
    assert_eq!(
        "osc99".parse::<HostNotificationProtocol>().unwrap(),
        HostNotificationProtocol::Osc99
    );
    assert_eq!(
        "bell".parse::<HostNotificationProtocol>().unwrap(),
        HostNotificationProtocol::Bell
    );
    assert_eq!(
        "off".parse::<HostNotificationProtocol>().unwrap(),
        HostNotificationProtocol::Off
    );
    assert!("bogus".parse::<HostNotificationProtocol>().is_err());
}

#[test]
fn every_host_notification_protocol_variant_stringifies_back_to_itself() {
    for variant in [
        HostNotificationProtocol::Auto,
        HostNotificationProtocol::Osc9,
        HostNotificationProtocol::Osc99,
        HostNotificationProtocol::Bell,
        HostNotificationProtocol::Off,
    ] {
        assert_eq!(
            variant
                .as_str()
                .parse::<HostNotificationProtocol>()
                .unwrap(),
            variant
        );
    }
}

#[test]
fn the_host_notification_protocol_defaults_to_auto() {
    assert_eq!(
        HostNotificationProtocol::default(),
        HostNotificationProtocol::Auto
    );
}

#[test]
fn a_configured_host_notification_protocol_is_overridden_by_the_merged_in_one() {
    let config = Options {
        host_notification_protocol: Some(HostNotificationProtocol::Osc9),
        ..Default::default()
    };
    let layout = Options {
        host_notification_protocol: Some(HostNotificationProtocol::Bell),
        ..Default::default()
    };
    assert_eq!(
        config.merge(layout).host_notification_protocol,
        Some(HostNotificationProtocol::Bell)
    );
}

#[test]
fn an_unset_host_notification_protocol_does_not_clobber_the_configured_one() {
    let config = Options {
        host_notification_protocol: Some(HostNotificationProtocol::Osc9),
        ..Default::default()
    };
    assert_eq!(
        config.merge(Options::default()).host_notification_protocol,
        Some(HostNotificationProtocol::Osc9)
    );
}

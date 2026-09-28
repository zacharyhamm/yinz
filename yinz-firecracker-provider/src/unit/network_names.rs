use super::*;

/// `network setup` and a teardown without a recorded config must name the same interfaces:
/// teardown's errors are discarded, so a disagreement would leak the bridge, TAPs and firewall
/// chain setup made without a word.
#[test]
fn setup_without_options_describes_the_network_teardown_falls_back_to() {
    for uid in [0, 1000, u32::from(u16::MAX)] {
        let setup = setup_network_config(uid, &strings(&["--user", "someone"])).unwrap();
        assert_eq!(setup, default_network_config(uid).unwrap(), "uid {uid}");
        validate_network_config(&setup).unwrap();
    }
}

/// Options change the subnet and the pool, never the names.
#[test]
fn setup_options_leave_the_uid_derived_names_alone() {
    let setup = setup_network_config(
        1000,
        &strings(&[
            "--subnet",
            "172.31.0.0/24",
            "--taps",
            "4",
            "--dns",
            "9.9.9.9",
        ]),
    )
    .unwrap();
    let default = default_network_config(1000).unwrap();

    assert_eq!(setup.bridge, default.bridge);
    assert_eq!(setup.tap_prefix, default.tap_prefix);
    assert_eq!(firewall_chain(&setup), firewall_chain(&default));
    assert_eq!(setup.subnet, "172.31.0.0/24");
    assert_eq!(setup.taps, 4);
    assert_eq!(setup.dns, Ipv4Addr::new(9, 9, 9, 9));
}

/// The names installed hosts already carry; changing any of them strands those hosts.
#[test]
fn uid_derived_names_are_unchanged() {
    let config = default_network_config(1000).unwrap();

    assert_eq!(config.bridge, "yfc1000");
    assert_eq!(config.tap_prefix, "yft1000-");
    assert_eq!(config.tap(0), "yft1000-00");
    assert_eq!(config.tap(15), "yft1000-15");
    assert_eq!(config.dns, "1.1.1.1".parse::<Ipv4Addr>().unwrap());
    assert_eq!(config.taps, 16);
    assert_eq!(firewall_chain(&config), "YFC_YFC1000");
    assert_eq!(network_unit(1000), "yinz-firecracker-network@1000.service");
}

/// A UID above 65535 has no derived subnet, so it has no default config, but setup still
/// names its interfaces from the UID once given an explicit subnet.
#[test]
fn a_uid_above_65535_is_named_from_the_uid_given_an_explicit_subnet() {
    let uid = 100_000;
    assert!(default_network_config(uid).is_err());
    assert!(setup_network_config(uid, &[]).is_err());

    let setup = setup_network_config(uid, &strings(&["--subnet", "10.200.0.0/24"])).unwrap();

    assert_eq!(setup.bridge, "yfc100000");
    assert_eq!(setup.tap_prefix, "yft100000-");
    assert_eq!(setup.tap(2), "yft100000-02");
    assert_eq!(setup.taps, DEFAULT_TAPS);
    assert_eq!(setup.dns, DEFAULT_DNS);
    assert_eq!(
        setup,
        NetworkConfig::for_uid(uid, "10.200.0.0/24".to_owned(), DEFAULT_TAPS, DEFAULT_DNS)
    );
    validate_network_config(&setup).unwrap();
}

/// The network help states the defaults setup falls back to; it spells them as text, so it must
/// follow the constants.
#[test]
fn network_help_states_the_setup_defaults() {
    let help = cli_help(&strings(&["network", "--help"])).unwrap();

    assert!(
        help.contains(&format!("(default: {DEFAULT_DNS})")),
        "{help}"
    );
    assert!(
        help.contains(&format!("(default: {DEFAULT_TAPS})")),
        "{help}"
    );
}

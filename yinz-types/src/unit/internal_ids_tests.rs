use super::*;

#[test]
fn supervisor_identity_keeps_handoff_strings_and_map_keys() {
    for (value, json) in [("", "\"\""), ("abcdef012345", "\"abcdef012345\"")] {
        let id = SupervisorId::new(value);
        assert_eq!(serde_json::to_string(&id).unwrap(), json);
        assert_eq!(serde_json::from_str::<SupervisorId>(json).unwrap(), id);
    }
    let map = std::collections::BTreeMap::from([(SupervisorId::new("abcdef012345"), "pane")]);
    assert_eq!(
        serde_json::to_string(&map).unwrap(),
        r#"{"abcdef012345":"pane"}"#
    );
}

#[test]
fn host_identity_keeps_legacy_strings_and_map_keys() {
    for (value, json) in [
        ("", "\"\""),
        ("build-box-1", "\"build-box-1\""),
        ("λ", "\"λ\""),
    ] {
        let id = HostId::new(value);
        assert_eq!(id.is_empty(), value.is_empty());
        assert_eq!(serde_json::to_string(&id).unwrap(), json);
        assert_eq!(serde_json::from_str::<HostId>(json).unwrap(), id);
        assert_eq!(
            crate::plugin_data::DataValue::from(&id),
            crate::plugin_data::DataValue::Text(value.to_owned())
        );
    }
    let map = std::collections::BTreeMap::from([(HostId::new("host-a"), "main")]);
    assert_eq!(serde_json::to_string(&map).unwrap(), r#"{"host-a":"main"}"#);
}

#[test]
fn process_ids_check_narrowing_and_preserve_legacy_signed_responses() {
    use crate::data::GetPanePidResponse;
    for (raw, signed) in [
        (0, 0),
        (2147483647, 2147483647),
        (2147483648, -2147483648),
        (4294967295, -1),
    ] {
        let pid = ProcessId::new(raw);
        assert_eq!(serde_json::to_string(&pid).unwrap(), raw.to_string());
        assert_eq!(
            serde_json::from_str::<ProcessId>(&raw.to_string()).unwrap(),
            pid
        );
        assert_eq!(pid.legacy_signed_bits(), signed);
        assert_eq!(ProcessId::from_legacy_signed_bits(signed), pid);
        let response = GetPanePidResponse::Ok(pid);
        let json = format!("{{\"Ok\":{signed}}}");
        assert_eq!(serde_json::to_string(&response).unwrap(), json);
        assert_eq!(
            serde_json::from_str::<GetPanePidResponse>(&json).unwrap(),
            response
        );
        assert_eq!(i32::try_from(pid).is_ok(), raw <= i32::MAX as u32);
    }
    assert!(ProcessId::try_from(-1_i32).is_err());
    assert!(ProcessId::try_from(4294967296_u64).is_err());
}

#[test]
fn asynchronous_ids_keep_numeric_values_and_map_keys() {
    type Ids = (
        DataWatchId,
        NotificationId,
        ContextMenuRequestId,
        ContextMenuItemId,
    );
    let json = "[0,7,8,9]";
    let ids: Ids = serde_json::from_str(json).unwrap();
    assert_eq!(
        ids,
        (
            DataWatchId::new(0),
            NotificationId::new(7),
            ContextMenuRequestId::new(8),
            ContextMenuItemId::new(9)
        )
    );
    assert_eq!(serde_json::to_string(&ids).unwrap(), json);
    let json = r#"{"0":"first","18446744073709551615":"last"}"#;
    let ids: std::collections::BTreeMap<DataWatchId, String> = serde_json::from_str(json).unwrap();
    assert_eq!(ids[&DataWatchId::new(u64::MAX)], "last");
    assert_eq!(serde_json::to_string(&ids).unwrap(), json);
    assert!(DataWatchId::try_from(-1_i64).is_err());
    assert!(serde_json::from_str::<ContextMenuRequestId>("18446744073709551616").is_err());
}

#[test]
fn tab_identities_preserve_values_and_check_conversion() {
    let id = TabId::new(42);
    assert_eq!(serde_json::to_string(&id).unwrap(), "42");
    assert_eq!(serde_json::from_str::<TabId>("42").unwrap(), id);
    let keyed = std::collections::BTreeMap::from([(id, "work")]);
    assert_eq!(serde_json::to_string(&keyed).unwrap(), r#"{"42":"work"}"#);
    assert!(TabId::try_from(-1_i64).is_err());
    assert_eq!(TabNumber::new(0).position(), TabPosition::new(0));
    assert_eq!(TabNumber::new(1).position(), TabPosition::new(0));
    assert_eq!(
        TabNumber::try_from(TabPosition::new(3)),
        Ok(TabNumber::new(4))
    );
    assert!(TabNumber::try_from(TabPosition::new(usize::MAX)).is_err());
}

#[test]
fn ids_display_as_plain_numbers() {
    for id in [0, u32::MAX] {
        assert_eq!(TerminalId::new(id).to_string(), id.to_string());
        assert_eq!(PluginId::new(id).to_string(), id.to_string());
    }
    for id in [0, u32::MAX] {
        assert_eq!(ClientId::new(id).to_string(), id.to_string());
    }
}

#[test]
fn client_ids_check_narrowing_and_preserve_map_keys() {
    use std::collections::BTreeMap;

    for value in [0, 1, 65_536, u32::MAX] {
        let client = ClientId::new(value);
        assert_eq!(ClientId::from(value), client);
        assert_eq!(ClientId::try_from(i64::from(value)), Ok(client));
        assert_eq!(ClientId::try_from(u64::from(value)), Ok(client));
        assert_eq!(value.to_string().parse::<ClientId>(), Ok(client));
        let json = format!("{{\"{value}\":\"client\"}}");
        let map = BTreeMap::from([(client, "client".to_owned())]);
        assert_eq!(serde_json::to_string(&map).unwrap(), json);
        assert_eq!(
            serde_json::from_str::<BTreeMap<ClientId, String>>(&json).unwrap(),
            map
        );
    }
    assert!(ClientId::try_from(-1_i64).is_err());
    assert!(ClientId::try_from(i64::MAX).is_err());
    assert!(ClientId::try_from(u64::from(u32::MAX) + 1).is_err());
    assert!("4294967296".parse::<ClientId>().is_err());
    assert_eq!(ClientId::default(), ClientId::NONE);
}
#[test]
fn ids_preserve_json_numbers_and_map_keys() {
    use std::collections::BTreeMap;
    for value in [0, u32::MAX] {
        let terminal = TerminalId::new(value);
        assert_eq!(terminal.get(), value);
        let json = value.to_string();
        assert_eq!(serde_json::to_string(&terminal).unwrap(), json);
        assert_eq!(serde_json::from_str::<TerminalId>(&json).unwrap(), terminal);
        let plugin = PluginId::new(value);
        assert_eq!(plugin.get(), value);
        assert_eq!(serde_json::to_string(&plugin).unwrap(), json);
        assert_eq!(serde_json::from_str::<PluginId>(&json).unwrap(), plugin);
        let keys = BTreeMap::from([(terminal, "pane")]);
        let json = format!("{{\"{value}\":\"pane\"}}");
        assert_eq!(serde_json::to_string(&keys).unwrap(), json);
        assert_eq!(
            serde_json::from_str::<BTreeMap<TerminalId, String>>(&json).unwrap()[&terminal],
            "pane"
        );
    }
    for value in [0, 65_536, u32::MAX] {
        let client = ClientId::new(value);
        assert_eq!(client.get(), value);
        let json = value.to_string();
        assert_eq!(serde_json::to_string(&client).unwrap(), json);
        assert_eq!(serde_json::from_str::<ClientId>(&json).unwrap(), client);
    }
    assert!(serde_json::from_str::<ClientId>("4294967296").is_err());
    assert!(serde_json::from_str::<TerminalId>("4294967296").is_err());
    assert!(serde_json::from_str::<PluginId>("-1").is_err());
}

use super::*;

#[test]
fn pane_ids_preserve_json_display_and_parsing() {
    assert_eq!(PaneId::default(), PaneId::Terminal(TerminalId::new(0)));
    for value in [0, 1, u32::MAX] {
        for (id, display, json) in [
            (
                PaneId::Terminal(TerminalId::new(value)),
                format!("terminal_{value}"),
                format!("{{\"Terminal\":{value}}}"),
            ),
            (
                PaneId::Plugin(PluginId::new(value)),
                format!("plugin_{value}"),
                format!("{{\"Plugin\":{value}}}"),
            ),
        ] {
            assert_eq!(id.to_string(), display);
            assert_eq!(display.parse::<PaneId>().unwrap(), id);
            assert_eq!(serde_json::to_string(&id).unwrap(), json);
            assert_eq!(serde_json::from_str::<PaneId>(&json).unwrap(), id);
        }
        assert_eq!(
            value.to_string().parse::<PaneId>().unwrap(),
            PaneId::Terminal(TerminalId::new(value))
        );
    }
    for invalid in [
        "",
        "-1",
        "terminal_-1",
        "plugin_4294967296",
        "4294967296",
        "pane_0",
    ] {
        assert!(invalid.parse::<PaneId>().is_err(), "{invalid}");
    }
    for invalid in [r#"{"Terminal":-1}"#, r#"{"Plugin":4294967296}"#] {
        assert!(serde_json::from_str::<PaneId>(invalid).is_err());
    }
}

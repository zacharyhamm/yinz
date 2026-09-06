use zellij_config::config::Config;
use zellij_types::{
    data::InputMode,
    keys::{BareKey, KeyWithModifier},
};
fn new_tab_config(metadata: &str, layout_dir: &std::path::Path) -> Config {
    Config::from_kdl(
        &format!(
            r#"
            layout_dir {:?}
            default_layout "custom"
            keybinds clear-defaults=true {{
                normal {{
                    bind "Ctrl t" {{ NewTab {metadata}; SwitchToMode "Normal"; }}
                }}
            }}
            "#,
            layout_dir.display().to_string(),
        ),
        None,
    )
    .unwrap()
}
#[test]
fn new_tab_without_swap_layouts_can_be_saved_after_ipc() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("custom.kdl"), "layout { pane; }\n").unwrap();
    let mut config = new_tab_config(r#"{ layout "custom"; }"#, dir.path());
    let binding = config
        .keybinds
        .0
        .get_mut(&InputMode::Normal)
        .unwrap()
        .get_mut(&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier())
        .unwrap();
    // The plugin action wire, like IPC, represents absent swap lists as empty lists.
    let wire: zellij_plugin_api::action::ProtobufAction = binding[0].clone().try_into().unwrap();
    binding[0] = wire.try_into().unwrap();
    config
        .write_config_to_disk(config.to_string(false), &dir.path().join("config.kdl"))
        .unwrap();
}
#[test]
fn new_tab_with_pane_colours_can_be_saved_after_ipc() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("custom.kdl"),
        r##"
layout {
    pane default_fg="#ff0000" default_bg="#000000" { pane; pane default_fg="#00ff00"; }
    floating_panes {
        pane default_fg="#0000ff" default_bg="#ffffff"
    }
    swap_tiled_layout name="columns" {
        tab max_panes=3 { pane default_bg="#101010"; pane; pane; }
    }
    swap_floating_layout name="floats" {
        floating_panes max_panes=2 { pane default_fg="#202020"; pane; }
    }
}
"##,
    )
    .unwrap();
    for metadata in ["{}", r#"{ layout "custom"; }"#] {
        let mut config = new_tab_config(metadata, dir.path());
        let binding = config
            .keybinds
            .0
            .get_mut(&InputMode::Normal)
            .unwrap()
            .get_mut(&KeyWithModifier::new(BareKey::Char('t')).with_ctrl_modifier())
            .unwrap();
        assert!(format!("{:?}", binding[0]).contains("default_fg: Some("));
        // The plugin action wire carries no pane colours.
        let wire: zellij_plugin_api::action::ProtobufAction =
            binding[0].clone().try_into().unwrap();
        binding[0] = wire.try_into().unwrap();
        assert!(!format!("{:?}", binding[0]).contains("default_fg: Some("));
        config
            .write_config_to_disk(config.to_string(false), &dir.path().join("config.kdl"))
            .unwrap_or_else(|e| panic!("{metadata}: {e:?}"));
    }
}

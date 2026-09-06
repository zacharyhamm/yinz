use super::*;

#[test]
fn plugins_to_string() {
    let fake_config = r##"
        plugins {
            tab-bar location="zellij:tab-bar"
            status-bar location="zellij:status-bar"
            strider location="zellij:strider"
            compact-bar location="zellij:compact-bar"
            session-manager location="zellij:session-manager"
            welcome-screen location="zellij:session-manager" {
                welcome_screen true
            }
            filepicker location="zellij:strider" {
                cwd "/"
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = PluginAliases::from_kdl(document.get("plugins").unwrap()).unwrap();
    let serialized = PluginAliases::to_kdl(&deserialized, true);
    let deserialized_from_serialized = PluginAliases::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("plugins")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn plugins_to_string_with_file_and_web() {
    let fake_config = r##"
        plugins {
            tab-bar location="https://foo.com/plugin.wasm"
            filepicker location="file:/path/to/my/plugin.wasm" {
                cwd "/"
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = PluginAliases::from_kdl(document.get("plugins").unwrap()).unwrap();
    let serialized = PluginAliases::to_kdl(&deserialized, true);
    let deserialized_from_serialized = PluginAliases::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("plugins")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn ui_config_to_string() {
    let fake_config = r##"
        ui {
            pane_frames {
                rounded_corners true
                hide_session_name true
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = UiConfig::from_kdl(document.get("ui").unwrap()).unwrap();
    let serialized = UiConfig::to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = UiConfig::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("ui")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn ui_config_to_string_with_no_ui_config() {
    let fake_config = r##"
        ui {
            pane_frames {
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = UiConfig::from_kdl(document.get("ui").unwrap()).unwrap();
    assert_eq!(UiConfig::to_kdl(&deserialized), None);
}

#[test]
fn env_vars_to_string() {
    let fake_config = r##"
        env {
            foo "bar"
            bar "foo"
            thing 1
            baz "true"
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = environment_variables_from_kdl(document.get("env").unwrap()).unwrap();
    let serialized = environment_variables_to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = environment_variables_from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("env")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn env_vars_to_string_with_no_env_vars() {
    let fake_config = r##"
        env {
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let deserialized = environment_variables_from_kdl(document.get("env").unwrap()).unwrap();
    assert_eq!(environment_variables_to_kdl(&deserialized), None);
}

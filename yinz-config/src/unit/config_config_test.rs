use super::*;
use yinz_types::input::actions::Action;
use yinz_types::input::hooks::HookPoint;
use {
    yinz_types::data::InputMode, yinz_types::data::Palette, yinz_types::data::PaletteColor,
    yinz_types::data::StyleDeclaration, yinz_types::data::Styling,
};

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use tempfile::tempdir;
use {
    crate::theme::FrameConfig, crate::theme::Theme, crate::theme::Themes, crate::theme::UiConfig,
};
use {yinz_types::input::options::Clipboard, yinz_types::input::options::OnForceClose};

#[test]
fn explicit_config_must_exist() {
    assert!(Config::load(Some(Path::new("nonexistent.yaml")), None, false).is_err());
}
#[test]
fn clean_skips_an_explicit_missing_config() {
    assert!(Config::load(Some(Path::new("nonexistent.yaml")), None, true).is_ok());
}
#[test]
fn discovery_rejects_an_invalid_config() {
    let tmp = tempdir().unwrap();
    File::create(tmp.path().join(DEFAULT_CONFIG_FILE_NAME))
        .unwrap()
        .write_all(b"keybinds: invalid\n")
        .unwrap();
    assert!(Config::load(None, Some(tmp.path()), false).is_err());
}
#[test]
fn empty_and_missing_config_directories_use_defaults() {
    let tmp = tempdir().unwrap();
    for dir in [tmp.path().to_path_buf(), tmp.path().join("missing")] {
        assert_eq!(
            Config::load(None, Some(&dir), false).unwrap(),
            Config::from_default_assets().unwrap()
        );
    }
}

#[test]
fn can_define_options_in_configfile() {
    let config_contents = r#"
            simplified_ui true
            theme "my cool theme"
            default_mode "locked"
            default_shell "/path/to/my/shell"
            default_cwd "/path"
            default_layout "/path/to/my/layout.kdl"
            layout_dir "/path/to/my/layout-dir"
            theme_dir "/path/to/my/theme-dir"
            mouse_mode false
            pane_frames false
            mirror_session true
            on_force_close "quit"
            scroll_buffer_size 100000
            copy_command "/path/to/my/copy-command"
            copy_clipboard "primary"
            copy_on_select false
            scrollback_editor "/path/to/my/scrollback-editor"
            session_name "my awesome session"
            attach_to_session true
        "#;
    let config = Config::from_kdl(config_contents, None).unwrap();
    assert_eq!(
        config.options.simplified_ui,
        Some(true),
        "Option set in config"
    );
    assert_eq!(
        config.options.theme,
        Some(String::from("my cool theme")),
        "Option set in config"
    );
    assert_eq!(
        config.options.default_mode,
        Some(InputMode::Locked),
        "Option set in config"
    );
    assert_eq!(
        config.options.default_shell,
        Some(PathBuf::from("/path/to/my/shell")),
        "Option set in config"
    );
    assert_eq!(
        config.options.default_cwd,
        Some(PathBuf::from("/path")),
        "Option set in config"
    );
    assert_eq!(
        config.options.default_layout,
        Some(PathBuf::from("/path/to/my/layout.kdl")),
        "Option set in config"
    );
    assert_eq!(
        config.options.layout_dir,
        Some(PathBuf::from("/path/to/my/layout-dir")),
        "Option set in config"
    );
    assert_eq!(
        config.options.theme_dir,
        Some(PathBuf::from("/path/to/my/theme-dir")),
        "Option set in config"
    );
    assert_eq!(
        config.options.mouse_mode,
        Some(false),
        "Option set in config"
    );
    assert_eq!(
        config.options.pane_frames,
        Some(false),
        "Option set in config"
    );
    assert_eq!(
        config.options.mirror_session,
        Some(true),
        "Option set in config"
    );
    assert_eq!(
        config.options.on_force_close,
        Some(OnForceClose::Quit),
        "Option set in config"
    );
    assert_eq!(
        config.options.scroll_buffer_size,
        Some(100000),
        "Option set in config"
    );
    assert_eq!(
        config.options.copy_command,
        Some(String::from("/path/to/my/copy-command")),
        "Option set in config"
    );
    assert_eq!(
        config.options.copy_clipboard,
        Some(Clipboard::Primary),
        "Option set in config"
    );
    assert_eq!(
        config.options.copy_on_select,
        Some(false),
        "Option set in config"
    );
    assert_eq!(
        config.options.scrollback_editor,
        Some(PathBuf::from("/path/to/my/scrollback-editor")),
        "Option set in config"
    );
    assert_eq!(
        config.options.session_name,
        Some(Into::into(String::from("my awesome session"))),
        "Option set in config"
    );
    assert_eq!(
        config.options.attach_to_session,
        Some(true),
        "Option set in config"
    );
}

#[test]
fn can_define_themes_in_configfile() {
    let config_contents = r#"
            themes {
                dracula {
                    fg 248 248 242
                    bg 40 42 54
                    red 255 85 85
                    green 80 250 123
                    yellow 241 250 140
                    blue 98 114 164
                    magenta 255 121 198
                    orange 255 184 108
                    cyan 139 233 253
                    black 0 0 0
                    white 255 255 255
                }
            }
        "#;
    let config = Config::from_kdl(config_contents, None).unwrap();
    let mut expected_themes = HashMap::new();
    expected_themes.insert(
        "dracula".into(),
        Theme {
            terminal: None,
            palette: Palette {
                fg: PaletteColor::Rgb((248, 248, 242)),
                bg: PaletteColor::Rgb((40, 42, 54)),
                red: PaletteColor::Rgb((255, 85, 85)),
                green: PaletteColor::Rgb((80, 250, 123)),
                yellow: PaletteColor::Rgb((241, 250, 140)),
                blue: PaletteColor::Rgb((98, 114, 164)),
                magenta: PaletteColor::Rgb((255, 121, 198)),
                orange: PaletteColor::Rgb((255, 184, 108)),
                cyan: PaletteColor::Rgb((139, 233, 253)),
                black: PaletteColor::Rgb((0, 0, 0)),
                white: PaletteColor::Rgb((255, 255, 255)),
                ..Default::default()
            }
            .into(),
            sourced_from_external_file: false,
        },
    );
    let expected_themes = Themes::from_data(expected_themes);
    assert_eq!(config.themes, expected_themes, "Theme defined in config");
}

#[test]
fn can_define_multiple_themes_including_hex_themes_in_configfile() {
    let config_contents = r##"
            themes {
                dracula {
                    fg 248 248 242
                    bg 40 42 54
                    red 255 85 85
                    green 80 250 123
                    yellow 241 250 140
                    blue 98 114 164
                    magenta 255 121 198
                    orange 255 184 108
                    cyan 139 233 253
                    black 0 0 0
                    white 255 255 255
                }
                nord {
                    fg "#D8DEE9"
                    bg "#2E3440"
                    black "#3B4252"
                    red "#BF616A"
                    green "#A3BE8C"
                    yellow "#EBCB8B"
                    blue "#81A1C1"
                    magenta "#B48EAD"
                    cyan "#88C0D0"
                    white "#E5E9F0"
                    orange "#D08770"
                }
            }
        "##;
    let config = Config::from_kdl(config_contents, None).unwrap();
    let mut expected_themes = HashMap::new();
    expected_themes.insert(
        "dracula".into(),
        Theme {
            terminal: None,
            palette: Palette {
                fg: PaletteColor::Rgb((248, 248, 242)),
                bg: PaletteColor::Rgb((40, 42, 54)),
                red: PaletteColor::Rgb((255, 85, 85)),
                green: PaletteColor::Rgb((80, 250, 123)),
                yellow: PaletteColor::Rgb((241, 250, 140)),
                blue: PaletteColor::Rgb((98, 114, 164)),
                magenta: PaletteColor::Rgb((255, 121, 198)),
                orange: PaletteColor::Rgb((255, 184, 108)),
                cyan: PaletteColor::Rgb((139, 233, 253)),
                black: PaletteColor::Rgb((0, 0, 0)),
                white: PaletteColor::Rgb((255, 255, 255)),
                ..Default::default()
            }
            .into(),
            sourced_from_external_file: false,
        },
    );
    expected_themes.insert(
        "nord".into(),
        Theme {
            terminal: None,
            palette: Palette {
                fg: PaletteColor::Rgb((216, 222, 233)),
                bg: PaletteColor::Rgb((46, 52, 64)),
                black: PaletteColor::Rgb((59, 66, 82)),
                red: PaletteColor::Rgb((191, 97, 106)),
                green: PaletteColor::Rgb((163, 190, 140)),
                yellow: PaletteColor::Rgb((235, 203, 139)),
                blue: PaletteColor::Rgb((129, 161, 193)),
                magenta: PaletteColor::Rgb((180, 142, 173)),
                cyan: PaletteColor::Rgb((136, 192, 208)),
                white: PaletteColor::Rgb((229, 233, 240)),
                orange: PaletteColor::Rgb((208, 135, 112)),
                ..Default::default()
            }
            .into(),
            sourced_from_external_file: false,
        },
    );
    let expected_themes = Themes::from_data(expected_themes);
    assert_eq!(config.themes, expected_themes, "Theme defined in config");
}

#[test]
fn can_define_eight_bit_themes() {
    let config_contents = r#"
            themes {
                eight_bit_theme {
                    fg 248
                    bg 40
                    red 255
                    green 80
                    yellow 241
                    blue 98
                    magenta 255
                    orange 255
                    cyan 139
                    black 1
                    white 255
                }
            }
        "#;
    let config = Config::from_kdl(config_contents, None).unwrap();
    let mut expected_themes = HashMap::new();
    expected_themes.insert(
        "eight_bit_theme".into(),
        Theme {
            terminal: None,
            palette: Palette {
                fg: PaletteColor::EightBit(248),
                bg: PaletteColor::EightBit(40),
                red: PaletteColor::EightBit(255),
                green: PaletteColor::EightBit(80),
                yellow: PaletteColor::EightBit(241),
                blue: PaletteColor::EightBit(98),
                magenta: PaletteColor::EightBit(255),
                orange: PaletteColor::EightBit(255),
                cyan: PaletteColor::EightBit(139),
                black: PaletteColor::EightBit(1),
                white: PaletteColor::EightBit(255),
                ..Default::default()
            }
            .into(),
            sourced_from_external_file: false,
        },
    );
    let expected_themes = Themes::from_data(expected_themes);
    assert_eq!(config.themes, expected_themes, "Theme defined in config");
}

#[test]
fn can_define_style_for_theme_with_hex() {
    let config_contents = r##"
            themes {
                named_theme {
                    text_unselected {
                        base "#DCD7BA"
                        emphasis_0 "#DCD7CD"
                        emphasis_1 "#DCD8DD"
                        emphasis_2 "#DCD899"
                        emphasis_3 "#ACD7CD"
                        background   "#1F1F28"
                    }
                    text_selected {
                        base "#16161D"
                        emphasis_0 "#16161D"
                        emphasis_1 "#16161D"
                        emphasis_2 "#16161D"
                        emphasis_3 "#16161D"
                        background   "#9CABCA"
                    }
                    ribbon_unselected {
                        base "#DCD7BA"
                        emphasis_0 "#7FB4CA"
                        emphasis_1 "#A3D4D5"
                        emphasis_2 "#7AA89F"
                        emphasis_3 "#DCD819"
                        background   "#252535"
                    }
                    ribbon_selected {
                        base "#16161D"
                        emphasis_0 "#181820"
                        emphasis_1 "#1A1A22"
                        emphasis_2 "#2A2A37"
                        emphasis_3 "#363646"
                        background   "#76946A"
                    }
                    table_title {
                        base "#DCD7BA"
                        emphasis_0 "#7FB4CA"
                        emphasis_1 "#A3D4D5"
                        emphasis_2 "#7AA89F"
                        emphasis_3 "#DCD819"
                        background   "#252535"
                    }
                    table_cell_unselected {
                        base "#DCD7BA"
                        emphasis_0 "#DCD7CD"
                        emphasis_1 "#DCD8DD"
                        emphasis_2 "#DCD899"
                        emphasis_3 "#ACD7CD"
                        background   "#1F1F28"
                    }
                    table_cell_selected {
                        base "#16161D"
                        emphasis_0 "#181820"
                        emphasis_1 "#1A1A22"
                        emphasis_2 "#2A2A37"
                        emphasis_3 "#363646"
                        background   "#76946A"
                    }
                    list_unselected {
                        base "#DCD7BA"
                        emphasis_0 "#DCD7CD"
                        emphasis_1 "#DCD8DD"
                        emphasis_2 "#DCD899"
                        emphasis_3 "#ACD7CD"
                        background   "#1F1F28"
                    }
                    list_selected {
                        base "#16161D"
                        emphasis_0 "#181820"
                        emphasis_1 "#1A1A22"
                        emphasis_2 "#2A2A37"
                        emphasis_3 "#363646"
                        background   "#76946A"
                    }
                    frame_unselected {
                        base "#DCD8DD"
                        emphasis_0 "#7FB4CA"
                        emphasis_1 "#A3D4D5"
                        emphasis_2 "#7AA89F"
                        emphasis_3 "#DCD819"
                    }
                    frame_selected {
                        base "#76946A"
                        emphasis_0 "#C34043"
                        emphasis_1 "#C8C093"
                        emphasis_2 "#ACD7CD"
                        emphasis_3 "#DCD819"
                    }
                    exit_code_success {
                        base "#76946A"
                        emphasis_0 "#76946A"
                        emphasis_1 "#76946A"
                        emphasis_2 "#76946A"
                        emphasis_3 "#76946A"
                    }
                    exit_code_error {
                        base "#C34043"
                        emphasis_0 "#C34043"
                        emphasis_1 "#C34043"
                        emphasis_2 "#C34043"
                        emphasis_3 "#C34043"
                    }
                }
            }
            "##;

    let config = Config::from_kdl(config_contents, None).unwrap();
    let mut expected_themes = HashMap::new();
    expected_themes.insert(
        "named_theme".into(),
        Theme {
            terminal: None,
            sourced_from_external_file: false,
            palette: Styling {
                pane_surround: None,
                text_unselected: StyleDeclaration {
                    base: PaletteColor::Rgb((220, 215, 186)),
                    emphasis_0: PaletteColor::Rgb((220, 215, 205)),
                    emphasis_1: PaletteColor::Rgb((220, 216, 221)),
                    emphasis_2: PaletteColor::Rgb((220, 216, 153)),
                    emphasis_3: PaletteColor::Rgb((172, 215, 205)),
                    background: PaletteColor::Rgb((31, 31, 40)),
                },
                text_selected: StyleDeclaration {
                    base: PaletteColor::Rgb((22, 22, 29)),
                    emphasis_0: PaletteColor::Rgb((22, 22, 29)),
                    emphasis_1: PaletteColor::Rgb((22, 22, 29)),
                    emphasis_2: PaletteColor::Rgb((22, 22, 29)),
                    emphasis_3: PaletteColor::Rgb((22, 22, 29)),
                    background: PaletteColor::Rgb((156, 171, 202)),
                },
                ribbon_unselected: StyleDeclaration {
                    base: PaletteColor::Rgb((220, 215, 186)),
                    emphasis_0: PaletteColor::Rgb((127, 180, 202)),
                    emphasis_1: PaletteColor::Rgb((163, 212, 213)),
                    emphasis_2: PaletteColor::Rgb((122, 168, 159)),
                    emphasis_3: PaletteColor::Rgb((220, 216, 25)),
                    background: PaletteColor::Rgb((37, 37, 53)),
                },
                ribbon_selected: StyleDeclaration {
                    base: PaletteColor::Rgb((22, 22, 29)),
                    emphasis_0: PaletteColor::Rgb((24, 24, 32)),
                    emphasis_1: PaletteColor::Rgb((26, 26, 34)),
                    emphasis_2: PaletteColor::Rgb((42, 42, 55)),
                    emphasis_3: PaletteColor::Rgb((54, 54, 70)),
                    background: PaletteColor::Rgb((118, 148, 106)),
                },
                table_title: StyleDeclaration {
                    base: PaletteColor::Rgb((220, 215, 186)),
                    emphasis_0: PaletteColor::Rgb((127, 180, 202)),
                    emphasis_1: PaletteColor::Rgb((163, 212, 213)),
                    emphasis_2: PaletteColor::Rgb((122, 168, 159)),
                    emphasis_3: PaletteColor::Rgb((220, 216, 25)),
                    background: PaletteColor::Rgb((37, 37, 53)),
                },
                table_cell_unselected: StyleDeclaration {
                    base: PaletteColor::Rgb((220, 215, 186)),
                    emphasis_0: PaletteColor::Rgb((220, 215, 205)),
                    emphasis_1: PaletteColor::Rgb((220, 216, 221)),
                    emphasis_2: PaletteColor::Rgb((220, 216, 153)),
                    emphasis_3: PaletteColor::Rgb((172, 215, 205)),
                    background: PaletteColor::Rgb((31, 31, 40)),
                },
                table_cell_selected: StyleDeclaration {
                    base: PaletteColor::Rgb((22, 22, 29)),
                    emphasis_0: PaletteColor::Rgb((24, 24, 32)),
                    emphasis_1: PaletteColor::Rgb((26, 26, 34)),
                    emphasis_2: PaletteColor::Rgb((42, 42, 55)),
                    emphasis_3: PaletteColor::Rgb((54, 54, 70)),
                    background: PaletteColor::Rgb((118, 148, 106)),
                },
                list_unselected: StyleDeclaration {
                    base: PaletteColor::Rgb((220, 215, 186)),
                    emphasis_0: PaletteColor::Rgb((220, 215, 205)),
                    emphasis_1: PaletteColor::Rgb((220, 216, 221)),
                    emphasis_2: PaletteColor::Rgb((220, 216, 153)),
                    emphasis_3: PaletteColor::Rgb((172, 215, 205)),
                    background: PaletteColor::Rgb((31, 31, 40)),
                },
                list_selected: StyleDeclaration {
                    base: PaletteColor::Rgb((22, 22, 29)),
                    emphasis_0: PaletteColor::Rgb((24, 24, 32)),
                    emphasis_1: PaletteColor::Rgb((26, 26, 34)),
                    emphasis_2: PaletteColor::Rgb((42, 42, 55)),
                    emphasis_3: PaletteColor::Rgb((54, 54, 70)),
                    background: PaletteColor::Rgb((118, 148, 106)),
                },
                frame_unselected: Some(StyleDeclaration {
                    base: PaletteColor::Rgb((220, 216, 221)),
                    emphasis_0: PaletteColor::Rgb((127, 180, 202)),
                    emphasis_1: PaletteColor::Rgb((163, 212, 213)),
                    emphasis_2: PaletteColor::Rgb((122, 168, 159)),
                    emphasis_3: PaletteColor::Rgb((220, 216, 25)),
                    ..Default::default()
                }),
                frame_selected: StyleDeclaration {
                    base: PaletteColor::Rgb((118, 148, 106)),
                    emphasis_0: PaletteColor::Rgb((195, 64, 67)),
                    emphasis_1: PaletteColor::Rgb((200, 192, 147)),
                    emphasis_2: PaletteColor::Rgb((172, 215, 205)),
                    emphasis_3: PaletteColor::Rgb((220, 216, 25)),
                    ..Default::default()
                },
                exit_code_success: StyleDeclaration {
                    base: PaletteColor::Rgb((118, 148, 106)),
                    emphasis_0: PaletteColor::Rgb((118, 148, 106)),
                    emphasis_1: PaletteColor::Rgb((118, 148, 106)),
                    emphasis_2: PaletteColor::Rgb((118, 148, 106)),
                    emphasis_3: PaletteColor::Rgb((118, 148, 106)),
                    ..Default::default()
                },
                exit_code_error: StyleDeclaration {
                    base: PaletteColor::Rgb((195, 64, 67)),
                    emphasis_0: PaletteColor::Rgb((195, 64, 67)),
                    emphasis_1: PaletteColor::Rgb((195, 64, 67)),
                    emphasis_2: PaletteColor::Rgb((195, 64, 67)),
                    emphasis_3: PaletteColor::Rgb((195, 64, 67)),
                    ..Default::default()
                },
                ..Default::default()
            },
        },
    );
    let expected_themes = Themes::from_data(expected_themes);
    assert_eq!(config.themes, expected_themes, "Theme defined in config")
}

#[test]
fn omitting_required_style_errors() {
    let config_contents = r##"
            themes {
                named_theme {
                    text_unselected {
                        base "#DCD7BA"
                        emphasis_1 "#DCD8DD"
                        emphasis_2 "#DCD899"
                        emphasis_3 "#ACD7CD"
                        background   "#1F1F28"
                    }
                }
            }
            "##;

    let config = Config::from_kdl(config_contents, None);
    assert!(config.is_err());
    if let Err(ConfigError::KdlError(KdlError {
        error_message,
        src: _,
        offset: _,
        len: _,
        help_message: _,
    })) = config
    {
        assert_eq!(error_message, "Missing theme color: emphasis_0")
    }
}

#[test]
fn partial_declaration_of_styles_defaults_omitted() {
    let config_contents = r##"
            themes {
                named_theme {
                    text_unselected {
                        base "#DCD7BA"
                        emphasis_0 "#DCD7CD"
                        emphasis_1 "#DCD8DD"
                        emphasis_2 "#DCD899"
                        emphasis_3 "#ACD7CD"
                        background   "#1F1F28"
                    }
                }
            }
            "##;

    let config = Config::from_kdl(config_contents, None).unwrap();
    let mut expected_themes = HashMap::new();
    expected_themes.insert(
        "named_theme".into(),
        Theme {
            terminal: None,
            sourced_from_external_file: false,
            palette: Styling {
                pane_surround: None,
                text_unselected: StyleDeclaration {
                    base: PaletteColor::Rgb((220, 215, 186)),
                    emphasis_0: PaletteColor::Rgb((220, 215, 205)),
                    emphasis_1: PaletteColor::Rgb((220, 216, 221)),
                    emphasis_2: PaletteColor::Rgb((220, 216, 153)),
                    emphasis_3: PaletteColor::Rgb((172, 215, 205)),
                    background: PaletteColor::Rgb((31, 31, 40)),
                },
                ..Default::default()
            },
        },
    );
    let expected_themes = Themes::from_data(expected_themes);
    assert_eq!(config.themes, expected_themes, "Theme defined in config")
}

#[test]
fn can_define_plugin_configuration_in_configfile() {
    let config_contents = r#"
            plugins {
                tab-bar location="yinz:tab-bar"
                status-bar location="yinz:status-bar"
                strider location="yinz:strider"
                compact-bar location="yinz:compact-bar"
                session-palette location="yinz:session-palette"
                session-browser location="yinz:session-palette" {
                    shared true
                }
                filepicker location="yinz:strider"
            }
        "#;
    let config = Config::from_kdl(config_contents, None).unwrap();
    let mut expected_plugin_configuration = BTreeMap::new();
    expected_plugin_configuration.insert(
        "tab-bar".to_owned(),
        yinz_runtime::plugin_location::run_plugin_from_url("yinz:tab-bar").unwrap(),
    );
    expected_plugin_configuration.insert(
        "status-bar".to_owned(),
        yinz_runtime::plugin_location::run_plugin_from_url("yinz:status-bar").unwrap(),
    );
    expected_plugin_configuration.insert(
        "strider".to_owned(),
        yinz_runtime::plugin_location::run_plugin_from_url("yinz:strider").unwrap(),
    );
    expected_plugin_configuration.insert(
        "compact-bar".to_owned(),
        yinz_runtime::plugin_location::run_plugin_from_url("yinz:compact-bar").unwrap(),
    );
    expected_plugin_configuration.insert(
        "session-palette".to_owned(),
        yinz_runtime::plugin_location::run_plugin_from_url("yinz:session-palette").unwrap(),
    );
    let mut shared_session_configuration = BTreeMap::new();
    shared_session_configuration.insert("shared".to_owned(), "true".to_owned());
    expected_plugin_configuration.insert(
        "session-browser".to_owned(),
        yinz_runtime::plugin_location::run_plugin_from_url("yinz:session-palette")
            .unwrap()
            .with_configuration(shared_session_configuration),
    );
    expected_plugin_configuration.insert(
        "filepicker".to_owned(),
        yinz_runtime::plugin_location::run_plugin_from_url("yinz:strider").unwrap(),
    );
    assert_eq!(
        config.plugins,
        PluginAliases::from_data(expected_plugin_configuration),
        "Plugins defined in config"
    );
}

#[test]
fn can_define_ui_configuration_in_configfile() {
    let config_contents = r#"
            ui {
                pane_frames {
                    rounded_corners true
                    hide_session_name true
                }
            }
        "#;
    let config = Config::from_kdl(config_contents, None).unwrap();
    let expected_ui_config = UiConfig {
        pane_frames: FrameConfig {
            rounded_corners: true,
            hide_session_name: true,
        },
    };
    assert_eq!(config.ui, expected_ui_config, "Ui config defined in config");
}

#[test]
fn can_define_env_variables_in_config_file() {
    let config_contents = r#"
            env {
                RUST_BACKTRACE 1
                SOME_OTHER_VAR "foo"
            }
        "#;
    let config = Config::from_kdl(config_contents, None).unwrap();
    let mut expected_env_config = HashMap::new();
    expected_env_config.insert("RUST_BACKTRACE".into(), "1".into());
    expected_env_config.insert("SOME_OTHER_VAR".into(), "foo".into());
    assert_eq!(
        config.env,
        EnvironmentVariables::from_data(expected_env_config),
        "Env variables defined in config"
    );
}

#[test]
fn context_menu_items_parse_labels_and_actions() {
    let config = Config::from_kdl(
        r#"
            context_menu {
                "Copy" {
                    Copy
                }
                separator
                "Close pane" shortcut="Ctrl-w" {
                    CloseFocus
                }
            }
            "#,
        None,
    )
    .unwrap();
    let items = config.context_menu.items();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].label, "Copy");
    assert_eq!(items[0].actions, vec![Action::Copy]);
    assert!(items[1].separator);
    assert_eq!(items[2].label, "Close pane");
    assert_eq!(items[2].shortcut.as_deref(), Some("Ctrl-w"));
    assert_eq!(items[2].actions, vec![Action::CloseFocus]);
}

/// Unlike `load_plugins`, a `context_menu` block is not additive: a menu somebody wrote out is
/// the menu they want. Asserted rather than merely documented, because the divergence is the
/// sort of thing a later change would "fix" without noticing.
#[test]
fn a_context_menu_block_replaces_the_defaults() {
    let config = Config::from_kdl("context_menu {\n \"Only this\" {\n Copy\n }\n}", None).unwrap();
    assert_eq!(config.context_menu.items().len(), 1);
    assert_eq!(config.context_menu.items()[0].label, "Only this");
}

#[test]
fn an_empty_context_menu_block_means_no_configured_items() {
    let config = Config::from_kdl("context_menu {\n}", None).unwrap();
    assert!(
        config.context_menu.is_empty(),
        "which is a fine thing to write when the hook plugin supplies everything"
    );
}

#[test]
fn a_config_that_says_nothing_about_the_context_menu_keeps_the_defaults() {
    let config = Config::from_kdl("keybinds {\n}", None).unwrap();
    assert!(!config.context_menu.is_empty());
}

#[test]
fn a_context_menu_item_with_no_actions_is_a_config_error() {
    let err = Config::from_kdl("context_menu {\n \"Does nothing\"\n}", None).unwrap_err();
    let message = format!("{err:?}");
    assert!(message.contains("Does nothing"), "{message}");
}

#[test]
fn a_separator_with_children_is_a_config_error() {
    let err =
        Config::from_kdl("context_menu {\n separator {\n CloseFocus\n }\n}", None).unwrap_err();
    let message = format!("{err:?}");
    assert!(message.contains("separator"), "{message}");
}

#[test]
fn a_separator_with_a_property_is_a_config_error() {
    let err = Config::from_kdl("context_menu {\n separator shortcut=\"x\"\n}", None).unwrap_err();
    let message = format!("{err:?}");
    assert!(message.contains("separator"), "{message}");
}

#[test]
fn context_menu_survives_serialization() {
    let config = Config::from_kdl(
        "context_menu {\n \"Copy\" shortcut=\"Ctrl-c\" {\n Copy\n }\n separator\n}",
        None,
    )
    .unwrap();
    let round_tripped = Config::from_kdl(&config.to_string(false).unwrap(), None).unwrap();
    assert_eq!(round_tripped.context_menu, config.context_menu);
}

#[test]
fn the_default_hooks_bind_the_shipped_context_menu() {
    let config = Config::from_kdl("keybinds {\n}", None).unwrap();
    assert_eq!(
        config
            .hooks
            .get(HookPoint::ContextMenu)
            .map(|p| p.location_string()),
        Some("yinz:context-menu".to_owned())
    );
}

#[test]
fn hooks_bind_a_point_to_a_plugin() {
    let config = Config::from_kdl("hooks {\n agent_exit \"file:/tmp/mine.scm\"\n}", None).unwrap();
    assert_eq!(
        config
            .hooks
            .get(HookPoint::AgentExit)
            .map(|p| p.location_string()),
        Some("file:/tmp/mine.scm".to_owned())
    );
}

#[test]
fn a_config_that_says_nothing_about_hooks_keeps_the_defaults() {
    let config = Config::from_kdl("keybinds {\n}", None).unwrap();
    assert_eq!(
        config
            .hooks
            .get(HookPoint::AgentExit)
            .map(|p| p.location_string()),
        Some("yinz:agent-exit".to_owned())
    );
}

#[test]
fn an_empty_hook_url_unbinds_the_point() {
    let config = Config::from_kdl("hooks {\n agent_exit \"\"\n}", None).unwrap();
    assert!(
        config.hooks.get(HookPoint::AgentExit).is_none(),
        "an empty string is how a default hook is turned off"
    );
}

#[test]
fn an_unknown_hook_point_is_a_config_error() {
    assert!(Config::from_kdl("hooks {\n agent_started \"file:/tmp/x.scm\"\n}", None).is_err());
}

#[test]
fn every_hook_point_binds_from_kdl_and_survives_serialization() {
    // The KDL surface is generic over `HookPoint::ALL`, and this is what keeps it so: a
    // point that can be named can be bound, and comes back bound.
    for point in HookPoint::ALL {
        let kdl = format!("hooks {{\n {point} \"file:/tmp/{point}.scm\"\n}}");
        let config = Config::from_kdl(&kdl, None).unwrap_or_else(|e| panic!("{point}: {e:?}"));
        assert_eq!(
            config.hooks.get(*point).map(|p| p.location_string()),
            Some(format!("file:/tmp/{point}.scm")),
            "{point}"
        );
        let round_tripped = Config::from_kdl(&config.to_string(false).unwrap(), None).unwrap();
        assert_eq!(round_tripped.hooks, config.hooks, "{point}");
    }
}

#[test]
fn hooks_survive_serialization() {
    // Both directions matter: a bound point must come back bound, and an unbound one must
    // stay unbound rather than falling back to the default.
    let bound = Config::from_kdl("hooks {\n agent_exit \"file:/tmp/mine.scm\"\n}", None).unwrap();
    let round_tripped = Config::from_kdl(&bound.to_string(false).unwrap(), None).unwrap();
    assert_eq!(round_tripped.hooks, bound.hooks);

    let unbound = Config::from_kdl("hooks {\n agent_exit \"\"\n}", None).unwrap();
    let round_tripped = Config::from_kdl(&unbound.to_string(false).unwrap(), None).unwrap();
    assert!(round_tripped.hooks.get(HookPoint::AgentExit).is_none());

    // A hook is a whole plugin declaration, so its cwd and configuration have to survive
    // too - otherwise the plugin gets a different `load(config)` after a restart.
    let configured = Config::from_kdl(
        r#"
            hooks {
                agent_exit "file:/tmp/mine.scm" {
                    cwd "/tmp/hook-cwd"
                    greeting "hello"
                    loud true
                }
            }
            "#,
        None,
    )
    .unwrap();
    let round_tripped = Config::from_kdl(&configured.to_string(false).unwrap(), None).unwrap();
    assert_eq!(round_tripped.hooks, configured.hooks);
    // And the written form has to be stable, not merely re-readable: `cwd` is parsed into
    // the configuration as well as into the cwd, so a naive writer duplicates it and the
    // copies multiply with every write.
    assert_eq!(
        Config::from_kdl(&round_tripped.to_string(false).unwrap(), None)
            .unwrap()
            .to_string(false)
            .unwrap(),
        round_tripped.to_string(false).unwrap()
    );
}

#[test]
fn load_plugins_adds_to_defaults_unless_cleared() {
    let base = Config::from_kdl("load_plugins {\n \"yinz:link\"\n}", None).unwrap();
    let names = |config: &Config| -> Vec<String> {
        config
            .background_plugins
            .iter()
            .map(|p| p.location_string())
            .collect()
    };

    let merged = Config::from_kdl(
        "load_plugins {\n \"yinz:configuration\"\n}",
        Some(base.clone()),
    )
    .unwrap();
    assert_eq!(names(&merged), vec!["yinz:configuration", "yinz:link"]);

    let cleared = Config::from_kdl(
        "load_plugins clear-defaults=true {\n \"yinz:configuration\"\n}",
        Some(base),
    )
    .unwrap();
    assert_eq!(names(&cleared), vec!["yinz:configuration"]);
}

#[test]
fn management_palettes_are_defaults_and_clear_defaults_removes_them() {
    let config = Config::from_default_assets().unwrap();
    let serialized = config.to_string(false).unwrap();
    for name in ["session-palette", "layout-palette", "plugin-palette"] {
        assert!(serialized.contains(name), "missing {name}");
        assert!(
            config
                .background_plugins
                .iter()
                .any(|plugin| { plugin.location_string() == name }),
            "{name} must load by default"
        );
    }
    for removed in ["session-manager", "layout-manager", "plugin-manager"] {
        assert!(!serialized.contains(removed), "obsolete {removed}");
    }
    let cleared = Config::from_kdl("load_plugins clear-defaults=true {}", Some(config)).unwrap();
    assert!(cleared.background_plugins.is_empty());
}

#[test]
fn load_plugins_clear_defaults_survives_serialization() {
    let base = Config::from_kdl("load_plugins {\n \"yinz:link\"\n}", None).unwrap();
    let names = |config: &Config| -> Vec<String> {
        config
            .background_plugins
            .iter()
            .map(|p| p.location_string())
            .collect()
    };

    let cleared = Config::from_kdl(
        "load_plugins clear-defaults=true {\n \"yinz:configuration\"\n}",
        Some(base.clone()),
    )
    .unwrap();
    let round_tripped =
        Config::from_kdl(&cleared.to_string(false).unwrap(), Some(base.clone())).unwrap();
    assert_eq!(names(&round_tripped), vec!["yinz:configuration"]);

    // the additive case must not grow a clear-defaults out of nowhere
    let merged = Config::from_kdl(
        "load_plugins {\n \"yinz:configuration\"\n}",
        Some(base.clone()),
    )
    .unwrap();
    let round_tripped = Config::from_kdl(&merged.to_string(false).unwrap(), Some(base)).unwrap();
    assert_eq!(
        names(&round_tripped),
        vec!["yinz:configuration", "yinz:link"]
    );
}

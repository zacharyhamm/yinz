use super::Setup;
use insta::assert_snapshot;
use std::path::PathBuf;
use yinz_types::data::LayoutInfo;
use {crate::args::CliArgs, crate::args::Command};
use {yinz_config::config::Config, yinz_types::input::options::Options};

fn from_cli_args(cli_args: &CliArgs) -> (Config, Option<LayoutInfo>, Options, Config, Options) {
    // Path discovery lives in another crate, so its cfg(test) cannot isolate these tests.
    // Supply a real empty directory instead of reading the developer's config and overlays.
    let config_dir = tempfile::tempdir().unwrap();
    let mut cli_args = cli_args.clone();
    cli_args
        .config_dir
        .get_or_insert_with(|| config_dir.path().to_path_buf());
    let mut result = Setup::from_cli_args(&cli_args).unwrap();
    // Config is now a dependency, so builtin themes are loaded here as in production.
    // Keep these precedence snapshots focused on the config and layout theme declarations.
    for config in [&mut result.0, &mut result.3] {
        assert!(config
            .themes
            .inner()
            .values()
            .any(|theme| theme.sourced_from_external_file));
        config.themes = yinz_config::theme::Themes::from_data(
            config
                .themes
                .inner()
                .iter()
                .filter(|(_, theme)| !theme.sourced_from_external_file)
                .map(|(name, theme)| (name.clone(), theme.clone()))
                .collect(),
        );
    }
    result
}

#[test]
fn default_config_with_no_cli_arguments() {
    let cli_args = CliArgs::default();
    let (config, layout_info, options, _, _) = from_cli_args(&cli_args);
    assert_snapshot!(format!("{:#?}", config));
    assert_snapshot!(format!("{:#?}", layout_info));
    assert_snapshot!(format!("{:#?}", options));
}
#[test]
fn cli_arguments_override_config_options() {
    let cli_args = CliArgs {
        command: Some(Command::Options(Box::new(crate::options::CliOptions {
            simplified_ui: Some(true),
            ..Default::default()
        }))),
        ..Default::default()
    };
    let (_config, _layout_info, options, _, _) = from_cli_args(&cli_args);
    assert_snapshot!(format!("{:#?}", options));
}
#[test]
fn layout_options_override_config_options() {
    let cli_args = CliArgs {
        layout: Some(PathBuf::from(format!(
            "{}/src/test-fixtures/layout-with-options.kdl",
            env!("CARGO_MANIFEST_DIR")
        ))),
        ..Default::default()
    };
    let (_config, layout_info, options, _, _) = from_cli_args(&cli_args);
    assert_snapshot!(format!("{:#?}", options));
    let Some(LayoutInfo::File(layout_path, _)) = layout_info else {
        panic!("layout info doesn't have expected format");
    };
    assert_eq!(
        layout_path,
        format!(
            "{}/src/test-fixtures/layout-with-options.kdl",
            env!("CARGO_MANIFEST_DIR")
        )
    );
}
#[test]
fn cli_arguments_override_layout_options() {
    let cli_args = CliArgs {
        layout: Some(PathBuf::from(format!(
            "{}/src/test-fixtures/layout-with-options.kdl",
            env!("CARGO_MANIFEST_DIR")
        ))),
        command: Some(Command::Options(Box::new(crate::options::CliOptions {
            pane_frames: Some(true),
            ..Default::default()
        }))),
        ..Default::default()
    };
    let (_config, layout_info, options, _, _) = from_cli_args(&cli_args);
    assert_snapshot!(format!("{:#?}", options));
    let Some(LayoutInfo::File(layout_path, _)) = layout_info else {
        panic!("layout info doesn't have expected format");
    };
    assert_eq!(
        layout_path,
        format!(
            "{}/src/test-fixtures/layout-with-options.kdl",
            env!("CARGO_MANIFEST_DIR")
        )
    );
}
#[test]
fn layout_env_vars_override_config_env_vars() {
    let cli_args = CliArgs {
        config: Some(PathBuf::from(format!(
            "{}/src/test-fixtures/config-with-env-vars.kdl",
            env!("CARGO_MANIFEST_DIR")
        ))),
        layout: Some(PathBuf::from(format!(
            "{}/src/test-fixtures/layout-with-env-vars.kdl",
            env!("CARGO_MANIFEST_DIR")
        ))),
        ..Default::default()
    };
    let (config, _layout_info, _options, _, _) = from_cli_args(&cli_args);
    assert_snapshot!(format!("{:#?}", config));
}
#[test]
fn layout_ui_config_overrides_config_ui_config() {
    let cli_args = CliArgs {
        config: Some(PathBuf::from(format!(
            "{}/src/test-fixtures/config-with-ui-config.kdl",
            env!("CARGO_MANIFEST_DIR")
        ))),
        layout: Some(PathBuf::from(format!(
            "{}/src/test-fixtures/layout-with-ui-config.kdl",
            env!("CARGO_MANIFEST_DIR")
        ))),
        ..Default::default()
    };
    let (config, _layout_info, _options, _, _) = from_cli_args(&cli_args);
    assert_snapshot!(format!("{:#?}", config));
}
#[test]
fn layout_themes_override_config_themes() {
    let cli_args = CliArgs {
        config: Some(PathBuf::from(format!(
            "{}/src/test-fixtures/config-with-themes-config.kdl",
            env!("CARGO_MANIFEST_DIR")
        ))),
        layout: Some(PathBuf::from(format!(
            "{}/src/test-fixtures/layout-with-themes-config.kdl",
            env!("CARGO_MANIFEST_DIR")
        ))),
        ..Default::default()
    };
    let (config, _layout_info, _options, _, _) = from_cli_args(&cli_args);
    assert_snapshot!(format!("{:#?}", config));
}
#[test]
fn layout_keybinds_override_config_keybinds() {
    let cli_args = CliArgs {
        config: Some(PathBuf::from(format!(
            "{}/src/test-fixtures/config-with-keybindings-config.kdl",
            env!("CARGO_MANIFEST_DIR")
        ))),
        layout: Some(PathBuf::from(format!(
            "{}/src/test-fixtures/layout-with-keybindings-config.kdl",
            env!("CARGO_MANIFEST_DIR")
        ))),
        ..Default::default()
    };
    let (config, _layout_info, _options, _, _) = from_cli_args(&cli_args);
    assert_snapshot!(format!("{:#?}", config));
}
#[test]
fn layout_repository_aliases_resolve_after_cli_merges() {
    use yinz_types::input::layout::RunPluginLocation;
    let directory = tempfile::tempdir().unwrap();
    let layout_directory = tempfile::tempdir().unwrap();
    let layout = layout_directory.path().join("layout.kdl");
    let source = "plugins { panel repo=\"tools\"; }\nlayout { pane; }";
    std::fs::write(&layout, source).unwrap();
    for string_layout in [false, true] {
        let args = CliArgs {
            config_dir: Some(directory.path().to_owned()),
            layout: (!string_layout).then_some(layout.clone()),
            layout_string: string_layout.then(|| source.to_owned()),
            ..Default::default()
        };
        let config = Setup::from_cli_args(&args).unwrap().0;
        let RunPluginLocation::Git(plugin) = &config.plugins.aliases["panel"].location else {
            panic!("Git alias")
        };
        assert_eq!(
            std::path::Path::new(&plugin.repo),
            directory.path().join("tools")
        );
        assert_eq!(
            config
                .clone()
                .resolve_plugin_repositories(args.config.as_deref(), args.config_dir.as_deref())
                .unwrap(),
            config
        );
    }
}

#[test]
fn cli_config_dir_overrides_defaults() {
    let config_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("test-fixtures")
        .join("config-dirs")
        .join("layout-upside-down");
    let cli_args = CliArgs {
        config_dir: Some(config_dir.clone()),
        ..Default::default()
    };
    let (_, layout_info, _, _, _) = from_cli_args(&cli_args);
    let Some(LayoutInfo::File(layout_path, _)) = layout_info else {
        panic!("layout info has unexpected format");
    };
    let expected = config_dir
        .join("layouts")
        .join("upside-down.kdl")
        .canonicalize()
        .unwrap();
    assert_eq!(layout_path, expected.display().to_string());
}
#[test]
fn cli_config_dir_finds_custom_default() {
    let config_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("test-fixtures")
        .join("config-dirs")
        .join("custom-default-layout");
    let cli_args = CliArgs {
        config_dir: Some(config_dir.clone()),
        ..Default::default()
    };
    let (_, layout_info, _, _, _) = from_cli_args(&cli_args);
    let Some(LayoutInfo::File(layout_path, _)) = layout_info else {
        panic!("layout info has unexpected format");
    };
    let expected = config_dir
        .join("layouts")
        .join("default.kdl")
        .canonicalize()
        .unwrap();
    assert_eq!(layout_path, expected.display().to_string());
}

#[test]
fn cli_with_relative_layout_and_extension() {
    // NOTE: We assume to be in `yinz-cli` root directory. If this doesn't hold, path
    // resolution cannot work (as it actually reads path to ensure they exist).
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(cwd, PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    let cli_args = CliArgs {
        layout: Some(PathBuf::from("src/test-fixtures/layout-with-options.kdl")),
        ..Default::default()
    };
    let (_, layout_info, _, _, _) = from_cli_args(&cli_args);
    let Some(LayoutInfo::File(layout_path, _)) = layout_info else {
        panic!("layout info has unexpected format: {:?}", layout_info);
    };
    let expected = cwd.join("src/test-fixtures/layout-with-options.kdl");
    assert_eq!(layout_path, expected.display().to_string());
}

#[test]
fn cli_with_relative_layout_and_separator() {
    // NOTE: We assume to be in `yinz-cli` root directory. If this doesn't hold, path
    // resolution cannot work (as it actually reads path to ensure they exist).
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(cwd, PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    let cli_args = CliArgs {
        layout: Some(PathBuf::from("src/test-fixtures/layout-with-options")),
        ..Default::default()
    };
    let (_, layout_info, _, _, _) = from_cli_args(&cli_args);
    let Some(LayoutInfo::File(layout_path, _)) = layout_info else {
        panic!("layout info has unexpected format");
    };
    let expected = cwd.join("src/test-fixtures/layout-with-options");
    assert_eq!(layout_path, expected.display().to_string());
}

#[test]
fn layout_string_cli_argument() {
    let layout_kdl = "layout {\n    pane\n    pane\n}\n".to_string();
    let cli_args = CliArgs {
        layout_string: Some(layout_kdl.clone()),
        ..Default::default()
    };
    let (_, layout_info, _, _, _) = from_cli_args(&cli_args);
    let Some(LayoutInfo::Stringified(content)) = layout_info else {
        panic!(
            "layout info should be Stringified variant, got: {:#?}",
            layout_info
        );
    };
    assert_eq!(content, layout_kdl);
}

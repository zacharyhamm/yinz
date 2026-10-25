use super::{PluginConfig, RunPlugin, RunPluginLocation};
use std::fs;
use tempfile::tempdir;
use yinz_types::data::PluginTag;

#[test]
fn steel_sources_resolve_by_extension_and_user_plugin_directory() {
    let plugins = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    for extension in ["scm", "ss", "sld"] {
        let name = format!("keys.{extension}");
        let source = "(define (update event) #f)";
        fs::write(plugins.path().join(&name), source).unwrap();
        let run = yinz_runtime::plugin_location::run_plugin_from_url_with_cwd(
            &format!("file:{name}"),
            Some(cwd.path().to_owned()),
        )
        .unwrap();
        let plugin = PluginConfig::from_run_plugin(&run).unwrap();
        assert_eq!(
            plugin.resolve_source(Some(plugins.path())).unwrap(),
            (plugins.path().join(&name), source.to_owned())
        );
        let explicit = cwd.path().join(&name);
        assert!(file_plugin(explicit.to_str().unwrap())
            .resolve_source(Some(plugins.path()))
            .is_err());
        fs::write(&explicit, "(define explicit #t)").unwrap();
        assert_eq!(
            plugin.resolve_source(Some(plugins.path())).unwrap().1,
            "(define explicit #t)"
        );
    }
}

#[test]
fn unsupported_source_extensions_are_rejected() {
    for name in [
        "keys",
        "keys.wasm",
        "keys.js",
        "keys.ts",
        "keys.mjs",
        "keys.mts",
        "keys.SCM",
    ] {
        assert!(
            PluginConfig::from_run_plugin(&RunPlugin {
                location: RunPluginLocation::File(name.into()),
                ..Default::default()
            })
            .is_none(),
            "{name}"
        );
    }
}

fn builtin_plugin(name: &str) -> PluginConfig {
    PluginConfig::from_run_plugin(&RunPlugin {
        location: RunPluginLocation::Yinz(PluginTag::new(name)),
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn agent_exit_is_client_specific() {
    assert!(!builtin_plugin("agent-exit").is_shared());
}

#[test]
fn file_finder_has_one_driver_per_launch_even_when_sharing_is_disabled() {
    let mut plugin = builtin_plugin("file-finder");
    assert!(plugin.is_shared());
    plugin
        .initial_userspace_configuration
        .insert("shared", "false");
    assert!(plugin.is_shared());
    assert!(!file_plugin("file-finder.scm").is_shared());
}

fn file_plugin(path: &str) -> PluginConfig {
    PluginConfig::from_run_plugin(&RunPlugin {
        location: RunPluginLocation::File(std::path::PathBuf::from(path)),
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn a_script_plugin_is_found_in_the_user_plugin_dir_by_name() {
    let plugin_dir = tempdir().unwrap();
    fs::write(plugin_dir.path().join("keys.scm"), "-- from the dir").unwrap();

    assert_eq!(
        file_plugin("keys.scm")
            .resolve_source(Some(plugin_dir.path()))
            .unwrap(),
        (
            plugin_dir.path().join("keys.scm"),
            "-- from the dir".to_owned()
        )
    );
}

/// A plugin as the `yinz plugin -- file:keys.scm` path builds one: the URL was relative, so
/// the cwd has already been joined onto it and the path is absolute by now. Only
/// `relative_path` still knows the plugin was named `keys.scm`, and without it the folder
/// would be searched for `$cwd/keys.scm` - which `Path::join` returns unchanged, so nothing
/// in the folder could ever match.
#[test]
fn a_relative_url_launched_from_elsewhere_is_found_in_the_user_plugin_dir() {
    let plugin_dir = tempdir().unwrap();
    fs::write(plugin_dir.path().join("keys.scm"), "-- from the dir").unwrap();
    let cwd = tempdir().unwrap();

    let (location, relative_path) =
        yinz_runtime::plugin_location::parse_plugin_location_with_relative(
            "file:keys.scm",
            Some(cwd.path().to_owned()),
        )
        .unwrap();
    assert!(
        matches!(&location, RunPluginLocation::File(path) if path.is_absolute()),
        "the cwd should already have been joined on: {location:?}"
    );
    let plugin = PluginConfig::from_run_plugin(&RunPlugin {
        location,
        relative_path,
        ..Default::default()
    })
    .unwrap();

    assert_eq!(
        plugin.resolve_source(Some(plugin_dir.path())).unwrap(),
        (
            plugin_dir.path().join("keys.scm"),
            "-- from the dir".to_owned()
        )
    );
}

/// The other half of the same rule, and the reason relativity is carried rather than the
/// file name simply being retried: a path the user gave in full names one file. If it is not
/// there, that is the answer - loading a same-named plugin out of the user's folder instead
/// would run something they did not ask for.
#[test]
fn an_absolute_url_that_is_missing_does_not_fall_back_to_the_user_plugin_dir() {
    let plugin_dir = tempdir().unwrap();
    fs::write(plugin_dir.path().join("keys.scm"), "-- from the dir").unwrap();
    let elsewhere = tempdir().unwrap();
    let missing = elsewhere.path().join("keys.scm");

    let (location, relative_path) =
        yinz_runtime::plugin_location::parse_plugin_location_with_relative(
            &format!("file:{}", missing.display()),
            Some(elsewhere.path().to_owned()),
        )
        .unwrap();
    assert_eq!(relative_path, None, "the URL was absolute");
    let plugin = PluginConfig::from_run_plugin(&RunPlugin {
        location,
        relative_path,
        ..Default::default()
    })
    .unwrap();

    let error = plugin
        .resolve_source(Some(plugin_dir.path()))
        .unwrap_err()
        .to_string();
    assert!(error.contains("keys.scm"), "{error}");
}

/// The path as given always wins, so an existing declaration cannot start meaning a
/// different file the day a same-named plugin appears in the folder.
#[test]
fn a_path_that_exists_beats_the_user_plugin_dir() {
    let plugin_dir = tempdir().unwrap();
    fs::write(plugin_dir.path().join("keys.scm"), "-- from the dir").unwrap();
    let elsewhere = tempdir().unwrap();
    let explicit = elsewhere.path().join("keys.scm");
    fs::write(&explicit, "-- from the explicit path").unwrap();

    assert_eq!(
        file_plugin(&explicit.display().to_string())
            .resolve_source(Some(plugin_dir.path()))
            .unwrap(),
        (explicit, "-- from the explicit path".to_owned())
    );
}

/// The plugin host serves a Scheme builtin's compiled-in source before it would ever ask
/// for a file, so a request that reaches here anyway must fail rather than find a same-named
/// file posing as the builtin.
#[test]
fn a_builtin_tag_is_never_read_from_the_user_plugin_directory() {
    let plugin_dir = tempdir().unwrap();
    for name in yinz_runtime::consts::BUILTIN_PLUGIN_NAMES {
        fs::write(plugin_dir.path().join(name), "-- impostor").unwrap();
        fs::write(plugin_dir.path().join(format!("{name}.scm")), "-- impostor").unwrap();
        let plugin = builtin_plugin(name);
        assert!(plugin.is_builtin());
        let error = plugin
            .resolve_source(Some(plugin_dir.path()))
            .unwrap_err()
            .to_string();
        assert!(error.contains("installed builtin"), "{error}");
    }
    assert!(!file_plugin("context-menu.scm").is_builtin());
}

/// A typo is much harder to diagnose if the error names only one of the places looked in.
#[test]
fn a_missing_script_plugin_names_every_path_tried() {
    let plugin_dir = tempdir().unwrap();
    let error = file_plugin("nope.scm")
        .resolve_source(Some(plugin_dir.path()))
        .unwrap_err()
        .to_string();

    assert!(error.contains("nope.scm"), "{error}");
    assert!(
        error.contains(&plugin_dir.path().display().to_string()),
        "the folder that was searched should be named too: {error}"
    );
}

#[test]
fn a_script_plugin_resolves_with_no_user_plugin_dir_at_all() {
    let elsewhere = tempdir().unwrap();
    let explicit = elsewhere.path().join("keys.scm");
    fs::write(&explicit, "-- standalone").unwrap();

    assert_eq!(
        file_plugin(&explicit.display().to_string())
            .resolve_source(None)
            .unwrap(),
        (explicit, "-- standalone".to_owned())
    );
}

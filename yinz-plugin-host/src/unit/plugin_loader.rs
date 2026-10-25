use super::script_source;
use yinz_config::plugins::PluginConfig;
use yinz_types::data::PluginTag;
use yinz_types::input::layout::{RunPlugin, RunPluginLocation};

fn builtin_plugin(name: &str) -> PluginConfig {
    PluginConfig::from_run_plugin(&RunPlugin {
        location: RunPluginLocation::Yinz(PluginTag::new(name)),
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn builtins_use_only_selected_data_directory_and_reload_disk_edits() {
    let data = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    std::fs::write(user.path().join("agent-exit.scm"), "; impostor").unwrap();
    let plugin = builtin_plugin("agent-exit");
    let error = script_source(&plugin, Some(user.path()), data.path()).unwrap_err();
    assert!(error.to_string().contains("reinstall"));
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), data.path()).unwrap();
    let path = data.path().join("plugins/builtins/agent-exit.scm");
    for source in ["; first", "; second"] {
        std::fs::write(&path, source).unwrap();
        assert_eq!(
            script_source(&plugin, Some(user.path()), data.path()).unwrap(),
            (Some(path.clone()), source.to_owned())
        );
    }
    assert!(yinz_steel_builtins::builtin_path(data.path(), "../agent-exit").is_err());
    assert!(script_source(&plugin, None, user.path()).is_err());
}

#[test]
fn builtin_entrypoint_symlink_escape_is_rejected() {
    let data = tempfile::tempdir().unwrap();
    let outside = tempfile::Builder::new().suffix(".scm").tempfile().unwrap();
    let root = data.path().join("plugins/builtins");
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("agent-exit.scm")).unwrap();
    assert!(script_source(&builtin_plugin("agent-exit"), None, data.path()).is_err());
}

#[test]
fn builtin_dependencies_and_source_sizes_remain_confined() {
    let data = tempfile::tempdir().unwrap();
    let outside = tempfile::Builder::new().suffix(".scm").tempfile().unwrap();
    let root = data.path().join("plugins/builtins");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("agent-exit.scm");
    std::fs::write(&path, "; entry").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("dependency.scm")).unwrap();
    let (path, _) = script_source(&builtin_plugin("agent-exit"), None, data.path()).unwrap();
    let mut sources = yinz_steel::modules::SourceRoot::file(&path.unwrap(), None).unwrap();
    let entry = sources.entry_id().to_owned();
    assert!(sources.load(&entry, "./dependency.scm").is_err());
    assert!(sources.load(&entry, "../escape.scm").is_err());
    std::fs::write(
        root.join("agent-exit.scm"),
        vec![b' '; yinz_steel::MAX_SOURCE_BYTES + 1],
    )
    .unwrap();
    assert!(script_source(&builtin_plugin("agent-exit"), None, data.path()).is_err());
}

#[test]
fn source_resolves_the_file_and_reads_edits_on_reload() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("plugin.scm");
    let config = PluginConfig::from_run_plugin(&RunPlugin {
        location: RunPluginLocation::File(path.clone()),
        ..Default::default()
    })
    .unwrap();
    for source in ["(define title \"first\")", "(define title \"second\")"] {
        std::fs::write(&path, source).unwrap();
        assert_eq!(
            script_source(&config, None, directory.path()).unwrap(),
            (Some(path.canonicalize().unwrap()), source.to_owned())
        );
    }
}

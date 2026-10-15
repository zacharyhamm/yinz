use super::plugin_lifecycle_request;
use std::{fs, path::PathBuf};
use yinz_config::plugins::PluginConfig;
use yinz_types::{cli::PluginOperation, input::layout::RunPluginLocation};

#[test]
fn relative_file_url_preserves_plugin_directory_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let name = format!(
        "lifecycle-{}.scm",
        directory.path().file_name().unwrap().to_string_lossy()
    );
    let declared = std::env::current_dir().unwrap().join(&name);
    assert!(!declared.exists());
    let source = "(void)";
    let path = directory.path().join(&name);
    fs::write(&path, source).unwrap();

    let request = plugin_lifecycle_request(
        PluginOperation::Start,
        Some(&format!("file:{name}")),
        None,
        None,
        Some(directory.path()),
        None,
    )
    .unwrap();
    let plugin = request.fallback.unwrap();
    assert_eq!(plugin.relative_path, Some(PathBuf::from(name)));
    assert_eq!(plugin.declared_path, Some(declared.clone()));
    assert_eq!(plugin.location, RunPluginLocation::File(declared));
    assert_eq!(
        PluginConfig::from_run_plugin(&plugin)
            .unwrap()
            .resolve_source(Some(directory.path()))
            .unwrap(),
        (path, source.to_owned())
    );
}

#[test]
#[cfg(unix)]
fn symlink_file_url_preserves_declaration_for_identity_refresh() {
    use yinz_types::input::layout::RunPluginOrAlias;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let first = root.join("first.scm");
    let second = root.join("second.scm");
    let link = root.join("current.scm");
    for path in [&first, &second] {
        fs::write(path, "(void)").unwrap();
    }
    std::os::unix::fs::symlink(&first, &link).unwrap();
    let target = url::Url::from_file_path(&link).unwrap().to_string();
    let request = plugin_lifecycle_request(
        PluginOperation::Reload,
        Some(&target),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let plugin = request.fallback.unwrap();
    assert_eq!(plugin.relative_path, None);
    assert_eq!(plugin.declared_path, Some(link.clone()));
    assert_eq!(plugin.location, RunPluginLocation::File(first));

    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&second, &link).unwrap();
    let mut plugin = RunPluginOrAlias::RunPlugin(plugin);
    yinz_runtime::plugin_location::refresh_file_identity(&mut plugin);
    let refreshed = plugin.get_run_plugin().unwrap();
    assert_eq!(refreshed.declared_path, Some(link));
    assert_eq!(refreshed.location, RunPluginLocation::File(second));
}

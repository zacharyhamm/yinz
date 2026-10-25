use super::*;

#[test]
fn the_user_plugin_dir_hangs_off_the_config_dir() {
    assert_eq!(
        get_user_plugin_dir(Some(PathBuf::from("/etc/yinz"))),
        Some(PathBuf::from("/etc/yinz/plugins")),
    );
    assert_eq!(get_user_plugin_dir(None), None);
}

/// The whole point of the helper: a session started with `--config-dir` must not be handed
/// the conventional config dir's plugin folder, which is what the argument-less
/// `default_user_plugin_dir` would give it.
#[test]
fn a_supplied_config_dir_beats_the_conventional_ones() {
    assert_eq!(
        user_plugin_dir_or_default(None, Some(PathBuf::from("/etc/yinz"))),
        Some(PathBuf::from("/etc/yinz/plugins")),
    );
}

/// And a configured folder beats both, wherever the config dir happens to be.
#[test]
fn a_configured_folder_beats_the_config_dir() {
    assert_eq!(
        user_plugin_dir_or_default(
            Some(PathBuf::from("/srv/plugins")),
            Some(PathBuf::from("/etc/yinz")),
        ),
        Some(PathBuf::from("/srv/plugins")),
    );
}

/// `find_default_config_dir` is `None` under `cfg(test)`, so with nothing supplied there is
/// nothing to fall back to and the answer is honestly "no folder" rather than a guess.
#[test]
fn no_config_dir_anywhere_resolves_to_no_plugin_dir() {
    assert_eq!(user_plugin_dir_or_default(None, None), None);
}

/// The reason the helper absolutizes at all: a plugin is handed this path and has no cwd to
/// join it onto, so a relative folder has to be resolved here or not at all.
#[test]
fn a_relative_configured_folder_is_resolved_against_the_cwd() {
    let cwd = std::env::current_dir().expect("a cwd");
    assert_eq!(
        user_plugin_dir_or_default(Some(PathBuf::from("plugins")), None),
        Some(cwd.join("plugins")),
    );
}

#[test]
fn a_relative_config_dir_yields_an_absolute_plugin_folder() {
    let cwd = std::env::current_dir().expect("a cwd");
    assert_eq!(
        user_plugin_dir_or_default(None, Some(PathBuf::from("relcfg"))),
        Some(cwd.join("relcfg/plugins")),
    );
}

#[test]
fn absolutizing_folds_away_here_and_parent_segments() {
    assert_eq!(
        absolute_lexical(Path::new("/a/b/../c")),
        PathBuf::from("/a/c")
    );
    assert_eq!(absolute_lexical(Path::new("/a/./b")), PathBuf::from("/a/b"));
    assert_eq!(absolute_lexical(Path::new("/a/b/")), PathBuf::from("/a/b"));
}

/// As on the filesystem: the root is its own parent.
#[test]
fn the_root_has_no_parent_to_climb_to() {
    assert_eq!(absolute_lexical(Path::new("/..")), PathBuf::from("/"));
    assert_eq!(absolute_lexical(Path::new("/")), PathBuf::from("/"));
}

#[test]
fn a_relative_path_climbs_out_of_the_cwd_lexically() {
    let cwd = std::env::current_dir().expect("a cwd");
    let parent = cwd.parent().unwrap_or(&cwd);
    assert_eq!(
        absolute_lexical(Path::new("../plugins")),
        parent.join("plugins")
    );
}

/// It must remain under the configuration directory, independently of data storage
/// entirely - the two are told apart only by which of these functions produced them.
#[test]
fn the_user_plugin_dir_is_not_created_by_asking_for_it() {
    let config_dir = tempfile::tempdir().expect("a temp dir");
    let plugin_dir =
        get_user_plugin_dir(Some(config_dir.path().to_path_buf())).expect("a dir was supplied");
    assert!(
        !plugin_dir.exists(),
        "asking for the path must not make the folder"
    );
}

use std::{collections::BTreeMap, fs};
use yinz_runtime::session_checkpoint::{write_checkpoint, SessionCheckpoint, LAYOUT};
use yinz_types::input::cli_assets::CliAssets;
use {yinz_types::data::LayoutInfo, yinz_types::data::SessionInfo};
const CURRENT: &str = "current";

fn layout(contents: &str) -> (String, BTreeMap<String, String>) {
    (
        "layout {\n tab {\n pane contents_file=\"initial_contents_1\"\n }\n}\n".to_owned(),
        BTreeMap::from([("initial_contents_1".to_owned(), contents.to_owned())]),
    )
}

#[test]
fn server_startup_loads_the_checkpoint_after_a_session_rename() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("old");
    let renamed = root.path().join("renamed");
    write_checkpoint(
        &old,
        &yinz_session::metadata::session_info::to_string(&SessionInfo::default()).unwrap(),
        &layout("restored"),
    )
    .unwrap();
    fs::rename(old, &renamed).unwrap();
    let assets = CliAssets {
        should_ignore_config: true,
        layout: Some(LayoutInfo::File(
            renamed.join(LAYOUT).display().to_string(),
            Default::default(),
        )),
        ..Default::default()
    };
    let (_, loaded) = yinz_config::cli_assets::load_config_and_layout(&assets).unwrap();
    assert_eq!(
        loaded.tabs()[0].1.children[0]
            .pane_initial_contents
            .as_deref(),
        Some("restored")
    );
    // Corrupt current pointers must not resurrect a stale flat layout or the default layout.
    fs::write(renamed.join(LAYOUT), layout("stale").0).unwrap();
    fs::remove_file(renamed.join(CURRENT)).unwrap();
    std::os::unix::fs::symlink(".checkpoint-missing", renamed.join(CURRENT)).unwrap();
    assert!(SessionCheckpoint::read(&renamed).is_err());
    assert!(yinz_config::cli_assets::load_config_and_layout(&assets).is_err());
}

#[test]
fn checkpoint_lock_covers_parsing_pane_contents() {
    let root = tempfile::tempdir().unwrap();
    write_checkpoint(root.path(), "old", &layout("restored")).unwrap();
    let checkpoint = SessionCheckpoint::read(root.path()).unwrap();
    let writer = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.path().join(".checkpoint.lock"))
        .unwrap();
    assert!(matches!(
        writer.try_lock(),
        Err(fs::TryLockError::WouldBlock)
    ));
    let parsed = yinz_config::layout::from_path_without_config(&checkpoint.layout_path()).unwrap();
    assert_eq!(
        parsed.tabs()[0].1.children[0]
            .pane_initial_contents
            .as_deref(),
        Some("restored")
    );
    assert!(matches!(
        writer.try_lock(),
        Err(fs::TryLockError::WouldBlock)
    ));
    drop(checkpoint);
    writer.try_lock().unwrap();
}

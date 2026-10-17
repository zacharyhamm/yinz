use super::LastProject;
use std::io::Read;

#[test]
fn last_project_replaces_atomically_and_round_trips_names() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let mut preference = LastProject::load(data_dir.clone());
    assert_eq!(preference.name(), None);
    preference.remember("old");
    drop(preference);
    let mut old_file = std::fs::File::open(data_dir.join("last-project.json")).unwrap();
    let name = "project \"quoted\" 日本語";
    let mut preference = LastProject::load(data_dir.clone());
    preference.remember(name);
    drop(preference);
    assert_eq!(LastProject::load(data_dir.clone()).name().unwrap(), name);
    let mut old_contents = String::new();
    old_file.read_to_string(&mut old_contents).unwrap();
    assert_eq!(old_contents, "\"old\"");
    assert_eq!(std::fs::read_dir(data_dir).unwrap().count(), 1);
}

#[test]
fn slow_saves_do_not_block_selection_and_only_the_latest_pending_project_is_saved() {
    use std::sync::mpsc::channel;
    use std::time::Duration;

    let (started, received) = channel();
    let (release, released) = channel();
    let (saved, saves) = channel();
    let writer = super::Writer::start(move |name| {
        saved.send(name.to_owned()).unwrap();
        if name == "first" {
            started.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(10)).unwrap();
            // A failed write must not stop the latest selection from being saved.
            anyhow::bail!("simulated disk error");
        }
        Ok(())
    })
    .unwrap();
    let mut preference = LastProject {
        name: None,
        writer: Some(writer),
    };
    preference.remember("first");
    received.recv_timeout(Duration::from_secs(10)).unwrap();
    for i in 0..1000 {
        preference.remember(&format!("project-{i}"));
    }
    assert_eq!(preference.name().map(String::as_str), Some("project-999"));
    release.send(()).unwrap();
    // Dropping the preference must finish the final write before shutdown returns.
    drop(preference);
    assert_eq!(
        saves.into_iter().collect::<Vec<_>>(),
        ["first", "project-999"]
    );
}

#[test]
fn last_project_ignores_malformed_files_and_survives_write_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("last-project.json");
    for contents in ["not json", "null", "{}", "123"] {
        std::fs::write(&path, contents).unwrap();
        assert_eq!(LastProject::load(dir.path().into()).name(), None);
    }
    let mut preference = LastProject::load(path);
    preference.remember("selected");
    assert_eq!(preference.name().map(String::as_str), Some("selected"));
}

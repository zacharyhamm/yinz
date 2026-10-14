use super::LastProject;
use std::io::Read;

#[test]
fn last_project_replaces_atomically_and_round_trips_names() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let mut preference = LastProject::load(data_dir.clone());
    assert_eq!(preference.name(), None);
    preference.remember("old");
    let mut old_file = std::fs::File::open(data_dir.join("last-project.json")).unwrap();
    let name = "project \"quoted\" 日本語";
    preference.remember(name);
    assert_eq!(LastProject::load(data_dir.clone()).name().unwrap(), name);
    let mut old_contents = String::new();
    old_file.read_to_string(&mut old_contents).unwrap();
    assert_eq!(old_contents, "\"old\"");
    assert_eq!(std::fs::read_dir(data_dir).unwrap().count(), 1);
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

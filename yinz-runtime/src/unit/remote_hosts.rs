use super::*;
#[test]
fn reload_deduplicate_and_merge_concurrent_writers() {
    let dir = tempfile::tempdir().unwrap();
    std::thread::scope(|scope| {
        for host in ["alias", "user@host", "alias", "HOST"] {
            let path = dir.path();
            scope.spawn(move || remember(path, host).unwrap());
        }
    });
    assert_eq!(load(dir.path()).unwrap(), ["HOST", "alias", "user@host"]);
    remember(dir.path(), "alias").unwrap();
    assert_eq!(load(dir.path()).unwrap().len(), 3);
}
#[test]
fn missing_is_empty_and_corrupt_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load(dir.path()).unwrap().is_empty());
    let file = dir.path().join("remote-hosts.json");
    std::fs::write(&file, "broken").unwrap();
    assert!(load(dir.path()).is_err());
    assert!(remember(dir.path(), "host").is_err());
    assert_eq!(std::fs::read_to_string(file).unwrap(), "broken");
}

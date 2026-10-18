use super::*;
use serde_json::json;
use yinz_types::project_provider::{register, unregister_owner, ProjectProvider};

#[test]
fn project_matches_validate_containment_namespaces_and_generation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("child")).unwrap();
    let make = |id: &str| ProjectProvider {
        id: id.into(),
        priority: 1,
        ..Default::default()
    };
    register(make("marker"), 10, 1).unwrap();
    register(make("other"), 20, 2).unwrap();
    let input = json!({"provider":"marker", "generation":10, "cwd":root.join("child"), "result":{"identity":"same", "name":"Work", "root": root}});
    let first = validate(input.clone()).unwrap();
    assert!(first.get("common_dir").is_none());
    assert!(first.get("host").is_none());
    let mut other = input.clone();
    other["provider"] = json!("other");
    other["generation"] = json!(20);
    assert_ne!(
        first["provider_identity"],
        validate(other).unwrap()["provider_identity"]
    );
    for (key, value) in [
        ("identity", json!("")),
        ("name", json!(" \n")),
        ("root", json!("relative")),
        ("host", json!("forged")),
        ("provider_identity", json!("forged")),
    ] {
        let mut bad = input.clone();
        bad["result"][key] = value;
        assert!(validate(bad).is_err(), "{key}");
    }
    let elsewhere = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), root.join("escape")).unwrap();
    let mut bad = input.clone();
    bad["cwd"] = json!(root.join("escape"));
    assert!(validate(bad).is_err());
    let publication = json!({"providers":yinz_types::project_provider::providers(), "locations": {root.to_str().unwrap():first}});
    assert!(publish(30, publication.clone()).unwrap());
    assert!(!publish(30, publication.clone()).unwrap());
    assert!(!clear(29));
    assert!(!snapshot().is_empty());
    unregister_owner(10);
    assert!(validate(input).is_err());
    assert!(publish(30, publication).is_err());
    assert!(clear(30));
    assert!(snapshot().is_empty());
    unregister_owner(20);
}

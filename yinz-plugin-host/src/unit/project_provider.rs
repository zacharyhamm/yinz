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

fn git(directory: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn resolve_project_primary_subdirectory_linked_worktree_and_unregistered_folder() {
    use project_data::projects::{self, Registry};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-b", "main"]);
    git(
        &root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
    std::fs::create_dir(root.join("src")).unwrap();
    git(
        &root,
        &["worktree", "add", "-b", "topic", ".worktrees/topic"],
    );
    let linked = root.join(".worktrees/topic");
    std::fs::create_dir(linked.join("src")).unwrap();
    let mut project = projects::validate("Display name", root.to_str().unwrap()).unwrap();
    project.common_dir = Some(root.join(".git").to_str().unwrap().into());
    for discovered in [false, true] {
        project.discovered = discovered;
        let registry = Registry::from([(project.name.clone(), project.clone())]);
        for directory in [&root, &root.join("src"), &linked, &linked.join("src")] {
            let resolved = resolve_directory(directory, &registry, Default::default())
                .unwrap()
                .unwrap();
            assert_eq!(resolved.identity, format!("project:{}", root.display()));
            assert_eq!(resolved.name, "Display name");
            assert_eq!(resolved.root.as_deref(), root.to_str());
        }
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_directory(outside.path(), &registry, Default::default()).unwrap(),
            None
        );
    }
    // Even before discovery persists the checkout, arrangement groups its worktrees together.
    let primary = resolve_directory(&root, &Registry::new(), Default::default())
        .unwrap()
        .unwrap();
    let worktree = resolve_directory(&linked, &Registry::new(), Default::default())
        .unwrap()
        .unwrap();
    assert_eq!(primary, worktree);
    assert_eq!(primary.root.as_deref(), root.to_str());
    assert!(
        resolve_directory(&root.join("missing"), &Registry::new(), Default::default()).is_err()
    );
    assert!(resolve_directory(
        &root.join(".git/HEAD"),
        &Registry::new(),
        Default::default()
    )
    .is_err());
}

#[test]
fn resolve_project_multi_repository_root_and_rename_keep_one_identity() {
    use project_data::projects::{self, Registry};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut project = projects::validate("Both repositories", root.to_str().unwrap()).unwrap();
    for name in ["first", "second"] {
        let repository = root.join(name);
        std::fs::create_dir(&repository).unwrap();
        git(&repository, &["init", "-b", "main"]);
    }
    for name in ["Both repositories", "Renamed"] {
        project.name = name.into();
        let registry = Registry::from([(name.into(), project.clone())]);
        for directory in [&root, &root.join("first"), &root.join("second")] {
            let resolved = resolve_directory(directory, &registry, Default::default())
                .unwrap()
                .unwrap();
            assert_eq!(resolved.identity, format!("project:{}", root.display()));
            assert_eq!(resolved.name, name);
            assert_eq!(resolved.root.as_deref(), root.to_str());
        }
    }
}

#[test]
fn resolve_project_symlinked_registration_preserves_identity_and_containment() {
    use project_data::projects::{self, Registry};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let alias = temp.path().join("alias");
    let outside = temp.path().join("project-old");
    std::fs::create_dir_all(root.join("child")).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
    let project = projects::validate("Remembered folder", alias.to_str().unwrap()).unwrap();
    let missing =
        projects::validate("Missing", temp.path().join("missing").to_str().unwrap()).unwrap();
    let registry = Registry::from([
        (project.name.clone(), project.clone()),
        (missing.name.clone(), missing),
    ]);
    for directory in [&alias, &root, &alias.join("child"), &root.join("child")] {
        let resolved = resolve_directory(directory, &registry, Default::default())
            .unwrap()
            .unwrap();
        assert_eq!(resolved.identity, format!("project:{}", project.root));
        assert_eq!(resolved.name, project.name);
        assert_eq!(resolved.root.as_deref(), Some(project.root.as_str()));
    }
    for directory in [&outside, &alias.join("escape")] {
        assert_eq!(
            resolve_directory(directory, &registry, Default::default()).unwrap(),
            None
        );
    }
}

#[test]
fn resolve_project_prefers_deepest_canonical_registered_root() {
    use project_data::projects::{self, Registry};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let parent_alias = temp.path().join("long/alias/path");
    let child_alias = temp.path().join("child-alias");
    std::fs::create_dir_all(root.join("child/src")).unwrap();
    std::fs::create_dir_all(parent_alias.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&root, &parent_alias).unwrap();
    std::os::unix::fs::symlink(root.join("child"), &child_alias).unwrap();
    let parent = projects::validate("Parent", parent_alias.to_str().unwrap()).unwrap();
    let child = projects::validate("Child", child_alias.to_str().unwrap()).unwrap();
    let registry = Registry::from([
        (parent.name.clone(), parent),
        (child.name.clone(), child.clone()),
    ]);
    let resolved = resolve_directory(&root.join("child/src"), &registry, Default::default())
        .unwrap()
        .unwrap();
    assert_eq!(resolved.identity, format!("project:{}", child.root));
    assert_eq!(resolved.name, child.name);
    assert_eq!(resolved.root.as_deref(), Some(child.root.as_str()));
}

#[test]
fn resolve_project_preserves_published_provider_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-b", "main"]);
    std::fs::create_dir(root.join("child")).unwrap();
    let location = Location::local(root.to_string_lossy());
    let repository = Repository {
        root: location.path.clone(),
        provider_identity: Some("provider-id".into()),
        display_name: Some("Custom project".into()),
        ..Default::default()
    };
    let probed = Probed::from([(location.clone(), repository)]);
    assert_eq!(
        resolve_directory(&root.join("child"), &Default::default(), probed.clone()).unwrap(),
        project_data::projects::resolve(&probed, &Default::default(), &location)
    );
}

use crate::location::Location;
use crate::projects::*;

fn repository(provider: &str, root: &str) -> Repository {
    Repository {
        root: root.into(),
        provider_identity: Some(serde_json::to_string(&(provider, "work")).unwrap()),
        display_name: Some("Workspace".into()),
        branch: Some("topic".into()),
        ..Repository::default()
    }
}

#[test]
fn provider_identity_drives_discovery_labels_storage_and_overrides() {
    let first = repository("marker", "/a");
    let second = repository("marker", "/b");
    let other = repository("other", "/c");
    let mut registry = Registry::new();
    discover(
        &mut registry,
        &[first.clone(), second.clone(), other.clone()],
    );
    assert_eq!(registry.len(), 2);
    assert_eq!(
        for_repository(&registry, &first),
        for_repository(&registry, &second)
    );
    assert_ne!(
        for_repository(&registry, &first),
        for_repository(&registry, &other)
    );
    assert_eq!(first.checkout_label().as_deref(), Some("Workspace:topic"));
    assert!(first.common_dir.is_none());
    assert!(first.remote.is_none());
    assert_eq!(from_rows(&to_rows(&registry)), registry);
    assert_eq!(
        parse_legacy(serde_json::to_string(&registry).unwrap().as_bytes()),
        registry
    );
    let location = Location::local("/b/src");
    let probed = Probed::from([(location.clone(), second)]);
    assert_eq!(
        registered_for_cwd(&probed, &registry, &location)
            .unwrap()
            .name,
        "Workspace"
    );
    registry.insert("Explicit".into(), validate("Explicit", "/b").unwrap());
    assert_eq!(
        registered_for_cwd(&probed, &registry, &location)
            .unwrap()
            .name,
        "Explicit"
    );
    assert_eq!(
        label_for_cwd(&probed, &Registry::new(), &location).as_deref(),
        Some("Workspace")
    );
}

#[test]
fn nested_provider_projects_stay_distinct_and_reload_updates_discovery() {
    let parent = repository("first", "/work");
    let child = repository("second", "/work/child");
    let mut registry = Registry::new();
    discover(&mut registry, &[parent, child.clone()]);
    assert_eq!(registry.len(), 2);
    assert_eq!(
        for_repository(&registry, &child).unwrap().root,
        "/work/child"
    );
    let replacement = repository("replacement", "/work/child");
    assert!(discover(&mut registry, std::slice::from_ref(&replacement)).changed());
    assert!(for_repository(&registry, &child).is_none());
    assert!(for_repository(&registry, &replacement).is_some());
}

#[test]
fn provider_storage_upgrade_preserves_rows_and_guarded_rename_and_merge() {
    use crate::store::{self, Backend};
    use yinz_tile::prelude::{DataValue, FetchRequest, Mutation};
    let backend = store::Local::private("provider-test");
    let mut old = store::project_def();
    old.attrs.retain(|attr| attr.name != "provider_identity");
    backend.define(&old).unwrap();
    backend
        .apply(vec![Mutation::put("project")
            .uid("legacy")
            .set("name", "legacy")
            .set("root", "/legacy")
            .build()])
        .unwrap();
    let before = backend.fetch(&FetchRequest::entity("project")).unwrap();
    backend.define_all(&store::shared_defs()).unwrap();
    let after = backend.fetch(&FetchRequest::entity("project")).unwrap();
    for key in ["id", "rev", "root", "name"] {
        assert_eq!(
            before.get(&before.rows[0], key),
            after.get(&after.rows[0], key)
        );
    }
    assert_eq!(
        after.get(&after.rows[0], "provider_identity"),
        Some(&DataValue::Null)
    );
    let mut first = validate("first", "/a").unwrap();
    first.provider_identity = repository("marker", "/a").provider_identity;
    let mut second = validate("second", "/b").unwrap();
    second
        .provider_identity
        .clone_from(&first.provider_identity);
    backend
        .apply(vec![to_mutation(&first), to_mutation(&second)])
        .unwrap();
    let rows = backend.fetch(&FetchRequest::entity("project")).unwrap();
    let rename = plan_rename(&rows, "first", "renamed").unwrap();
    backend.apply(rename.mutations).unwrap();
    assert!(
        backend
            .apply(plan_merge(&rows, "first", "second").unwrap().mutations)
            .is_err(),
        "old revisions must conflict"
    );
    let rows = backend.fetch(&FetchRequest::entity("project")).unwrap();
    let merged = plan_merge(&rows, "renamed", "second").unwrap();
    backend.apply(merged.mutations).unwrap();
    let rows = backend.fetch(&FetchRequest::entity("project")).unwrap();
    let registry = from_rows(&rows);
    assert_eq!(
        registry["second"].provider_identity,
        first.provider_identity
    );
    assert!(!registry.contains_key("renamed"));
    assert!(!registry["second"].replaced_ids.is_empty());
}

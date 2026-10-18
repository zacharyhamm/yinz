use crate::project_provider::*;

#[test]
fn providers_sort_validate_ownership_and_remove_generations() {
    let provider = |id: &str, priority| ProjectProvider {
        id: id.into(),
        priority,
        ..Default::default()
    };
    assert!(register(provider("git", 10), 1, 1).is_err());
    assert!(register(provider("bad:id", 10), 1, 1).is_err());
    register(provider("beta", 4), 1, 11).unwrap();
    register(provider("alpha", 4), 1, 11).unwrap();
    register(provider("first", 5), 2, 12).unwrap();
    assert!(register(provider("alpha", 99), 2, 12).is_err());
    let snapshot = providers();
    assert_eq!(
        snapshot.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["first", "alpha", "beta"]
    );
    assert_eq!((snapshot[0].generation, snapshot[0].plugin_id), (2, 12));
    assert!(unregister_owner(1));
    assert_eq!(providers().len(), 1);
    assert!(!unregister_owner(1));
    assert!(unregister_owner(2));
}

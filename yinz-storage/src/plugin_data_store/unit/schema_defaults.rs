use super::*;
use yinz_types::plugin_data::AttrDef;

#[test]
fn quality_pass_invalid_json_defaults_cannot_create_or_extend_an_entity() {
    let dir = tempfile::tempdir().unwrap();
    let store = PluginDataStore::new(dir.path());
    let namespace = "shared:test";
    let original = EntityDef::new("note", 1).attr(AttrDef::text("title"));
    let invalid = original
        .clone()
        .attr(AttrDef::json("payload").default("{not json"));

    let result = store.define_entity(namespace, "test", &invalid, false);
    assert!(
        matches!(result, Err(DataError::InvalidSchema(_))),
        "{result:?}"
    );
    assert!(store.catalog(Some(namespace)).unwrap().is_empty());

    store
        .define_entity(namespace, "test", &original, false)
        .unwrap();
    store
        .apply(
            namespace,
            &[Mutation::put("note").set("title", "retained").build()],
        )
        .unwrap();
    let result = store.define_entity(namespace, "test", &invalid, false);
    assert!(
        matches!(result, Err(DataError::InvalidSchema(_))),
        "{result:?}"
    );
    assert_eq!(store.catalog(Some(namespace)).unwrap()[0].def, original);
    let rows = store
        .fetch(namespace, &FetchRequest::entity("note"))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!rows.columns.iter().any(|column| column == "payload"));

    let valid = original.attr(AttrDef::json("payload").default(r#"{"ready":true}"#));
    store
        .define_entity(namespace, "test", &valid, false)
        .unwrap();
    let rows = store
        .fetch(namespace, &FetchRequest::entity("note"))
        .unwrap();
    assert_eq!(
        rows.get(&rows.rows[0], "payload"),
        Some(&yinz_types::plugin_data::DataValue::Text(
            r#"{"ready":true}"#.into()
        ))
    );
}

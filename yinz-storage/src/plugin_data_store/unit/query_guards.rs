use super::*;
use yinz_types::internal_ids::DataRowId;

#[test]
fn quality_count_ignores_pagination_without_losing_filters_or_search() {
    let (store, _dir) = defined();
    put(&store, "keep", "needle", 1);
    put(&store, "skip", "needle", 2);
    put(&store, "keep", "other", 3);
    let request = FetchRequest::entity("agent")
        .filter(Pred::eq("project", "keep"))
        .limit(1)
        .offset(1)
        .after_id(DataRowId::new(100));
    assert!(store.fetch(NS, &request).unwrap().is_empty());
    assert_eq!(store.count(NS, &request).unwrap(), 2);
    assert_eq!(store.count(NS, &request.search("needle")).unwrap(), 1);
}

#[test]
fn quality_foreign_schema_declarations_cannot_remove_existing_indexes() {
    let (store, _dir) = store();
    let def = EntityDef::new("record", 1).attr(AttrDef::text("key").unique().indexed().full_text());
    store.define_entity(NS, OWNER, &def, false).unwrap();
    store
        .apply(NS, &[Mutation::put("record").set("key", "kept").build()])
        .unwrap();
    for flag in ["unique", "indexed", "full_text"] {
        let mut changed = def.clone();
        match flag {
            "unique" => changed.attrs[0].unique = false,
            "indexed" => changed.attrs[0].indexed = false,
            _ => changed.attrs[0].full_text = false,
        }
        let error = store
            .define_entity(NS, "other-plugin", &changed, false)
            .unwrap_err();
        assert_err!(error, DataError::RefusedMigration(_));
        assert_eq!(store.catalog(Some(NS)).unwrap()[0].def, def);
    }
    assert!(store
        .apply(NS, &[Mutation::put("record").set("key", "kept").build()])
        .is_err());
    assert_eq!(
        store
            .fetch(NS, &FetchRequest::entity("record").search("kept"))
            .unwrap()
            .len(),
        1
    );
    let extended = def.attr(AttrDef::text("extra"));
    store
        .define_entity(NS, "other-plugin", &extended, false)
        .unwrap();
}

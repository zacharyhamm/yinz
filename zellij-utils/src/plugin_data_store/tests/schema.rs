use super::*;

#[test]
fn defining_an_entity_reports_that_it_was_created() {
    let (store, _dir) = store();
    let info = store.define_entity(NS, OWNER, &agent_def(), false).unwrap();
    assert!(info.created);
    assert_eq!(info.namespace, NS);
    assert_eq!(info.entity, "agent");
    assert!(info.full_text_rebuilt);
    assert_eq!(
        info.indexes_created,
        vec!["project", "branch", "updated_at_ms"]
    );
}

#[test]
fn long_attribute_indexes_enforce_uniqueness_independently_and_can_be_removed() {
    let (store, _dir) = store();
    let prefix = "a".repeat(62);
    let first = format!("{prefix}b");
    let second = format!("{prefix}c");
    let mut def = EntityDef::new("agent", 1)
        .attr(AttrDef::text(&first).indexed().unique())
        .attr(AttrDef::text(&second).indexed().unique());
    store.define_entity(NS, OWNER, &def, false).unwrap();
    let put = |a: &str, b: &str| {
        store.apply(
            NS,
            &[Mutation::put("agent")
                .set(&first, a)
                .set(&second, b)
                .build()],
        )
    };
    put("one", "one").unwrap();
    assert_err!(put("one", "two").unwrap_err(), DataError::Storage(_));
    assert_err!(put("two", "one").unwrap_err(), DataError::Storage(_));

    def.attrs[0].indexed = false;
    def.attrs[0].unique = false;
    store.define_entity(NS, OWNER, &def, false).unwrap();
    put("one", "two").unwrap();
    assert_err!(put("three", "one").unwrap_err(), DataError::Storage(_));

    def.attrs[1].indexed = false;
    def.attrs[1].unique = false;
    store.define_entity(NS, OWNER, &def, false).unwrap();
    put("one", "one").unwrap();
    assert_eq!(store.count(NS, &FetchRequest::entity("agent")).unwrap(), 3);

    // SQLite refuses to drop a column if its regular index was left behind.
    def.version = 2;
    def.attrs = vec![AttrDef::text("remaining")];
    store.define_entity(NS, OWNER, &def, true).unwrap();
}

#[test]
fn redeclaring_an_identical_definition_changes_nothing() {
    let (store, _dir) = defined();
    put(&store, "zellij", "hello", 1);
    let info = store.define_entity(NS, OWNER, &agent_def(), false).unwrap();
    assert!(!info.created);
    assert!(info.attrs_added.is_empty());
    assert!(info.indexes_created.is_empty());
    assert!(!info.full_text_rebuilt);
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn adding_an_attribute_preserves_existing_rows() {
    let (store, _dir) = defined();
    put(&store, "zellij", "hello", 1);
    let extended = agent_def().attr(AttrDef::text("worktree").indexed());
    let info = store.define_entity(NS, OWNER, &extended, false).unwrap();
    assert_eq!(info.attrs_added, vec!["worktree"]);
    assert_eq!(info.indexes_created, vec!["worktree"]);

    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(texts(&rows, "project"), vec!["zellij"]);
    // the new column exists, and is null on the row that predates it
    assert_eq!(column(&rows, "worktree"), vec![DataValue::Null]);
}

#[test]
fn a_required_attribute_added_without_a_default_is_refused() {
    let (store, _dir) = defined();
    let extended = agent_def().attr(AttrDef::text("worktree").required());
    let err = store
        .define_entity(NS, OWNER, &extended, false)
        .unwrap_err();
    assert_err!(err, DataError::RefusedMigration(_));
}

#[test]
fn a_required_attribute_added_with_a_default_backfills_the_existing_rows() {
    let (store, _dir) = defined();
    put(&store, "zellij", "hello", 1);
    let extended = agent_def().attr(AttrDef::text("worktree").required().default("primary"));
    store.define_entity(NS, OWNER, &extended, false).unwrap();
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert_eq!(texts(&rows, "worktree"), vec!["primary"]);
}

#[test]
fn changing_the_type_of_an_attribute_is_refused() {
    let (store, _dir) = defined();
    let retyped = EntityDef::new("agent", 2)
        .attr(AttrDef::int("project"))
        .attr(AttrDef::text("branch"))
        .attr(AttrDef::text("summary"))
        .attr(AttrDef::int("updated_at_ms"));
    let err = store.define_entity(NS, OWNER, &retyped, false).unwrap_err();
    assert_err!(err, DataError::RefusedMigration(_));
}

#[test]
fn dropping_an_attribute_is_refused_unless_it_is_asked_for() {
    let (store, _dir) = defined();
    let shrunk = EntityDef::new("agent", 2)
        .attr(AttrDef::text("project").indexed())
        .attr(AttrDef::text("branch").indexed());
    let err = store.define_entity(NS, OWNER, &shrunk, false).unwrap_err();
    assert_err!(err, DataError::RefusedMigration(_));
    // the definition is untouched, so nothing was half-migrated
    assert!(store.catalog(Some(NS)).unwrap()[0]
        .def
        .attr_named("summary")
        .is_some());
}

#[test]
fn dropping_an_attribute_also_needs_a_version_above_the_stored_one() {
    let (store, _dir) = defined();
    let shrunk = EntityDef::new("agent", 1)
        .attr(AttrDef::text("project").indexed())
        .attr(AttrDef::text("branch").indexed())
        .attr(AttrDef::int("updated_at_ms").indexed());
    let err = store.define_entity(NS, OWNER, &shrunk, true).unwrap_err();
    assert_err!(err, DataError::RefusedMigration(_));
}

#[test]
fn dropping_an_attribute_works_when_asked_for_with_a_new_version() {
    let (store, _dir) = defined();
    put(&store, "zellij", "hello", 1);
    let shrunk = EntityDef::new("agent", 2)
        .attr(AttrDef::text("project").indexed())
        .attr(AttrDef::text("branch").indexed())
        .attr(AttrDef::int("updated_at_ms").indexed());
    store.define_entity(NS, OWNER, &shrunk, true).unwrap();
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    assert!(rows.column_index("summary").is_none());
    assert_eq!(texts(&rows, "project"), vec!["zellij"]);
}

#[test]
fn marking_an_attribute_searchable_later_indexes_the_rows_already_stored() {
    let (store, _dir) = store();
    let plain = EntityDef::new("agent", 1)
        .attr(AttrDef::text("project"))
        .attr(AttrDef::text("summary"));
    store.define_entity(NS, OWNER, &plain, false).unwrap();
    store
        .apply(
            NS,
            &[Mutation::put("agent")
                .set("project", "zellij")
                .set("summary", "a rebase conflict")
                .build()],
        )
        .unwrap();
    // nothing searchable yet, so searching is a mistake rather than an empty result
    let err = store
        .fetch(NS, &FetchRequest::entity("agent").search("rebase"))
        .unwrap_err();
    assert_err!(err, DataError::InvalidQuery(_));

    let searchable = EntityDef::new("agent", 2)
        .attr(AttrDef::text("project"))
        .attr(AttrDef::text("summary").full_text());
    let info = store.define_entity(NS, OWNER, &searchable, false).unwrap();
    assert!(info.full_text_rebuilt);
    // the rebuild backfills, so a row written before the index existed is findable
    let rows = store
        .fetch(NS, &FetchRequest::entity("agent").search("rebase"))
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn a_second_plugin_may_extend_a_shared_entity_but_not_shrink_it() {
    let (store, _dir) = store();
    let shared = "shared:agents";
    store
        .define_entity(shared, "zellij:hocus", &agent_def(), false)
        .unwrap();
    // extending is fine: the owner's columns are all still there
    let extended = agent_def().attr(AttrDef::text("worktree"));
    store
        .define_entity(shared, "zellij:other", &extended, false)
        .unwrap();
    // shrinking is not, whatever it claims, because the owner is still reading those columns
    let shrunk = EntityDef::new("agent", 9).attr(AttrDef::text("project"));
    let err = store
        .define_entity(shared, "zellij:other", &shrunk, true)
        .unwrap_err();
    assert_err!(err, DataError::RefusedMigration(_));
}

#[test]
fn namespaces_are_separate_stores() {
    let (store, _dir) = store();
    store
        .define_entity("plugin:a", "a", &agent_def(), false)
        .unwrap();
    store
        .define_entity("plugin:b", "b", &agent_def(), false)
        .unwrap();
    store
        .apply(
            "plugin:a",
            &[Mutation::put("agent").set("project", "only-in-a").build()],
        )
        .unwrap();
    let a = store
        .fetch("plugin:a", &FetchRequest::entity("agent"))
        .unwrap();
    let b = store
        .fetch("plugin:b", &FetchRequest::entity("agent"))
        .unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(b.len(), 0);
}

#[test]
fn a_private_namespace_is_derived_from_the_plugin_url() {
    assert_eq!(private_namespace("zellij:hocus"), "plugin:zellij_hocus");
    // a debug and a release build of the same plugin are different plugins here, exactly as
    // they are for permissions
    assert_ne!(
        private_namespace("file:/target/debug/x.wasm"),
        private_namespace("file:/target/release/x.wasm")
    );
    assert!(!is_shared_namespace(&private_namespace("zellij:hocus")));
    assert!(is_shared_namespace("shared:agents"));
}

#[test]
fn using_an_undeclared_entity_says_so() {
    let (store, _dir) = store();
    let err = store.fetch(NS, &FetchRequest::entity("agent")).unwrap_err();
    assert_err!(err, DataError::UnknownEntity(_));
}

/// The catalog is the discovery seam: an author who does not know an entity's name cannot fetch
/// it, so what it must answer with is the definition itself, not a summary of it.
#[test]
fn the_catalog_lists_declared_entities_with_their_definitions() {
    let (store, _dir) = defined();
    let entities = store.catalog(None).unwrap();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].namespace, NS);
    assert_eq!(entities[0].owner_plugin, OWNER);
    assert_eq!(entities[0].def, agent_def());
}

/// Ordered by namespace and then entity, so a listing does not shuffle between calls - and
/// filtered to one namespace when asked, which is what a plugin reading someone else's data does.
#[test]
fn the_catalog_spans_namespaces_and_filters_to_one() {
    let (store, _dir) = defined();
    let other = "shared:elsewhere";
    store
        .define_entity(
            other,
            "zellij:other",
            &EntityDef::new("note", 1).attr(AttrDef::text("text")),
            false,
        )
        .unwrap();
    store
        .define_entity(
            other,
            "zellij:other",
            &EntityDef::new("chat", 1).attr(AttrDef::text("title")),
            false,
        )
        .unwrap();

    let all: Vec<(String, String)> = store
        .catalog(None)
        .unwrap()
        .into_iter()
        .map(|e| (e.namespace, e.def.name))
        .collect();
    assert_eq!(
        all,
        vec![
            (NS.to_owned(), "agent".to_owned()),
            (other.to_owned(), "chat".to_owned()),
            (other.to_owned(), "note".to_owned()),
        ]
    );

    let scoped: Vec<String> = store
        .catalog(Some(other))
        .unwrap()
        .into_iter()
        .map(|e| e.def.name)
        .collect();
    assert_eq!(scoped, vec!["chat".to_owned(), "note".to_owned()]);
}

/// A namespace nobody has declared anything in is empty, not an error: asking what is there is
/// exactly what a plugin does before it knows whether anything is.
#[test]
fn the_catalog_of_an_unknown_namespace_is_empty() {
    let (store, _dir) = defined();
    assert!(store.catalog(Some("shared:nothing")).unwrap().is_empty());
}

#[test]
fn reserved_attributes_cannot_create_or_migrate_an_entity() {
    let (store, _dir) = defined();
    put(&store, "zellij", "preserved", 1);
    let catalog = store.catalog(None).unwrap();
    let rows = store.fetch(NS, &FetchRequest::entity("agent")).unwrap();
    for name in ["xmin", "xmax", "cmin", "cmax", "ctid", "tableoid"] {
        let new = EntityDef::new("invalid", 1).attr(AttrDef::text(name));
        let migration = EntityDef::new("agent", 2).attr(AttrDef::text(name));
        for def in [new, migration] {
            let err = store.define_entity(NS, OWNER, &def, true).unwrap_err();
            assert_err!(err, DataError::InvalidName(_));
            assert_eq!(store.catalog(None).unwrap(), catalog);
            assert_eq!(
                store.fetch(NS, &FetchRequest::entity("agent")).unwrap(),
                rows
            );
        }
    }
}

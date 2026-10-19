use super::*;
use yinz_types::plugin_data::AttrDef;

#[test]
fn identical_schema_declarations_and_searches_do_not_wait_for_another_writer() {
    let directory = tempfile::tempdir().unwrap();
    let backend = SqlitePluginDataBackend::new(directory.path());
    let def = EntityDef::new("note", 1).attr(AttrDef::text("body").full_text());
    backend
        .define_entity("shared:test", "notes", &def, false)
        .unwrap();
    backend
        .apply(
            "shared:test",
            &[Mutation::put("note").set("body", "searchable").build()],
        )
        .unwrap();
    backend
        .with_conn(|conn| {
            conn.busy_timeout(Duration::ZERO)
                .ctx("disable waiting for the test's held writer")
        })
        .unwrap();
    let mut writer = Connection::open(&backend.db_path).unwrap();
    let transaction = writer
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();

    for owner in ["notes", "native-note-palette"] {
        let info = backend
            .define_entity("shared:test", owner, &def, false)
            .unwrap();
        assert!(!info.created);
        assert!(info.attrs_added.is_empty());
        assert!(info.indexes_created.is_empty());
        assert!(!info.full_text_rebuilt);
        let rows = backend
            .fetch(
                "shared:test",
                &FetchRequest::entity("note").search("searchable"),
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
    }

    let extended = def.attr(AttrDef::text("title"));
    assert!(backend
        .define_entity("shared:test", "notes", &extended, false)
        .is_err());
    transaction.rollback().unwrap();
    let info = backend
        .define_entity("shared:test", "notes", &extended, false)
        .unwrap();
    assert_eq!(info.attrs_added, ["title"]);
}

#[test]
fn schema_redeclarations_recheck_external_changes_and_preserve_ownership_checks() {
    let directory = tempfile::tempdir().unwrap();
    let backend = SqlitePluginDataBackend::new(directory.path());
    let def = EntityDef::new("note", 1).attr(AttrDef::text("body"));
    backend
        .define_entity("shared:test", "notes", &def, false)
        .unwrap();
    backend
        .define_entity("shared:test", "native-note-palette", &def, false)
        .unwrap();
    let other = SqlitePluginDataBackend::new(directory.path());
    let extended = def.clone().attr(AttrDef::text("title"));
    other
        .define_entity("shared:test", "notes", &extended, false)
        .unwrap();
    assert!(matches!(
        backend.define_entity("shared:test", "notes", &def, false),
        Err(DataError::RefusedMigration(_))
    ));
    backend
        .define_entity("shared:test", "native-note-palette", &extended, false)
        .unwrap();
    assert!(matches!(
        backend.define_entity("shared:test", "native-note-palette", &extended, true),
        Err(DataError::RefusedMigration(_))
    ));
    let catalog = backend.catalog(Some("shared:test")).unwrap();
    assert_eq!(catalog[0].owner_plugin, "notes");
    assert_eq!(catalog[0].def, extended);
}

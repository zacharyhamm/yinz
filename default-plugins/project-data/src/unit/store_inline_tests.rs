use super::*;
fn record_def() -> EntityDef {
    EntityDef::new("record", 2)
        .attr(AttrDef::text("title").required())
        .attr(AttrDef::text("plan"))
}
fn fixture_defs() -> Vec<EntityDef> {
    vec![record_def(), import_marker_def()]
}

#[test]
fn record_plan_is_an_additive_migration() {
    let backend = Local::private("agents");
    let mut old = record_def();
    old.attrs.retain(|attr| attr.name != "plan");
    backend.define(&old).expect("old schema");
    backend
        .apply(vec![Mutation::put("record")
            .set("title", "old record")
            .build()])
        .expect("old archive");
    backend.define(&record_def()).expect("additive migration");
    let rows = backend
        .fetch(&FetchRequest::entity("record"))
        .expect("read migrated record");
    assert_eq!(
        rows.get(&rows.rows[0], "title"),
        Some(&DataValue::Text("old record".to_owned()))
    );
    assert_eq!(rows.get(&rows.rows[0], "plan"), Some(&DataValue::Null));
}

/// Every definition has to survive the same validation the host applies, or
/// the failure would land at plugin load with no test having noticed.
#[test]
fn every_definition_validates() {
    for def in shared_defs().iter().chain(fixture_defs().iter()) {
        def.validate()
            .unwrap_or_else(|e| panic!("`{}` is not a valid entity: {e}", def.name));
    }
}

#[test]
fn the_searched_entities_declare_a_full_text_attribute() {
    assert_eq!(agent_run_entry_def().full_text_attrs().len(), 1);
}

/// The marker entity is declared in both namespaces, so it has to be one
/// definition rather than two that could drift.
#[test]
fn the_import_marker_is_declared_in_both_namespaces() {
    assert!(shared_defs().iter().any(|d| d.name == "import_marker"));
    assert!(fixture_defs().iter().any(|d| d.name == "import_marker"));
}

/// The machine these tests are on. Named rather than empty, so a helper that forgot to
/// write `host_id` fails a test instead of matching the pre-migration rows.
const HOST: &str = "test-host";

/// A marker written before the uid carried a host still counts, and is claimed *beside*
/// rather than instead of.
///
/// Without the first half, the launch after the upgrade is told the import has not happened
/// on every machine at once. The old JSON is never unlinked and nothing dedupes a record, so the whole archive would double. The second half is the same worry from the other
/// end: the legacy row names no machine, so deleting it is deleting somebody else's only
/// record that they have already imported — see
/// `a_legacy_marker_is_not_deleted_out_from_under_the_machine_that_wrote_it`.
#[test]
fn an_import_marker_written_before_the_host_key_still_counts_and_is_claimed() {
    let backend = Local::private("agents");
    // The definition as it was, and a marker under the uid it used.
    backend
        .define(&EntityDef::new("import_marker", 1).attr(AttrDef::text("what").required()))
        .expect("the old definition still declares");
    backend
        .apply(vec![Mutation::put("import_marker")
            .uid("chats")
            .set("what", "chats")
            .build()])
        .expect("a marker from before the host key");
    backend
        .define_all(&fixture_defs())
        .expect("the new definition migrates rather than being refused");

    assert!(
        import_recorded(&backend, &HostId::new(HOST), "chats"),
        "the import would have been done a second time"
    );

    // Ours is there now, and the legacy row is still there too: it might be another
    // machine's, and nothing here can tell.
    let hosts = marker_hosts(&backend);
    assert_eq!(hosts, vec![String::new(), HOST.to_owned()]);

    // And asking again is a read, not another claim.
    assert!(import_recorded(&backend, &HostId::new(HOST), "chats"));
    assert_eq!(marker_hosts(&backend), hosts);
}

/// The bug the claim used to have, and the one that costs a whole archive.
///
/// Two host identities in one store. Host A imported years ago, before the marker carried a host.
/// Host B has never imported. B launches first: it is told the import is done — which is
/// the accommodation working as designed, since a bare marker is one every machine answers
/// to — and claims a marker of its own. What it must *not* do is take A's row away with it,
/// because then A's next launch finds nothing that answers for it and reads its entire
/// archive in a second time, with no dedupe key to collapse the result.
#[test]
fn a_legacy_marker_is_not_deleted_out_from_under_the_machine_that_wrote_it() {
    let backend = Local::private("agents");
    backend
        .define(&EntityDef::new("import_marker", 1).attr(AttrDef::text("what").required()))
        .expect("the old definition still declares");
    backend
        .apply(vec![Mutation::put("import_marker")
            .uid("chats")
            .set("what", "chats")
            .build()])
        .expect("host A imported, before the host key");
    backend.define_all(&fixture_defs()).expect("migrated");

    assert!(
        import_recorded(&backend, &HostId::new("host-b"), "chats"),
        "a marker naming no machine answers for every machine"
    );
    assert_eq!(
        marker_hosts(&backend),
        vec![String::new(), "host-b".to_owned()],
        "host B may claim a row of its own, but not take the legacy one"
    );

    assert!(
        import_recorded(&backend, &HostId::new("host-a"), "chats"),
        "host A, which actually did the import, must still be told so"
    );
}

/// Every marker in the store, by host, sorted — the shape these tests assert on.
fn marker_hosts(backend: &Local) -> Vec<String> {
    let rows = backend
        .fetch(&FetchRequest::entity("import_marker"))
        .expect("fetched");
    let mut hosts: Vec<String> = rows
        .rows
        .iter()
        .map(|row| match rows.get(row, "host_id") {
            Some(DataValue::Text(host)) => host.clone(),
            other => panic!("a marker with no host: {other:?}"),
        })
        .collect();
    hosts.sort();
    hosts
}

/// The other direction, which the tolerance must not swallow: a machine that has not
/// imported has to be told so, and another machine's claimed marker is not an answer for us.
#[test]
fn a_machine_that_has_not_imported_is_told_so() {
    let backend = Local::private("agents");
    backend.define_all(&fixture_defs()).expect("declared");
    assert!(!import_recorded(&backend, &HostId::new(HOST), "chats"));

    backend
        .apply(vec![import_marker_mutation(
            &HostId::new("another-host"),
            "chats",
        )])
        .expect("another machine imported");
    assert!(
        !import_recorded(&backend, &HostId::new(HOST), "chats"),
        "another machine's files are not ours"
    );
    assert!(
        import_recorded(&backend, &HostId::new("another-host"), "chats"),
        "and theirs still are"
    );
}

/// Each import is asked about separately, so claiming one does not answer for the other.
#[test]
fn claiming_one_marker_leaves_the_other_alone() {
    let backend = Local::private("agents");
    backend
        .define(&EntityDef::new("import_marker", 1).attr(AttrDef::text("what").required()))
        .expect("declared");
    backend
        .apply(vec![
            Mutation::put("import_marker")
                .uid("chats")
                .set("what", "chats")
                .build(),
            Mutation::put("import_marker")
                .uid("prompts")
                .set("what", "prompts")
                .build(),
        ])
        .expect("two markers from before the host key");
    backend.define_all(&fixture_defs()).expect("migrated");

    assert!(import_recorded(&backend, &HostId::new(HOST), "chats"));
    assert_eq!(
        marker_hosts(&backend),
        vec![String::new(), String::new(), HOST.to_owned()],
        "only `chats` should have been claimed, and neither legacy row touched"
    );
    assert!(import_recorded(&backend, &HostId::new(HOST), "prompts"));
    assert_eq!(
        marker_hosts(&backend),
        vec![
            String::new(),
            String::new(),
            HOST.to_owned(),
            HOST.to_owned()
        ]
    );
}

#[test]
fn declaring_the_shared_entities_creates_them() {
    let backend = Local::shared("project_store");
    for def in shared_defs() {
        let info = backend.define(&def).expect("declared");
        assert!(info.created, "`{}` should have been created", def.name);
        assert_eq!(info.namespace, SHARED);
    }
}

/// The case the whole "one set of constructors" rule exists for: whichever
/// plugin loads second declares the same entities again, and must find that
/// a no-op rather than a migration.
#[test]
fn a_second_plugin_declaring_the_same_entities_changes_nothing() {
    let first = Local::shared("project_store");
    first.define_all(&shared_defs()).expect("first declared");

    let second = first.joined(SHARED, "agents");
    for def in shared_defs() {
        let info = second.define(&def).expect("second declared");
        assert!(!info.created, "`{}` already existed", def.name);
        assert!(info.attrs_added.is_empty(), "nothing should be added");
        assert!(!info.full_text_rebuilt, "the index should be left alone");
    }
}

/// And the failure that rule prevents. A definition missing an attribute
/// reads as a request to drop it, which the store refuses — this is what
/// would happen at load, in whichever plugin lost the race, if the two ever
/// built their own `EntityDef`s.
#[test]
fn a_narrower_redeclaration_is_refused() {
    let first = Local::shared("project_store");
    first.define(&project_def()).expect("declared");

    let narrower = EntityDef::new("project", 1)
        .attr(AttrDef::text("name").required())
        .attr(AttrDef::text("root").indexed().required());
    let error = first
        .joined(SHARED, "agents")
        .define(&narrower)
        .expect_err("dropping attributes should be refused");
    assert!(
        error.contains("discard the stored values"),
        "the refusal should say what is at stake, got: {error}"
    );
}

#[test]
fn project_worktree_directory_migrates_and_round_trips() {
    let store = Local::shared("agents");
    let mut legacy = project_def();
    legacy.attrs.retain(|attr| attr.name != "worktree_dir");
    store.define(&legacy).unwrap();
    store
        .apply(vec![Mutation::put("project")
            .uid("old")
            .set("name", "old")
            .set("root", "/old")
            .build()])
        .unwrap();
    store.define(&project_def()).unwrap();
    let rows = store.fetch(&FetchRequest::entity("project")).unwrap();
    let mut projects = crate::projects::from_rows(&rows);
    assert_eq!(projects["old"].worktree_dir, None);
    projects.get_mut("old").unwrap().worktree_dir = Some("../trees with spaces".into());
    store
        .apply(vec![crate::projects::to_mutation(&projects["old"])])
        .unwrap();
    let loaded =
        crate::projects::from_rows(&store.fetch(&FetchRequest::entity("project")).unwrap());
    assert_eq!(loaded, projects);
    assert_eq!(
        crate::projects::from_rows(&crate::projects::to_rows(&projects)),
        projects
    );
    projects.get_mut("old").unwrap().worktree_dir = None;
    store
        .apply(vec![crate::projects::to_mutation(&projects["old"])])
        .unwrap();
    assert_eq!(
        crate::projects::from_rows(&store.fetch(&FetchRequest::entity("project")).unwrap()),
        projects
    );
}

/// The reason the namespace is shared at all: one plugin's write is another
/// plugin's read, with no message in between.
#[test]
fn two_plugins_see_the_same_rows() {
    let store = Local::shared("project_store");
    store.define_all(&shared_defs()).expect("declared");
    let agent = store.joined(SHARED, "agents");
    agent.define_all(&shared_defs()).expect("declared");

    store
        .apply(vec![Mutation::put("project")
            .uid("yinz")
            .set("name", "yinz")
            .set("root", "/src/yinz")
            .build()])
        .expect("written");

    let rows = agent
        .fetch(&FetchRequest::entity("project"))
        .expect("fetched");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows.get(&rows.rows[0], "root"),
        Some(&DataValue::Text("/src/yinz".to_owned()))
    );
}

/// A private namespace is the opposite guarantee, and transcripts rely on
/// it: another plugin declaring the same entity name gets its own table.
#[test]
fn a_private_namespace_is_not_shared() {
    let agent = Local::private("agents");
    agent.define_all(&fixture_defs()).expect("declared");
    agent
        .apply(vec![Mutation::put("record")
            .uid("1")
            .set("title", "a record")
            .build()])
        .expect("written");

    let other = agent.joined("plugin:somebody_else", "somebody_else");
    other.define(&record_def()).expect("declared");
    assert!(
        other
            .fetch(&FetchRequest::entity("record"))
            .expect("fetched")
            .is_empty(),
        "a private namespace should not show another plugin's rows"
    );
}

/// The backend owns the namespace, so a request that names the wrong one
/// cannot reach the store. Without the stamp this would read `shared:yinz`.
#[test]
fn a_backend_overrides_the_namespace_on_a_request() {
    let agent = Local::private("agents");
    agent.define_all(&fixture_defs()).expect("declared");

    let misdirected = FetchRequest::entity("record").in_namespace(SHARED);
    assert!(
        agent.fetch(&misdirected).is_ok(),
        "the backend's namespace should win"
    );
}

/// `define_all` stops at the first refusal and says which entity it was,
/// because the message is all a plugin author gets.
#[test]
fn define_all_names_the_entity_that_failed() {
    let backend = Local::shared("project_store");
    backend.define(&record_def()).expect("declared");

    let narrower = EntityDef::new("record", 2).attr(AttrDef::text("title").required());
    let error = backend
        .joined(SHARED, "agents")
        .define_all(&[project_def(), narrower])
        .expect_err("the second definition should be refused");
    assert!(
        error.starts_with("could not declare `record`"),
        "got: {error}"
    );
}
#[test]
fn incomplete_pagination_never_returns_partial_rows() {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    struct Pages(RefCell<VecDeque<Result<DataRows, String>>>);
    impl Backend for Pages {
        fn fetch(&self, _: &FetchRequest) -> Result<DataRows, String> {
            self.0
                .borrow_mut()
                .pop_front()
                .expect("unexpected extra fetch")
        }
        fn define(&self, _: &EntityDef) -> Result<DataSchemaInfo, String> {
            unreachable!()
        }
        fn count(&self, _: &FetchRequest) -> Result<u64, String> {
            unreachable!()
        }
        fn apply(&self, _: Vec<Mutation>) -> Result<ApplyReport, WriteError> {
            unreachable!()
        }
        fn watch(&self, _: &FetchRequest, _: WatchMode) -> Result<DataWatchHandle, String> {
            unreachable!()
        }
    }
    let page = |cursor: Option<i64>| {
        Ok(DataRows {
            truncated: true,
            next_cursor: cursor.map(DataRowId::new),
            ..Default::default()
        })
    };
    for (pages, message) in [
        (vec![Err("fetch failed".into())], "fetch failed"),
        (vec![page(None)], "missing cursor"),
        (
            vec![page(Some(1)), Err("fetch failed".into())],
            "fetch failed",
        ),
        (vec![page(Some(1)), page(None)], "missing cursor"),
        (vec![page(Some(1)), page(Some(1))], "did not advance"),
    ] {
        let backend = Pages(RefCell::new(pages.into()));
        assert!(fetch_all(&backend, FetchRequest::entity("project"))
            .unwrap_err()
            .contains(message));
        assert!(backend.0.borrow().is_empty());
    }
}

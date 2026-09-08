use super::*;
use hocus_proto::store::Local;
use std::cell::RefCell;
use zellij_tile::prelude::Mutation;

struct Interleaved<F> {
    backend: Local,
    before_apply: RefCell<F>,
}

impl<F: FnMut(&Local) -> Result<(), WriteError>> Backend for Interleaved<F> {
    fn define(&self, def: &EntityDef) -> Result<DataSchemaInfo, String> {
        self.backend.define(def)
    }
    fn fetch(&self, request: &FetchRequest) -> Result<DataRows, String> {
        self.backend.fetch(request)
    }
    fn count(&self, request: &FetchRequest) -> Result<u64, String> {
        self.backend.count(request)
    }
    fn apply(&self, mutations: Vec<Mutation>) -> Result<ApplyReport, WriteError> {
        (self.before_apply.borrow_mut())(&self.backend)?;
        self.backend.apply(mutations)
    }
    fn watch(&self, request: &FetchRequest, mode: WatchMode) -> Result<DataWatchHandle, String> {
        self.backend.watch(request, mode)
    }
}

fn backend() -> Local {
    let backend = Local::shared("hocus-store");
    backend.define_all(&store::shared_defs()).unwrap();
    backend
}

fn lists() -> todos::Store {
    serde_json::from_value(serde_json::json!({
        "/work": {"items": [{"id": 1, "text": "parent", "children": [
            {"id": 2, "text": "child"}
        ]}]},
        "/z-other": {"items": [{"id": 1, "text": "other project"}]}
    }))
    .unwrap()
}

fn three_level_list(done: bool) -> todos::Store {
    serde_json::from_value(serde_json::json!({
        "/work": {"items": [{"id": 1, "text": "grandparent", "children": [
            {"id": 2, "text": "parent", "children": [
                {"id": 3, "text": "child", "done": done},
                {"id": 4, "text": "sibling", "done": done}
            ]}
        ]}]}
    }))
    .unwrap()
}

#[test]
fn resumed_legacy_children_reopen_ancestors_only_when_open() {
    for done in [false, true] {
        let host = HostId::new("host-a");
        let lists = three_level_list(done);
        let mut applies = 0;
        let backend = Interleaved {
            backend: backend(),
            before_apply: RefCell::new(move |_: &Local| {
                applies += 1;
                if applies == 3 {
                    return Err(WriteError::Failed("interrupted".into()));
                }
                Ok(())
            }),
        };
        assert!(import_todos(&backend, &host, &lists).is_err());
        let other = backend.backend.joined(store::SHARED, "other-server");
        let list = todos::fetch_list(&other, "/work").unwrap();
        other
            .apply(
                todos::plan_set_done(&list, "/work", list.items[0].id, true)
                    .unwrap()
                    .0,
            )
            .unwrap();
        let completed = todos::fetch_list(&other, "/work").unwrap();
        assert!(completed.items[0].done);
        assert!(completed.items[0].children[0].done);

        import_todos(&backend, &host, &lists).unwrap();
        assert!(imported(&backend, &host, "todos"));
        let list = todos::fetch_list(&backend, "/work").unwrap();
        let grandparent = &list.items[0];
        let parent = &grandparent.children[0];
        assert_eq!(grandparent.done, done);
        assert_eq!(parent.done, done);
        assert_eq!(parent.children.len(), 2);
        assert!(parent.children.iter().all(|child| child.done == done));
        if done {
            assert_eq!(grandparent.rev, completed.items[0].rev);
            assert_eq!(parent.rev, completed.items[0].children[0].rev);
        }
        let listed: serde_json::Value = serde_json::from_str(
            &ops::run_query(
                &backend,
                "todo_list",
                &serde_json::json!({"project": "/work"}),
            )
            .unwrap(),
        )
        .unwrap();
        if done {
            assert_eq!(listed["items"], serde_json::json!([]));
        } else {
            assert_eq!(
                listed["items"][0]["children"][0]["children"][0]["text"],
                "child"
            );
            assert_eq!(
                listed["items"][0]["children"][0]["children"][1]["text"],
                "sibling"
            );
        }
        import_todos(&backend, &host, &lists).unwrap();
        assert_eq!(todos::fetch_list(&backend, "/work").unwrap(), list);
        assert_eq!(backend.count(&FetchRequest::entity("todo")).unwrap(), 4);
    }
}

#[test]
fn resumed_legacy_import_conflict_keeps_ancestors_done_and_level_resumable() {
    let host = HostId::new("host-a");
    let lists = three_level_list(false);
    let mut applies = 0;
    let backend = Interleaved {
        backend: backend(),
        before_apply: RefCell::new(move |backend: &Local| {
            applies += 1;
            if applies == 3 {
                return Err(WriteError::Failed("interrupted".into()));
            }
            if applies == 4 {
                let other = backend.joined(store::SHARED, "other-server");
                other.apply(vec![Mutation::update("todo", Pred::eq("text", "parent"))
                    .set("text", "edited parent")
                    .build()])?;
            }
            Ok(())
        }),
    };
    assert!(import_todos(&backend, &host, &lists).is_err());
    let other = backend.backend.joined(store::SHARED, "other-server");
    let list = todos::fetch_list(&other, "/work").unwrap();
    other
        .apply(
            todos::plan_set_done(&list, "/work", list.items[0].id, true)
                .unwrap()
                .0,
        )
        .unwrap();

    assert!(import_todos(&backend, &host, &lists).is_err());
    assert!(!imported(&backend, &host, "todos"));
    let list = todos::fetch_list(&backend, "/work").unwrap();
    assert!(list.items[0].done);
    let parent = &list.items[0].children[0];
    assert!(parent.done);
    assert_eq!(parent.text, "edited parent");
    assert!(parent.children.is_empty());
    assert_eq!(backend.count(&FetchRequest::entity("todo")).unwrap(), 2);
    assert_eq!(
        backend
            .count(&FetchRequest::entity("import_marker"))
            .unwrap(),
        2
    );

    import_todos(&backend, &host, &lists).unwrap();
    assert!(imported(&backend, &host, "todos"));
    let list = todos::fetch_list(&backend, "/work").unwrap();
    assert!(!list.items[0].done);
    let parent = &list.items[0].children[0];
    assert!(!parent.done);
    assert_eq!(parent.text, "edited parent");
    assert_eq!(parent.children.len(), 2);
    assert_eq!(backend.count(&FetchRequest::entity("todo")).unwrap(), 4);
}

#[test]
fn legacy_import_resumes_after_conflict_without_rewriting_committed_levels() {
    let host = HostId::new("host-a");
    let mut applies = 0;
    let backend = Interleaved {
        backend: backend(),
        before_apply: RefCell::new(move |backend: &Local| {
            applies += 1;
            if applies == 2 {
                // Another server edits the parent between our snapshot and apply.
                let other = backend.joined(store::SHARED, "other-server");
                other.apply(vec![Mutation::update("todo", Pred::eq("root", "/work"))
                    .set("text", "edited parent")
                    .build()])?;
            }
            Ok(())
        }),
    };
    assert!(import_todos(&backend, &host, &lists()).is_err());
    assert!(!imported(&backend, &host, "todos"));
    assert_eq!(backend.count(&FetchRequest::entity("todo")).unwrap(), 1);
    // Only the first level's checkpoint survived the rejected transaction.
    assert_eq!(
        backend
            .count(&FetchRequest::entity("import_marker"))
            .unwrap(),
        1
    );
    import_todos(&backend, &host, &lists()).unwrap();
    assert!(imported(&backend, &host, "todos"));
    let list = todos::fetch_list(&backend, "/work").unwrap();
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].text, "edited parent");
    assert_eq!(list.items[0].children[0].text, "child");
    assert_eq!(backend.count(&FetchRequest::entity("todo")).unwrap(), 3);
    import_todos(&backend, &host, &lists()).unwrap();
    assert_eq!(backend.count(&FetchRequest::entity("todo")).unwrap(), 3);
}

#[test]
fn legacy_import_preserves_missing_or_moved_parents_on_resume() {
    for moved in [false, true] {
        let host = HostId::new("host-a");
        let mut applies = 0;
        let backend = Interleaved {
            backend: backend(),
            before_apply: RefCell::new(move |_: &Local| {
                applies += 1;
                if applies == 2 {
                    return Err(WriteError::Failed("interrupted".into()));
                }
                Ok(())
            }),
        };
        assert!(import_todos(&backend, &host, &lists()).is_err());
        backend
            .backend
            .apply(vec![if moved {
                Mutation::update("todo", Pred::eq("root", "/work"))
                    .set("root", "/moved")
                    .build()
            } else {
                Mutation::delete("todo", Pred::eq("root", "/work"))
            }])
            .unwrap();
        assert!(import_todos(&backend, &host, &lists())
            .unwrap_err()
            .contains("parent"));
        assert!(!imported(&backend, &host, "todos"));
        assert_eq!(
            backend.count(&FetchRequest::entity("todo")).unwrap(),
            u64::from(moved)
        );
    }
}

#[test]
fn competing_legacy_imports_cannot_commit_a_level_twice() {
    let host = HostId::new("host-a");
    let mut raced = false;
    let backend = Interleaved {
        backend: backend(),
        before_apply: RefCell::new(move |backend: &Local| {
            if !raced {
                raced = true;
                let other = backend.joined(store::SHARED, "other-server");
                import_todos(&other, &HostId::new("host-a"), &lists()).unwrap();
            }
            Ok(())
        }),
    };
    assert!(import_todos(&backend, &host, &lists()).is_err());
    import_todos(&backend, &host, &lists()).unwrap();
    assert_eq!(backend.count(&FetchRequest::entity("todo")).unwrap(), 3);
    assert!(imported(&backend, &host, "todos"));
}

#[test]
fn failed_completion_marker_resumes_without_resurrecting_deleted_todos() {
    let host = HostId::new("host-a");
    let mut applies = 0;
    let backend = Interleaved {
        backend: backend(),
        before_apply: RefCell::new(move |_: &Local| {
            applies += 1;
            if applies == 4 {
                return Err(WriteError::Failed("completion interrupted".into()));
            }
            Ok(())
        }),
    };
    assert!(import_todos(&backend, &host, &lists()).is_err());
    assert!(!imported(&backend, &host, "todos"));
    backend
        .backend
        .apply(vec![Mutation::delete("todo", Pred::eq("root", "/work"))])
        .unwrap();
    import_todos(&backend, &host, &lists()).unwrap();
    assert!(imported(&backend, &host, "todos"));
    assert_eq!(backend.count(&FetchRequest::entity("todo")).unwrap(), 1);
}

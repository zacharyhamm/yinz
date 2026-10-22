use super::*;

#[test]
fn reopening_and_declaring_current_entities_preserves_retired_records() {
    let original = Local::shared("project_store");
    // Historical schemas stay independent of the current registry.
    for def in [
        EntityDef::new("todo", 1)
            .attr(AttrDef::text("root").indexed().required())
            .attr(AttrDef::int("parent_id"))
            .attr(AttrDef::text("text").full_text().required())
            .attr(AttrDef::boolean("done").indexed().required().default(false)),
        EntityDef::new("note", 1)
            .attr(AttrDef::text("root").indexed())
            .attr(AttrDef::text("title").full_text().required())
            .attr(AttrDef::text("body").full_text().required())
            .attr(AttrDef::text("author").required().default("user"))
            .attr(
                AttrDef::boolean("archived")
                    .indexed()
                    .required()
                    .default(false),
            ),
        EntityDef::new("note_link", 1)
            .attr(AttrDef::int("note_id").indexed().required())
            .attr(AttrDef::int("todo_id").indexed().required()),
        EntityDef::new("note_tag", 1)
            .attr(AttrDef::int("note_id").indexed().required())
            .attr(AttrDef::text("tag").indexed().required()),
    ] {
        original.define(&def).unwrap();
    }
    original
        .apply(vec![
            Mutation::put("todo")
                .uid("task")
                .set("root", "/work")
                .set("text", "task")
                .build(),
            Mutation::put("note")
                .uid("memo")
                .set("root", "/work")
                .set("title", "Decision")
                .set("body", "Keep this body")
                .build(),
            Mutation::put("note_link")
                .uid("1:1")
                .set("note_id", 1i64)
                .set("todo_id", 1i64)
                .build(),
            Mutation::put("note_tag")
                .uid("1:design")
                .set("note_id", 1i64)
                .set("tag", "design")
                .build(),
        ])
        .unwrap();
    let entities = ["todo", "note", "note_link", "note_tag"];
    let before: Vec<_> = entities
        .iter()
        .map(|entity| original.fetch(&FetchRequest::entity(*entity)).unwrap())
        .collect();
    let reopened = original.joined(SHARED, "project_store");
    reopened.define_all(&shared_defs()).unwrap();
    for (entity, expected) in entities.iter().zip(before) {
        assert!(!shared_defs().iter().any(|def| def.name == *entity));
        assert_eq!(
            reopened.fetch(&FetchRequest::entity(*entity)).unwrap(),
            expected
        );
    }
}

#[test]
fn shared_declarations_preserve_retired_project_attribute() {
    let store = Local::shared("project_store");
    // Keep this historical definition independent of the current schema constructor.
    let legacy = EntityDef::new("project", 1)
        .attr(AttrDef::text("name").required())
        .attr(AttrDef::text("root").indexed().required())
        .attr(AttrDef::text("firecracker_image"))
        .attr(AttrDef::text("worktree_dir"))
        .attr(AttrDef::text("remote"))
        .attr(AttrDef::text("worktree_status"))
        .attr(AttrDef::text("common_dir"))
        .attr(AttrDef::json("replaced_ids"))
        .attr(AttrDef::boolean("discovered").required().default(false));
    store.define(&legacy).unwrap();
    store
        .apply(vec![Mutation::put("project")
            .uid("legacy")
            .set("name", "legacy")
            .set("root", "/src/legacy")
            .set("firecracker_image", "old-image")
            .build()])
        .unwrap();

    store.define_all(&shared_defs()).unwrap();
    let agent = store.joined(SHARED, "agents");
    agent.define_all(&shared_defs()).unwrap();
    let rows = agent.fetch(&FetchRequest::entity("project")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows.get(&rows.rows[0], "firecracker_image"),
        Some(&DataValue::Text("old-image".into()))
    );
    assert_eq!(
        crate::projects::from_rows(&rows)["legacy"].root,
        "/src/legacy"
    );
}

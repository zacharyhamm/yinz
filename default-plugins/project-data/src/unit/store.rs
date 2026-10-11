use super::*;

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

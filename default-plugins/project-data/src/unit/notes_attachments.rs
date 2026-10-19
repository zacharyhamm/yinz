use super::*;
use crate::store::{self, Backend};

#[test]
fn quality_pass_tag_attachment_invalidates_stale_tag_replacement() {
    let backend = store::Local::shared("agents");
    backend.define_all(&store::shared_defs()).unwrap();
    let id = backend
        .apply(
            plan_add(0, None, "title", "body", "user", &[], &[])
                .unwrap()
                .0,
        )
        .unwrap()
        .ids[0]
        .get() as u64;
    let before = fetch_note(&backend, id).unwrap().unwrap();
    let replacement = plan_set_tags(&before, &["replacement".into()], Some(before.rev), true)
        .unwrap()
        .0;
    backend
        .apply(plan_attach(&backend, id, &["attached".into()], &[]).unwrap())
        .unwrap();

    let error = backend.apply(replacement).unwrap_err();
    assert!(error.is_conflict(), "{error:?}");
    assert_eq!(
        fetch_note(&backend, id).unwrap().unwrap().tags,
        ["attached"]
    );
}

#[test]
fn attachments_cannot_recreate_side_rows_after_note_deletion() {
    for delete_first in [false, true] {
        let backend = store::Local::shared("agents");
        backend.define_all(&store::shared_defs()).unwrap();
        let todo = backend
            .apply(vec![Mutation::put("todo")
                .set("root", "/work")
                .set("text", "task")
                .build()])
            .unwrap()
            .ids[0]
            .get() as u64;
        let note = backend
            .apply(
                plan_add(0, None, "title", "body", "user", &[], &[])
                    .unwrap()
                    .0,
            )
            .unwrap()
            .ids[0]
            .get() as u64;
        let attachment = plan_attach(&backend, note, &["tag".into()], &[todo]).unwrap();
        let deletion = plan_delete(note);

        if delete_first {
            backend.apply(deletion).unwrap();
            let error = backend.apply(attachment).unwrap_err();
            assert!(error.is_conflict(), "{error:?}");
            assert!(plan_link(&backend, note, todo).is_err());
        } else {
            backend.apply(attachment).unwrap();
            backend.apply(deletion).unwrap();
        }
        for entity in ["note", "note_tag", "note_link"] {
            assert!(
                backend
                    .fetch(&FetchRequest::entity(entity))
                    .unwrap()
                    .is_empty(),
                "{entity}"
            );
        }
    }
}

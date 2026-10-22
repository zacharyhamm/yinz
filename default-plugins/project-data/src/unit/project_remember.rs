use super::*;
use crate::store::{self, Backend};
use yinz_tile::prelude::FetchRequest;

fn backend() -> store::Local {
    let backend = store::Local::shared("project_store");
    backend.define_all(&store::shared_defs()).unwrap();
    backend
}

fn project_rows(backend: &impl Backend) -> DataRows {
    backend.fetch(&FetchRequest::entity("project")).unwrap()
}

fn writes(mutations: &[Mutation]) -> usize {
    mutations
        .iter()
        .filter(|mutation| !matches!(mutation, Mutation::AssertRows { .. }))
        .count()
}

#[test]
fn remember_directory_changes_write_once_and_repeats_preserve_revisions() {
    for root in [None, Some("/work")] {
        let backend = backend();
        let mut expected = validate("work", "/work").unwrap();
        backend.apply(vec![to_mutation(&expected)]).unwrap();
        for (directory, changed) in [
            (Some("../trees"), true),
            (Some("../trees"), false),
            (None, false),
            (Some("/other"), true),
            (Some(""), true),
            (Some(""), false),
        ] {
            let before = project_rows(&backend);
            let (project, mutations) = plan_remember(&before, "work", root, directory).unwrap();
            if let Some(directory) = directory {
                expected.worktree_dir = (!directory.is_empty()).then(|| directory.to_owned());
            }
            assert_eq!(project, expected);
            assert_eq!(writes(&mutations), usize::from(changed));
            backend.apply(mutations).unwrap();
            let after = project_rows(&backend);
            assert_eq!(from_rows(&after)["work"], expected);
            assert_eq!(row_ids(&after), row_ids(&before));
            if changed {
                let Some(DataValue::Int(rev)) = before.get(&before.rows[0], "rev") else {
                    panic!("stored project must have a revision");
                };
                assert_eq!(
                    after.get(&after.rows[0], "rev"),
                    Some(&DataValue::Int(rev + 1))
                );
            } else {
                assert_eq!(after, before);
            }
        }
    }
}

#[test]
fn remember_new_project_writes_only_the_final_directory() {
    for directory in [None, Some("../trees"), Some("")] {
        let backend = backend();
        let (project, mutations) =
            plan_remember(&project_rows(&backend), "work", Some("/work"), directory).unwrap();
        assert_eq!(writes(&mutations), 1);
        assert_eq!(mutations.last(), Some(&to_mutation(&project)));
        assert_eq!(
            project.worktree_dir.as_deref(),
            directory.filter(|dir| !dir.is_empty())
        );
        backend.apply(mutations).unwrap();
        assert_eq!(from_rows(&project_rows(&backend))["work"], project);
    }
}

#[test]
fn remember_adoption_preserves_history_and_applies_directory_after_inheritance() {
    for directory in [None, Some("../trees"), Some("/other"), Some("")] {
        let backend = backend();
        let mut found = validate("found", "/work").unwrap();
        found.discovered = true;
        found.worktree_dir = Some("../trees".into());
        found.remote = Some("git@example.com:work.git".into());
        found.common_dir = Some("/work/.git".into());
        found.worktree_status = Some(WorktreeStatus::Primary);
        found.replaced_ids = vec![DataRowId::new(900)];
        backend.apply(vec![to_mutation(&found)]).unwrap();
        let before = project_rows(&backend);
        let (project, mutations) =
            plan_remember(&before, "mine", Some("/work"), directory).unwrap();
        let mut expected = found;
        expected.name = "mine".into();
        expected.discovered = false;
        expected.replaced_ids.insert(0, row_ids(&before)["found"]);
        if let Some(directory) = directory {
            expected.worktree_dir = (!directory.is_empty()).then(|| directory.to_owned());
        }
        assert_eq!(project, expected);
        let changes: Vec<_> = mutations
            .iter()
            .filter(|mutation| !matches!(mutation, Mutation::AssertRows { .. }))
            .collect();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0], &to_mutation(&expected));
        assert!(matches!(changes[1], Mutation::Delete { .. }));
        backend.apply(mutations).unwrap();
        assert_eq!(
            from_rows(&project_rows(&backend)),
            Registry::from([("mine".into(), expected)])
        );
    }
}

#[test]
fn remember_stale_directory_and_adoption_plans_roll_back_atomically() {
    for (root, directory, concurrent_name) in [
        (None, "new", "work"),
        (None, "old", "work"),
        (Some("/work"), "new", "work"),
        (Some("/work"), "", "found"),
        (Some("/work"), "new", "sharer"),
    ] {
        let backend = backend();
        let mut project = validate("work", "/work").unwrap();
        project.worktree_dir = Some("old".into());
        let mut found = validate("found", "/work").unwrap();
        found.discovered = true;
        backend
            .apply(vec![to_mutation(&project), to_mutation(&found)])
            .unwrap();
        let (_, planned) =
            plan_remember(&project_rows(&backend), "work", root, Some(directory)).unwrap();
        let mut concurrent = validate(concurrent_name, "/work").unwrap();
        concurrent.worktree_dir = Some("concurrent".into());
        backend.apply(vec![to_mutation(&concurrent)]).unwrap();
        let before = project_rows(&backend);
        // A write before the assertion must roll back along with adoption's replacement deletion.
        let mut mutations = vec![to_mutation(&validate("rolled-back", "/rollback").unwrap())];
        mutations.extend(planned);
        assert!(backend.apply(mutations).unwrap_err().is_conflict());
        assert_eq!(project_rows(&backend), before);
    }
}

#[test]
fn remember_validates_directories_roots_and_row_revisions() {
    let backend = backend();
    backend
        .apply(vec![to_mutation(&validate("work", "/work").unwrap())])
        .unwrap();
    let rows = project_rows(&backend);
    assert!(plan_remember(&rows, "missing", None, Some("trees")).is_err());
    assert!(plan_remember(&rows, "work", Some("relative"), None).is_err());
    for root in [None, Some("/work")] {
        assert!(plan_remember(&rows, "work", root, Some("bad\0dir")).is_err());
        let mut truncated = rows.clone();
        truncated.truncated = true;
        assert!(plan_remember(&truncated, "work", root, Some("trees")).is_err());
        let fixtures = to_rows(&from_rows(&rows));
        assert!(plan_remember(&fixtures, "work", root, Some("trees")).is_err());
    }
}

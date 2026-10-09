use super::*;

fn backend() -> (tempfile::TempDir, DataAccess) {
    let directory = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(directory.path());
    Store(&data).define_all(&store::shared_defs()).unwrap();
    (directory, data)
}

fn add(backend: &impl Backend, root: &str, title: &str, body: &str) -> notes::Note {
    let (mutations, _) = notes::plan_add(0, Some(root), title, body, "user", &[], &[]).unwrap();
    let report = backend.apply(mutations).unwrap();
    notes::fetch_note(backend, report.ids[0].get() as u64)
        .unwrap()
        .unwrap()
}

#[test]
fn scoped_search_includes_recent_archived_title_and_body_matches_and_limits() {
    let (_directory, data) = backend();
    let backend = Store(&data);
    let first = add(&backend, "/project", "Architecture", "A searchable body");
    add(&backend, "/other", "Architecture", "A searchable body");
    backend.apply(notes::plan_archive(first.id, true)).unwrap();
    let second = add(&backend, "/project", "Newest", "Other contents");
    let recent = search(&backend, "/project", "").unwrap();
    assert_eq!(
        recent.notes.iter().map(|note| note.id).collect::<Vec<_>>(),
        [second.id, first.id]
    );
    assert!(recent.notes[1].archived);
    assert_eq!(
        search(&backend, "/project", "Architecture").unwrap().notes[0].id,
        first.id
    );
    assert_eq!(
        search(&backend, "/project", "searchable").unwrap().notes[0].id,
        first.id
    );
    assert_eq!(
        search(&backend, "/project", "chitec").unwrap().notes[0].id,
        first.id
    );
    assert!(search(&backend, "/empty", "").unwrap().notes.is_empty());
    assert!(search(&backend, "/project", "nonexistent")
        .unwrap()
        .notes
        .is_empty());
    for i in 0..51 {
        add(&backend, "/project", &format!("Extra {i}"), "shared needle");
    }
    let recent = search(&backend, "/project", "").unwrap();
    assert_eq!(recent.notes.len(), 50);
    assert!(recent.truncated);
    assert!(search(&backend, "/project", "needle").unwrap().truncated);
}

#[test]
fn document_round_trip_and_validation() {
    let note = notes::Note {
        title: "Title".into(),
        body: "First\n\n# Body heading\n  code".into(),
        ..Default::default()
    };
    assert_eq!(parse(&document(&note)).unwrap(), (note.title, note.body));
    assert_eq!(
        parse("# New\r\n\r\nBody\r\n").unwrap(),
        ("New".into(), "Body".into())
    );
    for invalid in [
        "Title\n\nBody",
        "# Title\nBody",
        "# \n\nBody",
        "# Title\n\n ",
        "# Title",
    ] {
        assert!(parse(invalid).is_err(), "{invalid:?}");
    }
    assert!(parse(&format!(
        "# {}\n\nBody",
        "x".repeat(notes::MAX_TITLE_CHARS + 1)
    ))
    .is_err());
}

#[test]
fn replacement_saves_both_fields_preserves_metadata_and_skips_unchanged() {
    let (directory, data) = backend();
    let backend = Store(&data);
    let mut note = add(&backend, "/project", "Original", "Original body");
    backend.apply(notes::plan_archive(note.id, true)).unwrap();
    backend
        .apply(
            notes::plan_set_tags(&note, &["tag".into()], None, false)
                .unwrap()
                .0,
        )
        .unwrap();
    let (todo, _) =
        project_data::todos::plan_add(&Default::default(), "/project", "Linked task", None)
            .unwrap();
    let todo_id = backend.apply(todo).unwrap().ids[0].get() as u64;
    backend
        .apply(notes::plan_link(&backend, note.id, todo_id).unwrap())
        .unwrap();
    note = notes::fetch_note(&backend, note.id).unwrap().unwrap();
    assert_eq!(note.todo_ids, [todo_id]);
    let path = directory.path().join("note.md");
    fs::write(&path, document(&note)).unwrap();
    finish(&backend, &note, &path, Ok(())).unwrap();
    assert_eq!(
        notes::fetch_note(&backend, note.id).unwrap().unwrap().rev,
        note.rev
    );
    let replacement = directory.path().join("replacement");
    fs::write(&replacement, "# Updated\n\nUpdated body\n").unwrap();
    fs::rename(replacement, &path).unwrap();
    finish(&backend, &note, &path, Ok(())).unwrap();
    let after = notes::fetch_note(&backend, note.id).unwrap().unwrap();
    assert_eq!((&*after.title, &*after.body), ("Updated", "Updated body"));
    assert_eq!(after.tags, note.tags);
    assert_eq!(after.todo_ids, note.todo_ids);
    assert_eq!(after.root, note.root);
    assert_eq!(after.author, note.author);
    assert_eq!(after.archived, note.archived);
}

#[test]
fn failures_invalid_contents_deletion_and_concurrent_edits_leave_store_unchanged() {
    let (directory, data) = backend();
    let backend = Store(&data);
    let original = add(&backend, "/project", "Original", "Body");
    let path = directory.path().join("note.md");
    fs::write(&path, "# Edited\n\nChanged body").unwrap();
    for error in ["editor failed", "forced closure"] {
        assert!(finish(&backend, &original, &path, Err(error.into())).is_err());
        assert_eq!(
            notes::fetch_note(&backend, original.id).unwrap().unwrap(),
            original
        );
    }
    fs::write(&path, "broken").unwrap();
    assert!(finish(&backend, &original, &path, Ok(())).is_err());
    assert_eq!(
        notes::fetch_note(&backend, original.id).unwrap().unwrap(),
        original
    );
    fs::write(&path, "# Edited\n\nChanged body").unwrap();
    let unusable = directory.path().join("not-a-directory");
    fs::write(&unusable, "file").unwrap();
    let broken_store = DataAccess::sqlite(&unusable);
    assert!(finish(&Store(&broken_store), &original, &path, Ok(())).is_err());
    assert_eq!(
        notes::fetch_note(&backend, original.id).unwrap().unwrap(),
        original
    );
    backend
        .apply(
            notes::plan_edit(&original, Some("Their title"), Some("Their body"), None)
                .unwrap()
                .0,
        )
        .unwrap();
    let theirs = notes::fetch_note(&backend, original.id).unwrap().unwrap();
    assert!(finish(&backend, &original, &path, Ok(()))
        .unwrap_err()
        .contains("changed during editing"));
    assert_eq!(
        notes::fetch_note(&backend, original.id).unwrap().unwrap(),
        theirs
    );
    backend
        .apply(vec![Mutation::delete("note", Pred::eq("id", original.id))])
        .unwrap();
    assert!(finish(&backend, &original, &path, Ok(()))
        .unwrap_err()
        .contains("deleted"));
    assert!(path.exists());
}

#[test]
fn worker_retains_changed_recovery_file_and_cleans_successful_edits() {
    use std::sync::{atomic::AtomicBool, Arc};
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_types::internal_ids::ClientId;
    let (_directory, data) = backend();
    let backend = Store(&data);
    let project = project_data::projects::validate("project", "/project").unwrap();
    backend
        .apply(vec![project_data::projects::to_mutation(&project)])
        .unwrap();
    let note = add(&backend, "/project", "Title", "Body");
    for outcome in ["unchanged", "saved", "cancelled", "invalid", "unconfirmed"] {
        let (tx, rx) = unbounded();
        let senders = ThreadSenders {
            to_screen: Some(SenderWithContext::new(tx)),
            ..Default::default()
        };
        let request = NoteRequest {
            client: ClientId::new(1),
            token: 1,
            revision: 1,
            project: "project".into(),
            action: NoteAction::Open(note.id),
            active: Arc::new(AtomicBool::new(true)),
        };
        let current = notes::fetch_note(&backend, note.id).unwrap().unwrap();
        let active = request.active.clone();
        std::thread::scope(|scope| {
            scope.spawn(|| handle(request, &senders, &data));
            let (
                ScreenInstruction::CommandPalette(Instruction::NoteReady {
                    path,
                    pane_title,
                    exit,
                    ..
                }),
                _,
            ) = rx.recv().unwrap()
            else {
                panic!("expected ready");
            };
            assert_eq!(pane_title, format!("#{} {}", current.id, current.title));
            if outcome == "invalid" {
                fs::write(&path, "invalid changed contents").unwrap();
            } else if outcome != "unchanged" && outcome != "unconfirmed" {
                fs::write(&path, format!("# {outcome}\n\nUpdated body")).unwrap();
            }
            if outcome == "cancelled" {
                active.store(false, Ordering::Release);
            }
            exit.send(if outcome == "unconfirmed" {
                Err("process exit unconfirmed after 5s".into())
            } else {
                Ok(())
            })
            .unwrap();
            let (ScreenInstruction::CommandPalette(Instruction::NoteFinished { result, .. }), _) =
                rx.recv().unwrap()
            else {
                panic!("expected completion");
            };
            if outcome == "unconfirmed" {
                let error = result.unwrap_err();
                assert!(error.contains("process exit unconfirmed"));
                assert!(error.contains(&path.display().to_string()));
                assert!(path.exists());
                // A delayed editor write must still have its recovery directory.
                fs::write(&path, "late edit").unwrap();
                fs::remove_dir_all(path.parent().unwrap()).unwrap();
            } else if outcome == "invalid" {
                assert!(result.unwrap_err().contains(&path.display().to_string()));
                assert_eq!(
                    fs::read_to_string(&path).unwrap(),
                    "invalid changed contents"
                );
                fs::remove_dir_all(path.parent().unwrap()).unwrap();
            } else {
                assert!(result.is_ok());
                assert!(!path.exists());
                if outcome != "unchanged" {
                    assert_eq!(
                        notes::fetch_note(&backend, note.id).unwrap().unwrap().title,
                        outcome
                    );
                }
            }
        });
    }
}

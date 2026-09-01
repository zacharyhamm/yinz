//! The note ops the store answers over the pipes, beside the todo ones in
//! [`crate::ops`].
//!
//! The same shape: a pure step that decides what to write from what was
//! loaded, and a runner that loads, applies and answers. Notes have one
//! wrinkle the todos do not — a new note's tags and links need its id, which
//! the store hands back from the first write — so `note_add` is two applies,
//! and says so when the second one fails.

use hocus_proto::notes::{self, Filter, Note, Where};
use hocus_proto::store::{self, Backend};
use hocus_proto::{projects, todos};
use serde_json::{json, Value};
use zellij_tile::prelude::FetchRequest;

pub type OpResult = Result<String, String>;

/// Whether an op is one of ours.
pub fn is_note_op(op: &str) -> bool {
    op.starts_with("note_")
}

fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn id_arg(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}

fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A list of strings, or `None` when the key is absent — which for tags means
/// "leave them", not "clear them".
fn strings_arg(args: &Value, key: &str) -> Result<Option<Vec<String>>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{key} must be a list of strings"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        // One tag as a bare string is a natural thing to type.
        Some(Value::String(one)) => Ok(Some(vec![one.clone()])),
        Some(_) => Err(format!("{key} must be a list of strings")),
    }
}

fn ids_arg(args: &Value, key: &str) -> Result<Vec<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_u64()
                    .ok_or_else(|| format!("{key} must be a list of todo item ids"))
            })
            .collect(),
        Some(Value::Number(one)) => one
            .as_u64()
            .map(|id| vec![id])
            .ok_or_else(|| format!("{key} must be a list of todo item ids")),
        Some(_) => Err(format!("{key} must be a list of todo item ids")),
    }
}

/// Wall-clock milliseconds, for the age a reply carries. The store stamps its
/// rows from the same clock.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

fn load_projects(backend: &impl Backend) -> Result<projects::Registry, String> {
    Ok(projects::from_rows(&backend.fetch(
        &FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT),
    )?))
}

/// The scope a call names: `global` for the notes of no project, `project`
/// for one project's, neither for every note. A write has to pick one of the
/// first two; a read may take all.
fn where_arg(projects: &projects::Registry, args: &Value, writing: bool) -> Result<Where, String> {
    if flag(args, "global") {
        return Ok(Where::Global);
    }
    match arg(args, "project") {
        Some(selector) => {
            let (root, name) = todos::resolve(projects, selector)?;
            Ok(Where::Project { root, name })
        },
        None if writing => Err(
            "project is required for a note, or global: true for one tied to no project; \
             call project_list"
                .to_owned(),
        ),
        None => Ok(Where::All),
    }
}

/// The note an id names, or the error that says what to call.
fn existing(backend: &impl Backend, id: u64) -> Result<Note, String> {
    notes::fetch_note(backend, id)?.ok_or_else(|| format!("no note {id}; call note_list"))
}

fn reply(projects: &projects::Registry, note: &Note, now: i64) -> Value {
    let mut value = notes::note_json(note, now);
    value["project"] = json!(notes::project_name(projects, note.root.as_deref()));
    value
}

/// Answer a write.
pub fn run_mutate(backend: &impl Backend, op: &str, args: &Value) -> OpResult {
    let projects = load_projects(backend)?;
    let now = now_ms();
    match op {
        "note_add" => {
            let (Some(title), Some(body)) = (arg(args, "title"), arg(args, "body")) else {
                return Err("title and body are required".to_owned());
            };
            let scope = where_arg(&projects, args, true)?;
            let root = scope.root();
            let author = arg(args, "author").unwrap_or("user");
            let tags = strings_arg(args, "tags")?.unwrap_or_default();
            let todo_ids = ids_arg(args, "todo_ids")?;
            notes::require_todos(backend, &todo_ids)?;
            let count = notes::count_in_scope(backend, scope.scope())?;
            let (mutations, draft) =
                notes::plan_add(count, root, title, body, author, &tags, &todo_ids)?;
            let report = backend.apply(mutations)?;
            let id = report.ids.first().copied().unwrap_or_default().max(0) as u64;
            let attach = notes::plan_attach(id, &draft.tags, &draft.todo_ids);
            let attach_error = if attach.is_empty() {
                None
            } else {
                backend.apply(attach).err()
            };
            let mut value = reply(&projects, &existing(backend, id)?, now);
            if let Some(error) = attach_error {
                value["warning"] = json!(format!(
                    "the note was written but its tags and links were not: {error}"
                ));
            }
            Ok(value.to_string())
        },
        "note_edit" => {
            let Some(id) = id_arg(args, "id") else {
                return Err("id is required".to_owned());
            };
            let note = existing(backend, id)?;
            let title = arg(args, "title");
            let body = arg(args, "body");
            let tags = strings_arg(args, "tags")?;
            let mut mutations = Vec::new();
            if title.is_some() || body.is_some() {
                mutations.extend(notes::plan_edit(&note, title, body)?.0);
            }
            if let Some(tags) = &tags {
                mutations.extend(notes::plan_set_tags(id, tags)?.0);
            }
            if mutations.is_empty() {
                return Err("give a title, a body, tags, or some of each".to_owned());
            }
            backend.apply(mutations)?;
            Ok(reply(&projects, &existing(backend, id)?, now).to_string())
        },
        "note_archive" => {
            let Some(id) = id_arg(args, "id") else {
                return Err("id is required".to_owned());
            };
            existing(backend, id)?;
            let archived = !flag(args, "undo");
            backend.apply(notes::plan_archive(id, archived))?;
            Ok(json!({ "id": id, "archived": archived }).to_string())
        },
        "note_delete" => {
            let Some(id) = id_arg(args, "id") else {
                return Err("id is required".to_owned());
            };
            let note = existing(backend, id)?;
            backend.apply(notes::plan_delete(id))?;
            Ok(json!({ "id": id, "title": note.title, "deleted": true }).to_string())
        },
        "note_link" => {
            let (Some(note_id), Some(todo_id)) = (id_arg(args, "note_id"), id_arg(args, "todo_id"))
            else {
                return Err("note_id and todo_id are required".to_owned());
            };
            existing(backend, note_id)?;
            let unlink = flag(args, "unlink");
            if unlink {
                backend.apply(notes::plan_unlink(note_id, todo_id))?;
            } else {
                notes::require_todos(backend, &[todo_id])?;
                backend.apply(notes::plan_link(note_id, todo_id))?;
            }
            Ok(json!({ "note_id": note_id, "todo_id": todo_id, "linked": !unlink }).to_string())
        },
        _ => Err(format!("unknown op {op}")),
    }
}

/// Answer a read.
pub fn run_query(backend: &impl Backend, op: &str, args: &Value) -> OpResult {
    let projects = load_projects(backend)?;
    let now = now_ms();
    match op {
        "note_get" => {
            let Some(id) = id_arg(args, "id") else {
                return Err("id is required".to_owned());
            };
            Ok(reply(&projects, &existing(backend, id)?, now).to_string())
        },
        "note_list" => {
            let scope = where_arg(&projects, args, false)?;
            let found = notes::list_notes(
                backend,
                Filter {
                    scope: Some(scope.scope()),
                    include_archived: flag(args, "include_archived"),
                    tag: arg(args, "tag"),
                    todo_id: id_arg(args, "todo_id"),
                },
            )?;
            let listed: Vec<Value> = found
                .iter()
                .map(|note| reply(&projects, note, now))
                .collect();
            Ok(json!({
                "scope": scope.label(),
                "count": listed.len(),
                "notes": listed,
            })
            .to_string())
        },
        "note_search" => {
            let Some(query) = arg(args, "query") else {
                return Err("query is required".to_owned());
            };
            let scope = where_arg(&projects, args, false)?;
            let found = notes::search(backend, scope.scope(), query)?;
            let matches: Vec<Value> = found
                .iter()
                .map(|note| {
                    let mut value = reply(&projects, note, now);
                    value["snippet"] = json!(notes::snippet(&note.body));
                    value.as_object_mut().map(|object| object.remove("body"));
                    value
                })
                .collect();
            Ok(json!({
                "query": query,
                "scope": scope.label(),
                "matches": matches,
                "truncated": found.len() >= notes::MAX_MATCHES,
            })
            .to_string())
        },
        _ => Err(format!("unknown op {op}")),
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hocus_proto::store::Local;

    /// The real engine over a temporary file, with one project remembered.
    fn backend() -> Local {
        let backend = Local::shared("hocus_store");
        backend
            .define_all(&store::shared_defs())
            .expect("declared the shared entities");
        crate::ops::run_mutate(
            &backend,
            "project_remember",
            &json!({ "name": "work", "root": "/tmp/work" }),
        )
        .expect("remembered");
        backend
    }

    fn parsed(out: &str) -> Value {
        serde_json::from_str(out).expect("json")
    }

    fn todo(backend: &impl Backend, text: &str) -> u64 {
        let out = crate::ops::run_mutate(
            backend,
            "todo_add",
            &json!({ "project": "work", "text": text }),
        )
        .expect("added a todo");
        parsed(&out)["id"].as_u64().expect("an id")
    }

    #[test]
    fn adding_a_note_returns_it_whole_with_its_project_tags_and_links() {
        let backend = backend();
        let item = todo(&backend, "ship it");
        let out = run_mutate(
            &backend,
            "note_add",
            &json!({
                "project": "work",
                "title": " Why ",
                "body": "# because\n\nreasons",
                "tags": ["Design", "rust"],
                "todo_ids": [item],
            }),
        )
        .expect("added");
        let note = parsed(&out);
        assert_eq!(note["project"], "work");
        assert_eq!(note["root"], "/tmp/work");
        assert_eq!(note["title"], "Why");
        assert_eq!(note["body"], "# because\n\nreasons");
        assert_eq!(
            note["author"], "user",
            "a pipe write is the user's unless it says otherwise"
        );
        assert_eq!(note["tags"], json!(["design", "rust"]));
        assert_eq!(note["todo_ids"], json!([item]));
        assert!(note["created_at"].as_i64().is_some_and(|ms| ms > 0));
        assert!(note["age_seconds"].as_i64().is_some());
        assert!(note.get("warning").is_none());
    }

    #[test]
    fn a_global_note_needs_no_project_and_a_project_note_needs_one() {
        let backend = backend();
        let out = run_mutate(
            &backend,
            "note_add",
            &json!({ "global": true, "title": "t", "body": "b", "author": "hocus" }),
        )
        .expect("added");
        let note = parsed(&out);
        assert_eq!(note["global"], true);
        assert_eq!(note["project"], Value::Null);
        assert_eq!(note["author"], "hocus");

        let error = run_mutate(&backend, "note_add", &json!({ "title": "t", "body": "b" }))
            .expect_err("no scope");
        assert!(error.contains("global"), "got {error}");
    }

    #[test]
    fn a_link_to_a_missing_todo_writes_nothing() {
        let backend = backend();
        let error = run_mutate(
            &backend,
            "note_add",
            &json!({ "project": "work", "title": "t", "body": "b", "todo_ids": [99] }),
        )
        .expect_err("no such todo");
        assert!(error.contains("no todo item 99"), "got {error}");
        let listed = parsed(&run_query(&backend, "note_list", &json!({})).unwrap());
        assert_eq!(listed["count"], 0);
    }

    #[test]
    fn listing_is_scoped_and_hides_archived_notes_unless_asked() {
        let backend = backend();
        run_mutate(
            &backend,
            "note_add",
            &json!({ "project": "work", "title": "in work", "body": "b" }),
        )
        .unwrap();
        let global = parsed(
            &run_mutate(
                &backend,
                "note_add",
                &json!({ "global": true, "title": "global", "body": "b" }),
            )
            .unwrap(),
        );
        run_mutate(&backend, "note_archive", &json!({ "id": global["id"] })).unwrap();

        let all = parsed(&run_query(&backend, "note_list", &json!({})).unwrap());
        assert_eq!(all["scope"], "all");
        assert_eq!(all["count"], 1, "the archived one is hidden");
        let with_archived = parsed(
            &run_query(&backend, "note_list", &json!({ "include_archived": true })).unwrap(),
        );
        assert_eq!(with_archived["count"], 2);
        let work =
            parsed(&run_query(&backend, "note_list", &json!({ "project": "work" })).unwrap());
        assert_eq!(work["scope"], "work");
        assert_eq!(work["notes"][0]["title"], "in work");
        assert!(
            work["notes"][0].get("body").is_none(),
            "a listing carries no bodies"
        );
        let global_only = parsed(
            &run_query(
                &backend,
                "note_list",
                &json!({ "global": true, "include_archived": true }),
            )
            .unwrap(),
        );
        assert_eq!(global_only["scope"], "global");
        assert_eq!(global_only["notes"][0]["archived"], true);

        run_mutate(
            &backend,
            "note_archive",
            &json!({ "id": global["id"], "undo": true }),
        )
        .unwrap();
        let back = parsed(&run_query(&backend, "note_list", &json!({ "global": true })).unwrap());
        assert_eq!(back["count"], 1);
    }

    #[test]
    fn editing_changes_what_was_given_and_keeps_the_rest() {
        let backend = backend();
        let note = parsed(
            &run_mutate(
                &backend,
                "note_add",
                &json!({ "project": "work", "title": "t", "body": "b", "tags": ["a"] }),
            )
            .unwrap(),
        );
        let id = note["id"].clone();
        let edited = parsed(
            &run_mutate(
                &backend,
                "note_edit",
                &json!({ "id": id, "body": "new body" }),
            )
            .unwrap(),
        );
        assert_eq!(edited["title"], "t");
        assert_eq!(edited["body"], "new body");
        assert_eq!(edited["tags"], json!(["a"]));
        let retagged = parsed(
            &run_mutate(
                &backend,
                "note_edit",
                &json!({ "id": id, "tags": ["z", "y"] }),
            )
            .unwrap(),
        );
        assert_eq!(retagged["tags"], json!(["y", "z"]));
        assert!(run_mutate(&backend, "note_edit", &json!({ "id": id })).is_err());
        assert!(
            run_mutate(&backend, "note_edit", &json!({ "id": 999, "title": "x" }))
                .unwrap_err()
                .contains("note_list")
        );
    }

    #[test]
    fn deleting_a_note_removes_its_links_and_the_todo_listing_stops_naming_it() {
        let backend = backend();
        let item = todo(&backend, "ship it");
        let note = parsed(
            &run_mutate(
                &backend,
                "note_add",
                &json!({ "project": "work", "title": "t", "body": "b", "todo_ids": [item] }),
            )
            .unwrap(),
        );
        let id = note["id"].as_u64().unwrap();

        let listed = parsed(
            &crate::ops::run_query(&backend, "todo_list", &json!({ "project": "work" })).unwrap(),
        );
        assert_eq!(listed["items"][0]["notes"], json!([id]));

        let deleted = parsed(&run_mutate(&backend, "note_delete", &json!({ "id": id })).unwrap());
        assert_eq!(deleted["deleted"], true);
        let listed = parsed(
            &crate::ops::run_query(&backend, "todo_list", &json!({ "project": "work" })).unwrap(),
        );
        assert!(listed["items"][0].get("notes").is_none());
        assert!(run_query(&backend, "note_get", &json!({ "id": id })).is_err());
    }

    #[test]
    fn linking_and_unlinking_round_trip() {
        let backend = backend();
        let item = todo(&backend, "ship it");
        let note = parsed(
            &run_mutate(
                &backend,
                "note_add",
                &json!({ "global": true, "title": "t", "body": "b" }),
            )
            .unwrap(),
        );
        let id = note["id"].as_u64().unwrap();
        let linked = parsed(
            &run_mutate(
                &backend,
                "note_link",
                &json!({ "note_id": id, "todo_id": item }),
            )
            .unwrap(),
        );
        assert_eq!(linked["linked"], true);
        let by_item =
            parsed(&run_query(&backend, "note_list", &json!({ "todo_id": item })).unwrap());
        assert_eq!(by_item["count"], 1);
        assert!(run_mutate(
            &backend,
            "note_link",
            &json!({ "note_id": id, "todo_id": 999 })
        )
        .is_err());
        run_mutate(
            &backend,
            "note_link",
            &json!({ "note_id": id, "todo_id": item, "unlink": true }),
        )
        .unwrap();
        let got = parsed(&run_query(&backend, "note_get", &json!({ "id": id })).unwrap());
        assert_eq!(got["todo_ids"], json!([]));
    }

    #[test]
    fn search_finds_a_word_in_a_body_and_answers_with_a_snippet() {
        let backend = backend();
        run_mutate(
            &backend,
            "note_add",
            &json!({ "project": "work", "title": "Plan", "body": "rebase before merging" }),
        )
        .unwrap();
        run_mutate(
            &backend,
            "note_add",
            &json!({ "global": true, "title": "Other", "body": "nothing here" }),
        )
        .unwrap();
        let found =
            parsed(&run_query(&backend, "note_search", &json!({ "query": "rebase" })).unwrap());
        assert_eq!(found["matches"].as_array().unwrap().len(), 1);
        assert_eq!(found["matches"][0]["snippet"], "rebase before merging");
        assert!(found["matches"][0].get("body").is_none());
        assert_eq!(found["truncated"], false);
        let scoped = parsed(
            &run_query(
                &backend,
                "note_search",
                &json!({ "query": "rebase", "global": true }),
            )
            .unwrap(),
        );
        assert_eq!(scoped["matches"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn missing_arguments_are_reported_rather_than_panicking() {
        let backend = backend();
        for (op, args) in [
            ("note_add", json!({ "project": "work", "title": "t" })),
            ("note_add", json!({ "project": "work", "body": "b" })),
            ("note_edit", json!({})),
            ("note_archive", json!({})),
            ("note_delete", json!({})),
            ("note_link", json!({ "note_id": 1 })),
            (
                "note_add",
                json!({ "project": "work", "title": "t", "body": "b", "tags": "two words" }),
            ),
            (
                "note_add",
                json!({ "project": "work", "title": "t", "body": "b", "todo_ids": ["x"] }),
            ),
        ] {
            assert!(run_mutate(&backend, op, &args).is_err(), "{op} should fail");
        }
        assert!(run_query(&backend, "note_get", &json!({})).is_err());
        assert!(run_query(&backend, "note_search", &json!({})).is_err());
        assert!(run_query(&backend, "note_nope", &json!({})).is_err());
    }
}

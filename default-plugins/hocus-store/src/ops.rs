//! The mutations and queries the store answers.
//!
//! Free functions over the two stores rather than methods on the plugin, so
//! that every one of them is testable on the host without a Zellij at all —
//! which matters more here than elsewhere, because this is the only plugin that
//! owns anything the user would miss if it were lost.

use hocus_proto::{projects, todos};
use serde_json::{json, Value};

/// What a mutation did, or why it did nothing. The `Err` text goes straight
/// back to the model, so it says what to call next wherever it can.
pub type OpResult = Result<String, String>;

fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Resolve which project's todo list a call means.
///
/// The caller passes the selector — hocus knows its own active project and the
/// CLI has to say. Returns the normalized root, which is what the todo store is
/// keyed by, and the project's name for the reply, which is absent for a list
/// whose project is gone.
fn todo_root(
    projects: &projects::Registry,
    args: &Value,
) -> Result<(String, Option<String>), String> {
    let Some(selector) = arg(args, "project") else {
        return Err("project is required; call project_list".to_owned());
    };
    todos::resolve(projects, selector)
}

/// Apply a mutation. Returns `Ok` with what to tell the caller, and whether the
/// stores changed — a failed call must not trigger a write or a snapshot.
pub fn mutate(
    projects: &mut projects::Registry,
    todos_store: &mut todos::Store,
    op: &str,
    args: &Value,
) -> OpResult {
    match op {
        "project_remember" => {
            let (Some(name), Some(root)) = (arg(args, "name"), arg(args, "root")) else {
                return Err("name and root are required".to_owned());
            };
            let project = projects::validate(name, root)?;
            let name = project.name.clone();
            match projects::adopt(projects, project) {
                Some(stale) => Ok(format!("remembered {name}, replacing discovered {stale}")),
                None => Ok(format!("remembered {name}")),
            }
        },
        "project_forget" => {
            let Some(name) = arg(args, "name") else {
                return Err("name is required".to_owned());
            };
            // The folder's todo list stays. It is keyed by root rather than by
            // project, `todos::resolve` still answers for it, and forgetting a
            // project is not a reason to throw away the user's own notes.
            match projects.remove(name) {
                Some(project) => Ok(projects::forgotten(&project)),
                None => Err(format!("unknown project {name}; call project_list")),
            }
        },
        "todo_add" => {
            let Some(text) = arg(args, "text") else {
                return Err("text is required".to_owned());
            };
            let (root, name) = todo_root(projects, args)?;
            let parent_id = args.get("parent_id").and_then(Value::as_u64);
            let id = todos::add(todos_store, &root, text, parent_id)?;
            Ok(json!({ "project": name, "id": id, "text": text }).to_string())
        },
        "todo_done" => {
            let Some(id) = args.get("id").and_then(Value::as_u64) else {
                return Err("id is required".to_owned());
            };
            let (root, name) = todo_root(projects, args)?;
            // `undo` absent means crossing off, which is the common call.
            let done = !args.get("undo").and_then(Value::as_bool).unwrap_or(false);
            let change = todos::set_done(todos_store, &root, id, done)?;
            Ok(json!({
                "project": name,
                "id": change.id,
                "text": change.text,
                "done": change.done,
                "cascaded": change.cascaded,
                "reopened": change.reopened,
            })
            .to_string())
        },
        _ => Err(format!("unknown op {op}")),
    }
}

/// Answer a read. Separate from `mutate` so that nothing here can write, rather
/// than that being a property of each arm that someone has to keep true.
pub fn query(
    projects: &projects::Registry,
    todos_store: &todos::Store,
    op: &str,
    args: &Value,
) -> OpResult {
    match op {
        "project_list" => {
            let items: Vec<Value> = projects
                .values()
                .map(|project| {
                    json!({
                        "name": project.name,
                        "root": project.root,
                        "remote": project.remote,
                        "worktree_status": project.worktree_status,
                        "discovered": project.discovered,
                    })
                })
                .collect();
            Ok(json!({ "projects": items }).to_string())
        },
        "todo_list" => {
            let (root, name) = todo_root(projects, args)?;
            let include_done = args
                .get("include_done")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut value = todos::list_json(todos_store, &root, include_done);
            value["project"] = json!(name);
            Ok(value.to_string())
        },
        "todo_search" => {
            let Some(query) = arg(args, "query") else {
                return Err("query is required".to_owned());
            };
            // Absent project means every project, which is the useful default
            // for a search and the opposite of the default for a list.
            let roots: Vec<String> = match arg(args, "project") {
                Some(_) => vec![todo_root(projects, args)?.0],
                None => projects
                    .values()
                    .filter_map(|project| todos::normalize_root(&project.root).ok())
                    .collect(),
            };
            let matches: Vec<Value> = todos::search(todos_store, &roots, query)
                .into_iter()
                .map(|found| {
                    json!({
                        "project": projects
                            .values()
                            .find(|project| todos::normalize_root(&project.root).ok().as_deref() == Some(found.root.as_str()))
                            .map(|project| project.name.clone()),
                        "id": found.id,
                        "text": found.text,
                        "done": found.done,
                        "trail": found.trail,
                    })
                })
                .collect();
            Ok(json!({ "matches": matches }).to_string())
        },
        _ => Err(format!("unknown op {op}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_project() -> (projects::Registry, todos::Store) {
        let mut projects = projects::Registry::new();
        projects.insert(
            "work".to_owned(),
            projects::Project {
                name: "work".to_owned(),
                root: "/tmp/work".to_owned(),
                remote: None,
                worktree_status: None,
                common_dir: None,
                discovered: false,
            },
        );
        (projects, todos::Store::new())
    }

    #[test]
    fn adding_a_todo_names_the_project_and_returns_its_id() {
        let (mut projects, mut todos_store) = store_with_project();
        let out = mutate(
            &mut projects,
            &mut todos_store,
            "todo_add",
            &json!({ "project": "work", "text": "ship it" }),
        )
        .expect("added");
        let value: Value = serde_json::from_str(&out).expect("json");
        assert_eq!(value["project"], "work");
        assert_eq!(value["text"], "ship it");
        assert!(value["id"].as_u64().is_some());
    }

    #[test]
    fn a_todo_for_an_unknown_project_says_what_to_call() {
        let (mut projects, mut todos_store) = store_with_project();
        let error = mutate(
            &mut projects,
            &mut todos_store,
            "todo_add",
            &json!({ "project": "nope", "text": "x" }),
        )
        .expect_err("unknown project");
        assert!(error.contains("project_list"), "got {error}");
    }

    /// A list outlives the project that made it, and the read tools still find
    /// it by root, so a write by root has to land too.
    #[test]
    fn a_todo_can_be_added_by_root_for_a_list_whose_project_is_gone() {
        let (mut projects, mut todos_store) = store_with_project();
        let out = mutate(
            &mut projects,
            &mut todos_store,
            "todo_add",
            &json!({ "project": "/tmp/orphan", "text": "still mine" }),
        )
        .expect("added");
        let value: Value = serde_json::from_str(&out).expect("json");
        assert_eq!(value["project"], Value::Null);
        let id = value["id"].as_u64().expect("id");
        assert!(mutate(
            &mut projects,
            &mut todos_store,
            "todo_done",
            &json!({ "project": "/tmp/orphan", "id": id }),
        )
        .is_ok());
        let listed = query(
            &projects,
            &todos_store,
            "todo_list",
            &json!({ "project": "/tmp/orphan", "include_done": true }),
        )
        .expect("listed");
        assert!(listed.contains("still mine"), "got {listed}");
    }

    /// The store is driven by a model and by a CLI, so a missing argument has
    /// to be an answer rather than a panic.
    #[test]
    fn missing_arguments_are_reported_rather_than_panicking() {
        let (mut projects, mut todos_store) = store_with_project();
        for (op, args) in [
            ("todo_add", json!({ "project": "work" })),
            ("todo_done", json!({ "project": "work" })),
            ("project_remember", json!({ "name": "x" })),
            ("project_forget", json!({})),
        ] {
            assert!(mutate(&mut projects, &mut todos_store, op, &args).is_err());
        }
        assert!(query(&projects, &todos_store, "todo_search", &json!({})).is_err());
    }

    #[test]
    fn an_unknown_op_is_an_error_on_both_paths() {
        let (mut projects, mut todos_store) = store_with_project();
        assert!(mutate(
            &mut projects,
            &mut todos_store,
            "drop_everything",
            &json!({})
        )
        .is_err());
        assert!(query(&projects, &todos_store, "drop_everything", &json!({})).is_err());
    }

    /// A failed mutation must leave nothing behind — the reply is what the
    /// caller sees, but the store is what everyone else does.
    #[test]
    fn a_failed_mutation_changes_nothing() {
        let (mut projects, mut todos_store) = store_with_project();
        let before = (projects.clone(), todos_store.clone());
        let _ = mutate(
            &mut projects,
            &mut todos_store,
            "todo_add",
            &json!({ "project": "nope", "text": "x" }),
        );
        assert_eq!(projects, before.0);
        assert_eq!(todos_store, before.1);
    }

    #[test]
    fn crossing_off_and_reopening_round_trips() {
        let (mut projects, mut todos_store) = store_with_project();
        let added = mutate(
            &mut projects,
            &mut todos_store,
            "todo_add",
            &json!({ "project": "work", "text": "ship it" }),
        )
        .expect("added");
        let id = serde_json::from_str::<Value>(&added).expect("json")["id"]
            .as_u64()
            .expect("id");

        let done = mutate(
            &mut projects,
            &mut todos_store,
            "todo_done",
            &json!({ "project": "work", "id": id }),
        )
        .expect("crossed off");
        assert_eq!(
            serde_json::from_str::<Value>(&done).expect("json")["done"],
            true
        );

        let undone = mutate(
            &mut projects,
            &mut todos_store,
            "todo_done",
            &json!({ "project": "work", "id": id, "undo": true }),
        )
        .expect("reopened");
        assert_eq!(
            serde_json::from_str::<Value>(&undone).expect("json")["done"],
            false
        );
    }

    #[test]
    fn search_covers_every_project_when_none_is_named() {
        let (mut projects, mut todos_store) = store_with_project();
        projects.insert(
            "other".to_owned(),
            projects::Project {
                name: "other".to_owned(),
                root: "/tmp/other".to_owned(),
                remote: None,
                worktree_status: None,
                common_dir: None,
                discovered: true,
            },
        );
        for project in ["work", "other"] {
            mutate(
                &mut projects,
                &mut todos_store,
                "todo_add",
                &json!({ "project": project, "text": "shared word" }),
            )
            .expect("added");
        }
        let out = query(
            &projects,
            &todos_store,
            "todo_search",
            &json!({"query": "shared"}),
        )
        .expect("searched");
        let matches = serde_json::from_str::<Value>(&out).expect("json")["matches"]
            .as_array()
            .expect("array")
            .len();
        assert_eq!(matches, 2, "both projects are searched");
    }

    #[test]
    fn forgetting_a_project_removes_it_and_only_it() {
        let (mut projects, mut todos_store) = store_with_project();
        mutate(
            &mut projects,
            &mut todos_store,
            "project_remember",
            &json!({ "name": "other", "root": "/tmp/other" }),
        )
        .expect("remembered");
        mutate(
            &mut projects,
            &mut todos_store,
            "project_forget",
            &json!({ "name": "other" }),
        )
        .expect("forgot");
        assert!(projects.contains_key("work"));
        assert!(!projects.contains_key("other"));
    }

    /// Forgetting a project is not a way to lose notes: the list is keyed by
    /// the folder, and the read tools still answer for it by root.
    #[test]
    fn forgetting_a_project_keeps_the_todo_list_of_its_folder() {
        let (mut projects, mut todos_store) = store_with_project();
        mutate(
            &mut projects,
            &mut todos_store,
            "todo_add",
            &json!({ "project": "work", "text": "ship it" }),
        )
        .expect("added");
        mutate(
            &mut projects,
            &mut todos_store,
            "project_forget",
            &json!({ "name": "work" }),
        )
        .expect("forgot");

        let list = query(
            &projects,
            &todos_store,
            "todo_list",
            &json!({ "project": "/tmp/work" }),
        )
        .expect("listed");
        assert!(list.contains("ship it"), "got {list}");
    }

    /// A discovered project comes back on the next poll, so the answer says so
    /// rather than reading as a deletion that did not take.
    #[test]
    fn forgetting_a_discovered_project_says_a_poll_makes_it_again() {
        let (mut projects, mut todos_store) = store_with_project();
        projects.insert(
            "found".to_owned(),
            projects::Project {
                name: "found".to_owned(),
                root: "/tmp/found".to_owned(),
                remote: None,
                worktree_status: Some(projects::WorktreeStatus::Primary),
                common_dir: Some("/tmp/found/.git".to_owned()),
                discovered: true,
            },
        );
        let said = mutate(
            &mut projects,
            &mut todos_store,
            "project_forget",
            &json!({ "name": "found" }),
        )
        .expect("forgot");

        assert!(said.contains("/tmp/found"), "got {said}");
        assert_eq!(
            mutate(
                &mut projects,
                &mut todos_store,
                "project_forget",
                &json!({ "name": "work" }),
            ),
            Ok("forgot work".to_owned()),
            "a project the user named goes quietly"
        );
    }
}

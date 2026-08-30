//! The mutations and queries the store answers.
//!
//! Free functions rather than methods on the plugin, so that every one of them
//! is testable on the host without a Zellij at all — which matters more here
//! than elsewhere, because this is the only plugin that owns anything the user
//! would miss if it were lost.
//!
//! Each op is split in two. A **planner** takes a [`StoreView`] — the rows this
//! call needs, already materialized — and answers with the mutations to apply
//! and what to tell the caller. A **runner** loads that view, calls the planner
//! and applies the result, and is three lines long.
//!
//! The split is what keeps the testability the free functions were for. A
//! planner is pure, and `Mutation` compares by value, so most of what used to be
//! asserted about two `BTreeMap`s is now asserted about a `Vec<Mutation>`. The
//! runners are then exercised end to end against the real store engine, so the
//! round trips below are checking SQLite rather than a fake.
//!
//! Planners work by cloning the materialized registry, running the *existing*
//! in-memory function on the copy, and diffing. `adopt` and `set_done` are not
//! rewritten as predicates: there would then be two copies of each rule.

use hocus_proto::store::Backend;
use hocus_proto::{projects, store, todos};
use serde_json::{json, Value};
use zellij_tile::prelude::{ApplyReport, FetchRequest, Mutation};

/// What a mutation did, or why it did nothing. The `Err` text goes straight
/// back to the model, so it says what to call next wherever it can.
pub type OpResult = Result<String, String>;

/// The part of the store one call needs to see.
///
/// `list` holds the single project's todo list the call is about, if it is about
/// one. Loading only that list is the point: the old code held every list of
/// every project in memory because they all lived in one file.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StoreView {
    pub projects: projects::Registry,
    pub list: Option<(String, todos::List)>,
}

impl StoreView {
    /// The list this call is about, or an empty one. A project with no todos yet
    /// has no rows, which is not an error.
    fn list(&self) -> todos::List {
        self.list
            .as_ref()
            .map(|(_, list)| list.clone())
            .unwrap_or_default()
    }
}

/// A reply, which may still be waiting on an id the write will hand back.
///
/// `todo_add` has to tell the model the id of the item it just made, and that id
/// is allocated by the store. So the planner says what it will say, and the
/// runner finishes it once the apply reports.
#[derive(Debug, PartialEq, Eq)]
pub enum Reply {
    Fixed(String),
    AddedTodo {
        project: Option<String>,
        text: String,
    },
}

impl Reply {
    pub fn finish(self, report: &ApplyReport) -> String {
        match self {
            Reply::Fixed(message) => message,
            Reply::AddedTodo { project, text } => json!({
                "project": project,
                // The first put of the batch is the new item; any others reopen
                // its ancestors and insert nothing.
                "id": report.ids.first().copied().unwrap_or_default(),
                "text": text,
            })
            .to_string(),
        }
    }
}

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

/// Whether an op is about one project's todo list, and so needs it loaded.
///
/// Kept beside the planners rather than inside them because the runner has to
/// know before it can build the view they take.
fn needs_list(op: &str) -> bool {
    matches!(op, "todo_add" | "todo_done" | "todo_edit" | "todo_list")
}

/// Plan a mutation: what to write, and what to say about it.
///
/// Nothing here writes. A call that fails returns `Err` having produced no
/// mutations, which is how a rejected call leaves the store alone — the old
/// version had to be careful not to persist after a failure, and now there is
/// nothing to be careful about.
pub fn plan_mutate(
    view: &StoreView,
    op: &str,
    args: &Value,
) -> Result<(Vec<Mutation>, Reply), String> {
    match op {
        "project_remember" => {
            let (Some(name), Some(root)) = (arg(args, "name"), arg(args, "root")) else {
                return Err("name and root are required".to_owned());
            };
            let project = projects::validate(name, root)?;
            let name = project.name.clone();
            // `adopt` on a copy, then diff. It resolves a discovered project at
            // the same root by inheriting its Git identity and removing it, and
            // the diff turns that into the delete and the put — in one batch, so
            // no reader ever sees the project twice or not at all.
            let mut after = view.projects.clone();
            let stale = projects::adopt(&mut after, project);
            let message = match &stale {
                Some(stale) => format!("remembered {name}, replacing discovered {stale}"),
                None => format!("remembered {name}"),
            };
            Ok((
                projects::diff_mutations(&view.projects, &after),
                Reply::Fixed(message),
            ))
        },
        "project_forget" => {
            let Some(name) = arg(args, "name") else {
                return Err("name is required".to_owned());
            };
            // The folder's todo list stays. It is keyed by root rather than by
            // project, `todos::resolve` still answers for it, and forgetting a
            // project is not a reason to throw away the user's own notes.
            match view.projects.get(name) {
                Some(project) => Ok((
                    vec![projects::delete_mutation(name)],
                    Reply::Fixed(projects::forgotten(project)),
                )),
                None => Err(format!("unknown project {name}; call project_list")),
            }
        },
        "todo_add" => {
            let Some(text) = arg(args, "text") else {
                return Err("text is required".to_owned());
            };
            let (root, name) = todo_root(&view.projects, args)?;
            let parent_id = args.get("parent_id").and_then(Value::as_u64);
            let (mutations, text) = todos::plan_add(&view.list(), &root, text, parent_id)?;
            Ok((
                mutations,
                Reply::AddedTodo {
                    project: name,
                    text,
                },
            ))
        },
        "todo_done" => {
            let Some(id) = args.get("id").and_then(Value::as_u64) else {
                return Err("id is required".to_owned());
            };
            let (root, name) = todo_root(&view.projects, args)?;
            // `undo` absent means crossing off, which is the common call.
            let done = !args.get("undo").and_then(Value::as_bool).unwrap_or(false);
            let (mutations, change) = todos::plan_set_done(&view.list(), &root, id, done)?;
            Ok((
                mutations,
                Reply::Fixed(
                    json!({
                        "project": name,
                        "id": change.id,
                        "text": change.text,
                        "done": change.done,
                        "cascaded": change.cascaded,
                        "reopened": change.reopened,
                    })
                    .to_string(),
                ),
            ))
        },
        "todo_edit" => {
            let (Some(id), Some(text)) =
                (args.get("id").and_then(Value::as_u64), arg(args, "text"))
            else {
                return Err("id and text are required".to_owned());
            };
            let (_, name) = todo_root(&view.projects, args)?;
            let (mutations, text) = todos::plan_edit(&view.list(), id, text)?;
            Ok((
                mutations,
                Reply::Fixed(
                    json!({
                        "project": name,
                        "id": id,
                        "text": text,
                    })
                    .to_string(),
                ),
            ))
        },
        _ => Err(format!("unknown op {op}")),
    }
}

/// Answer a read from an already-loaded view. Separate from [`plan_mutate`] so
/// that nothing here can write, rather than that being a property of each arm
/// that someone has to keep true.
///
/// `todo_search` is not here: it is answered by the store's full-text index
/// rather than out of a view, so it has its own runner below.
pub fn answer_query(view: &StoreView, op: &str, args: &Value) -> OpResult {
    match op {
        "project_list" => {
            let items: Vec<Value> = view
                .projects
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
            let (root, name) = todo_root(&view.projects, args)?;
            let include_done = args
                .get("include_done")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut one = todos::Store::new();
            one.insert(root.clone(), view.list());
            let mut value = todos::list_json(&one, &root, include_done);
            value["project"] = json!(name);
            Ok(value.to_string())
        },
        "todo_search" => Err("todo_search is answered by run_search".to_owned()),
        _ => Err(format!("unknown op {op}")),
    }
}

// ---------------------------------------------------------------------------
// Runners — the only functions here that touch the store
// ---------------------------------------------------------------------------

/// Materialize what an op needs to see.
fn load_view(backend: &impl Backend, op: &str, args: &Value) -> Result<StoreView, String> {
    let projects = projects::from_rows(
        &backend.fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))?,
    );
    let mut view = StoreView {
        projects,
        list: None,
    };
    if needs_list(op) {
        // A selector that names nothing is the op's error to report, not this
        // one's — planning it produces the message that says what to call.
        if let Ok((root, _)) = todo_root(&view.projects, args) {
            let list = todos::fetch_list(backend, &root)?;
            view.list = Some((root, list));
        }
    }
    Ok(view)
}

/// Load, plan, apply.
pub fn run_mutate(backend: &impl Backend, op: &str, args: &Value) -> OpResult {
    let view = load_view(backend, op, args)?;
    let (mutations, reply) = plan_mutate(&view, op, args)?;
    let report = backend.apply(mutations)?;
    Ok(reply.finish(&report))
}

/// Load and answer, or search.
pub fn run_query(backend: &impl Backend, op: &str, args: &Value) -> OpResult {
    if op == "todo_search" {
        return run_search(backend, args);
    }
    let view = load_view(backend, op, args)?;
    answer_query(&view, op, args)
}

/// Search the todo text through the store's index.
///
/// Two passes, because they mean different things. Full-text matches whole
/// words and ranks by relevance, which is what someone typing a word wants. But
/// the tool this replaced matched substrings, so `reba` used to find `rebase`
/// and would silently stop — hence the fallback, which is only paid for when the
/// index found nothing at all.
fn run_search(backend: &impl Backend, args: &Value) -> OpResult {
    let Some(query) = arg(args, "query") else {
        return Err("query is required".to_owned());
    };
    let projects = projects::from_rows(
        &backend.fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))?,
    );
    // Absent project means every project, which is the useful default for a
    // search and the opposite of the default for a list.
    let scope: Option<String> = match arg(args, "project") {
        Some(selector) => Some(todos::resolve(&projects, selector)?.0),
        None => None,
    };

    let matches: Vec<Value> = todos::search(backend, scope.as_deref(), query)?
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
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hocus_proto::store::Local;

    /// A store with the entities declared and one project in it.
    ///
    /// The backend is the real engine over a temporary file, so everything below
    /// is a genuine round trip: the predicates, the upserts and the full-text
    /// index are the ones a session runs.
    fn backend() -> Local {
        let backend = Local::shared("hocus_store");
        backend
            .define_all(&store::shared_defs())
            .expect("declared the shared entities");
        remember(&backend, "work", "/tmp/work");
        backend
    }

    fn remember(backend: &impl Backend, name: &str, root: &str) {
        run_mutate(
            backend,
            "project_remember",
            &json!({ "name": name, "root": root }),
        )
        .expect("remembered");
    }

    fn registry(backend: &impl Backend) -> projects::Registry {
        load_view(backend, "project_list", &json!({}))
            .expect("loaded")
            .projects
    }

    fn field(out: &str, key: &str) -> Value {
        serde_json::from_str::<Value>(out).expect("json")[key].clone()
    }

    #[test]
    fn adding_a_todo_names_the_project_and_returns_its_id() {
        let backend = backend();
        let out = run_mutate(
            &backend,
            "todo_add",
            &json!({ "project": "work", "text": " **ship it**\n\t- 日本語\u{1b} " }),
        )
        .expect("added");
        assert_eq!(field(&out, "project"), "work");
        assert_eq!(field(&out, "text"), "**ship it**\n    - 日本語");
        assert!(field(&out, "id").as_u64().is_some_and(|id| id > 0));
        let listed =
            run_query(&backend, "todo_list", &json!({ "project": "work" })).expect("listed");
        assert_eq!(field(&listed, "items")[0]["text"], field(&out, "text"));
    }

    #[test]
    fn a_todo_for_an_unknown_project_says_what_to_call() {
        let backend = backend();
        let error = run_mutate(
            &backend,
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
        let backend = backend();
        let out = run_mutate(
            &backend,
            "todo_add",
            &json!({ "project": "/tmp/orphan", "text": "still mine" }),
        )
        .expect("added");
        assert_eq!(field(&out, "project"), Value::Null);
        let id = field(&out, "id").as_u64().expect("id");

        assert!(run_mutate(
            &backend,
            "todo_done",
            &json!({ "project": "/tmp/orphan", "id": id }),
        )
        .is_ok());
        let listed = run_query(
            &backend,
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
        let backend = backend();
        for (op, args) in [
            ("todo_add", json!({ "project": "work" })),
            ("todo_done", json!({ "project": "work" })),
            ("todo_edit", json!({ "project": "work" })),
            ("todo_edit", json!({ "project": "work", "id": 1 })),
            ("project_remember", json!({ "name": "x" })),
            ("project_forget", json!({})),
        ] {
            assert!(run_mutate(&backend, op, &args).is_err(), "{op} should fail");
        }
        assert!(run_query(&backend, "todo_search", &json!({})).is_err());
    }

    #[test]
    fn an_unknown_op_is_an_error_on_both_paths() {
        let backend = backend();
        assert!(run_mutate(&backend, "drop_everything", &json!({})).is_err());
        assert!(run_query(&backend, "drop_everything", &json!({})).is_err());
    }

    /// A failed mutation must leave nothing behind — the reply is what the
    /// caller sees, but the store is what everyone else does.
    ///
    /// This used to need care: the plugin wrote both files after a call and had
    /// to skip that on failure. Now a planner that returns `Err` has produced no
    /// mutations, so there is nothing to skip.
    #[test]
    fn a_failed_mutation_changes_nothing() {
        let backend = backend();
        let before = registry(&backend);
        let _ = run_mutate(
            &backend,
            "todo_add",
            &json!({ "project": "nope", "text": "x" }),
        );
        assert_eq!(registry(&backend), before);
        let listed =
            run_query(&backend, "todo_list", &json!({ "project": "work" })).expect("listed");
        assert_eq!(field(&listed, "open"), 0);
    }

    #[test]
    fn crossing_off_and_reopening_round_trips() {
        let backend = backend();
        let added = run_mutate(
            &backend,
            "todo_add",
            &json!({ "project": "work", "text": "ship it" }),
        )
        .expect("added");
        let id = field(&added, "id").as_u64().expect("id");

        let done = run_mutate(
            &backend,
            "todo_done",
            &json!({ "project": "work", "id": id }),
        )
        .expect("crossed off");
        assert_eq!(field(&done, "done"), true);

        let undone = run_mutate(
            &backend,
            "todo_done",
            &json!({ "project": "work", "id": id, "undo": true }),
        )
        .expect("reopened");
        assert_eq!(field(&undone, "done"), false);
    }

    #[test]
    fn editing_a_todo_changes_its_text() {
        let backend = backend();
        let added = run_mutate(
            &backend,
            "todo_add",
            &json!({ "project": "work", "text": "ship it" }),
        )
        .expect("added");
        let id = field(&added, "id").as_u64().expect("id");

        let edited = run_mutate(
            &backend,
            "todo_edit",
            &json!({ "project": "work", "id": id, "text": "ship it faster" }),
        )
        .expect("edited");
        assert_eq!(field(&edited, "id"), id);
        assert_eq!(field(&edited, "text"), "ship it faster");

        let listed = run_query(
            &backend,
            "todo_list",
            &json!({ "project": "work", "include_done": true }),
        )
        .expect("listed");
        assert!(listed.contains("ship it faster"), "got {listed}");
    }

    #[test]
    fn editing_an_unknown_todo_is_an_error() {
        let backend = backend();
        let error = run_mutate(
            &backend,
            "todo_edit",
            &json!({ "project": "work", "id": 999, "text": "x" }),
        )
        .expect_err("no such item");
        assert!(error.contains("no todo item 999"), "got {error}");
    }

    #[test]
    fn search_covers_every_project_when_none_is_named() {
        let backend = backend();
        remember(&backend, "other", "/tmp/other");
        for project in ["work", "other"] {
            run_mutate(
                &backend,
                "todo_add",
                &json!({ "project": project, "text": "shared word" }),
            )
            .expect("added");
        }
        let out =
            run_query(&backend, "todo_search", &json!({"query": "shared"})).expect("searched");
        assert_eq!(
            field(&out, "matches").as_array().expect("array").len(),
            2,
            "both projects are searched"
        );
    }

    #[test]
    fn search_can_be_scoped_to_one_project() {
        let backend = backend();
        remember(&backend, "other", "/tmp/other");
        for project in ["work", "other"] {
            run_mutate(
                &backend,
                "todo_add",
                &json!({ "project": project, "text": "shared word" }),
            )
            .expect("added");
        }
        let out = run_query(
            &backend,
            "todo_search",
            &json!({"query": "shared", "project": "work"}),
        )
        .expect("searched");
        let matches = field(&out, "matches");
        let matches = matches.as_array().expect("array");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["project"], "work");
    }

    /// The behaviour the substring fallback exists to keep. FTS5 matches whole
    /// tokens, so without it a partial word would silently stop finding
    /// anything.
    #[test]
    fn search_still_finds_a_fragment_of_a_word() {
        let backend = backend();
        run_mutate(
            &backend,
            "todo_add",
            &json!({ "project": "work", "text": "fixing a rebase conflict" }),
        )
        .expect("added");

        for query in ["rebase", "reba"] {
            let out =
                run_query(&backend, "todo_search", &json!({ "query": query })).expect("searched");
            assert_eq!(
                field(&out, "matches").as_array().expect("array").len(),
                1,
                "searching {query} should find the item"
            );
        }
    }

    /// A match carries the items above it so the model can tell two similar
    /// items apart. The trail is rebuilt from the list, not stored.
    #[test]
    fn a_match_carries_the_trail_of_items_above_it() {
        let backend = backend();
        let parent = run_mutate(
            &backend,
            "todo_add",
            &json!({ "project": "work", "text": "the epic" }),
        )
        .expect("added");
        run_mutate(
            &backend,
            "todo_add",
            &json!({
                "project": "work",
                "text": "the needle",
                "parent_id": field(&parent, "id").as_u64().expect("id"),
            }),
        )
        .expect("added");

        let out =
            run_query(&backend, "todo_search", &json!({ "query": "needle" })).expect("searched");
        let matches = field(&out, "matches");
        assert_eq!(
            matches.as_array().expect("array")[0]["trail"],
            json!(["the epic", "the needle"])
        );
    }

    /// A matched parent and a matched child are two results. Building a tree out
    /// of the hits would hide the child inside the parent.
    #[test]
    fn a_matched_parent_and_child_are_both_reported() {
        let backend = backend();
        let parent = run_mutate(
            &backend,
            "todo_add",
            &json!({ "project": "work", "text": "widget one" }),
        )
        .expect("added");
        run_mutate(
            &backend,
            "todo_add",
            &json!({
                "project": "work",
                "text": "widget two",
                "parent_id": field(&parent, "id").as_u64().expect("id"),
            }),
        )
        .expect("added");

        let out =
            run_query(&backend, "todo_search", &json!({ "query": "widget" })).expect("searched");
        assert_eq!(field(&out, "matches").as_array().expect("array").len(), 2);
    }

    #[test]
    fn forgetting_a_project_removes_it_and_only_it() {
        let backend = backend();
        remember(&backend, "other", "/tmp/other");
        run_mutate(&backend, "project_forget", &json!({ "name": "other" })).expect("forgot");

        let stored = registry(&backend);
        assert!(stored.contains_key("work"));
        assert!(!stored.contains_key("other"));
    }

    /// Forgetting a project is not a way to lose notes: the list is keyed by
    /// the folder, and the read tools still answer for it by root.
    #[test]
    fn forgetting_a_project_keeps_the_todo_list_of_its_folder() {
        let backend = backend();
        run_mutate(
            &backend,
            "todo_add",
            &json!({ "project": "work", "text": "ship it" }),
        )
        .expect("added");
        run_mutate(&backend, "project_forget", &json!({ "name": "work" })).expect("forgot");

        let list =
            run_query(&backend, "todo_list", &json!({ "project": "/tmp/work" })).expect("listed");
        assert!(list.contains("ship it"), "got {list}");
    }

    /// A discovered project comes back on the next poll, so the answer says so
    /// rather than reading as a deletion that did not take.
    #[test]
    fn forgetting_a_discovered_project_says_a_poll_makes_it_again() {
        let backend = backend();
        let found = projects::Project {
            name: "found".to_owned(),
            root: "/tmp/found".to_owned(),
            remote: None,
            worktree_status: Some(projects::WorktreeStatus::Primary),
            common_dir: Some("/tmp/found/.git".to_owned()),
            discovered: true,
        };
        backend
            .apply(vec![projects::to_mutation(&found)])
            .expect("written");

        let said =
            run_mutate(&backend, "project_forget", &json!({ "name": "found" })).expect("forgot");
        assert!(said.contains("/tmp/found"), "got {said}");
        assert_eq!(
            run_mutate(&backend, "project_forget", &json!({ "name": "work" })),
            Ok("forgot work".to_owned()),
            "a project the user named goes quietly"
        );
    }

    /// Remembering a folder a poll already found takes over its Git identity
    /// and leaves one row, not two.
    #[test]
    fn remembering_a_discovered_folder_replaces_it() {
        let backend = backend();
        let found = projects::Project {
            name: "tmp-found".to_owned(),
            root: "/tmp/found".to_owned(),
            remote: Some("git@example.com:me/found".to_owned()),
            worktree_status: Some(projects::WorktreeStatus::Primary),
            common_dir: None,
            discovered: true,
        };
        backend
            .apply(vec![projects::to_mutation(&found)])
            .expect("written");

        let said = run_mutate(
            &backend,
            "project_remember",
            &json!({ "name": "mine", "root": "/tmp/found" }),
        )
        .expect("remembered");
        assert!(
            said.contains("replacing discovered tmp-found"),
            "got {said}"
        );

        let stored = registry(&backend);
        assert!(!stored.contains_key("tmp-found"));
        let mine = &stored["mine"];
        assert!(!mine.discovered, "a remembered project is the user's");
        assert_eq!(
            mine.remote.as_deref(),
            Some("git@example.com:me/found"),
            "the Git identity should carry over"
        );
    }

    // ------------------------------------------------------------- planning

    /// The planners are pure, so what a call would write can be asserted
    /// without applying it.
    #[test]
    fn planning_a_forget_is_one_delete_and_says_so() {
        let view = StoreView {
            projects: projects::Registry::from([(
                "work".to_owned(),
                projects::validate("work", "/tmp/work").unwrap(),
            )]),
            list: None,
        };
        let (mutations, reply) =
            plan_mutate(&view, "project_forget", &json!({ "name": "work" })).expect("planned");
        assert_eq!(mutations, vec![projects::delete_mutation("work")]);
        assert_eq!(reply, Reply::Fixed("forgot work".to_owned()));
    }

    #[test]
    fn a_rejected_plan_produces_no_mutations() {
        let view = StoreView::default();
        assert!(plan_mutate(&view, "project_forget", &json!({ "name": "gone" })).is_err());
        assert!(plan_mutate(&view, "todo_add", &json!({ "project": "gone" })).is_err());
    }

    /// The id of a new todo is the store's to allocate, so the planner says what
    /// it will say and the runner fills the id in.
    #[test]
    fn the_reply_for_a_new_todo_takes_the_id_from_the_write() {
        let reply = Reply::AddedTodo {
            project: Some("work".to_owned()),
            text: "ship it".to_owned(),
        };
        let report = ApplyReport {
            rows_written: 1,
            rows_deleted: 0,
            ids: vec![42],
        };
        assert_eq!(field(&reply.finish(&report), "id"), 42);
    }
}

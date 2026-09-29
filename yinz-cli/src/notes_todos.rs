//! Named shell access to the existing shared-store operations.
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{io::Read, path::PathBuf};
use yinz_types::input::actions::Action;

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
#[serde(tag = "op", content = "args", rename_all = "snake_case")]
pub enum NotesTodos {
    /// Rename a project, preserving linked records.
    ProjectRename { name: String, new_name: String },
    /// Merge duplicate project identities, preserving the target's settings.
    ProjectMerge { source: String, target: String },
    /// List agent run headers with pagination across the archive.
    AgentRuns {
        #[clap(long)]
        project: Option<String>,
        #[clap(long)]
        host_id: Option<String>,
        #[clap(long)]
        run_session: Option<String>,
        #[clap(long)]
        kind: Option<String>,
        #[clap(long)]
        since_ms: Option<i64>,
        #[clap(long)]
        active: Option<bool>,
        #[clap(long, default_value_t = 20)]
        limit: usize,
        #[clap(long, default_value_t = 0)]
        offset: usize,
    },
    /// Read an exact run and a bounded page of its timeline.
    AgentRun {
        run_id: String,
        #[clap(long, conflicts_with = "from_revision")]
        revision: Option<u64>,
        #[clap(long)]
        from_revision: Option<u64>,
        #[clap(long, default_value_t = 20)]
        limit: usize,
    },
    /// Search recorded agent messages and prompts.
    AgentHistorySearch {
        query: String,
        #[clap(long)]
        run_id: Option<String>,
        #[clap(long)]
        project: Option<String>,
        #[clap(long)]
        host_id: Option<String>,
        #[clap(long)]
        run_session: Option<String>,
        #[clap(long)]
        kind: Option<String>,
        #[clap(long)]
        since_ms: Option<i64>,
        #[clap(long)]
        active: Option<bool>,
        #[clap(long, default_value_t = 20)]
        limit: usize,
    },
    /// Follow recorded predecessor links from an exact run ID.
    AgentPredecessors { run_id: String },
    /// List remembered projects.
    ProjectList,
    /// Remember a project name and absolute root.
    ProjectRemember {
        name: String,
        #[clap(long)]
        root: String,
        #[clap(long)]
        worktree_dir: Option<String>,
        #[clap(long)]
        firecracker_image: Option<String>,
    },
    /// Forget a project name, preserving its notes and todos.
    ProjectForget { name: String },
    /// List a project’s todo tree.
    TodoList {
        #[clap(long)]
        project: String,
        #[clap(long)]
        include_done: bool,
    },
    /// Search todos across projects by default.
    TodoSearch {
        query: String,
        #[clap(long)]
        project: Option<String>,
    },
    /// Add a todo or subtask.
    TodoAdd {
        text: String,
        #[clap(long)]
        project: String,
        #[clap(long)]
        parent_id: Option<u64>,
    },
    /// Replace todo text.
    TodoEdit {
        id: u64,
        #[clap(long)]
        text: String,
        #[clap(long)]
        project: String,
        #[clap(long)]
        expect_rev: Option<i64>,
    },
    /// Complete a todo and its subtree, or reopen it.
    TodoDone {
        id: u64,
        #[clap(long)]
        project: String,
        #[clap(long)]
        undo: bool,
    },
    /// Delete a leaf, or a subtree with --recursive. Notes survive.
    TodoDelete {
        id: u64,
        #[clap(long)]
        project: String,
        #[clap(long)]
        recursive: bool,
    },
    /// Move a subtree within its project.
    TodoMove {
        id: u64,
        #[clap(long)]
        project: String,
        #[clap(
            long,
            conflicts_with = "top_level",
            required_unless_present = "top_level"
        )]
        parent_id: Option<u64>,
        #[clap(long, conflicts_with = "parent_id")]
        top_level: bool,
        #[clap(long)]
        expect_rev: Option<i64>,
    },
    /// List notes across all scopes by default.
    NoteList {
        #[clap(long, conflicts_with = "global")]
        project: Option<String>,
        #[clap(long)]
        global: bool,
        #[clap(long)]
        include_archived: bool,
        #[clap(long)]
        tag: Option<String>,
        #[clap(long)]
        todo_id: Option<u64>,
    },
    /// Search notes across all scopes by default.
    NoteSearch {
        query: String,
        #[clap(long, conflicts_with = "global")]
        project: Option<String>,
        #[clap(long)]
        global: bool,
    },
    /// Read a whole note, including its revision.
    NoteGet { id: u64 },
    /// Create a project or global note.
    NoteAdd {
        title: String,
        #[clap(long, conflicts_with = "global", required_unless_present = "global")]
        project: Option<String>,
        #[clap(long)]
        global: bool,
        #[clap(
            long,
            conflicts_with = "body_file",
            required_unless_present = "body_file"
        )]
        body: Option<String>,
        #[clap(long)]
        body_file: Option<PathBuf>,
        #[clap(long = "tag")]
        tags: Vec<String>,
        #[clap(long)]
        author: Option<String>,
        #[clap(long = "todo-id")]
        todo_ids: Vec<u64>,
    },
    /// Replace a note’s title, body, or tags.
    NoteEdit {
        id: u64,
        #[clap(long)]
        title: Option<String>,
        #[clap(long, conflicts_with = "body_file")]
        body: Option<String>,
        #[clap(long)]
        body_file: Option<PathBuf>,
        #[clap(long = "tag", conflicts_with = "clear_tags")]
        tags: Vec<String>,
        #[clap(long)]
        clear_tags: bool,
        #[clap(long)]
        expect_rev: Option<i64>,
    },
    /// Archive a note, or restore it with --undo.
    NoteArchive {
        id: u64,
        #[clap(long)]
        undo: bool,
    },
    /// Delete a note and its tags and links.
    NoteDelete { id: u64 },
    /// Link a note to a todo, or unlink it.
    NoteLink {
        note_id: u64,
        #[clap(long)]
        todo_id: u64,
        #[clap(long)]
        unlink: bool,
    },
}

impl NotesTodos {
    /// Read body input once, before connecting. Everything else remains store validation.
    pub fn request(&self, stdin: &mut impl Read) -> Result<Value, String> {
        let mut request = serde_json::to_value(self).map_err(|e| e.to_string())?;
        if request.get("args").is_none() {
            request["args"] = json!({});
        }
        let args = request["args"]
            .as_object_mut()
            .ok_or("invalid store arguments")?;
        if let Some(Value::String(path)) = args.remove("body_file") {
            let mut body = String::new();
            if path == "-" {
                stdin
                    .read_to_string(&mut body)
                    .map_err(|e| format!("cannot read note body from stdin: {e}"))?;
            } else {
                body = std::fs::read_to_string(&path)
                    .map_err(|e| format!("cannot read note body from {path}: {e}"))?;
            }
            args.insert("body".into(), json!(body));
        }
        let clear_tags = args.remove("clear_tags") == Some(json!(true));
        if !clear_tags && args.get("tags") == Some(&json!([])) {
            args.remove("tags");
        }
        args.remove("top_level");
        args.retain(|_, value| !value.is_null());
        request["structured_reply"] = json!(true);
        Ok(request)
    }

    pub fn action(&self, stdin: &mut impl Read) -> Result<Action, String> {
        let request = self.request(stdin)?;
        let query = matches!(
            self,
            Self::ProjectList
                | Self::AgentRuns { .. }
                | Self::AgentRun { .. }
                | Self::AgentHistorySearch { .. }
                | Self::AgentPredecessors { .. }
                | Self::TodoList { .. }
                | Self::TodoSearch { .. }
                | Self::NoteList { .. }
                | Self::NoteSearch { .. }
                | Self::NoteGet { .. }
        );
        Ok(pipe_action(
            if query { "store_query" } else { "store_mutate" },
            request,
        ))
    }
}

pub fn pipe_action(name: &str, request: Value) -> Action {
    Action::CliPipe {
        pipe_id: uuid::Uuid::new_v4().to_string(),
        name: Some(name.into()),
        payload: Some(request.to_string()),
        plugin: None,
        args: None,
        configuration: None,
        launch_new: false,
        skip_cache: false,
        floating: None,
        in_place: None,
        cwd: None,
        pane_title: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{CliAction, CliArgs, Command};
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<NotesTodos, clap::Error> {
        // The full debug clap builder already needs a larger stack (see args.rs tests).
        let argv: Vec<String> = ["yinz", "action"]
            .into_iter()
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect();
        let parsed = std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(move || CliArgs::try_parse_from(argv))
            .unwrap()
            .join()
            .unwrap()?;
        match parsed.command {
            Some(Command::Action(action)) => match *action {
                CliAction::NotesTodos(command) => Ok(command),
                other => panic!("unexpected action: {other:?}"),
            },
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn every_named_action_maps_to_the_existing_store_protocol() {
        let cases: &[(&[&str], &str, Value, bool)] = &[
            (&["project-list"], "project_list", json!({}), true),
            (
                &["project-remember", "work", "--root", "/work"],
                "project_remember",
                json!({"name":"work","root":"/work"}),
                false,
            ),
            (
                &["project-forget", "work"],
                "project_forget",
                json!({"name":"work"}),
                false,
            ),
            (
                &["todo-list", "--project", "work", "--include-done"],
                "todo_list",
                json!({"project":"work","include_done":true}),
                true,
            ),
            (
                &["todo-search", "ship"],
                "todo_search",
                json!({"query":"ship"}),
                true,
            ),
            (
                &["todo-add", "ship", "--project", "work", "--parent-id", "7"],
                "todo_add",
                json!({"text":"ship","project":"work","parent_id":7}),
                false,
            ),
            (
                &[
                    "todo-edit",
                    "7",
                    "--project",
                    "work",
                    "--text",
                    "ship",
                    "--expect-rev",
                    "2",
                ],
                "todo_edit",
                json!({"id":7,"text":"ship","project":"work","expect_rev":2}),
                false,
            ),
            (
                &["todo-done", "7", "--project", "work", "--undo"],
                "todo_done",
                json!({"id":7,"project":"work","undo":true}),
                false,
            ),
            (
                &["todo-delete", "7", "--project", "work", "--recursive"],
                "todo_delete",
                json!({"id":7,"project":"work","recursive":true}),
                false,
            ),
            (
                &[
                    "todo-move",
                    "7",
                    "--project",
                    "work",
                    "--top-level",
                    "--expect-rev",
                    "2",
                ],
                "todo_move",
                json!({"id":7,"project":"work","expect_rev":2}),
                false,
            ),
            (
                &[
                    "note-list",
                    "--global",
                    "--include-archived",
                    "--tag",
                    "design",
                    "--todo-id",
                    "7",
                ],
                "note_list",
                json!({"global":true,"include_archived":true,"tag":"design","todo_id":7}),
                true,
            ),
            (
                &["note-search", "why"],
                "note_search",
                json!({"query":"why","global":false}),
                true,
            ),
            (&["note-get", "4"], "note_get", json!({"id":4}), true),
            (
                &[
                    "note-add",
                    "why",
                    "--project",
                    "work",
                    "--body-file",
                    "-",
                    "--tag",
                    "a",
                    "--tag",
                    "b",
                    "--todo-id",
                    "7",
                    "--todo-id",
                    "8",
                    "--author",
                    "codex",
                ],
                "note_add",
                json!({"title":"why","project":"work","global":false,"body":"line one\n\nline two\n","tags":["a","b"],"todo_ids":[7,8],"author":"codex"}),
                false,
            ),
            (
                &["note-edit", "4", "--clear-tags", "--expect-rev", "2"],
                "note_edit",
                json!({"id":4,"tags":[],"expect_rev":2}),
                false,
            ),
            (
                &["note-archive", "4", "--undo"],
                "note_archive",
                json!({"id":4,"undo":true}),
                false,
            ),
            (&["note-delete", "4"], "note_delete", json!({"id":4}), false),
            (
                &["note-link", "4", "--todo-id", "7", "--unlink"],
                "note_link",
                json!({"note_id":4,"todo_id":7,"unlink":true}),
                false,
            ),
        ];
        for (argv, op, args, query) in cases {
            let command = parse(argv).unwrap();
            let action = command
                .action(&mut "line one\n\nline two\n".as_bytes())
                .unwrap();
            let Action::CliPipe {
                name,
                payload,
                plugin,
                launch_new,
                ..
            } = action
            else {
                panic!("not a pipe")
            };
            assert_eq!(
                name.as_deref(),
                Some(if *query {
                    "store_query"
                } else {
                    "store_mutate"
                }),
                "{argv:?}"
            );
            assert!(plugin.is_none() && !launch_new);
            let request: Value = serde_json::from_str(&payload.unwrap()).unwrap();
            assert_eq!(
                request,
                json!({"op":op,"args":args,"structured_reply":true}),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn invalid_combinations_and_missing_scope_are_rejected() {
        for argv in [
            vec!["todo-list"],
            vec!["todo-add", "x"],
            vec!["todo-edit", "1", "--text", "x"],
            vec!["todo-done", "1"],
            vec!["todo-delete", "1"],
            vec!["todo-move", "1", "--project", "p"],
            vec![
                "todo-move",
                "1",
                "--project",
                "p",
                "--top-level",
                "--parent-id",
                "2",
            ],
            vec!["note-add", "x", "--body", "b"],
            vec!["note-add", "x", "--global"],
            vec!["note-add", "x", "--global", "--project", "p", "--body", "b"],
            vec!["note-edit", "1", "--body", "b", "--body-file", "-"],
            vec!["note-edit", "1", "--tag", "x", "--clear-tags"],
            vec!["note-list", "--global", "--project", "p"],
            vec!["todo-add", "x", "--project", "p", "--parent-id", "wrong"],
        ] {
            assert!(parse(&argv).is_err(), "{argv:?}");
        }
        assert!(parse(&["todo-move", "1", "--project", "p", "--parent-id", "2"]).is_ok());
    }

    #[test]
    fn body_files_and_inline_bodies_preserve_lines_and_omitted_tags() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("body.md");
        std::fs::write(&path, "first\n\nsecond\n").unwrap();
        let request = parse(&["note-edit", "1", "--body-file", path.to_str().unwrap()])
            .unwrap()
            .request(&mut std::io::empty())
            .unwrap();
        assert_eq!(request["args"], json!({"id":1,"body":"first\n\nsecond\n"}));
        let request = parse(&["note-edit", "1", "--body", "a\nb", "--tag", "new"])
            .unwrap()
            .request(&mut std::io::empty())
            .unwrap();
        assert_eq!(
            request["args"],
            json!({"id":1,"body":"a\nb","tags":["new"]})
        );
        std::fs::remove_file(&path).unwrap();
        assert!(
            parse(&["note-edit", "1", "--body-file", path.to_str().unwrap()])
                .unwrap()
                .request(&mut std::io::empty())
                .is_err()
        );
    }
}

//! Native note work uses the session's store, away from screen and PTY threads.
use super::{agent_control, DataAccess};
use project_data::{
    notes,
    store::{self, Backend},
};
use std::{fs, path::Path, sync::atomic::Ordering};
use yinz_bus::{
    command_palette::{Instruction, NoteAction, NoteChoice, NoteRequest, NoteResults},
    screen::ScreenInstruction,
    thread_bus::ThreadSenders,
};
use yinz_runtime::errors::prelude::*;
use yinz_tile::prelude::WriteError;
use yinz_types::plugin_data::*;

struct Store<'a>(&'a DataAccess);

impl Backend for Store<'_> {
    fn define(&self, def: &EntityDef) -> Result<DataSchemaInfo, String> {
        self.0
            .plugin_data()
            .define_entity(store::SHARED, "native-note-palette", def, false)
            .map_err(|e| e.to_string())
    }
    fn fetch(&self, request: &FetchRequest) -> Result<DataRows, String> {
        self.0
            .plugin_data()
            .fetch(store::SHARED, request)
            .map_err(|e| e.to_string())
    }
    fn count(&self, request: &FetchRequest) -> Result<u64, String> {
        self.0
            .plugin_data()
            .count(store::SHARED, request)
            .map_err(|e| e.to_string())
    }
    fn apply(&self, mutations: Vec<Mutation>) -> Result<ApplyReport, WriteError> {
        self.0
            .plugin_data()
            .apply(store::SHARED, &mutations)
            .map(|outcome| outcome.report)
            .map_err(|e| match e {
                DataError::Conflict(message) => WriteError::Conflict(message),
                other => WriteError::Failed(other.to_string()),
            })
    }
    fn watch(&self, _: &FetchRequest, _: WatchMode) -> Result<DataWatchHandle, String> {
        Err("Native note search does not register watches".into())
    }
}

fn root(request: &NoteRequest, data: &DataAccess) -> Result<String, String> {
    let registry = project_data::projects::from_rows(&agent_control::projects(data)?);
    registry
        .get(&request.project)
        .map(|project| project.root.clone())
        .ok_or_else(|| "Project no longer exists; switch current projects".into())
}

fn search(backend: &impl Backend, root: &str, query: &str) -> Result<NoteResults, String> {
    let found = if query.trim().is_empty() {
        let rows = backend.fetch(
            &FetchRequest::entity("note")
                .filter(Pred::eq("root", root))
                .sort_by("updated_at", SortDir::Desc)
                .sort_by("id", SortDir::Desc)
                .limit(notes::MAX_MATCHES as u32),
        )?;
        notes::SearchResult {
            truncated: rows.truncated || rows.len() >= notes::MAX_MATCHES,
            notes: notes::from_rows(&rows),
        }
    } else {
        notes::search(backend, notes::Scope::Project(root), query)?
    };
    Ok(NoteResults {
        notes: found
            .notes
            .into_iter()
            .map(|note| NoteChoice {
                id: note.id,
                title: note.title,
                snippet: notes::snippet(&note.body),
                archived: note.archived,
            })
            .collect(),
        truncated: found.truncated,
    })
}

fn document(note: &notes::Note) -> String {
    format!("# {}\n\n{}\n", note.title, note.body)
}

fn parse(text: &str) -> Result<(String, String), String> {
    let text = text.replace("\r\n", "\n");
    let (heading, rest) = text
        .split_once('\n')
        .ok_or("Expected # Title followed by a blank line and Markdown body")?;
    let title = heading
        .strip_prefix("# ")
        .ok_or("The first line must be a # Title heading")?;
    let body = rest
        .strip_prefix('\n')
        .ok_or("Separate the title and body with a blank line")?;
    Ok((notes::clean_title(title)?, notes::clean_body(body)?))
}

fn finish(
    backend: &impl Backend,
    original: &notes::Note,
    path: &Path,
    exit: Result<(), String>,
) -> Result<(), String> {
    exit?;
    let text = fs::read_to_string(path).map_err(|e| format!("Cannot read edited note: {e}"))?;
    let (title, body) = parse(&text)?;
    if title == original.title && body == original.body {
        return Ok(());
    }
    // A deleted row must be an error, not a successful update affecting zero rows.
    notes::fetch_note(backend, original.id)?.ok_or("Note was deleted during editing")?;
    let (mutations, _) = notes::plan_edit(original, Some(&title), Some(&body), Some(original.rev))?;
    backend.apply(mutations).map_err(|e| -> String {
        if e.is_conflict() {
            "Note changed during editing; stored contents were preserved".into()
        } else {
            e.into()
        }
    })?;
    Ok(())
}

pub(crate) fn handle(request: NoteRequest, senders: &ThreadSenders, data: &DataAccess) {
    let backend = Store(data);
    let resolved = root(&request, data).and_then(|root| {
        backend.define_all(&[
            store::note_def(),
            store::note_tag_def(),
            store::note_link_def(),
        ])?;
        Ok(root)
    });
    let reply = match &request.action {
        NoteAction::Search(query) => Instruction::NoteSearch {
            client: request.client,
            token: request.token,
            revision: request.revision,
            reply: resolved.and_then(|root| search(&backend, &root, query)),
        },
        NoteAction::Open(id) => {
            let result = resolved.and_then(|root| {
                let original = notes::fetch_note(&backend, *id)?.ok_or("Note no longer exists")?;
                if original.root.as_deref() != Some(&root) {
                    return Err("Note no longer belongs to this project".into());
                }
                if !request.active.load(Ordering::Acquire) {
                    return Ok(());
                }
                let directory = tempfile::Builder::new()
                    .prefix("yinz-note-")
                    .tempdir()
                    .map_err(|e| format!("Cannot create note directory: {e}"))?;
                let path = directory.path().join("note.md");
                fs::write(&path, document(&original))
                    .map_err(|e| format!("Cannot write temporary note: {e}"))?;
                let (exit, receiver) = crossbeam::channel::bounded(1);
                senders
                    .send_to_screen(ScreenInstruction::CommandPalette(Instruction::NoteReady {
                        client: request.client,
                        token: request.token,
                        revision: request.revision,
                        path: path.clone(),
                        exit,
                    }))
                    .map_err(|e| format!("Cannot open note editor: {e}"))?;
                let exit = receiver
                    .recv()
                    .unwrap_or_else(|e| Err(format!("Note editor disconnected: {e}")));
                let editor_failed = exit.is_err();
                let result = finish(&backend, &original, &path, exit);
                if let Err(error) = result {
                    // Compare the actual file, not an open descriptor: editors may replace it.
                    let changed = fs::read_to_string(&path)
                        .map(|text| text != document(&original))
                        .unwrap_or(true);
                    if editor_failed || changed {
                        let _recovery_directory = directory.into_path();
                        return Err(format!(
                            "Recover edited contents: {}. {error}",
                            path.display()
                        ));
                    }
                    return Err(error);
                }
                Ok(())
            });
            Instruction::NoteFinished {
                client: request.client,
                token: request.token,
                revision: request.revision,
                result,
            }
        },
    };
    senders
        .send_to_screen(ScreenInstruction::CommandPalette(reply))
        .context("replying to native note palette")
        .non_fatal();
}

#[cfg(test)]
#[path = "unit/palette_notes.rs"]
mod tests;

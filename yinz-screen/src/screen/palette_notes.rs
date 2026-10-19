use super::Screen;
use crate::ui::command_palette::{Entry, EntryKind, Outcome, PluginSection};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use yinz_bus::{
    command_palette::{Instruction, NoteAction, NoteEditorCompletion, NoteRequest, NoteResults},
    plugins::PluginInstruction,
    pty::PtyInstruction,
    screen::ScreenInstruction,
    ServerInstruction,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::ClientId;

pub(super) const VIEW: u64 = 1 << 62;
pub(super) fn owns_view(view: u64) -> bool {
    (VIEW..=VIEW + 1).contains(&view)
}
static TOKEN: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
pub(super) struct State {
    token: u64,
    revision: u64,
    project: String,
    active: Option<Arc<AtomicBool>>,
    opening_editor: bool,
    recovery: Option<String>,
}

impl State {
    pub(super) fn cancel(&mut self) {
        if let Some(active) = self.active.take() {
            active.store(false, Ordering::Release);
        }
        self.opening_editor = false;
    }
    fn accepts(&self, token: u64, revision: u64) -> bool {
        self.token == token
            && self.revision == revision
            && self
                .active
                .as_ref()
                .is_some_and(|active| active.load(Ordering::Acquire))
    }
}

impl Screen {
    pub(super) fn open_note_search(&mut self, client: ClientId) {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        let Some(project) = state.context.current_project.clone() else {
            state
                .widget
                .set_error("No current project; use Switch current project… first".into());
            return;
        };
        state.cancel_requests(&self.bus.senders, client);
        state.agents.cancel();
        state.notes.cancel();
        state.notes = State {
            token: TOKEN.fetch_add(1, Ordering::Relaxed),
            project: project.clone(),
            ..Default::default()
        };
        state
            .widget
            .push_view(VIEW, format!("{project} · Search project notes"));
        state.widget.set_sections(vec![PluginSection {
            title: "Project notes".into(),
            semantic: true,
            status: Some("Loading…".into()),
            entries: vec![],
        }]);
        self.refresh_note_search(client, false);
    }

    pub(super) fn refresh_note_search(&mut self, client: ClientId, debounce: bool) -> bool {
        if self
            .command_palettes
            .get(&client)
            .is_some_and(|state| state.widget.view == VIEW + 1)
        {
            return true;
        }
        let Some(state) = self
            .command_palettes
            .get_mut(&client)
            .filter(|state| state.widget.view == VIEW)
        else {
            return false;
        };
        if state.notes.opening_editor {
            return true;
        }
        state.notes.cancel();
        state.notes.revision += 1;
        state.notes.active = Some(Arc::new(AtomicBool::new(true)));
        state.widget.pending = true;
        state.widget.set_section_status(Some("Loading…".into()));
        let token = state.notes.token;
        let revision = state.notes.revision;
        if debounce {
            let senders = self.bus.senders.clone();
            crate::global_async_runtime::get_tokio_runtime().spawn(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                senders
                    .send_to_screen(ScreenInstruction::CommandPalette(
                        Instruction::NoteDiscover {
                            client,
                            token,
                            revision,
                        },
                    ))
                    .context("debouncing note search")
                    .non_fatal();
            });
        } else {
            self.discover_note_search(client, token, revision);
        }
        true
    }

    pub(super) fn discover_note_search(&mut self, client: ClientId, token: u64, revision: u64) {
        let Some(state) = self
            .command_palettes
            .get(&client)
            .filter(|state| state.widget.view == VIEW && state.notes.accepts(token, revision))
        else {
            return;
        };
        self.send_note_request(client, NoteAction::Search(state.widget.query().into()));
    }

    fn send_note_request(&mut self, client: ClientId, action: NoteAction) {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        let Some(active) = state.notes.active.clone() else {
            return;
        };
        let request = NoteRequest {
            client,
            token: state.notes.token,
            revision: state.notes.revision,
            project: state.notes.project.clone(),
            action,
            active,
        };
        if let Err(error) = self
            .bus
            .senders
            .send_to_plugin(PluginInstruction::PaletteNote(Box::new(request)))
        {
            if state.notes.opening_editor {
                super::notify(&self.bus.senders, format!("Could not edit note: {error}"));
            }
            state.notes.cancel();
            state.widget.pending = false;
            state.widget.busy = false;
            state
                .widget
                .set_error(format!("Cannot request note operation: {error}"));
        }
    }

    pub(super) fn note_search_reply(
        &mut self,
        client: ClientId,
        token: u64,
        revision: u64,
        reply: Result<NoteResults, String>,
    ) {
        let Some(state) = self.command_palettes.get_mut(&client).filter(|state| {
            state.widget.view == VIEW
                && !state.notes.opening_editor
                && state.notes.accepts(token, revision)
        }) else {
            return;
        };
        state.widget.pending = false;
        let (results, search_error) = match reply {
            Ok(results) => (results, None),
            Err(error) => (
                NoteResults {
                    notes: vec![],
                    truncated: false,
                },
                Some(error),
            ),
        };
        let status = search_error.clone().or_else(|| {
            if results.truncated {
                Some("Showing up to 50 notes; more results may exist".into())
            } else if results.notes.is_empty() {
                Some("No project notes found".into())
            } else {
                None
            }
        });
        let mut entries: Vec<_> = results
            .notes
            .into_iter()
            .map(|note| Entry {
                id: format!("native-note:{}", note.id),
                label: format!(
                    "#{} {}{}",
                    note.id,
                    note.title,
                    if note.archived { " [archived]" } else { "" }
                ),
                description: note.snippet,
                shortcut: String::new(),
                aliases: vec![],
                kind: EntryKind::Plugin,
            })
            .collect();
        state.widget.clear_error();
        if let Some(error) = state.notes.recovery.clone() {
            entries.push(Entry {
                id: "native-note-recovery".into(),
                label: "Note editor details…".into(),
                description: error.clone(),
                shortcut: String::new(),
                aliases: vec![],
                kind: EntryKind::Plugin,
            });
            state.widget.set_error(error);
        } else if let Some(error) = search_error {
            state.widget.set_error(error);
        }
        state.widget.set_sections(vec![PluginSection {
            title: "Project notes".into(),
            semantic: true,
            status,
            entries,
        }]);
    }

    pub(super) fn note_palette_outcome(&mut self, client: ClientId, outcome: &Outcome) -> bool {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return false;
        };
        if *outcome == Outcome::Cancel && state.widget.view == VIEW {
            state.notes.cancel();
            state.widget.busy = false;
            state.widget.pending = false;
            state.widget.back();
            self.refresh_plugin_palette(client, false);
            return true;
        }
        let Outcome::Activate(id) = outcome else {
            return false;
        };
        if state.widget.view != VIEW {
            return false;
        }
        if id == "native-note-recovery" {
            if let Some(error) = state.notes.recovery.clone() {
                state
                    .widget
                    .push_view(VIEW + 1, "Note editor details".into());
                state.widget.open_text(error);
            }
            return true;
        }
        let Some(id) = id
            .strip_prefix("native-note:")
            .and_then(|id| id.parse().ok())
        else {
            return false;
        };
        // Old rows remain visible during debounce, but cannot open under a new query.
        if state.widget.pending {
            return true;
        }
        state.notes.cancel();
        state.notes.revision += 1;
        state.notes.active = Some(Arc::new(AtomicBool::new(true)));
        state.notes.opening_editor = true;
        state.widget.busy = true;
        state.widget.clear_error();
        self.send_note_request(client, NoteAction::Open(id));
        true
    }

    pub(super) fn note_editor_ready(
        &mut self,
        client: ClientId,
        token: u64,
        revision: u64,
        path: PathBuf,
        pane_title: String,
        exit: NoteEditorCompletion,
    ) {
        let valid = self.command_palettes.get(&client).is_some_and(|state| {
            state.widget.view == VIEW
                && state.notes.opening_editor
                && state.notes.accepts(token, revision)
                && self.active_tab_ids.get(&client) == Some(&state.context.tab_id)
                && self.tabs.contains_key(&state.context.tab_id)
        });
        if !valid {
            let _ = exit.send(Err("Note editor request was cancelled".into()));
            return;
        }
        let Some(state) = self.take_command_palette(client, false) else {
            return;
        };
        let tab = state.context.tab_id;
        self.note_edit_palettes.insert(token, (client, state));
        if let Err(error) = self
            .bus
            .senders
            .send_to_pty(PtyInstruction::OpenNoteEditor {
                path,
                pane_title,
                client,
                tab,
                exit: exit.clone(),
            })
        {
            let _ = exit.send(Err(format!("Cannot open note editor: {error}")));
        }
    }

    pub(super) fn note_editor_finished(
        &mut self,
        client: ClientId,
        token: u64,
        revision: u64,
        result: Result<bool, String>,
    ) {
        if self
            .note_edit_palettes
            .get(&token)
            .is_some_and(|(origin, state)| *origin != client || state.notes.revision != revision)
        {
            return;
        }
        let accepted = self.note_edit_palettes.contains_key(&token)
            || self
                .command_palettes
                .get(&client)
                .is_some_and(|state| state.notes.accepts(token, revision));
        if accepted {
            super::notify(
                &self.bus.senders,
                match &result {
                    Ok(true) => "Completed: Save note".into(),
                    Ok(false) => "Note unchanged".into(),
                    Err(error) => format!("Could not edit note: {error}"),
                },
            );
        }
        if let Some((_, mut state)) = self.note_edit_palettes.remove(&token) {
            if self.active_tab_ids.get(&client) != Some(&state.context.tab_id)
                || !self.tabs.contains_key(&state.context.tab_id)
                || self.command_palettes.contains_key(&client)
            {
                self.dismiss_plugin_palette(client, &mut state);
                self.release_palette_input(client);
                if let Err(error) = result {
                    log::error!("{error}");
                }
                return;
            }
            if result == Ok(true) {
                state.widget.clear_searches();
            }
            state.notes.recovery = result.err();
            state.widget.busy = false;
            self.command_palettes.insert(client, state);
            self.bus
                .senders
                .send_to_server(ServerInstruction::SetCommandPaletteOpen(client, true))
                .context("restoring note search input")
                .non_fatal();
            if self.command_palettes.len() == 1 {
                self.bus
                    .senders
                    .send_to_background_jobs(
                        yinz_bus::background_jobs::BackgroundJob::WatchCommandPaletteSessions(true),
                    )
                    .context("restoring palette session watch")
                    .non_fatal();
            }
            self.refresh_note_search(client, false);
        } else if let Some(state) = self
            .command_palettes
            .get_mut(&client)
            .filter(|state| state.notes.accepts(token, revision))
        {
            state.notes.cancel();
            state.widget.busy = false;
            if let Err(error) = result {
                state.widget.set_error(error);
            }
        } else if let Err(error) = result {
            log::error!("{error}");
        }
    }
}

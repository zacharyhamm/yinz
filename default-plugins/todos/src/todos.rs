use super::*;
use agent_proto::location::Location;
use agent_proto::store::{self, Backend, Host};
use agent_proto::{projects, todos};
use std::collections::HashMap;
use std::ops::Range;
use yinz_tile::prelude::{
    DataRevision, DataRows, DataWatchId, FetchRequest, Mutation, Pred, SortDir, WatchMode,
};
use yinz_tile::ui_components::forms::{self, TextInput};

#[derive(Default)]
pub(super) struct View {
    initial_cwd: String,
    projects: projects::Registry,
    project_root: Option<String>,
    project_watch: Option<DataWatchId>,
    todo_watch: Option<DataWatchId>,
    list: todos::List,
    selected: Option<u64>,
    show_done: bool,
    /// Visible to main.rs, which asks whether a box is being typed in (`Mode::Input`), where
    /// `?` is a character rather than the key sheet.
    pub(super) mode: Mode,
    /// Something wrong with the list itself - the store would not declare, watch or fetch it -
    /// that stays until a refresh gets past it.
    status: String,
    /// What the last action did, or why it did not: until the next key, rather than until the
    /// next change to the list, which for this pane's own write is the echo of that write a
    /// moment later.
    note: Option<Note>,
    /// The boxes put away with text in them, by Esc or by the panel going away, for the key that
    /// opened each to bring back: one for each entry a box was about - a new todo, a subtask
    /// under an item, an edit of an item. One slot for all of them used to let the second box
    /// put away drop the first one's text without a word.
    drafts: Vec<(InputAction, TextInput)>,
    /// The drafts of the projects not on screen, by root: each one waits for its own list, the
    /// only one whose items it can be about, rather than being dropped by a look at another.
    parked_drafts: HashMap<String, Vec<(InputAction, TextInput)>>,
    list_scroll: Option<usize>,
    last_window: usize,
    last_list_rows: usize,
    /// The rows above the list: the project it belongs to, or none on a pane too short for it.
    last_header_rows: usize,
    rendered_ids: Vec<u64>,
    rendered_projects: Vec<String>,
    /// The visible items wrapped and styled for a width. Parsing markdown is the expensive
    /// part of a render and the pane redraws on a timer, so this lives until the rows or the
    /// width change; the cursor is painted on top and never invalidates it.
    line_cache: Option<(usize, Vec<TodoLine>)>,
    /// Whether the refusal armed now has been drawn with its `Enter overwrites`. A pane too
    /// short for the refusal's rows cuts it from the tail, and an Enter the user was never
    /// told would overwrite somebody else's words must not: until it has been on screen, Enter
    /// in that box does nothing.
    overwrite_shown: bool,
}

#[derive(Default)]
pub(super) enum Mode {
    #[default]
    Normal,
    Projects {
        selected: usize,
    },
    Input {
        action: InputAction,
        input: TextInput,
        /// Why the last Enter did not save, drawn over the box until the next one. The box
        /// stays open with its text: a refusal is something to fix, not a reason to retype.
        refusal: Option<String>,
    },
    Delete {
        id: u64,
        plan: Vec<Mutation>,
        descendants: usize,
    },
    /// Completing `id` would cross off `cascaded` open items under it as well, which one key
    /// should not do unasked: reopening afterwards brings back only the item itself.
    Complete {
        id: u64,
        plan: Vec<Mutation>,
        cascaded: usize,
    },
    Move {
        id: u64,
        rev: DataRevision,
        target: Option<u64>,
    },
}

#[derive(Clone)]
pub(super) enum InputAction {
    Add {
        parent_id: Option<u64>,
    },
    Edit {
        id: u64,
        /// The revision the text in the box was composed from, which the write is guarded on.
        rev: DataRevision,
        /// The item's text at `rev`, which the box opened with: a box still holding it is no
        /// draft of anything, even once the item has been reworded elsewhere since.
        original: String,
        /// Somebody else's revision, once a guarded write has been refused and the user has
        /// been shown their text: the next Enter overwrites it, because they said so.
        theirs: Option<DataRevision>,
    },
}

impl InputAction {
    /// Whether a draft kept for `self` belongs in a box opened as `other`: the same kind of
    /// entry about the same item.
    fn same_target(&self, other: &InputAction) -> bool {
        match (self, other) {
            (InputAction::Add { parent_id: a }, InputAction::Add { parent_id: b }) => a == b,
            (InputAction::Edit { id: a, .. }, InputAction::Edit { id: b, .. }) => a == b,
            _ => false,
        }
    }

    /// As a draft keeps it: an overwrite is armed by what was on screen when it was armed, so
    /// it does not outlive the box.
    fn into_draft(self) -> Self {
        match self {
            InputAction::Edit {
                id, rev, original, ..
            } => InputAction::Edit {
                id,
                rev,
                original,
                theirs: None,
            },
            add => add,
        }
    }
}

/// What an action did or why it did not, as the divider says it until the next key.
struct Note {
    lead: String,
    /// The item it was about, quoted after `lead` and cut to what the row has room for.
    item: Option<String>,
    /// How many subtasks went with the item, counted after it.
    subtasks: usize,
    /// What to do about it, drawn after the note as a hint, and the first thing a narrow row
    /// gives up.
    hint: String,
    error: bool,
}

impl Note {
    fn error(text: impl Into<String>) -> Self {
        Note {
            lead: text.into(),
            item: None,
            subtasks: 0,
            hint: String::new(),
            error: true,
        }
    }

    fn then(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    /// `verb "item" +N subtasks`: names the item, since the list moving is not always
    /// visible - an item completed with done items hidden just vanishes.
    fn done(verb: &str, item: &str, subtasks: usize) -> Self {
        Note {
            lead: verb.to_owned(),
            item: Some(item.to_owned()),
            subtasks,
            hint: String::new(),
            error: false,
        }
    }

    /// The note in `room` columns, the item's excerpt taking what the rest leaves.
    ///
    /// Whole at every width a panel is drawn at, rather than cut from the tail: an excerpt
    /// too short to name anything is left out, and then the count is shortened, before the row
    /// would have to cut the note mid-word - `deleted "Old ide…" +` says neither what went nor
    /// how much.
    fn text(&self, room: usize) -> String {
        let Some(item) = &self.item else {
            return self.lead.clone();
        };
        let tail = match self.subtasks {
            0 => String::new(),
            count => format!(" +{}", subtasks(count)),
        };
        let width = room
            .saturating_sub(self.lead.width() + tail.width() + 3)
            .min(32);
        if width >= 6 {
            return format!("{} \"{}\"{tail}", self.lead, excerpt(item, width));
        }
        let short = format!("{}{tail}", self.lead);
        match (self.subtasks, short.width() <= room) {
            (0, _) | (_, true) => short,
            (count, false) => format!("{} +{count}", self.lead),
        }
    }
}

/// What a write came to.
enum Applied {
    /// Written, with the ids of the rows it created.
    Saved(Vec<u64>),
    /// Nothing written: the planner or the store said no.
    Refused(String),
    /// Nothing written because somebody else wrote first; the list on screen is theirs when
    /// the reload worked.
    Conflict { reloaded: Result<(), String> },
}

/// A row drawn dim but for the part that is not a hint - a note, a question, the text being
/// typed, the project's name - which is drawn at full weight, or in the error colour. Dim
/// overrides a colour on the same cell, so that part is left out of the dim range rather than
/// coloured on top of it.
struct TodoRow {
    line: String,
    bright: Option<Range<usize>>,
    error: bool,
}

/// What the panel says of a todo that went while the user was acting on it - in its own words,
/// rather than the planner's `no todo item N in this project; call todo_list`, which is the
/// model's.
const GONE: &str = "deleted elsewhere";

/// The part of an edit conflict's refusal that says what the next Enter does. Drawn or not, it
/// decides whether that Enter may overwrite: see `View::overwrite_shown`.
const OVERWRITES: &str = "Enter overwrites";

/// A move's ways out, as its divider draws them: what the scroll readout must leave room for.
const MOVE_KEYS: &str = "Enter nest · t top · Esc cancel";

#[derive(Clone)]
struct Row {
    id: u64,
    depth: usize,
    text: String,
    done: bool,
}

/// One screen row of a wrapped item: the checkbox gutter and markdown body, without the
/// cursor column, which is painted per frame.
#[derive(Clone)]
struct TodoLine {
    id: u64,
    line: markdown::MdLine,
    first: bool,
    done: bool,
}

impl View {
    pub(super) fn clear_dimensions(&mut self) {
        self.last_window = 0;
        self.last_list_rows = 0;
        self.last_header_rows = 0;
        self.rendered_ids.clear();
        self.rendered_projects.clear();
        self.line_cache = None;
        self.overwrite_shown = false;
    }
}

impl State {
    pub(super) fn open_todos(&mut self, initial_cwd: String) {
        self.todos.initial_cwd = initial_cwd;
        self.watch_todo_projects_with(&Host::shared());
    }

    /// Read the project registry, and keep it current: what the panel does on opening, and
    /// what `Ctrl-r` does again while that has not yet worked - reloading a list when there is
    /// no project to have one would only clear the error that says why.
    fn watch_todo_projects_with(&mut self, backend: &impl Backend) {
        if let Err(error) = backend.define_all(&store::shared_defs()) {
            self.todos.status = error;
            return;
        }
        match backend.watch(
            &FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT),
            WatchMode::Rows,
        ) {
            Ok(handle) => {
                self.todos.status.clear();
                self.todos.project_watch = Some(handle.watch_id);
                self.take_todo_projects(backend, projects::from_rows(&handle.rows));
            },
            Err(error) => self.todos.status = error,
        }
    }

    pub(super) fn todo_rows_changed(&mut self, watch_id: DataWatchId, rows: DataRows) -> bool {
        if self.todos.project_watch == Some(watch_id) {
            self.take_todo_projects(&Host::shared(), projects::from_rows(&rows));
            return true;
        }
        if self.todos.todo_watch != Some(watch_id) {
            return false;
        }
        self.take_todo_rows(&Host::shared(), &rows);
        true
    }

    /// The project to show once the registry changes: the one on screen while it is still
    /// remembered, else the one the panel was opened in.
    ///
    /// Else none. It used to fall back to whichever project's name sorts first, which filed
    /// todos into a project the user never chose, named only by the first word of a hint
    /// line; the empty list says instead that there is a choice to make, and `p` makes it.
    fn take_todo_projects(&mut self, backend: &impl Backend, registry: projects::Registry) {
        let previous = self.todos.project_root.clone();
        let root = previous
            .filter(|root| registry.values().any(|project| &project.root == root))
            .or_else(|| {
                projects::for_cwd(&registry, &Location::local(&self.todos.initial_cwd))
                    .map(|project| project.root.clone())
            });
        self.todos.projects = registry;
        if self.todos.project_root != root {
            self.watch_todo_root_with(backend, root);
        }
    }

    /// Show `root`'s list, and keep it current.
    ///
    /// The same root again is a refresh - `Ctrl-r`, or picking the project already open - and
    /// keeps what is on screen: the rows stay until fresh ones replace them, so the cursor
    /// stays on its item rather than going back to the top, and a refresh that fails leaves
    /// the list it could not improve on instead of a blank one. Another root is another list,
    /// and nothing of this one - its rows, its cursor - means anything there. Its draft is put
    /// away with it rather than dropped, and comes back with it: the text is the user's, and a
    /// look at another project is no reason to lose it. One whose item went meanwhile comes
    /// back as a new todo, as `check_todo_mode` makes of it once the rows are in; an edit
    /// keeps the revision it was written against, so a change made since still stops it.
    fn watch_todo_root_with(&mut self, backend: &impl Backend, root: Option<String>) {
        let same_root = root == self.todos.project_root;
        // A box open when the registry moves the list away is put away as Esc would put it,
        // with the rest of its project.
        if matches!(self.todos.mode, Mode::Input { .. }) {
            self.close_todo_input();
        }
        self.todos.line_cache = None;
        self.todos.mode = Mode::Normal;
        self.todos.list_scroll = None;
        self.todos.note = None;
        if !same_root {
            if let Some(watch_id) = self.todos.todo_watch.take() {
                unwatch(watch_id);
            }
            let drafts = std::mem::take(&mut self.todos.drafts);
            if let Some(old) = self
                .todos
                .project_root
                .take()
                .filter(|_| !drafts.is_empty())
            {
                self.todos.parked_drafts.insert(old, drafts);
            }
            self.todos.project_root = root.clone();
            self.todos.drafts = root
                .as_ref()
                .and_then(|root| self.todos.parked_drafts.remove(root))
                .unwrap_or_default();
            self.todos.list = Default::default();
            self.todos.selected = None;
        }
        let Some(root) = root else {
            // What to do about having no project is the empty list's to say, and it says it
            // whether or not this ever ran: the panel can open with none.
            self.todos.status.clear();
            return;
        };
        match backend.watch(
            &FetchRequest::entity("todo")
                .filter(Pred::eq("root", root.as_str()))
                .sort_by("id", SortDir::Asc)
                .limit(store::LIST_FETCH_LIMIT),
            WatchMode::Rows,
        ) {
            Ok(handle) => {
                // The old watch goes only once the new one is in place, so a refresh that
                // fails leaves the list on screen still watched.
                let old = self.todos.todo_watch.replace(handle.watch_id);
                if let Some(old) = old.filter(|old| *old != handle.watch_id) {
                    unwatch(old);
                }
                self.todos.status.clear();
                self.take_todo_rows(backend, &handle.rows);
            },
            Err(error) => self.todos.status = error,
        }
    }

    fn take_todo_rows(&mut self, backend: &impl Backend, rows: &DataRows) {
        let root = self.todos.project_root.as_deref().unwrap_or_default();
        let before = todo_rows(&self.todos.list, self.todos.show_done);
        let list = if rows.truncated {
            todos::fetch_list(backend, root)
        } else {
            Ok(todos::from_rows(rows).remove(root).unwrap_or_default())
        };
        match list {
            Ok(list) => self.todos.list = list,
            Err(error) => {
                self.todos.status = error;
                return;
            },
        }
        self.todos.line_cache = None;
        let visible = todo_rows(&self.todos.list, self.todos.show_done);
        self.todos.selected = nearest_surviving(&before, &visible, self.todos.selected);
        self.todos.list_scroll = None;
        // A fresh list is past whatever stopped the last one. A note is not the list's to
        // clear: this is also how this pane's own write comes back to it, a moment after the
        // note saying what the write did.
        self.todos.status.clear();
        self.check_todo_mode();
    }

    /// Re-read the list after a write, from the store rather than from the watch: a write
    /// beyond the watch's first page may not produce a notification, and a refused one
    /// produces none at all.
    fn reload_todos(&mut self, backend: &impl Backend) -> Result<(), String> {
        let root = self.todos.project_root.as_deref().unwrap_or_default();
        let rows = store::fetch_all(
            backend,
            FetchRequest::entity("todo").filter(Pred::eq("root", root)),
        )?;
        self.take_todo_rows(backend, &rows);
        Ok(())
    }

    /// The mode, once the list under it has changed.
    ///
    /// A question or a move about an item that has gone is dropped, saying why - its answer
    /// would only be refused, in words written for the model (`call todo_list`). A box about
    /// one keeps its text, since the text is the user's, and becomes a new top-level todo
    /// instead, which the box says before Enter makes it one - unless it is an edit nobody has
    /// typed in, whose text was only the item's, which closes like the question does. A draft
    /// put away about one is kept the same way.
    fn check_todo_mode(&mut self) {
        let gone = |state: &Self, id: u64| todos::item_of(&state.todos.list, id).is_none();
        match self.todos.mode {
            Mode::Delete { id, .. } | Mode::Complete { id, .. } | Mode::Move { id, .. }
                if gone(self, id) =>
            {
                self.todos.mode = Mode::Normal;
                self.todos.note = Some(Note::error(GONE));
            },
            Mode::Move { id, target, .. } => {
                let targets = self.move_targets(id);
                if target.is_some_and(|target| !targets.contains(&target)) {
                    if let Mode::Move { target, .. } = &mut self.todos.mode {
                        *target = targets.first().copied();
                    }
                }
            },
            _ => {},
        }
        let orphaned = |state: &Self, action: &InputAction| match action {
            InputAction::Edit { id, .. } if gone(state, *id) => {
                Some("deleted elsewhere · Enter adds it as new")
            },
            InputAction::Add {
                parent_id: Some(parent),
            } if gone(state, *parent) => {
                Some("its parent was deleted elsewhere · Enter adds it at the top")
            },
            _ => None,
        };
        let why = match &self.todos.mode {
            Mode::Input { action, .. } => orphaned(self, action),
            _ => None,
        };
        // An edit box nobody has typed in holds only the item's own text, which is no draft of
        // anything: with the item gone it closes, saying so. Made a new todo instead, Esc or a
        // hide kept it as the user's words - in place of a draft they had really typed.
        let untouched = matches!(
            &self.todos.mode,
            Mode::Input {
                action: InputAction::Edit { original, .. },
                input,
                ..
            } if input.get_text() == original
        );
        if why.is_some() && untouched {
            self.todos.mode = Mode::Normal;
            self.todos.note = Some(Note::error(GONE));
        } else if let (
            Some(why),
            Mode::Input {
                action, refusal, ..
            },
        ) = (why, &mut self.todos.mode)
        {
            *action = InputAction::Add { parent_id: None };
            *refusal = Some(why.to_owned());
        }
        let orphans: Vec<usize> = self
            .todos
            .drafts
            .iter()
            .enumerate()
            .filter(|(_, (action, _))| orphaned(self, action).is_some())
            .map(|(index, _)| index)
            .collect();
        for index in orphans {
            self.todos.drafts[index].0 = InputAction::Add { parent_id: None };
        }
    }

    fn todo_project_name(&self) -> &str {
        let root = self.todos.project_root.as_deref();
        self.todos
            .projects
            .values()
            .find(|project| Some(project.root.as_str()) == root)
            .map(|project| project.name.as_str())
            .unwrap_or("todos")
    }

    pub(super) fn todo_key(&mut self, key: KeyWithModifier) -> bool {
        self.todo_key_with(&Host::shared(), key)
    }

    /// `todo_key` against `backend`, which is the host in a session and a real store in a
    /// test.
    fn todo_key_with(&mut self, backend: &impl Backend, key: KeyWithModifier) -> bool {
        match &self.todos.mode {
            Mode::Input { .. } => return self.todo_input_key(backend, key),
            Mode::Delete { .. } | Mode::Complete { .. } => {
                return self.todo_confirm_key(backend, key)
            },
            Mode::Move { .. } => return self.todo_move_key(backend, key),
            Mode::Projects { .. } => return self.todo_project_key(backend, key),
            Mode::Normal => {},
        }
        // The note answered the key before this one; this key is the user moving on.
        let noted = self.todos.note.take().is_some();
        self.todo_normal_key(backend, key) || noted
    }

    /// Tab and Shift+Tab, which walk the panel's ring from every mode the tab has: to the
    /// conversation, or up to the tab bar. Whatever the mode was doing waits for the keys to
    /// come back - a box its text, a move or a pick its choice - as the conversation's box
    /// does; only a question, which answers one key, is dropped first (`todo_confirm_key`).
    fn todo_ring_key(&mut self, key: &KeyWithModifier) -> bool {
        if key.bare_key != BareKey::Tab {
            return false;
        }
        if key.has_modifiers(&[KeyModifier::Shift]) {
            self.tabs_focused = true;
        } else {
            self.focus_panel_bar(wire::STOP_TABS);
        }
        true
    }

    fn todo_normal_key(&mut self, backend: &impl Backend, key: KeyWithModifier) -> bool {
        if self.todo_ring_key(&key) {
            return true;
        }
        // Ctrl-o never gets this far: `on_key` takes it ahead of every surface.
        if key.has_modifiers(&[KeyModifier::Ctrl]) {
            match key.bare_key {
                // With no registry there is no list to reload, and the error on screen is the
                // registry's: that is what to try again.
                BareKey::Char('r') => match (&self.todos.project_watch, &self.todos.project_root) {
                    (None, None) => self.watch_todo_projects_with(backend),
                    _ => self.watch_todo_root_with(backend, self.todos.project_root.clone()),
                },
                _ => return false,
            }
            return true;
        }
        if key.bare_key == BareKey::Char('A')
            && (key.has_no_modifiers() || key.has_modifiers(&[KeyModifier::Shift]))
        {
            let Some(parent_id) = self.selected_todo().map(|item| item.id) else {
                return false;
            };
            return self.open_todo_add(Some(parent_id));
        }
        if !key.has_no_modifiers() {
            return false;
        }
        match key.bare_key {
            BareKey::Esc => {
                self.hide_the_panel();
                false
            },
            BareKey::Up | BareKey::Char('k') => self.step_todo(-1),
            BareKey::Down | BareKey::Char('j') => self.step_todo(1),
            BareKey::Char('p') => {
                let selected = self
                    .todo_projects()
                    .iter()
                    .position(|root| Some(root.as_str()) == self.todos.project_root.as_deref())
                    .unwrap_or(0);
                self.todos.mode = Mode::Projects { selected };
                self.todos.list_scroll = None;
                true
            },
            BareKey::Char('v') => {
                self.todos.show_done = !self.todos.show_done;
                self.take_todo_rows_from_list();
                true
            },
            BareKey::Char('a') => self.open_todo_add(None),
            // Enter opens the item, as it opens what is selected on every other tab - into
            // the box, since here opening an item is editing it. Completing is Space's alone:
            // it used to be Enter's too, so the key that means "open" on the tab next door
            // crossed off an item and everything under it.
            BareKey::Char('e') | BareKey::Enter => {
                let Some(item) = self.selected_todo().cloned() else {
                    return false;
                };
                self.begin_todo_input(
                    InputAction::Edit {
                        id: item.id,
                        rev: item.rev,
                        original: item.text.clone(),
                        theirs: None,
                    },
                    &item.text,
                )
            },
            BareKey::Char(' ') => self.toggle_todo(backend),
            BareKey::Char('n') => {
                let Some(configuration) = self.todo_agent_configuration() else {
                    return false;
                };
                // A fresh modal keeps each launch's project and prompt, even if another
                // agent form is already open.
                //
                // The panel stays up under it. A modal that closes hands the keyboard back to
                // the pane it took it from, so a launcher put away with Esc lands back on this
                // list; one that starts an agent points that at the new pane first. Hiding the
                // panel before opening it, as `activate` does before a focus, would turn the
                // Esc into a trip to whatever is under the panel.
                load_new_plugin("new-agent", configuration, false, false);
                true
            },
            BareKey::Char('d') => self.arm_todo_delete(),
            BareKey::Char('m') => self.arm_todo_move(),
            _ => false,
        }
    }

    /// `a`, or `A` under `parent_id`: the box, unless it is already clear that Enter would
    /// only be refused - there is no project to add to, the project is full, or the parent is
    /// as deep as items go - which is said now rather than after the typing.
    ///
    /// Unless there is a draft for it: then there is typing already, which the user can see
    /// only in the box, and saying no before it opens would leave it where no key reaches it.
    /// So the box opens with it, with the limit as its refusal - an Enter there is refused as
    /// it would have been, and Esc puts the text back - for the user to copy out, or to keep
    /// until the limit moves.
    fn open_todo_add(&mut self, parent_id: Option<u64>) -> bool {
        let action = InputAction::Add { parent_id };
        if self.todos.project_root.is_none() {
            self.todos.note = Some(match self.todos.projects.is_empty() {
                // The registry was never read: nothing is known about projects either way.
                true if self.todos.project_watch.is_none() && !self.todos.status.is_empty() => {
                    Note::error("projects not loaded").then("Ctrl-r retry")
                },
                true => Note::error("no projects yet").then("start an agent in a git checkout"),
                false => Note::error("no project picked").then("p to pick one"),
            });
            return true;
        }
        let limit = if todos::counts(&self.todos.list).0 >= todos::MAX_OPEN {
            Some(
                Note::error(format!("{} open todos max", todos::MAX_OPEN))
                    .then("complete some first"),
            )
        } else if parent_id.is_some_and(|parent| {
            todos::trail_of(&self.todos.list.items, parent).len() >= todos::MAX_DEPTH
        }) {
            Some(
                Note::error("too deep to nest")
                    .then(format!("todos nest {} levels at most", todos::MAX_DEPTH)),
            )
        } else {
            None
        };
        let drafted = self
            .todos
            .drafts
            .iter()
            .any(|(kept, _)| kept.same_target(&action));
        match limit {
            None => self.begin_todo_input(action, ""),
            Some(limit) if drafted => {
                self.begin_todo_input(action, "");
                self.refuse_todo_input(format!("{} · {}", limit.lead, limit.hint));
                true
            },
            Some(limit) => {
                self.todos.note = Some(limit);
                true
            },
        }
    }

    fn todo_agent_configuration(&self) -> Option<BTreeMap<String, String>> {
        let root = self.todos.project_root.as_ref()?;
        let item = self.selected_todo()?;
        // The task first: the launcher shows the prompt on one row, 15 columns of it on a
        // narrow display, and that much of "Work on project todo #2:" said nothing.
        let mut prompt = format!("Todo #{}: {}", item.id, item.text);
        let mut ancestors = todos::trail_of(&self.todos.list.items, item.id);
        ancestors.pop();
        if !ancestors.is_empty() {
            prompt.push_str("\n\nParent context (for reference):");
            for ancestor in ancestors {
                prompt.push_str(&format!("\n- {ancestor}"));
            }
        }
        let subtasks = todo_rows(
            &todos::List {
                items: item.children.clone(),
                ..Default::default()
            },
            false,
        );
        if !subtasks.is_empty() {
            prompt.push_str("\n\nOpen subtasks:");
            for row in subtasks {
                prompt.push_str(&format!("\n{}- [ ] {}", "  ".repeat(row.depth), row.text));
            }
        }
        prompt.push_str(
            "\n\nImplement the task, run relevant checks, and summarize the changes and anything still outstanding.",
        );
        Some(BTreeMap::from([
            ("caller_cwd".into(), root.clone()),
            ("initial_prompt".into(), prompt),
            ("overlay".into(), "true".into()),
            ("modal".into(), "true".into()),
        ]))
    }

    fn todo_projects(&self) -> Vec<String> {
        self.todos
            .projects
            .values()
            .map(|project| project.root.clone())
            .collect()
    }

    fn todo_project_key(&mut self, backend: &impl Backend, key: KeyWithModifier) -> bool {
        if self.todo_ring_key(&key) {
            return true;
        }
        if !key.has_no_modifiers() {
            return true;
        }
        let roots = self.todo_projects();
        let Mode::Projects { selected } = &mut self.todos.mode else {
            return false;
        };
        // The wheel's place in the picker is not a place in the list: leaving without a pick
        // puts the list back around its cursor, as a pick does (`watch_todo_root_with`).
        match key.bare_key {
            BareKey::Esc => {
                self.todos.mode = Mode::Normal;
                self.todos.list_scroll = None;
            },
            BareKey::Up | BareKey::Char('k') => {
                *selected = selected.saturating_sub(1);
                self.todos.list_scroll = None;
            },
            BareKey::Down | BareKey::Char('j') => {
                *selected = (*selected + 1).min(roots.len().saturating_sub(1));
                self.todos.list_scroll = None;
            },
            BareKey::Enter => match roots.get(*selected).cloned() {
                Some(root) => self.watch_todo_root_with(backend, Some(root)),
                // Nothing to open is not a choice of no project: the list stays as it was.
                None => {
                    self.todos.mode = Mode::Normal;
                    self.todos.list_scroll = None;
                },
            },
            _ => {},
        }
        true
    }

    /// Drop an armed y/n question - a delete, or a complete with open subtasks - for a switch to
    /// another tab, as the other tabs' questions are dropped there: left armed, it answered the
    /// first key of the next visit here, and a `y` then deleted or completed what was asked
    /// about a list the user had stopped looking at.
    pub(super) fn cancel_todo_question(&mut self) {
        if matches!(self.todos.mode, Mode::Delete { .. } | Mode::Complete { .. }) {
            self.todos.mode = Mode::Normal;
        }
    }

    /// The Todos side of `end_visit`: the panel went away, so a question it was asking or a
    /// pick it was part-way through does not wait to take the first keys of the next visit.
    ///
    /// Nor does a box: its text is put away as a draft, as Esc puts it, and the key that
    /// opened it brings it back. Left open, the letters of the next visit - an hour later,
    /// `j j Enter` meant for the list - would be typed into it and saved.
    pub(super) fn todos_end_visit(&mut self) {
        self.todos.note = None;
        match self.todos.mode {
            Mode::Input { .. } => self.close_todo_input(),
            Mode::Normal => {},
            Mode::Projects { .. } => {
                self.todos.mode = Mode::Normal;
                self.todos.list_scroll = None;
            },
            _ => self.todos.mode = Mode::Normal,
        }
    }

    /// A bracketed paste while the Todos tab has the keys, and whether it changed anything.
    /// Only the input box takes one; the list is single-letter commands.
    ///
    /// Line breaks are kept: a todo is markdown and the box has a key for a new line, so a
    /// pasted list lands as a list, drawn with a `⏎` for each break. The breaks a copied line
    /// ends with are dropped, and a tab becomes a space, since nothing draws one.
    pub(super) fn todo_paste(&mut self, text: &str) -> bool {
        let Mode::Input { input, .. } = &mut self.todos.mode else {
            return false;
        };
        let text = text.trim_end_matches(['\r', '\n']).replace('\t', " ");
        let before = input.get_text().len();
        input.insert_text(&text);
        input.get_text().len() != before
    }

    /// Where the terminal cursor belongs in the todo input box, in pane coordinates, given the
    /// `rows` and `cols` `render_todos` is about to draw in; `None` when no box is open.
    ///
    /// The box is the divider, the last of those rows, under the tab bar's one.
    pub(super) fn todo_input_cursor(&self, rows: usize, cols: usize) -> Option<(usize, usize)> {
        if rows == 0 {
            return None;
        }
        let (_, x) = self.input_divider(cols)?;
        Some((x, rows))
    }

    /// The keys the Todos tab answers right now, as `(keys, what they do)`, most used first.
    ///
    /// A box being typed in has no sheet of its own: `?` is a character there, so no key would
    /// open one (`typing` in main.rs). Its keys are on the list's sheet instead, which is where
    /// the box is opened from, and on a row of their own under it while the box is too full to
    /// carry them (`input_hint_row`).
    pub(super) fn todo_key_sheet(&self) -> Vec<(String, String)> {
        let rows: Vec<(&str, String)> = match &self.todos.mode {
            Mode::Delete { descendants, .. } => vec![
                (
                    "y",
                    match descendants {
                        0 => "delete the todo".into(),
                        _ => "delete the todo and its subtasks".into(),
                    },
                ),
                ("any other key", "keep it".into()),
            ],
            Mode::Complete { .. } => vec![
                ("y", "complete the todo and its open subtasks".into()),
                ("any other key", "leave them open".into()),
            ],
            Mode::Move { .. } => vec![
                ("↑ / ↓ / j / k", "choose the new parent, marked >".into()),
                ("Enter", "move the todo under it".into()),
                ("t", "move the todo to the top level".into()),
                ("Esc", "cancel the move".into()),
                ("click", "choose a parent".into()),
                ("Tab / Shift+Tab", "walk the panel, keeping the pick".into()),
            ],
            Mode::Projects { .. } => vec![
                ("↑ / ↓ / j / k", "choose a project".into()),
                ("Enter", "open its todos".into()),
                ("Esc", "cancel".into()),
                ("click / wheel", "choose, scroll".into()),
                ("Tab / Shift+Tab", "walk the panel, keeping the pick".into()),
            ],
            Mode::Normal if self.todos.project_root.is_none() => vec![
                ("p", "pick a project".into()),
                ("Ctrl-r", "reload".into()),
                ("Tab", "go to the panel tab bar".into()),
                ("Shift+Tab", "go to the panel's tabs".into()),
                ("Esc", "hide the panel".into()),
            ],
            // `Input` for completeness only: see above.
            Mode::Normal | Mode::Input { .. } => {
                let mut rows = Vec::new();
                for key in self.todo_draft_keys() {
                    let key = match key {
                        "e" => "Enter / e",
                        key => key,
                    };
                    rows.push((key, "bring back the draft Esc kept".to_owned()));
                }
                rows.extend([
                    ("Space", "complete the selected todo, or reopen it".into()),
                    ("Enter / e", "edit the selected todo".into()),
                    ("a", "add a todo".into()),
                    ("A", "add a subtask under the selected todo".into()),
                    ("d", "delete the selected todo, after y/n".into()),
                    ("n", "start an agent on the selected todo".into()),
                    ("m", "move the selected todo under another".into()),
                    ("p", "switch project".into()),
                    (
                        "v",
                        match self.todos.show_done {
                            true => "hide done todos".into(),
                            false => "show done todos".into(),
                        },
                    ),
                    ("↑ / ↓ / j / k", "move the selection".into()),
                    ("Ctrl-r", "reload the list".into()),
                    ("Tab", "go to the panel tab bar".into()),
                    ("Shift+Tab", "go to the panel's tabs".into()),
                    ("Esc", "hide the panel".into()),
                    ("click / wheel", "select a todo, scroll".into()),
                ]);
                rows.extend(todo_box_key_rows(match &self.todos.mode {
                    Mode::Input { action, .. } => Some(action),
                    _ => None,
                }));
                rows
            },
        };
        rows.into_iter()
            .map(|(keys, what)| (keys.to_owned(), what))
            .collect()
    }

    /// Open the box for `action`, starting from `text` - or from the draft put away for the
    /// same entry, which keeps the revision its text was composed from.
    fn begin_todo_input(&mut self, action: InputAction, text: &str) -> bool {
        let kept = self
            .todos
            .drafts
            .iter()
            .position(|(kept, _)| kept.same_target(&action));
        let (action, input) = match kept {
            Some(index) => self.todos.drafts.remove(index),
            None => (action, TextInput::new(text.to_owned())),
        };
        self.todos.mode = Mode::Input {
            action,
            input,
            refusal: None,
        };
        true
    }

    /// Put the box away, keeping what was typed in it as the draft the next `a`, `A` or `e`
    /// on the same item brings back - the conversation's box keeps its draft the same way.
    ///
    /// Beside the drafts kept for other entries: a draft for this entry, if there was one, is
    /// the text the box opened with (`begin_todo_input` takes it out), so nothing is kept twice.
    /// An edit nobody has changed yet is not a draft, and does not take the place of one.
    fn close_todo_input(&mut self) {
        if let Mode::Input { action, input, .. } = std::mem::take(&mut self.todos.mode) {
            if is_a_draft(&self.todos.list, &action, &input) {
                self.todos.drafts.push((action.into_draft(), input));
            }
        }
    }

    fn todo_input_key(&mut self, backend: &impl Backend, key: KeyWithModifier) -> bool {
        match key.bare_key {
            BareKey::Esc => {
                self.close_todo_input();
                return true;
            },
            // The ring, walked as the list walks it. The box stays open for when the keys
            // come back, as the conversation's does.
            BareKey::Tab => return self.todo_ring_key(&key),
            BareKey::Enter if key.has_no_modifiers() => return self.submit_todo_input(backend),
            // The conversation's keys for a line break. The box is one row, so the break is
            // drawn as a `⏎` rather than as a break.
            BareKey::Enter
                if key.has_modifiers(&[KeyModifier::Alt])
                    || key.has_modifiers(&[KeyModifier::Shift]) =>
            {
                if let Mode::Input { input, .. } = &mut self.todos.mode {
                    input.insert_char('\n');
                }
                return true;
            },
            BareKey::Enter => return true,
            // An Alt+letter is a shortcut somewhere, not a letter; the text input would type
            // it.
            BareKey::Char(_)
                if !(key.has_no_modifiers()
                    || key.has_only_modifiers(&[KeyModifier::Shift])
                    || key.has_modifiers(&[KeyModifier::Ctrl])) =>
            {
                return true;
            },
            _ => {},
        }
        let Mode::Input { input, .. } = &mut self.todos.mode else {
            return false;
        };
        // Ctrl-c is the input's own way out.
        if input.handle_key(key) == forms::InputAction::Cancel {
            self.close_todo_input();
        }
        true
    }

    /// Enter in the box: save it and close it, or say why not and leave it open.
    ///
    /// Planned while the box is still open, so a refusal - the list changed, the item went -
    /// costs the user nothing they typed. A conflict reloads the list first, and a new todo is
    /// planned once more against the fresh one if what it was checked against is as it was:
    /// an add guards the whole project, so it is refused over any change to any item, and one
    /// made elsewhere to an item it does not touch is no reason to stop. That is a fresh plan
    /// and not a replay, which is what a conflict asks for. An edit is never taken over on
    /// its own: the other writer's text goes on screen, and only another Enter overwrites it.
    fn submit_todo_input(&mut self, backend: &impl Backend) -> bool {
        let Some(root) = self.todos.project_root.clone() else {
            return true;
        };
        let Mode::Input {
            action,
            input,
            refusal,
        } = &mut self.todos.mode
        else {
            return false;
        };
        let text = input.get_text().to_owned();
        if text.trim().is_empty() {
            // A blank Enter is a way out, as Esc is, with nothing to keep or explain.
            self.todos.mode = Mode::Normal;
            return true;
        }
        // An overwrite the user was never shown is not one they chose: on a pane too short for
        // the refusal's rows, Enter waits for a taller one, and Esc still keeps theirs.
        if matches!(
            action,
            InputAction::Edit {
                theirs: Some(_),
                ..
            }
        ) && !self.todos.overwrite_shown
        {
            return true;
        }
        *refusal = None;
        let action = action.clone();
        let trail = match &action {
            InputAction::Add { parent_id } => trail_revs(&self.todos.list.items, *parent_id),
            InputAction::Edit { .. } => None,
        };
        let planned = self.plan_todo_input(&root, &action, &text);
        let mut applied = self.apply_todo_plan(backend, planned);
        if let (Applied::Conflict { reloaded: Ok(()) }, InputAction::Add { parent_id }) =
            (&applied, &action)
        {
            if !self.todo_input_refused()
                && trail.is_some()
                && trail == trail_revs(&self.todos.list.items, *parent_id)
            {
                let planned = self.plan_todo_input(&root, &action, &text);
                applied = self.apply_todo_plan(backend, planned);
            }
        }
        match applied {
            Applied::Saved(ids) => {
                self.todos.mode = Mode::Normal;
                let verb = match action {
                    InputAction::Add { .. } => {
                        // Only `plan_add` puts a row, so this is the new todo, which may be
                        // off screen at the foot of the list: the cursor goes to it.
                        if let Some(id) = ids.first() {
                            self.todos.selected = Some(*id);
                            self.todos.list_scroll = None;
                        }
                        "added"
                    },
                    InputAction::Edit { .. } => "saved",
                };
                self.todos.note = Some(Note::done(verb, &text, 0));
            },
            Applied::Refused(error) => {
                self.disarm_todo_overwrite();
                self.refuse_todo_input(error);
            },
            Applied::Conflict {
                reloaded: Err(error),
            } => {
                self.disarm_todo_overwrite();
                self.refuse_todo_input(format!("changed elsewhere · could not reload: {error}"));
            },
            // `check_todo_mode` has said what became of the item the box was about.
            Applied::Conflict { reloaded: Ok(()) } if self.todo_input_refused() => {},
            Applied::Conflict { reloaded: Ok(()) } => {
                let theirs = match action {
                    InputAction::Edit { id, .. } => {
                        todos::item_of(&self.todos.list, id).map(|item| item.rev)
                    },
                    InputAction::Add { .. } => None,
                };
                if let Mode::Input {
                    action: InputAction::Edit { theirs: armed, .. },
                    ..
                } = &mut self.todos.mode
                {
                    *armed = theirs;
                }
                // Keys first, and what becomes of the user's words last, as the part a narrow
                // pane can do without: the box still holds them either way.
                self.refuse_todo_input(match theirs {
                    Some(_) => format!(
                        "changed elsewhere · {OVERWRITES} · Esc keeps theirs · yours stays a draft"
                    ),
                    None => "changed elsewhere · list reloaded · Enter tries again".into(),
                });
            },
        }
        true
    }

    fn plan_todo_input(
        &self,
        root: &str,
        action: &InputAction,
        text: &str,
    ) -> Result<Vec<Mutation>, String> {
        match action {
            InputAction::Add { parent_id } => {
                todos::plan_add(&self.todos.list, root, text, *parent_id).map(|(plan, _)| plan)
            },
            InputAction::Edit {
                id, rev, theirs, ..
            } => todos::plan_edit(&self.todos.list, *id, text, Some(theirs.unwrap_or(*rev)))
                .map(|(plan, _)| plan),
        }
    }

    fn todo_input_refused(&self) -> bool {
        matches!(
            self.todos.mode,
            Mode::Input {
                refusal: Some(_),
                ..
            }
        )
    }

    /// An overwrite a conflict armed, dropped because the write was then refused for another
    /// reason - the store failing on it, or on the reload after it. The refusal now drawn is
    /// not the one that offered the overwrite, so an Enter waiting for `Enter overwrites` to be
    /// drawn again did nothing for good. The next Enter plans from the revision the box opened
    /// with, and a conflict then offers the overwrite again.
    fn disarm_todo_overwrite(&mut self) {
        if let Mode::Input {
            action: InputAction::Edit { theirs, .. },
            ..
        } = &mut self.todos.mode
        {
            *theirs = None;
        }
    }

    fn refuse_todo_input(&mut self, why: String) {
        if let Mode::Input { refusal, .. } = &mut self.todos.mode {
            *refusal = Some(why);
            // A new refusal has not been drawn yet, whatever the last one was.
            self.todos.overwrite_shown = false;
        }
    }

    fn toggle_todo(&mut self, backend: &impl Backend) -> bool {
        let (Some(root), Some(item)) = (self.todos.project_root.clone(), self.selected_todo())
        else {
            return false;
        };
        let (id, text, done) = (item.id, item.text.clone(), item.done);
        match todos::plan_set_done(&self.todos.list, &root, id, !done) {
            // Completing crosses off every open item under this one, which is the rule - work
            // is not finished while a part of it is open - but it is many items for one key,
            // and reopening afterwards brings back only this one. So that one key asks.
            Ok((plan, change)) if !done && !change.cascaded.is_empty() => {
                self.todos.mode = Mode::Complete {
                    id,
                    plan,
                    cascaded: change.cascaded.len(),
                };
            },
            Ok((plan, _)) => {
                let note = match done {
                    true => Note::done("reopened", &text, 0),
                    false => self.completed_note(&text, 0),
                };
                self.write_todos(backend, Ok(plan), note);
            },
            Err(error) => self.todos.note = Some(Note::error(error)),
        }
        true
    }

    fn completed_note(&self, text: &str, cascaded: usize) -> Note {
        let mut note = Note::done("completed", text, cascaded);
        if !self.todos.show_done {
            note.hint = "v shows done".into();
        }
        note
    }

    fn arm_todo_delete(&mut self) -> bool {
        let (Some(root), Some(id)) = (&self.todos.project_root, self.todos.selected) else {
            return false;
        };
        if todos::item_of(&self.todos.list, id).is_none() {
            self.todos.note = Some(Note::error(GONE));
            return true;
        }
        match todos::plan_delete(&self.todos.list, root, id, true) {
            Ok((plan, ids)) => {
                self.todos.mode = Mode::Delete {
                    id,
                    plan,
                    descendants: ids.len() - 1,
                };
            },
            Err(error) => self.todos.note = Some(Note::error(error)),
        }
        true
    }

    /// The answer to a delete or a complete question: `y` writes the plan made when it was
    /// asked, anything else leaves the list alone.
    fn todo_confirm_key(&mut self, backend: &impl Backend, key: KeyWithModifier) -> bool {
        let mode = std::mem::take(&mut self.todos.mode);
        // Tab drops the question, as any key but `y` does, and then does its job, as a Tab at
        // the other tabs' questions does: a question is not somewhere to be stuck.
        if self.todo_ring_key(&key) {
            return true;
        }
        if key.bare_key != BareKey::Char('y') || !key.has_no_modifiers() {
            return true;
        }
        let text_of = |id| todos::text_of(&self.todos.list, id).unwrap_or_default();
        match mode {
            Mode::Delete {
                id,
                plan,
                descendants,
            } => {
                let note = Note::done("deleted", &text_of(id), descendants);
                self.write_todos(backend, Ok(plan), note);
            },
            Mode::Complete { id, plan, cascaded } => {
                let note = self.completed_note(&text_of(id), cascaded);
                self.write_todos(backend, Ok(plan), note);
            },
            _ => {},
        }
        true
    }

    fn arm_todo_move(&mut self) -> bool {
        let Some(item) = self.selected_todo() else {
            return false;
        };
        let id = item.id;
        let rev = item.rev;
        let targets = self.move_targets(id);
        if targets.is_empty() {
            // Only a top-level todo with nothing else on show gets here: there is nowhere to
            // put it, and a move mode in which Enter and j/k did nothing was a dead end.
            self.todos.note =
                Some(Note::error("nowhere to move it").then("add another todo first"));
            return true;
        }
        self.todos.mode = Mode::Move {
            id,
            rev,
            target: targets.first().copied(),
        };
        true
    }

    fn todo_move_key(&mut self, backend: &impl Backend, key: KeyWithModifier) -> bool {
        if self.todo_ring_key(&key) {
            return true;
        }
        if key.bare_key == BareKey::Esc {
            self.todos.mode = Mode::Normal;
            return true;
        }
        let (id, rev, current) = match self.todos.mode {
            Mode::Move { id, rev, target } => (id, rev, target),
            _ => return false,
        };
        let targets = self.move_targets(id);
        match key.bare_key {
            BareKey::Up | BareKey::Char('k') | BareKey::Down | BareKey::Char('j')
                if key.has_no_modifiers() =>
            {
                let at = current
                    .and_then(|target| targets.iter().position(|id| *id == target))
                    .unwrap_or(0);
                let delta = if matches!(key.bare_key, BareKey::Up | BareKey::Char('k')) {
                    -1
                } else {
                    1
                };
                let target = targets
                    .get(
                        at.saturating_add_signed(delta)
                            .min(targets.len().saturating_sub(1)),
                    )
                    .copied();
                self.todos.mode = Mode::Move { id, rev, target };
                self.todos.list_scroll = None;
            },
            BareKey::Enter if key.has_no_modifiers() && current.is_some() => {
                self.commit_todo_move(backend, id, rev, current);
            },
            BareKey::Char('t') if key.has_no_modifiers() => {
                self.commit_todo_move(backend, id, rev, None);
            },
            _ => {},
        }
        true
    }

    fn commit_todo_move(
        &mut self,
        backend: &impl Backend,
        id: u64,
        rev: DataRevision,
        parent_id: Option<u64>,
    ) {
        self.todos.mode = Mode::Normal;
        let Some(root) = self.todos.project_root.clone() else {
            return;
        };
        // Where it already is: said, rather than written and reported as a move.
        if parent_of(&self.todos.list.items, id, None) == Some(parent_id) {
            self.todos.note = Some(Note::error(match parent_id {
                None => "already top level",
                Some(_) => "already under it",
            }));
            return;
        }
        let text = todos::text_of(&self.todos.list, id).unwrap_or_default();
        let planned = todos::plan_move(&self.todos.list, &root, id, parent_id, Some(rev))
            .map(|(plan, _)| plan);
        self.write_todos(backend, planned, Note::done("moved", &text, 0));
    }

    fn move_targets(&self, source: u64) -> Vec<u64> {
        let mut excluded = vec![source];
        if let Some(item) = todos::item_of(&self.todos.list, source) {
            collect_ids(&item.children, &mut excluded);
        }
        todo_rows(&self.todos.list, self.todos.show_done)
            .into_iter()
            .map(|row| row.id)
            .filter(|id| !excluded.contains(id))
            .collect()
    }

    fn apply_todo_plan(
        &mut self,
        backend: &impl Backend,
        planned: Result<Vec<Mutation>, String>,
    ) -> Applied {
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => return Applied::Refused(error),
        };
        match backend.apply(plan) {
            Ok(report) => {
                if let Err(error) = self.reload_todos(backend) {
                    self.todos.status = format!("saved, but could not refresh: {error}");
                }
                Applied::Saved(
                    report
                        .ids
                        .iter()
                        .filter_map(|id| u64::try_from(id.get()).ok())
                        .collect(),
                )
            },
            // Every plan but an edit guards the whole project, so this is anybody writing to
            // it from another process - another session, another machine - whose writes this
            // pane's watch never hears of. Until the list is read again every retry would be
            // refused the same way, so it is read now.
            Err(error) if error.is_conflict() => Applied::Conflict {
                reloaded: self.reload_todos(backend),
            },
            Err(error) => Applied::Refused(error.to_string()),
        }
    }

    /// Write a plan made from the list, noting `success` or saying why not.
    fn write_todos(
        &mut self,
        backend: &impl Backend,
        planned: Result<Vec<Mutation>, String>,
        success: Note,
    ) {
        self.todos.note = Some(match self.apply_todo_plan(backend, planned) {
            Applied::Saved(_) => success,
            Applied::Refused(error) => Note::error(error),
            Applied::Conflict { reloaded: Ok(()) } => {
                Note::error("changed elsewhere").then("list reloaded, try again")
            },
            Applied::Conflict { reloaded: Err(_) } => {
                Note::error("changed elsewhere").then("Ctrl-r reload")
            },
        });
    }

    fn selected_todo(&self) -> Option<&todos::Item> {
        todos::item_of(&self.todos.list, self.todos.selected?)
    }

    fn step_todo(&mut self, delta: isize) -> bool {
        let rows = todo_rows(&self.todos.list, self.todos.show_done);
        let at = self
            .todos
            .selected
            .and_then(|id| rows.iter().position(|row| row.id == id))
            .unwrap_or(0);
        let selected = rows
            .get(
                at.saturating_add_signed(delta)
                    .min(rows.len().saturating_sub(1)),
            )
            .map(|row| row.id);
        let changed = self.todos.selected != selected;
        self.todos.selected = selected;
        self.todos.list_scroll = None;
        changed
    }

    /// Re-derives the visible rows after `show_done` has just been flipped.
    fn take_todo_rows_from_list(&mut self) {
        self.todos.line_cache = None;
        let before = todo_rows(&self.todos.list, !self.todos.show_done);
        let rows = todo_rows(&self.todos.list, self.todos.show_done);
        self.todos.selected = nearest_surviving(&before, &rows, self.todos.selected);
        self.todos.list_scroll = None;
    }

    pub(super) fn todo_mouse(&mut self, event: Mouse) -> bool {
        match event {
            Mouse::LeftClick(line, _) if line >= 0 => {
                // The project's row above the list is a label, not a target.
                let Some(line) = (line as usize).checked_sub(self.todos.last_header_rows) else {
                    return false;
                };
                if line >= self.todos.last_list_rows {
                    return false;
                }
                if let Mode::Projects { selected } = &mut self.todos.mode {
                    if line < self.todos.rendered_projects.len() {
                        *selected = self.todos.last_window + line;
                    }
                } else if let Some(id) = self.todos.rendered_ids.get(line).copied() {
                    let valid_move_target = match &self.todos.mode {
                        Mode::Move { id: source, .. } => self.move_targets(*source).contains(&id),
                        _ => true,
                    };
                    match &mut self.todos.mode {
                        Mode::Move { target, .. } if valid_move_target => *target = Some(id),
                        Mode::Normal => self.todos.selected = Some(id),
                        _ => {},
                    }
                }
                true
            },
            Mouse::ScrollUp(lines) => {
                let scroll = self.todos.list_scroll.get_or_insert(self.todos.last_window);
                *scroll = scroll.saturating_sub(lines.max(1));
                true
            },
            Mouse::ScrollDown(lines) => {
                let scroll = self.todos.list_scroll.get_or_insert(self.todos.last_window);
                *scroll = scroll.saturating_add(lines.max(1));
                true
            },
            _ => false,
        }
    }

    /// The row the list marks: what a key would act on now. In a box that is the item the box
    /// is about - the parent, for a subtask - rather than wherever the selection has drifted
    /// since, and nothing for a new top-level todo, which goes to the foot of the list.
    fn todo_cursor(&self) -> Option<u64> {
        match &self.todos.mode {
            Mode::Move { target, .. } => *target,
            Mode::Input {
                action: InputAction::Edit { id, .. },
                ..
            }
            | Mode::Delete { id, .. }
            | Mode::Complete { id, .. } => Some(*id),
            Mode::Input {
                action: InputAction::Add { parent_id },
                ..
            } => *parent_id,
            Mode::Normal | Mode::Projects { .. } => self.todos.selected,
        }
    }

    pub(super) fn render_todos(&mut self, rows: usize, cols: usize) {
        self.todos.rendered_ids.clear();
        self.todos.rendered_projects.clear();
        if rows == 0 || cols == 0 {
            self.todos.clear_dimensions();
            return;
        }
        // Why the box did not save, over it, wrapped rather than cut: the part a narrow pane
        // would cut is the key that gets the user past it. Up to three rows, and never the
        // last row of the list.
        let refusal = match &self.todos.mode {
            Mode::Input {
                refusal: Some(refusal),
                ..
            } => refusal.clone(),
            _ => String::new(),
        };
        let segments: Vec<&str> = refusal.split(" · ").collect();
        let wanted = wrap_segments(&segments, cols, 3).len();
        // Whose list this is, over it: the one thing every key here acts within, on a row no
        // note or hint can take. It used to be the first word of the hint line, where it read
        // as one more hint and any message replaced it. Only a refusal comes before it, on a
        // pane too short for both: that says what the next Enter does.
        let header = self.todo_header(cols).filter(|_| rows >= 3 + wanted);
        let top = usize::from(header.is_some());
        if let Some(header) = &header {
            paint_todo_row(header, 0, cols);
        }
        self.todos.last_header_rows = top;
        let refusal = wrap_segments(&segments, cols, (rows - top).saturating_sub(2).min(3));
        if refusal
            .iter()
            .any(|row| row.split(" · ").any(|segment| segment == OVERWRITES))
        {
            self.todos.overwrite_shown = true;
        }
        // The box's keys, on a row of their own while the draft leaves the box's row none -
        // one of the list's, given back once they fit again. Not over a refusal, which says
        // what to press instead, nor where it would take the list's last row.
        let key_row = match &self.todos.mode {
            Mode::Input { refusal: None, .. } if rows - top >= 3 => self.input_hint_row(cols),
            _ => None,
        };
        let list_rows = rows - top - 1 - refusal.len() - usize::from(key_row.is_some());
        self.todos.last_list_rows = list_rows;
        if let Mode::Projects { selected } = self.todos.mode {
            let projects: Vec<_> = self.todos.projects.values().collect();
            let start = self
                .todos
                .list_scroll
                .unwrap_or_else(|| selected.saturating_sub(list_rows / 2))
                .min(projects.len().saturating_sub(list_rows));
            // Clamped where it is kept, or a wheel past the end would bank travel that has
            // to be unwound before the list moves back.
            if self.todos.list_scroll.is_some() {
                self.todos.list_scroll = Some(start);
            }
            self.todos.last_window = start;
            self.todos.rendered_projects = projects
                .iter()
                .skip(start)
                .take(list_rows)
                .map(|project| project.root.clone())
                .collect();
            for (line, project) in projects.iter().skip(start).take(list_rows).enumerate() {
                let index = start + line;
                let text = Text::new(agents::cut_to_width(
                    format!(
                        "{} {}  {}",
                        if index == selected { ">" } else { " " },
                        project.name,
                        project.root
                    ),
                    cols,
                ));
                paint(
                    if index == selected {
                        text.selected()
                    } else {
                        text
                    },
                    0,
                    top + line,
                    Some(cols),
                    None,
                );
            }
            if projects.is_empty() {
                paint_empty_text(&self.todo_empty_text(), top, list_rows, cols);
            }
            let below = projects.len().saturating_sub(start + list_rows);
            let row = self.todo_divider(cols, start, below);
            paint_todo_row(&row, rows - 1, cols);
            return;
        }
        if self
            .todos
            .line_cache
            .as_ref()
            .is_none_or(|(width, _)| *width != cols)
        {
            let rows = todo_rows(&self.todos.list, self.todos.show_done);
            self.todos.line_cache = Some((cols, todo_lines(&rows, cols)));
        }
        let lines = self
            .todos
            .line_cache
            .as_ref()
            .map(|(_, lines)| lines.as_slice())
            .unwrap_or_default();
        let cursor = self.todo_cursor();
        let moving = match &self.todos.mode {
            Mode::Move { id, .. } => Some(*id),
            _ => None,
        };
        let start = self
            .todos
            .list_scroll
            .unwrap_or_else(|| {
                // Nothing marked - a new top-level todo being typed - is no reason to go back
                // to the top: the view stays where the user was looking.
                let Some(cursor) = cursor else {
                    return self.todos.last_window;
                };
                // Scrolling is in screen rows, so a tall item is a tall stretch of the list.
                let first = lines.iter().position(|line| line.id == cursor).unwrap_or(0);
                let end = lines[first..]
                    .iter()
                    .position(|line| line.id != cursor)
                    .map_or(lines.len(), |offset| first + offset);
                let start = first.saturating_sub(list_rows / 2);
                // An item taller than what is left below shows its head.
                match end > start + list_rows {
                    true => first.min(end.saturating_sub(list_rows)),
                    false => start,
                }
            })
            .min(lines.len().saturating_sub(list_rows));
        if self.todos.list_scroll.is_some() {
            self.todos.list_scroll = Some(start);
        }
        self.todos.last_window = start;
        for (row, line) in lines.iter().skip(start).take(list_rows).enumerate() {
            self.todos.rendered_ids.push(line.id);
            let selected = cursor == Some(line.id);
            let body = line_body(line, selected, moving == Some(line.id));
            let text = markdown::style_text(&body.text, &body.styles, line.done);
            paint(
                if selected { text.selected() } else { text },
                0,
                top + row,
                Some(cols),
                None,
            );
        }
        if lines.is_empty() {
            paint_empty_text(&self.todo_empty_text(), top, list_rows, cols);
        }
        let below = lines.len().saturating_sub(start + list_rows);
        for (at, line) in refusal.iter().enumerate() {
            paint(
                Text::new(line).error_color_range(..),
                0,
                top + list_rows + at,
                Some(cols),
                None,
            );
        }
        if let Some(keys) = key_row {
            paint(Text::new(keys).dim_range(..), 0, rows - 2, Some(cols), None);
        }
        let row = match self.input_divider(cols) {
            Some((row, _)) => row,
            None => self.todo_divider(cols, start, below),
        };
        paint_todo_row(&row, rows - 1, cols);
    }

    /// The row above the list: the project's name, and the key that changes it while that key
    /// does - in the list itself, and not in a box, a move or a question, where `p` is a letter
    /// typed, a key ignored or an answer.
    fn todo_header(&self, cols: usize) -> Option<TodoRow> {
        let (label, hint) = match self.todos.mode {
            Mode::Projects { .. } => ("Pick a project".to_owned(), ""),
            _ => {
                self.todos.project_root.as_ref()?;
                let hint = match self.todos.mode {
                    Mode::Normal => "p switch",
                    _ => "",
                };
                (format!("Project: {}", self.todo_project_name()), hint)
            },
        };
        let line = fit_segments(&[label.as_str(), hint], cols);
        let bright = 0..label.chars().count().min(line.chars().count());
        Some(TodoRow {
            line,
            bright: Some(bright),
            error: false,
        })
    }

    /// What the list says when it has no rows to show: each line, and whether it is the dim
    /// hint under the plain statement. Wrapped to the pane where it is drawn
    /// (`empty_text_rows`).
    fn todo_empty_text(&self) -> Vec<(String, bool)> {
        if self.todos.project_root.is_none() || matches!(self.todos.mode, Mode::Projects { .. }) {
            // A registry that could not be read is not an empty one, and the way out of it is
            // not an agent in a checkout.
            if self.todos.projects.is_empty()
                && self.todos.project_watch.is_none()
                && !self.todos.status.is_empty()
            {
                return vec![
                    ("projects not loaded".into(), false),
                    ("Ctrl-r to retry".into(), true),
                ];
            }
            if self.todos.projects.is_empty() {
                // Projects are remembered for agents, not for every shell, so the way to one
                // is to start one - or to name one outright.
                return vec![
                    ("no projects yet".into(), false),
                    ("start an agent in a git checkout,".into(), true),
                    (
                        "or: yinz action project-remember NAME --root DIR".into(),
                        true,
                    ),
                ];
            }
            // Not "for this folder": the folder is the one the panel was opened in, which the
            // user never sees, and the list does not follow the pane they are in.
            return vec![
                ("no project picked".into(), false),
                ("p to pick one".into(), true),
            ];
        }
        // Nothing visible out of a list that has items: every one of them is done, since a
        // done item has nothing open under it.
        let (_, all) = todos::counts(&self.todos.list);
        if all > 0 && !self.todos.show_done {
            let items = if all == 1 { "todo" } else { "todos" };
            return vec![
                (format!("{all} done {items} hidden"), false),
                ("v to show them".into(), true),
            ];
        }
        vec![
            ("no todos yet".into(), false),
            ("a to add one".into(), true),
        ]
    }

    /// The divider in every mode but a box being typed in: a question, a note or a problem
    /// first, then as many of the keys as fit, most used first.
    fn todo_divider(&self, cols: usize, above: usize, below: usize) -> TodoRow {
        // The question is the row, so nothing crowds it: a delete of a subtree whose `y/n` is
        // cut off is a delete of something the user cannot see the size of. A note is said
        // once, until the next key, and is read whole in that time or not at all. A move's
        // keys are its only ways out - `t` is the only way to the top level - and the readout
        // gives way to them where they would not all fit beside it.
        let (above, below) = match self.todos.mode {
            Mode::Delete { .. } | Mode::Complete { .. } => (0, 0),
            Mode::Normal if self.todos.note.is_some() => (0, 0),
            Mode::Move { .. }
                if cols < 4 + scroll_readout(above, below).width() + MOVE_KEYS.width() =>
            {
                (0, 0)
            },
            _ => (above, below),
        };
        // What the scroll readout leaves of the row, so a label fitted to it is never cut
        // again, mid-word, to make room for it. No focus mark: this row is drawn with
        // `divider_line` rather than `render_divider`.
        let room = cols.saturating_sub(4 + scroll_readout(above, below).width());
        // While the panel's tab bar has the keys, this row says what they do there, rather
        // than a row more saying it: the tab's own keys are not what the next press does -
        // Enter activates a tab, Esc returns to the active list, and other keys are consumed.
        // Only a question never has the bar over it, since the Tab
        // that goes up to the bar drops it first (`todo_confirm_key`). An open box is drawn in
        // this row's place wherever it fits, and its row names the bar's keys the same way
        // (`input_row`).
        let bar = self.bar_has_keys();
        let bar_keys = || tabs::BAR_HINT.split(" · ").map(str::to_owned);
        let (lead, error, rest): (Option<String>, bool, Vec<String>) = match &self.todos.mode {
            // Questions lead with their answer and their scope, which is what a narrow pane
            // keeps.
            Mode::Delete { descendants, .. } => (
                Some(match descendants {
                    0 => "y/n delete this todo?".to_owned(),
                    count => format!("y/n delete with {}?", subtasks(*count)),
                }),
                true,
                Vec::new(),
            ),
            Mode::Complete { cascaded, .. } => (
                Some(format!("y/n complete with {}?", open_subtasks(*cascaded))),
                true,
                Vec::new(),
            ),
            Mode::Move { id, target, .. } => {
                let name = todos::text_of(&self.todos.list, *id).unwrap_or_default();
                // Short enough for the narrowest half: the `>` on the list and the sheet say
                // what the nesting is under. A move the bar has the keys over - Shift+Tab keeps
                // it - names the bar's keys and still what is being moved.
                let mut keys = Vec::new();
                if bar {
                    keys.extend(bar_keys());
                } else {
                    if target.is_some() {
                        keys.push("Enter nest".to_owned());
                    }
                    keys.extend(["t top".to_owned(), "Esc cancel".to_owned()]);
                }
                keys.push(format!("moving \"{}\"", excerpt(&name, 24)));
                (None, false, keys)
            },
            Mode::Projects { .. } => (
                None,
                false,
                match (bar, self.todos.projects.is_empty()) {
                    (true, _) => bar_keys().collect(),
                    (false, true) => vec!["Esc back".to_owned()],
                    (false, false) => vec!["Enter open".to_owned(), "Esc cancel".to_owned()],
                },
            ),
            Mode::Normal | Mode::Input { .. } => {
                // A note or a problem still leads the bar's keys, as it leads the list's.
                let hints = match bar {
                    true => bar_keys().collect(),
                    false => self.todo_normal_hints(),
                };
                if let Some(note) = &self.todos.note {
                    let rest = std::iter::once(note.hint.clone()).chain(hints).collect();
                    (Some(note.text(room)), note.error, rest)
                } else if !self.todos.status.is_empty() {
                    let rest = std::iter::once("Ctrl-r retry".to_owned())
                        .chain(hints.into_iter().filter(|hint| hint != "Ctrl-r reload"))
                        .collect();
                    (Some(self.todos.status.clone()), true, rest)
                } else {
                    (None, false, hints)
                }
            },
        };
        let segments: Vec<&str> = lead.iter().chain(rest.iter()).map(String::as_str).collect();
        let label = if bar {
            agents::fit_keys(&segments, room)
        } else {
            agents::fit_hints(&segments, room)
        };
        let line = divider_line(&label, above, below, cols);
        let start = line
            .find(&label)
            .map_or(0, |byte| line[..byte].chars().count());
        let bright =
            lead.map(|lead| start..start + lead.chars().count().min(label.chars().count()));
        TodoRow {
            line,
            bright,
            error,
        }
    }

    /// The Normal mode's keys, in the order they are worth: the selected item's own verbs
    /// first, and only keys that would do something now.
    fn todo_normal_hints(&self) -> Vec<String> {
        if self.todos.project_root.is_none() {
            return match self.todos.projects.is_empty() {
                true => vec!["Ctrl-r reload".into()],
                false => vec!["p pick a project".into(), "Ctrl-r reload".into()],
            };
        }
        let drafts = self.todo_draft_keys();
        let drafted = |key: &str| drafts.contains(&key);
        let mut hints = Vec::new();
        // Enter opens the item into the box as `e` does, and so brings back its draft too.
        for key in &drafts {
            match *key {
                "e" => hints.push("Enter/e resume draft".to_owned()),
                key => hints.push(format!("{key} resume draft")),
            }
        }
        let selected = self.selected_todo();
        if let Some(item) = selected {
            hints.push(
                if item.done {
                    "Space reopen"
                } else {
                    "Space done"
                }
                .into(),
            );
            if !drafted("e") {
                hints.push("Enter/e edit".into());
            }
        }
        if !drafted("a") {
            hints.push("a add".into());
        }
        if selected.is_some() {
            if !drafted("A") {
                hints.push("A subtask".into());
            }
            hints.extend(["d delete", "n agent", "m move"].map(String::from));
        }
        hints.push("p project".into());
        let (open, all) = todos::counts(&self.todos.list);
        if self.todos.show_done {
            hints.push("v hide done".into());
        } else if all > open {
            hints.push("v show done".into());
        }
        hints.push("Ctrl-r reload".into());
        hints
    }

    /// The keys that bring a draft back from here, in the order the hints name them: `e` on
    /// the item an edit was about, `A` on the item a subtask was for, and `a` for a new
    /// top-level todo.
    fn todo_draft_keys(&self) -> Vec<&'static str> {
        let key = |action: &InputAction| match action {
            InputAction::Add { parent_id: None } => Some("a"),
            InputAction::Add {
                parent_id: Some(parent),
            } if self.todos.selected == Some(*parent) => Some("A"),
            InputAction::Edit { id, .. } if self.todos.selected == Some(*id) => Some("e"),
            _ => None,
        };
        let found: Vec<&'static str> = self
            .todos
            .drafts
            .iter()
            .filter_map(|(action, _)| key(action))
            .collect();
        ["e", "A", "a"]
            .into_iter()
            .filter(|key| found.contains(key))
            .collect()
    }

    /// The box as its divider row, and the column the caret is drawn in; `None` when no box is
    /// open, or the pane is too narrow to draw one.
    ///
    /// Scrolled sideways around the caret, with the prompt pinned: a label cut from its tail,
    /// as it used to be, lost the caret and everything after it once the draft was a few
    /// words long, and the user typed blind. The keys go after the draft only while all of it
    /// is on screen.
    fn input_divider(&self, cols: usize) -> Option<(TodoRow, usize)> {
        self.input_row(cols).map(|(row, caret, ..)| (row, caret))
    }

    /// The box's keys on a row of their own, in `cols` columns: `None` while the box's own
    /// row holds all of them, or no box is drawn. Without `? keys`, which in the box types a
    /// question mark.
    fn input_hint_row(&self, cols: usize) -> Option<String> {
        let (_, _, hints, shown) = self.input_row(cols)?;
        (shown < hints.len()).then(|| fit_segments(&hints, cols))
    }

    /// [`Self::input_divider`], with the box's keys and how many of them its row holds.
    fn input_row(&self, cols: usize) -> Option<(TodoRow, usize, Vec<&'static str>, usize)> {
        let Mode::Input { action, input, .. } = &self.todos.mode else {
            return None;
        };
        let room = cols.saturating_sub(4);
        if room == 0 {
            return None;
        }
        // One char for one char, so the caret's index still points where it did: a line break
        // is drawn as `⏎` - written as itself on the box's one row, it would move the cursor
        // down and scroll the whole pane up - and any other control character as a space.
        let shown: String = input
            .get_text()
            .chars()
            .map(|ch| match ch {
                '\n' => '⏎',
                ch if ch.is_control() => ' ',
                ch => ch,
            })
            .collect();
        // While the panel's tab bar has the keys over the box - Shift+Tab leaves it open -
        // the box's keys give way to the bar's, here and on the row under the list, as the
        // list's own divider does (`todo_divider`): Enter and Esc on the bar step back down
        // into the box rather than saving or closing it, so the box's keys would name what the
        // next press does not do. The draft stays on the row, since it is what the box keeps.
        let hints: Vec<&'static str> = if self.bar_has_keys() {
            tabs::BAR_HINT.split(" · ").collect()
        } else {
            match (
                input.is_empty(),
                is_a_draft(&self.todos.list, action, input),
            ) {
                (true, _) => vec!["Esc cancel"],
                (false, true) => vec!["Enter save", "Esc keep draft", "Alt+Enter new line"],
                (false, false) => vec!["Enter save", "Esc cancel", "Alt+Enter new line"],
            }
        };
        let prompt = self.input_prompt(action, room);
        let (label, caret, text, appended) =
            input_label(&prompt, &shown, input.get_cursor_position(), &hints, room);
        let line = divider_line(&label, 0, 0, cols);
        let at = line.find(&label).unwrap_or(0);
        let (chars, width) = (line[..at].chars().count(), line[..at].width());
        let row = TodoRow {
            line,
            bright: Some(chars + text.start..chars + text.end),
            error: false,
        };
        Some((row, width + caret, hints, appended))
    }

    /// What the box is for, in the fewest columns the row can spare: a subtask names its
    /// parent where that leaves the draft room to be read, and is at least not a `new:`.
    fn input_prompt(&self, action: &InputAction, room: usize) -> String {
        let (short, long) = match action {
            InputAction::Add { parent_id: None } => {
                ("new: ".to_owned(), "new top-level todo: ".to_owned())
            },
            InputAction::Add {
                parent_id: Some(parent),
            } => {
                let parent = todos::text_of(&self.todos.list, *parent).unwrap_or_default();
                (
                    "subtask: ".to_owned(),
                    format!("subtask of \"{}\": ", excerpt(&parent, 20)),
                )
            },
            InputAction::Edit { .. } => ("edit: ".to_owned(), "edit: ".to_owned()),
        };
        match long.width() + 24 <= room {
            true => long,
            false => short,
        }
    }
}

/// Whether Esc on this box would keep its text: there is some, and it is not just the text of
/// the item being edited, which is no draft of anything - neither the text the box opened with
/// nor the item's text now, once somebody else has changed it.
fn is_a_draft(list: &todos::List, action: &InputAction, input: &TextInput) -> bool {
    let untouched = match action {
        InputAction::Edit { id, original, .. } => {
            input.get_text() == original
                || todos::text_of(list, *id).as_deref() == Some(input.get_text())
        },
        InputAction::Add { .. } => false,
    };
    !untouched && !input.get_text().trim().is_empty()
}

/// The box's keys for the list's key sheet, where the box is opened from - it has none of its
/// own. `action` is the box open now, which only changes what Enter is said to do.
fn todo_box_key_rows(action: Option<&InputAction>) -> Vec<(&'static str, String)> {
    let enter = match action {
        Some(InputAction::Edit {
            theirs: Some(_), ..
        }) => "in the box: save, over the change made elsewhere",
        _ => "in the box: save; on an empty box, close it",
    };
    [
        ("Enter", enter),
        ("Alt+Enter / Shift+Enter", "in the box: start a new line"),
        (
            "Esc / Ctrl-c",
            "in the box: close it, keeping the text as a draft",
        ),
        ("← / → / Home / End / Ctrl-a", "in the box: move the caret"),
        (
            "Ctrl+← / Ctrl+→ / Alt+← / Alt+→",
            "in the box: move by a word",
        ),
        (
            "Backspace / Delete / Ctrl-d",
            "in the box: delete a character",
        ),
        (
            "Ctrl-w / Alt+Backspace",
            "in the box: delete the word before the caret",
        ),
        (
            "Ctrl-u / Ctrl-k",
            "in the box: delete to the start, or to the end",
        ),
        ("Ctrl-z / Ctrl-y", "in the box: undo, redo"),
        (
            "Tab / Shift+Tab",
            "in the box: walk the panel, leaving the box open",
        ),
    ]
    .into_iter()
    .map(|(keys, what)| (keys, what.to_owned()))
    .collect()
}

/// The input box as one label of at most `room` columns: `prompt`, then `text` scrolled
/// sideways so the caret at `cursor` stays on screen, then as many of `hints` as fit - but
/// only after the whole of the text. Also the caret's column in the label, the chars of the
/// label that are the text, which is drawn brighter than the rest, and how many of `hints` it
/// holds.
fn input_label(
    prompt: &str,
    text: &str,
    cursor: usize,
    hints: &[&str],
    room: usize,
) -> (String, usize, Range<usize>, usize) {
    let (mut label, caret, start) = forms::viewport_window(prompt, text, cursor, room);
    let end = label.chars().count();
    let from = prompt.chars().count().min(end);
    let mut appended = 0;
    if start == 0 && end - from == text.chars().count() {
        for hint in hints {
            if label.width() + " · ".width() + hint.width() > room {
                break;
            }
            label.push_str(" · ");
            label.push_str(hint);
            appended += 1;
        }
    }
    (label, caret, from..end, appended)
}

/// `segments` joined with ` · ` while they fit `width`, dropping whole ones from the end and
/// cutting only a first one too wide on its own: `agents::fit_hints` without its `? keys`,
/// for a row that is not a hint line, or one drawn while `?` is a character.
fn fit_segments(segments: &[&str], width: usize) -> String {
    let mut segments = segments.iter().filter(|segment| !segment.is_empty());
    let Some(first) = segments.next() else {
        return String::new();
    };
    let mut line = agents::cut_to_width((*first).to_owned(), width);
    for segment in segments {
        if line.width() + " · ".width() + segment.width() > width {
            break;
        }
        line.push_str(" · ");
        line.push_str(segment);
    }
    line
}

/// `segments` over as few rows of `width` as they need, whole segments to a row and at most
/// `rows` rows; a segment wider than a row on its own is cut.
fn wrap_segments(segments: &[&str], width: usize, rows: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for segment in segments.iter().filter(|segment| !segment.is_empty()) {
        let full = lines.len() == rows;
        match lines.last_mut() {
            Some(line) if line.width() + " · ".width() + segment.width() <= width => {
                line.push_str(" · ");
                line.push_str(segment);
            },
            _ if full => break,
            _ => lines.push(agents::cut_to_width((*segment).to_owned(), width)),
        }
    }
    lines
}

fn paint_todo_row(row: &TodoRow, y: usize, cols: usize) {
    let len = row.line.chars().count();
    let mut text = Text::new(&row.line);
    match &row.bright {
        Some(bright) => {
            if bright.start > 0 {
                text = text.dim_range(..bright.start);
            }
            if bright.end < len {
                text = text.dim_range(bright.end..);
            }
            if row.error && !bright.is_empty() {
                text = text.error_color_range(bright.clone());
            }
        },
        None => text = text.dim_range(..),
    }
    paint(text, 0, y, Some(cols), None);
}

/// The empty list's lines from row `top`, as many as `rows` has room for.
fn paint_empty_text(lines: &[(String, bool)], top: usize, rows: usize, cols: usize) {
    for (row, (line, dim)) in empty_text_rows(lines, cols)
        .into_iter()
        .take(rows)
        .enumerate()
    {
        let text = Text::new(line);
        paint(
            if dim { text.dim_range(..) } else { text },
            0,
            top + row,
            Some(cols),
            None,
        );
    }
}

/// The empty list's lines as the rows `cols` columns draw them in: word-wrapped rather than
/// cut, each row as dim as its line. What the empty list says is the way out of it - a command
/// to run, the key to press - and a row cut at the pane's edge loses exactly that part.
fn empty_text_rows(lines: &[(String, bool)], cols: usize) -> Vec<(String, bool)> {
    lines
        .iter()
        .flat_map(|(line, dim)| {
            wrap_words(line, cols)
                .into_iter()
                .map(move |row| (row, *dim))
        })
        .collect()
}

/// Stop a watch the host is keeping for this pane. Natively there is no host: a
/// `store::Local` watch never notifies, and the shim would wait on stdin for an answer.
fn unwatch(watch_id: DataWatchId) {
    #[cfg(target_family = "wasm")]
    let _ = data_unwatch(watch_id);
    #[cfg(not(target_family = "wasm"))]
    let _ = watch_id;
}

/// A list row as drawn: `>` in the cursor column of the marked item's first row, and `↕` in
/// that of the item being moved - so it can be seen while the cursor is off choosing where it
/// goes, and not by colour alone.
fn line_body(line: &TodoLine, marked: bool, moving: bool) -> markdown::MdLine {
    match (marked && line.first, moving && line.first) {
        (true, _) => line.line.clone().prepend(">", markdown::MdStyle::default()),
        (false, true) => line.line.clone().prepend("↕", markdown::MdStyle::default()),
        (false, false) => line.line.clone().indent(1),
    }
}

/// The first line of an item's text, cut to `width` columns with a `…` where it was cut: how
/// a message names the item it is about.
fn excerpt(text: &str, width: usize) -> String {
    let line: String = text
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    if line.width() <= width {
        return line;
    }
    let mut cut = agents::cut_to_width(line, width.saturating_sub(1));
    cut.push('…');
    cut
}

fn subtasks(count: usize) -> String {
    match count {
        1 => "1 subtask".to_owned(),
        count => format!("{count} subtasks"),
    }
}

fn open_subtasks(count: usize) -> String {
    match count {
        1 => "1 open subtask".to_owned(),
        count => format!("{count} open subtasks"),
    }
}

/// Each item from the top of the list down to `parent`, with the revision it is at: what an
/// add under `parent` is checked against. Empty for the top level; `None` when `parent` is
/// not in the list.
fn trail_revs(items: &[todos::Item], parent: Option<u64>) -> Option<Vec<(u64, DataRevision)>> {
    fn walk(items: &[todos::Item], id: u64, trail: &mut Vec<(u64, DataRevision)>) -> bool {
        for item in items {
            trail.push((item.id, item.rev));
            if item.id == id || walk(&item.children, id, trail) {
                return true;
            }
            trail.pop();
        }
        false
    }
    let Some(parent) = parent else {
        return Some(Vec::new());
    };
    let mut trail = Vec::new();
    walk(items, parent, &mut trail).then_some(trail)
}

/// The parent of `id` - `Some(None)` at the top level - or `None` when `id` is not in `items`.
fn parent_of(items: &[todos::Item], id: u64, parent: Option<u64>) -> Option<Option<u64>> {
    for item in items {
        if item.id == id {
            return Some(parent);
        }
        if let Some(found) = parent_of(&item.children, id, Some(item.id)) {
            return Some(found);
        }
    }
    None
}

fn todo_rows(list: &todos::List, show_done: bool) -> Vec<Row> {
    fn walk(items: &[todos::Item], depth: usize, show_done: bool, out: &mut Vec<Row>) {
        for item in items {
            if item.done && !show_done {
                continue;
            }
            out.push(Row {
                id: item.id,
                depth,
                text: item.text.clone(),
                done: item.done,
            });
            walk(&item.children, depth + 1, show_done, out);
        }
    }
    let mut rows = Vec::new();
    walk(&list.items, 0, show_done, &mut rows);
    rows
}

/// `rows` word-wrapped as markdown to `cols`, the cursor column included in the budget.
///
/// A continuation row is indented to where the item's text starts, so the checkbox column
/// stays clear.
fn todo_lines(rows: &[Row], cols: usize) -> Vec<TodoLine> {
    let mut out = Vec::new();
    for row in rows {
        let marker = format!(
            "{}[{}] ",
            "  ".repeat(row.depth),
            if row.done { "x" } else { " " }
        );
        let gutter = marker.width();
        let width = cols.saturating_sub(gutter + 1).max(1);
        let rendered = markdown::render(&row.text, width, markdown::Options::compact());
        for (index, line) in rendered.into_iter().enumerate() {
            out.push(TodoLine {
                id: row.id,
                line: match index {
                    0 => line.prepend(&marker, markdown::MdStyle::default()),
                    _ => line.indent(gutter),
                },
                first: index == 0,
                done: row.done,
            });
        }
    }
    out
}

/// The id to keep the cursor on once the visible rows go from `before` to `after`: `selected` if
/// it is still visible, else the nearest survivor after where it was, else the nearest before it,
/// else the first row. Scanning `before` skips a completed item's hidden children as well.
fn nearest_surviving(before: &[Row], after: &[Row], selected: Option<u64>) -> Option<u64> {
    let survives = |id: u64| after.iter().any(|row| row.id == id);
    if let Some(id) = selected.filter(|id| survives(*id)) {
        return Some(id);
    }
    selected
        .and_then(|id| before.iter().position(|row| row.id == id))
        .and_then(|at| {
            before[at + 1..]
                .iter()
                .chain(before[..at].iter().rev())
                .map(|row| row.id)
                .find(|id| survives(*id))
        })
        .or_else(|| after.first().map(|row| row.id))
}

fn collect_ids(items: &[todos::Item], ids: &mut Vec<u64>) {
    for item in items {
        ids.push(item.id);
        collect_ids(&item.children, ids);
    }
}

#[cfg(test)]
#[path = "unit/todos.rs"]
mod tests;

use super::*;
use hocus_proto::location::Location;
use hocus_proto::store::{self, Backend, Host};
use hocus_proto::{projects, todos};
use std::collections::HashMap;
use std::ops::Range;
use zellij_tile::prelude::{
    DataRevision, DataRows, DataWatchId, FetchRequest, Mutation, Pred, SortDir, WatchMode,
};
use zellij_tile::ui_components::forms::{self, TextInput};

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
    /// A box put away with text in it, by Esc or by the panel going away, for the key that
    /// opened it to bring back.
    draft: Option<(InputAction, TextInput)>,
    /// The drafts of the projects not on screen, by root: each one waits for its own list, the
    /// only one whose items it can be about, rather than being dropped by a look at another.
    parked_drafts: HashMap<String, (InputAction, TextInput)>,
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
            return self.surface == Surface::Todos;
        }
        if self.todos.todo_watch != Some(watch_id) {
            return false;
        }
        self.take_todo_rows(&Host::shared(), &rows);
        self.surface == Surface::Todos
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
            if let (Some(old), Some(draft)) =
                (self.todos.project_root.take(), self.todos.draft.take())
            {
                self.todos.parked_drafts.insert(old, draft);
            }
            self.todos.project_root = root.clone();
            self.todos.draft = root
                .as_ref()
                .and_then(|root| self.todos.parked_drafts.remove(root));
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
    /// instead, which the box says before Enter makes it one. A draft put away about one is
    /// kept the same way.
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
        if let (
            Some(why),
            Mode::Input {
                action, refusal, ..
            },
        ) = (why, &mut self.todos.mode)
        {
            *action = InputAction::Add { parent_id: None };
            *refusal = Some(why.to_owned());
        }
        let draft_orphaned = self
            .todos
            .draft
            .as_ref()
            .is_some_and(|(action, _)| orphaned(self, action).is_some());
        if let (true, Some((action, _))) = (draft_orphaned, &mut self.todos.draft) {
            *action = InputAction::Add { parent_id: None };
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
            self.yield_to_chat(wire::STOP_TABS);
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
                load_new_plugin("hocus-new-agent", configuration, false, false);
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
            .draft
            .as_ref()
            .is_some_and(|(kept, _)| kept.same_target(&action));
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
                ("Tab", "go to the conversation".into()),
                ("Shift+Tab", "go to the panel's tabs".into()),
                ("Esc", "hide the panel".into()),
            ],
            // `Input` for completeness only: see above.
            Mode::Normal | Mode::Input { .. } => {
                let mut rows = Vec::new();
                if let Some(key) = self.todo_draft_key() {
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
                    ("Tab", "go to the conversation".into()),
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
        let (action, input) = match self.todos.draft.take() {
            Some((kept, input)) if kept.same_target(&action) => (kept, input),
            other => {
                self.todos.draft = other;
                (action, TextInput::new(text.to_owned()))
            },
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
    /// An edit nobody has changed yet is not a draft, and does not take the place of one.
    fn close_todo_input(&mut self) {
        if let Mode::Input { action, input, .. } = std::mem::take(&mut self.todos.mode) {
            if is_a_draft(&self.todos.list, &action, &input) {
                self.todos.draft = Some((action.into_draft(), input));
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
            Applied::Refused(error) => self.refuse_todo_input(error),
            Applied::Conflict {
                reloaded: Err(error),
            } => self.refuse_todo_input(format!("changed elsewhere · could not reload: {error}")),
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
                        "or: zellij action project-remember NAME --root DIR".into(),
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
        // Enter and Esc step into the list rather than acting - and a key the bar does not own
        // goes to the list anyway. Only a question never has the bar over it, since the Tab
        // that goes up to the bar drops it first (`todo_confirm_key`). An open box is drawn in
        // this row's place wherever it fits, and its row names the bar's keys the same way
        // (`input_row`).
        let bar = self.bar_has_keys() && !self.box_ahead_of_the_bar();
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
        let label = agents::fit_hints(&segments, room);
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
        let draft = self.todo_draft_key();
        let mut hints = Vec::new();
        // Enter opens the item into the box as `e` does, and so brings back its draft too.
        match draft {
            Some("e") => hints.push("Enter/e resume draft".to_owned()),
            Some(key) => hints.push(format!("{key} resume draft")),
            None => {},
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
            if draft != Some("e") {
                hints.push("Enter/e edit".into());
            }
        }
        if draft != Some("a") {
            hints.push("a add".into());
        }
        if selected.is_some() {
            if draft != Some("A") {
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

    /// The key that brings the draft back from here, if one would: `a` for a new top-level
    /// todo, and `A` or `e` on the item a subtask or an edit was about.
    fn todo_draft_key(&self) -> Option<&'static str> {
        let (action, _) = self.todos.draft.as_ref()?;
        match action {
            InputAction::Add { parent_id: None } => Some("a"),
            InputAction::Add {
                parent_id: Some(parent),
            } if self.todos.selected == Some(*parent) => Some("A"),
            InputAction::Edit { id, .. } if self.todos.selected == Some(*id) => Some("e"),
            _ => None,
        }
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
        let hints: Vec<&'static str> = if self.bar_has_keys() && !self.box_ahead_of_the_bar() {
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
mod tests {
    use super::*;
    use zellij_tile::prelude::DataValue;

    fn item(id: u64, text: &str, done: bool, children: Vec<todos::Item>) -> todos::Item {
        todos::Item {
            id,
            text: text.into(),
            done,
            rev: DataRevision::new(id as i64),
            children,
        }
    }

    fn key(ch: char) -> KeyWithModifier {
        KeyWithModifier::new(BareKey::Char(ch))
    }

    fn bare(key: BareKey) -> KeyWithModifier {
        KeyWithModifier::new(key)
    }

    fn typed(state: &mut State, text: &str) {
        for ch in text.chars() {
            state.todo_key(key(ch));
        }
    }

    fn draft(state: &State) -> Option<&str> {
        match &state.todos.mode {
            Mode::Input { input, .. } => Some(input.get_text()),
            _ => None,
        }
    }

    fn refusal(state: &State) -> Option<&str> {
        match &state.todos.mode {
            Mode::Input { refusal, .. } => refusal.as_deref(),
            _ => None,
        }
    }

    fn project(name: &str, root: &str) -> projects::Project {
        projects::Project {
            name: name.into(),
            root: root.into(),
            firecracker_image: None,
            worktree_dir: None,
            remote: None,
            worktree_status: None,
            common_dir: None,
            discovered: false,
            replaced_ids: Vec::new(),
        }
    }

    /// A panel over a real store, open on /work and holding what `seed` put there.
    fn stored(seed: impl FnOnce(&store::Local)) -> (store::Local, State) {
        let backend = store::Local::shared("hocus-agents");
        backend.define_all(&store::shared_defs()).unwrap();
        seed(&backend);
        let mut state = State {
            surface: Surface::Todos,
            ..Default::default()
        };
        state.watch_todo_root_with(&backend, Some("/work".into()));
        (backend, state)
    }

    /// A todo written straight to the store, as another session would: this pane hears
    /// nothing of it until it reads the list again.
    fn put(backend: &impl Backend, text: &str, parent: Option<u64>) -> u64 {
        let mut put = Mutation::put("todo")
            .set("root", "/work")
            .set("text", text)
            .set("done", false);
        if let Some(parent) = parent {
            put = put.set("parent_id", parent);
        }
        backend.apply(vec![put.build()]).unwrap().ids[0].get() as u64
    }

    /// Another session rewording a todo behind this pane's back.
    fn retext(backend: &impl Backend, id: u64, text: &str) {
        backend
            .apply(vec![Mutation::update("todo", Pred::eq("id", id))
                .set("text", text)
                .build()])
            .unwrap();
    }

    fn texts(state: &State) -> Vec<String> {
        todo_rows(&state.todos.list, true)
            .into_iter()
            .map(|row| row.text)
            .collect()
    }

    #[test]
    fn starting_a_todo_prefills_its_project_and_open_work_without_changing_the_list() {
        let mut state = State::default();
        state.todos.initial_cwd = "/somewhere/else".into();
        state.todos.project_root = Some("/work".into());
        state.todos.list.items = vec![item(
            1,
            "Improve the build",
            false,
            vec![
                item(
                    2,
                    "Handle `quotes` and $(commands)\nKeep each argument intact",
                    false,
                    vec![
                        item(3, "Already finished", true, vec![]),
                        item(
                            4,
                            "Cover multiline prompts",
                            false,
                            vec![item(5, "Check newlines", false, vec![])],
                        ),
                    ],
                ),
                item(6, "Unrelated sibling", false, vec![]),
            ],
        )];
        state.todos.selected = Some(2);
        let before = state.todos.list.clone();
        let configuration = state.todo_agent_configuration().unwrap();
        assert_eq!(configuration["caller_cwd"], "/work");
        assert_eq!(configuration["overlay"], "true");
        assert_eq!(configuration["modal"], "true");
        assert_eq!(
            configuration["initial_prompt"],
            "Todo #2: Handle `quotes` and $(commands)\nKeep each argument intact\n\nParent context (for reference):\n- Improve the build\n\nOpen subtasks:\n- [ ] Cover multiline prompts\n  - [ ] Check newlines\n\nImplement the task, run relevant checks, and summarize the changes and anything still outstanding.",
        );
        assert!(state.todo_key(key('n')));
        assert_eq!(state.todos.list, before);
        // The panel stays up under the launcher, so that its Esc hands the keys back here.
        assert_eq!(state.panel_hides, 0);
        state.todos.selected = None;
        assert!(!state.todo_key(key('n')));
        state.todos.selected = Some(999);
        assert!(state.todo_agent_configuration().is_none());
    }

    #[test]
    fn a_truncated_watch_loads_the_complete_list_and_keeps_it_on_fetch_failure() {
        let backend = store::Local::shared("hocus-agents");
        backend.define_all(&store::shared_defs()).unwrap();
        backend
            .apply(
                (0..1200)
                    .map(|id| {
                        Mutation::put("todo")
                            .set("root", "/work")
                            .set("text", format!("item {id}"))
                            .set("done", true)
                            .build()
                    })
                    .collect(),
            )
            .unwrap();
        let rows = backend
            .fetch(&FetchRequest::entity("todo").limit(store::LIST_FETCH_LIMIT))
            .unwrap();
        assert!(rows.truncated);
        let mut state = State::default();
        state.todos.project_root = Some("/work".into());
        state.take_todo_rows(&backend, &rows);
        assert_eq!(todos::counts(&state.todos.list), (0, 1200));
        let tail = state.todos.list.items.last().unwrap().id;
        let edit = todos::plan_edit(&state.todos.list, tail, "edited", None)
            .unwrap()
            .0;
        assert!(matches!(
            state.apply_todo_plan(&backend, Ok(edit)),
            Applied::Saved(_)
        ));
        assert_eq!(
            todos::item_of(&state.todos.list, tail).unwrap().text,
            "edited"
        );
        let delete = todos::plan_delete(&state.todos.list, "/work", tail, false)
            .unwrap()
            .0;
        state.apply_todo_plan(&backend, Ok(delete));
        assert!(todos::item_of(&state.todos.list, tail).is_none());
        let add = todos::plan_add(&state.todos.list, "/work", "new", None)
            .unwrap()
            .0;
        state.apply_todo_plan(&backend, Ok(add));
        assert_eq!(todos::counts(&state.todos.list), (1, 1200));
        let before = state.todos.list.clone();
        state.take_todo_rows(&Host::shared(), &rows);
        assert!(!state.todos.status.is_empty());
        assert_eq!(state.todos.list, before);
        // A fetch that works again is past it.
        state.take_todo_rows(&backend, &rows);
        assert!(state.todos.status.is_empty());
    }

    #[test]
    fn completed_subtrees_are_hidden_and_move_targets_exclude_the_source_subtree() {
        let list = todos::List {
            items: vec![
                item(1, "parent", false, vec![item(2, "child", false, vec![])]),
                item(3, "done", true, vec![item(4, "done child", true, vec![])]),
                item(5, "other", false, vec![]),
            ],
            ..Default::default()
        };
        assert_eq!(
            todo_rows(&list, false)
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            [1, 2, 5]
        );
        let mut state = State::default();
        state.todos.list = list;
        assert_eq!(state.move_targets(1), [5]);
        state.todos.show_done = true;
        assert_eq!(state.move_targets(1), [3, 4, 5]);
    }

    fn two_todos() -> State {
        let mut state = State {
            surface: Surface::Todos,
            ..Default::default()
        };
        state.todos.project_root = Some("/work".into());
        state.todos.list.items = vec![item(1, "one", false, vec![]), item(2, "two", false, vec![])];
        state.todos.selected = Some(1);
        state
    }

    /// A tab click, like the bar it no longer focuses, hands the keys to the list: `j` walks
    /// it, and a list key pressed on the bar does its job rather than vanishing.
    #[test]
    fn a_tab_click_or_a_key_off_the_bar_reaches_the_todo_list() {
        let mut state = two_todos();
        state.render(12, 40);
        let x = state.tab_cells[1].x;
        assert!(state.on_mouse(Mouse::LeftClick(0, x)));
        assert!(!state.tabs_focused);
        assert!(state.on_key(key('j')));
        assert_eq!(state.todos.selected, Some(2));

        state.on_key(bare(BareKey::Tab).with_shift_modifier());
        assert!(state.tabs_focused);
        assert!(state.on_key(key('a')));
        assert!(!state.tabs_focused);
        assert!(matches!(state.todos.mode, Mode::Input { .. }));

        // Ctrl-o is the panel's, taken before the box that answers every other key.
        state.on_key(key('x'));
        assert!(!state.on_key(key('o').with_ctrl_modifier()));
        assert_eq!(draft(&state), Some("x"));
    }

    /// While the tab bar has the keys the divider says what they do there, in place of the
    /// list's keys - on the row the list already gives it, not on one more - and a note or a
    /// problem still leads it.
    #[test]
    fn the_bar_hint_takes_the_divider_not_a_row() {
        let mut state = two_todos();
        state.render_todos(12, 60);
        let rows = state.todos.last_list_rows;
        state.tabs_focused = true;
        state.render_todos(12, 60);
        assert_eq!(state.todos.last_list_rows, rows);
        let line = state.todo_divider(60, 0, 0).line;
        assert!(line.contains("←/→ switch"), "{line}");
        // Enter on the bar steps into the list rather than editing.
        assert!(!line.contains("Enter/e edit"), "{line}");
        state.todos.status = "boom".into();
        let line = state.todo_divider(80, 0, 0).line;
        assert!(line.contains("boom · Ctrl-r retry · ←/→ switch"), "{line}");
        state.todos.status.clear();
        // Nor while another pane has the keyboard: the bar does not have it either.
        state.has_keyboard = Some(false);
        let line = state.todo_divider(60, 0, 0).line;
        assert!(
            line.contains("Space done") && !line.contains("←/→"),
            "{line}"
        );
        state.has_keyboard = None;

        // A move and a project pick keep across the Shift+Tab up to the bar, whose Enter and
        // Esc step back into the list rather than nesting or cancelling: the divider names
        // the bar's keys over them too - the move still what it is moving - and the mode's
        // own keys once the keys are back.
        for (arm, kept) in [('m', "moving \"one\""), ('p', "")] {
            assert!(state.on_key(key(arm)));
            let own = state.todo_divider(80, 0, 0).line;
            assert!(
                own.contains("Esc cancel") || own.contains("Esc back"),
                "{own}"
            );
            assert!(state.on_key(bare(BareKey::Tab).with_shift_modifier()));
            assert!(state.tabs_focused);
            assert!(!matches!(state.todos.mode, Mode::Normal), "{arm:?}");
            let line = state.todo_divider(80, 0, 0).line;
            assert!(line.contains("←/→ switch") && line.contains(kept), "{line}");
            assert!(
                !line.contains("Esc cancel") && !line.contains("Esc back"),
                "{line}"
            );
            assert!(state.on_key(bare(BareKey::Esc)));
            assert!(!state.tabs_focused);
            assert_eq!(state.todo_divider(80, 0, 0).line, own);
            assert!(state.on_key(bare(BareKey::Esc)));
            assert!(matches!(state.todos.mode, Mode::Normal), "{arm:?}");
        }
    }

    /// A box kept open across the Shift+Tab up to the bar is drawn in the divider's place, so
    /// its row is where the bar's keys go: the box's own would say Enter saves and Esc closes,
    /// and on the bar both step back down into the box instead. The draft stays on the row,
    /// the row under the list the draft pushes the keys onto names the bar's too, and the
    /// box's keys come back with the keys.
    #[test]
    fn the_bar_hint_takes_an_open_boxs_row_too() {
        let mut state = two_todos();
        state.render_todos(12, 60);
        assert!(state.on_key(key('a')));
        for ch in "Fix".chars() {
            state.on_key(key(ch));
        }
        state.render_todos(12, 60);
        let rows = state.todos.last_list_rows;
        let own = state.input_divider(60).unwrap().0.line;
        assert!(own.contains("Fix") && own.contains("Enter save"), "{own}");

        assert!(state.on_key(bare(BareKey::Tab).with_shift_modifier()));
        assert!(state.tabs_focused);
        assert!(matches!(state.todos.mode, Mode::Input { .. }));
        let line = state.input_divider(60).unwrap().0.line;
        assert!(
            line.contains("Fix") && line.contains("←/→ switch"),
            "{line}"
        );
        let spilled = state.input_hint_row(60).unwrap_or_default();
        let narrow = state.input_hint_row(24).unwrap_or_default();
        for text in [&line, &spilled, &narrow] {
            for gone in ["Enter save", "Esc keep draft", "Esc cancel", "Alt+Enter"] {
                assert!(!text.contains(gone), "{gone} in {text}");
            }
        }
        assert!(spilled.contains("←/→ switch"), "{spilled}");
        assert!(narrow.contains("←/→ switch"), "{narrow}");
        // Both sets of keys spill past a draft this long at this width, so the list keeps its
        // rows across the Shift+Tab.
        state.render_todos(12, 60);
        assert_eq!(state.todos.last_list_rows, rows);

        assert!(state.on_key(bare(BareKey::Esc)));
        assert!(!state.tabs_focused);
        assert!(matches!(state.todos.mode, Mode::Input { .. }));
        assert_eq!(draft(&state), Some("Fix"));
        assert_eq!(state.input_divider(60).unwrap().0.line, own);
    }

    /// Putting the panel away drops a question or a pick left open. A box is put away as a
    /// draft, which the key that opened it brings back - not left open to take the first
    /// letters of the next visit.
    #[test]
    fn a_hide_drops_todo_questions_and_keeps_the_draft() {
        let mut state = two_todos();
        state.todos.list.items[0].children = vec![item(3, "three", false, vec![])];
        for arm in ['d', 'm', 'p', ' '] {
            state.on_key(key(arm));
            assert!(!matches!(state.todos.mode, Mode::Normal), "{arm:?}");
            state.hide_the_panel();
            assert!(matches!(state.todos.mode, Mode::Normal), "{arm:?}");
        }
        state.on_key(key('a'));
        state.on_key(key('x'));
        state.hide_the_panel();
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert!(state.todo_divider(80, 0, 0).line.contains("a resume draft"));
        state.on_key(key('j'));
        assert!(matches!(state.todos.mode, Mode::Normal));
        state.on_key(key('a'));
        assert_eq!(draft(&state), Some("x"));
    }

    #[test]
    fn todo_modes_capture_the_item_and_accept_shifted_text() {
        let mut state = State::default();
        state.todos.project_root = Some("/work".into());
        state.todos.list.items = vec![
            item(1, "parent", false, vec![item(2, "child", false, vec![])]),
            item(3, "other", false, vec![]),
        ];
        state.todos.selected = Some(1);

        assert!(state.todo_key(key('A')));
        assert!(matches!(
            state.todos.mode,
            Mode::Input {
                action: InputAction::Add { parent_id: Some(1) },
                ..
            }
        ));
        state.todo_key(key('X').with_shift_modifier());
        assert_eq!(draft(&state), Some("X"));
        state.todo_key(bare(BareKey::Esc));

        state.todo_key(key('d'));
        assert!(matches!(
            state.todos.mode,
            Mode::Delete {
                id: 1,
                descendants: 1,
                ..
            }
        ));
        state.todo_key(key('n'));
        assert!(matches!(state.todos.mode, Mode::Normal));

        state.todo_key(key('m'));
        assert!(matches!(
            state.todos.mode,
            Mode::Move {
                id: 1,
                rev,
                target: Some(3)
            } if rev == DataRevision::new(1)
        ));
    }

    /// Rows as a watch delivers them: ids `1..=count` under /work, each one under 1 but 1.
    fn watched(count: i64) -> DataRows {
        DataRows {
            columns: ["id", "root", "text", "parent_id", "rev"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            rows: (1..=count)
                .map(|id| {
                    vec![
                        DataValue::Int(id),
                        DataValue::Text("/work".into()),
                        DataValue::Text(format!("todo {id}")),
                        if id == 1 {
                            DataValue::Null
                        } else {
                            DataValue::Int(1)
                        },
                        DataValue::Int(id),
                    ]
                })
                .collect(),
            truncated: false,
            next_cursor: None,
        }
    }

    fn parent_and_child() -> State {
        let mut state = State {
            surface: Surface::Todos,
            ..Default::default()
        };
        state.todos.project_root = Some("/work".into());
        state.todos.todo_watch = Some(DataWatchId::new(1));
        state.todos.list.items = vec![item(
            1,
            "parent",
            false,
            vec![item(2, "child", false, vec![])],
        )];
        state.todos.selected = Some(1);
        state
    }

    #[test]
    fn delete_confirmation_keeps_its_plan_across_watch_updates() {
        let mut state = parent_and_child();
        let expected = todos::plan_delete(&state.todos.list, "/work", 1, true)
            .unwrap()
            .0;
        state.todo_key(key('d'));
        assert!(state.todo_rows_changed(DataWatchId::new(1), watched(3)));
        assert_eq!(state.todos.list.items[0].children.len(), 2);
        let Mode::Delete {
            plan, descendants, ..
        } = &state.todos.mode
        else {
            panic!("confirmation was discarded");
        };
        assert_eq!(*descendants, 1);
        assert_eq!(*plan, expected);
        assert_ne!(
            *plan,
            todos::plan_delete(&state.todos.list, "/work", 1, true)
                .unwrap()
                .0
        );
    }

    /// The question answered is the one that was asked: a subtask arriving while it is up
    /// does not join what `y` completes.
    #[test]
    fn complete_confirmation_keeps_its_plan_across_watch_updates() {
        let mut state = parent_and_child();
        let expected = todos::plan_set_done(&state.todos.list, "/work", 1, true)
            .unwrap()
            .0;
        state.todo_key(key(' '));
        assert!(state.todo_rows_changed(DataWatchId::new(1), watched(3)));
        let Mode::Complete { plan, cascaded, .. } = &state.todos.mode else {
            panic!("confirmation was discarded");
        };
        assert_eq!(*cascaded, 1);
        assert_eq!(*plan, expected);
        // The item it was about going is another matter: nothing is left to ask about.
        assert!(state.todo_rows_changed(
            DataWatchId::new(1),
            DataRows {
                rows: Vec::new(),
                ..watched(0)
            }
        ));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert_eq!(state.todos.note.as_ref().unwrap().lead, GONE);
    }

    /// Enter opens the selected item, as it does on every other tab - into the box, here -
    /// and never completes it.
    #[test]
    fn enter_opens_the_todo_for_editing_instead_of_completing_it() {
        let mut state = two_todos();
        assert!(state.todo_key(bare(BareKey::Enter)));
        assert!(matches!(
            &state.todos.mode,
            Mode::Input {
                action: InputAction::Edit { id: 1, .. },
                ..
            }
        ));
        assert_eq!(draft(&state), Some("one"));
        assert!(!state.todos.list.items[0].done);
        assert!(state.todos.note.is_none());
        assert!(state.todo_divider(80, 0, 0).line.contains("Enter/e edit"));
    }

    /// Space on a todo with open work under it asks first, leading with the answer and the
    /// count so a narrow pane still shows both; anything but `y` writes nothing.
    #[test]
    fn completing_a_parent_with_open_subtasks_asks_first() {
        let (backend, mut state) = stored(|backend| {
            let parent = put(backend, "Fix resize", None);
            put(backend, "open child", Some(parent));
            let done = put(backend, "done child", Some(parent));
            backend
                .apply(vec![Mutation::update("todo", Pred::eq("id", done))
                    .set("done", true)
                    .build()])
                .unwrap();
        });
        let parent = state.todos.selected.unwrap();
        assert!(state.todo_key_with(&backend, key(' ')));
        assert!(matches!(
            state.todos.mode,
            Mode::Complete { id, cascaded: 1, .. } if id == parent
        ));
        let question = state.todo_divider(24, 5, 5);
        assert_eq!(question.line.width(), 24);
        assert!(
            question.line.contains("y/n complete with 1"),
            "{}",
            question.line
        );
        assert!(question.error);

        assert!(state.todo_key_with(&backend, key('n')));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert!(state.todos.note.is_none());
        assert_eq!(todos::counts(&state.todos.list), (2, 3));

        state.todo_key_with(&backend, key(' '));
        assert!(state.todo_key_with(&backend, key('y')));
        assert_eq!(todos::counts(&state.todos.list), (0, 3));
        let note = state.todos.note.as_ref().unwrap();
        assert_eq!(note.text(60), "completed \"Fix resize\" +1 subtask");
        assert_eq!(note.hint, "v shows done");
    }

    #[test]
    fn completing_a_leaf_needs_no_confirmation() {
        let (backend, mut state) = stored(|backend| {
            put(backend, "leaf", None);
        });
        let leaf = state.todos.selected.unwrap();
        assert!(state.todo_key_with(&backend, key(' ')));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert!(todos::item_of(&state.todos.list, leaf).unwrap().done);
        assert_eq!(
            state.todos.note.as_ref().unwrap().text(60),
            "completed \"leaf\""
        );
    }

    /// A subtree's delete question keeps its `y/n` and its size on the default panel's
    /// narrowest half, with no scroll readout crowding it.
    #[test]
    fn a_delete_question_leads_with_its_answer_and_its_size() {
        let mut state = parent_and_child();
        state.todos.list.items[0].children[0].children = vec![item(3, "grandchild", true, vec![])];
        state.todo_key(key('d'));
        let question = state.todo_divider(24, 7, 3);
        assert!(
            question.line.starts_with("─ y/n delete with 2"),
            "{}",
            question.line
        );
        assert!(!question.line.contains('↑'));
        assert!(question.error);
        state.todo_key(key('n'));
        state.todos.selected = Some(2);
        state.todos.list.items[0].children[0].children.clear();
        state.todo_key(key('d'));
        assert!(state
            .todo_divider(24, 0, 0)
            .line
            .contains("y/n delete this todo"));
    }

    #[test]
    fn delete_planning_errors_do_not_open_confirmation() {
        let mut state = State::default();
        state.todos.project_root = Some("/work".into());
        state.todos.selected = Some(1);
        assert!(state.todo_key(key('d')));
        assert!(matches!(state.todos.mode, Mode::Normal));
        // In the panel's words, not the model's `call todo_list`.
        assert_eq!(state.todos.note.as_ref().unwrap().lead, GONE);
    }

    /// A paste is typing: it goes in at the caret, and a pasted list stays a list, drawn on
    /// the box's one row with a `⏎` for each break rather than a break that scrolls the pane.
    #[test]
    fn a_paste_lands_at_the_caret_and_keeps_its_line_breaks() {
        let (backend, mut state) = stored(|_| {});
        assert!(!state.todo_paste("ignored"));
        state.todo_key_with(&backend, key('a'));
        typed(&mut state, "Fix ");
        assert!(state.todo_paste("flaky\r\nresize\ttest\n"));
        assert_eq!(draft(&state), Some("Fix flaky\nresize test"));
        let (row, _) = state.input_divider(60).unwrap();
        assert!(row.line.contains("Fix flaky⏎resize test"), "{}", row.line);
        assert!(!row.line.chars().any(char::is_control));
        state.todo_key_with(&backend, bare(BareKey::Home));
        assert!(state.todo_paste("Now: "));
        assert_eq!(draft(&state), Some("Now: Fix flaky\nresize test"));
        assert!(state.todo_key_with(&backend, bare(BareKey::Enter)));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert_eq!(texts(&state), ["Now: Fix flaky\nresize test"]);
    }

    /// Whatever the width, the box fits it, the caret is on it and the end of a draft being
    /// typed is what shows - and the keys follow the draft only once all of it fits.
    #[test]
    fn the_input_label_fits_every_width_and_keeps_the_caret_on_screen() {
        let draft: String = (0..60).map(|at| char::from(b'a' + at % 26)).collect();
        for room in 1..=80 {
            let (label, caret, text, appended) =
                input_label("new: ", &draft, 60, &["Enter save", "Esc keep draft"], room);
            assert!(label.width() <= room, "{room}: {label}");
            assert!(caret < room, "{room}: {label}");
            if room > "new: ".width() + 1 {
                let shown: String = label.chars().skip(text.start).take(text.len()).collect();
                assert!(
                    !shown.is_empty() && draft.ends_with(&shown),
                    "{room}: {label}"
                );
            }
            assert_eq!(label.contains("Enter save"), room >= 78, "{room}: {label}");
            assert_eq!(appended, label.matches(" · ").count(), "{room}: {label}");
        }
    }

    #[test]
    fn the_box_shows_the_end_of_a_long_draft_with_the_caret_after_it() {
        let mut state = two_todos();
        state.todo_key(key('a'));
        typed(&mut state, "Investigate flaky resize integration test");
        state.render_todos(12, 24);
        let (x, y) = state.todo_input_cursor(12, 24).unwrap();
        assert_eq!(y, 12, "the divider, the pane's last row under the tab bar");
        let (row, caret) = state.input_divider(24).unwrap();
        assert_eq!(x, caret);
        assert_eq!(row.line.width(), 24);
        let before: String = row
            .line
            .chars()
            .scan(0, |width, ch| {
                *width += ch.to_string().width();
                (*width <= caret).then_some(ch)
            })
            .collect();
        assert!(before.starts_with("─ new: "), "{}", row.line);
        assert!(before.ends_with("test"), "{}", row.line);
        // The typed text is the bright part; the prompt and the rule are dim.
        let bright = row.bright.unwrap();
        let text: String = row
            .line
            .chars()
            .skip(bright.start)
            .take(bright.len())
            .collect();
        assert!(text.ends_with("test") && !text.contains("new"), "{text}");
        assert_eq!(state.todo_input_cursor(12, 3), None);
        state.todo_key(bare(BareKey::Esc));
        assert_eq!(state.todo_input_cursor(12, 24), None);
    }

    /// A draft too long for the box's row to carry its keys gets a row for them above it, from
    /// the list's, and gives it back once they fit again; a refusal says what to press instead.
    #[test]
    fn the_box_keys_get_a_row_of_their_own_when_the_draft_leaves_no_room() {
        let mut state = two_todos();
        state.render_todos(12, 24);
        let normal = state.todos.last_list_rows;
        state.todo_key(key('a'));
        typed(&mut state, "Fix");
        state.render_todos(12, 24);
        let keys = state.input_hint_row(24).unwrap();
        assert!(keys.contains("Enter save"), "{keys}");
        assert!(!keys.contains("? keys"), "{keys}");
        assert_eq!(state.todos.last_list_rows, normal - 1);
        // The caret stays on the box, the pane's last row under the tab bar.
        assert_eq!(state.todo_input_cursor(12, 24).map(|(_, y)| y), Some(12));
        // `Enter save · Esc keep draft` is 27 columns: at 24 the row holds the first alone.
        let keys = state.input_hint_row(28).unwrap();
        assert!(
            keys.contains("Enter save") && keys.contains("Esc keep draft"),
            "{keys}"
        );

        state.render_todos(12, 80);
        assert_eq!(state.input_hint_row(80), None);
        assert_eq!(state.todos.last_list_rows, normal);

        state.refuse_todo_input("the list changed · Enter tries again".into());
        state.render_todos(12, 24);
        assert_eq!(
            state.todos.last_list_rows,
            normal - 2,
            "the refusal's two rows"
        );
    }

    #[test]
    fn edit_moves_the_caret_to_fix_a_mid_text_typo() {
        let (backend, mut state) = stored(|backend| {
            put(backend, "Fix teh resize bug", None);
        });
        state.todo_key_with(&backend, key('e'));
        state.todo_key_with(&backend, bare(BareKey::Home));
        for _ in 0..7 {
            state.todo_key_with(&backend, bare(BareKey::Right));
        }
        for _ in 0..2 {
            state.todo_key_with(&backend, bare(BareKey::Backspace));
        }
        typed(&mut state, "he");
        assert_eq!(draft(&state), Some("Fix the resize bug"));
        state.todo_key_with(&backend, bare(BareKey::Enter));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert_eq!(texts(&state), ["Fix the resize bug"]);
        assert_eq!(
            state.todos.note.as_ref().unwrap().text(60),
            "saved \"Fix the resize bug\""
        );
    }

    #[test]
    fn word_keys_edit_by_the_word_and_alt_letters_type_nothing() {
        let mut state = two_todos();
        state.todo_key(key('a'));
        typed(&mut state, "one two three");
        state.todo_key(key('w').with_ctrl_modifier());
        assert_eq!(draft(&state), Some("one two "));
        state.todo_key(bare(BareKey::Backspace).with_alt_modifier());
        assert_eq!(draft(&state), Some("one "));
        state.todo_key(key('x').with_alt_modifier());
        assert_eq!(draft(&state), Some("one "));
        state.todo_key(key('u').with_ctrl_modifier());
        assert_eq!(draft(&state), Some(""));
    }

    /// Alt+Enter and Shift+Enter are the conversation's keys for a line break, and here too;
    /// only a bare Enter saves. The break shows as `⏎`, never as a raw line feed.
    #[test]
    fn alt_enter_starts_a_new_line_shown_as_a_return_symbol() {
        let (backend, mut state) = stored(|backend| {
            put(backend, "a\nb", None);
        });
        state.todo_key_with(&backend, key('e'));
        state.todo_key_with(&backend, bare(BareKey::Backspace));
        assert_eq!(draft(&state), Some("a\n"));
        state.todo_key_with(&backend, bare(BareKey::Enter).with_alt_modifier());
        state.todo_key_with(&backend, bare(BareKey::Enter).with_shift_modifier());
        typed(&mut state, "c");
        assert_eq!(draft(&state), Some("a\n\n\nc"));
        let (row, _) = state.input_divider(40).unwrap();
        assert!(row.line.contains("a⏎⏎⏎c"), "{}", row.line);
        assert!(!row.line.chars().any(char::is_control));
        state.todo_key_with(&backend, bare(BareKey::Enter));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert_eq!(texts(&state), ["a\n\n\nc"]);
    }

    /// An add guards the whole project, so another session touching any item refuses it. One
    /// that left the add's own parent alone is no reason to stop: it is planned again against
    /// the fresh list and saved, and the new todo is selected.
    #[test]
    fn an_add_survives_an_unrelated_edit_from_another_writer() {
        let (backend, mut state) = stored(|backend| {
            put(backend, "one", None);
            put(backend, "two", None);
        });
        let two = state.todos.list.items[1].id;
        retext(&backend, two, "two, edited elsewhere");
        state.todo_key_with(&backend, key('a'));
        typed(&mut state, "three");
        state.todo_key_with(&backend, bare(BareKey::Enter));
        assert!(
            matches!(state.todos.mode, Mode::Normal),
            "{:?}",
            refusal(&state)
        );
        assert_eq!(texts(&state), ["one", "two, edited elsewhere", "three"]);
        let three = state.todos.list.items[2].id;
        assert_eq!(state.todos.selected, Some(three));
        assert_eq!(
            state.todos.note.as_ref().unwrap().text(60),
            "added \"three\""
        );
    }

    /// A change to what the add hangs from is a fresh decision for the user: the list is
    /// reloaded under the box, which keeps its text, and the next Enter saves against it.
    #[test]
    fn a_conflicting_add_reloads_the_list_and_keeps_the_typed_text() {
        let (backend, mut state) = stored(|backend| {
            put(backend, "parent", None);
        });
        let parent = state.todos.selected.unwrap();
        state.todo_key_with(&backend, key('A'));
        typed(&mut state, "child");
        retext(&backend, parent, "parent, renamed elsewhere");
        state.todo_key_with(&backend, bare(BareKey::Enter));
        assert_eq!(draft(&state), Some("child"));
        assert!(refusal(&state).unwrap().contains("changed elsewhere"));
        assert_eq!(texts(&state), ["parent, renamed elsewhere"]);
        state.todo_key_with(&backend, bare(BareKey::Enter));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert_eq!(texts(&state), ["parent, renamed elsewhere", "child"]);
    }

    /// An edit is never taken over quietly: the other writer's text goes on screen, Enter
    /// overwrites it because the user said so, and Esc keeps theirs - and the user's words as
    /// a draft.
    #[test]
    fn an_edit_conflict_asks_before_overwriting() {
        for overwrite in [true, false] {
            let (backend, mut state) = stored(|backend| {
                put(backend, "mine", None);
            });
            let id = state.todos.selected.unwrap();
            state.todo_key_with(&backend, key('e'));
            typed(&mut state, " and more");
            retext(&backend, id, "theirs");
            state.todo_key_with(&backend, bare(BareKey::Enter));
            assert_eq!(draft(&state), Some("mine and more"));
            assert!(refusal(&state).unwrap().contains("Enter overwrites"));
            assert!(refusal(&state).unwrap().contains("yours stays a draft"));
            assert_eq!(texts(&state), ["theirs"]);
            // Over the box, wrapped rather than cut, so a narrow pane still names both keys -
            // and gives up the words about the draft, which the box shows anyway, first.
            let segments: Vec<&str> = refusal(&state).unwrap().split(" · ").collect();
            assert_eq!(
                wrap_segments(&segments, 24, 3),
                ["changed elsewhere", "Enter overwrites", "Esc keeps theirs"]
            );
            let wide = wrap_segments(&segments, 40, 3);
            assert_eq!(wide.len(), 2, "{wide:?}");
            assert!(wide[1].ends_with("yours stays a draft"), "{wide:?}");
            state.render_todos(12, 24);
            assert_eq!(state.todos.last_list_rows, 12 - 1 - 3 - 1);
            if overwrite {
                state.todo_key_with(&backend, bare(BareKey::Enter));
                assert!(matches!(state.todos.mode, Mode::Normal));
                assert_eq!(texts(&state), ["mine and more"]);
            } else {
                state.todo_key_with(&backend, bare(BareKey::Esc));
                assert_eq!(texts(&state), ["theirs"]);
                // Enter opens the item into the box as `e` does, draft and all.
                assert!(state
                    .todo_divider(80, 0, 0)
                    .line
                    .contains("Enter/e resume draft"));
                state.todo_key_with(&backend, key('e'));
                assert_eq!(draft(&state), Some("mine and more"));
            }
        }
    }

    /// On a pane too short for all of an edit conflict's refusal the project's row gives way
    /// to it first, and an Enter whose `Enter overwrites` was never on screen overwrites
    /// nothing.
    #[test]
    fn an_edit_conflict_on_a_short_pane_never_overwrites_unseen() {
        for (rows, cols) in [(3, 24), (4, 24), (5, 24), (6, 24), (3, 40), (2, 40)] {
            let (backend, mut state) = stored(|backend| {
                put(backend, "mine", None);
            });
            let id = state.todos.selected.unwrap();
            state.todo_key_with(&backend, key('e'));
            typed(&mut state, " and more");
            retext(&backend, id, "theirs");
            state.todo_key_with(&backend, bare(BareKey::Enter));
            assert!(refusal(&state).is_some(), "{rows}x{cols}");
            state.render_todos(rows, cols);
            match (rows, cols) {
                (5, 24) => assert_eq!(state.todos.last_header_rows, 0),
                (6, 24) => assert_eq!(state.todos.last_header_rows, 1),
                _ => {},
            }
            let shown = !(rows == 2 || (rows == 3 && cols == 24));
            assert_eq!(state.todos.overwrite_shown, shown, "{rows}x{cols}");
            state.todo_key_with(&backend, bare(BareKey::Enter));
            if shown {
                assert!(matches!(state.todos.mode, Mode::Normal), "{rows}x{cols}");
                assert_eq!(texts(&state), ["mine and more"], "{rows}x{cols}");
            } else {
                assert_eq!(draft(&state), Some("mine and more"), "{rows}x{cols}");
                assert_eq!(texts(&state), ["theirs"], "{rows}x{cols}");
            }
        }
    }

    #[test]
    fn esc_keeps_a_draft_that_a_restores() {
        let mut state = two_todos();
        state.todo_key(key('a'));
        typed(&mut state, "half");
        state.todo_key(bare(BareKey::Esc));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert!(state.todo_divider(80, 0, 0).line.contains("a resume draft"));
        // An edit opened and put away unchanged is not a draft, and does not replace one.
        state.todo_key(key('e'));
        assert_eq!(draft(&state), Some("one"));
        state.todo_key(bare(BareKey::Esc));
        state.todo_key(key('A'));
        assert_eq!(draft(&state), Some(""), "a subtask is another entry");
        state.todo_key(bare(BareKey::Esc));
        state.todo_key(key('a'));
        assert_eq!(draft(&state), Some("half"));
        // Saving it spends it.
        state.todo_key(bare(BareKey::Esc));
        assert!(state.todos.draft.is_some());
        state.todo_key(key('a'));
        assert!(state.todos.draft.is_none());
    }

    /// A box still holding the text it opened with is no draft, even once the item has been
    /// reworded elsewhere since: Esc puts it away with nothing kept, and `e` opens the new text.
    #[test]
    fn an_untouched_edit_is_no_draft_after_the_item_changes_elsewhere() {
        let (backend, mut state) = stored(|backend| {
            put(backend, "one", None);
        });
        let id = state.todos.selected.unwrap();
        state.todo_key_with(&backend, key('e'));
        retext(&backend, id, "uno");
        state.reload_todos(&backend).unwrap();
        assert_eq!(draft(&state), Some("one"));
        let (row, _) = state.input_divider(80).unwrap();
        assert!(row.line.contains("Esc cancel"), "{}", row.line);
        state.todo_key_with(&backend, bare(BareKey::Esc));
        assert!(state.todos.draft.is_none());
        assert!(state.todo_divider(80, 0, 0).line.contains("Enter/e edit"));
        state.todo_key_with(&backend, key('e'));
        assert_eq!(draft(&state), Some("uno"));
    }

    /// A draft is its project's: a look at another one neither shows it nor drops it, and it
    /// is back when its project is - as a new todo, if its item went meanwhile.
    #[test]
    fn a_draft_kept_by_esc_waits_for_its_project() {
        let (backend, mut state) = stored(|backend| {
            put(backend, "one", None);
        });
        let one = state.todos.selected.unwrap();
        state.todo_key_with(&backend, key('a'));
        typed(&mut state, "half");
        state.todo_key_with(&backend, bare(BareKey::Esc));
        state.watch_todo_root_with(&backend, Some("/other".into()));
        assert!(!state.todo_divider(80, 0, 0).line.contains("resume draft"));
        state.todo_key_with(&backend, key('a'));
        assert_eq!(draft(&state), Some(""));
        state.todo_key_with(&backend, bare(BareKey::Esc));
        state.watch_todo_root_with(&backend, Some("/work".into()));
        assert!(state.todo_divider(80, 0, 0).line.contains("a resume draft"));
        state.todo_key_with(&backend, key('a'));
        assert_eq!(draft(&state), Some("half"));
        state.todo_key_with(&backend, bare(BareKey::Esc));

        state.todos.selected = Some(one);
        state.todo_key_with(&backend, key('e'));
        typed(&mut state, " more");
        state.todo_key_with(&backend, bare(BareKey::Esc));
        state.watch_todo_root_with(&backend, Some("/other".into()));
        backend
            .apply(vec![Mutation::delete("todo", Pred::eq("id", one))])
            .unwrap();
        state.watch_todo_root_with(&backend, Some("/work".into()));
        assert!(state.todo_divider(80, 0, 0).line.contains("a resume draft"));
        state.todo_key_with(&backend, key('a'));
        assert_eq!(draft(&state), Some("one more"));
    }

    #[test]
    fn blank_enter_closes_the_box_without_an_error() {
        let mut state = two_todos();
        for blank in ["", "   "] {
            state.todo_key(key('a'));
            typed(&mut state, blank);
            assert!(state.todo_key(bare(BareKey::Enter)));
            assert!(matches!(state.todos.mode, Mode::Normal));
            assert!(state.todos.note.is_none());
            assert!(state.todos.draft.is_none());
        }
    }

    #[test]
    fn add_is_refused_at_the_open_cap_before_the_box_opens() {
        let mut state = two_todos();
        state.todos.list.items = (1..=todos::MAX_OPEN as u64)
            .map(|id| item(id, "open", false, vec![]))
            .collect();
        assert!(state.todo_key(key('a')));
        assert!(matches!(state.todos.mode, Mode::Normal));
        let note = state.todos.note.as_ref().unwrap();
        assert!(note.error && note.lead.contains("200"), "{}", note.lead);
    }

    #[test]
    fn subtask_under_a_max_depth_parent_is_refused_before_typing() {
        let mut state = two_todos();
        let mut chain = item(todos::MAX_DEPTH as u64, "deepest", false, vec![]);
        for id in (1..todos::MAX_DEPTH as u64).rev() {
            chain = item(id, "level", false, vec![chain]);
        }
        state.todos.list.items = vec![chain];
        state.todos.selected = Some(todos::MAX_DEPTH as u64);
        assert!(state.todo_key(key('A')));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert!(state.todos.note.as_ref().unwrap().hint.contains("5 levels"));
        state.todos.selected = Some(todos::MAX_DEPTH as u64 - 1);
        state.todo_key(key('A'));
        assert!(matches!(state.todos.mode, Mode::Input { .. }));
    }

    /// A draft kept before the list reached a limit comes back when its key is pressed, with
    /// the limit as the box's refusal: text only the box can show is never out of reach.
    #[test]
    fn a_kept_draft_comes_back_at_the_limits() {
        let mut state = two_todos();
        state.todo_key(key('a'));
        typed(&mut state, "kept");
        state.todo_key(bare(BareKey::Esc));
        state.todos.list.items = (1..=todos::MAX_OPEN as u64)
            .map(|id| item(id, "open", false, vec![]))
            .collect();
        assert!(state.todo_divider(80, 0, 0).line.contains("a resume draft"));
        assert!(state.todo_key(key('a')));
        assert_eq!(draft(&state), Some("kept"));
        assert!(
            refusal(&state).unwrap().contains("200 open todos"),
            "{:?}",
            refusal(&state)
        );
        assert!(state.todo_key(bare(BareKey::Enter)));
        assert_eq!(draft(&state), Some("kept"));
        state.todo_key(bare(BareKey::Esc));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert!(state
            .todos
            .draft
            .as_ref()
            .is_some_and(|(_, input)| input.get_text() == "kept"));

        // A subtask whose parent has been pushed down to the depth limit meanwhile.
        let mut state = two_todos();
        let parent = todos::MAX_DEPTH as u64 - 1;
        let mut chain = item(parent, "level", false, vec![]);
        for id in (1..parent).rev() {
            chain = item(id, "level", false, vec![chain]);
        }
        state.todos.list.items = vec![chain.clone()];
        state.todos.selected = Some(parent);
        state.todo_key(key('A'));
        typed(&mut state, "child");
        state.todo_key(bare(BareKey::Esc));
        state.todos.list.items = vec![item(100, "above", false, vec![chain])];
        assert!(state.todo_key(key('A')));
        assert_eq!(draft(&state), Some("child"));
        assert!(
            refusal(&state).unwrap().contains("5 levels"),
            "{:?}",
            refusal(&state)
        );
    }

    /// Tab and Shift+Tab walk the ring from the box as from the list, and the box is still
    /// there with its text when the keys come back.
    #[test]
    fn tab_in_the_box_walks_the_ring_and_keeps_the_draft() {
        let mut state = two_todos();
        state.todo_key(key('a'));
        typed(&mut state, "x");
        assert!(state.todo_key(bare(BareKey::Tab)));
        assert_eq!(state.ring_yields, [wire::STOP_TABS]);
        assert_eq!(draft(&state), Some("x"));
        assert!(state.todo_key(bare(BareKey::Tab).with_shift_modifier()));
        assert!(state.tabs_focused);
        assert_eq!(draft(&state), Some("x"));
    }

    /// Tab and Shift+Tab walk the ring from a move and from the project picker as from the
    /// box, and the pick is still there when the keys come back; a question is dropped first.
    #[test]
    fn tab_walks_the_ring_from_a_move_and_a_project_pick_and_keeps_them() {
        let mut state = parent_and_child();
        state.todos.selected = Some(2);
        state.todo_key(key('m'));
        assert!(state.todo_key(bare(BareKey::Tab)));
        assert_eq!(state.ring_yields, [wire::STOP_TABS]);
        assert!(matches!(state.todos.mode, Mode::Move { id: 2, .. }));
        assert!(state.todo_key(bare(BareKey::Tab).with_shift_modifier()));
        assert!(state.tabs_focused);
        assert!(matches!(state.todos.mode, Mode::Move { id: 2, .. }));

        state.tabs_focused = false;
        state.todo_key(bare(BareKey::Esc));
        state.todo_key(key('p'));
        assert!(matches!(state.todos.mode, Mode::Projects { .. }));
        assert!(state.todo_key(bare(BareKey::Tab)));
        assert_eq!(state.ring_yields, [wire::STOP_TABS, wire::STOP_TABS]);
        assert!(matches!(state.todos.mode, Mode::Projects { .. }));
        assert!(state.todo_key(bare(BareKey::Tab).with_shift_modifier()));
        assert!(state.tabs_focused);
        assert!(matches!(state.todos.mode, Mode::Projects { .. }));

        state.tabs_focused = false;
        state.todo_key(bare(BareKey::Esc));
        state.todos.selected = Some(1);
        state.todo_key(key('d'));
        assert!(matches!(state.todos.mode, Mode::Delete { .. }));
        assert!(state.todo_key(bare(BareKey::Tab)));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert_eq!(state.ring_yields.len(), 3);
        assert_eq!(texts(&state), ["parent", "child"], "nothing deleted");
    }

    /// A move or a box about a todo that another writer deleted: the move goes, saying why,
    /// and the box keeps the user's text as a new todo, saying so before Enter adds it.
    #[test]
    fn a_todo_deleted_elsewhere_ends_its_move_and_its_box_keeps_the_text() {
        let mut state = parent_and_child();
        state.todos.selected = Some(2);
        state.todo_key(key('m'));
        assert!(matches!(state.todos.mode, Mode::Move { id: 2, .. }));
        state.todo_rows_changed(DataWatchId::new(1), watched(1));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert_eq!(state.todos.note.as_ref().unwrap().lead, GONE);

        let mut state = parent_and_child();
        state.todos.selected = Some(2);
        state.todo_key(key('e'));
        typed(&mut state, " reworded");
        state.todo_rows_changed(DataWatchId::new(1), watched(1));
        assert!(matches!(
            state.todos.mode,
            Mode::Input {
                action: InputAction::Add { parent_id: None },
                ..
            }
        ));
        assert_eq!(draft(&state), Some("child reworded"));
        assert!(refusal(&state).unwrap().contains("Enter adds it as new"));
        assert_eq!(state.todo_cursor(), None);
    }

    /// The box marks what it is about - an edit's item, a subtask's parent - wherever the
    /// selection goes meanwhile, and says which kind of entry it is.
    #[test]
    fn input_mode_marks_its_target() {
        let mut state = parent_and_child();
        state.todo_key(key('A'));
        assert_eq!(state.todo_cursor(), Some(1));
        state.todos.selected = Some(2);
        assert_eq!(state.todo_cursor(), Some(1));
        let (row, _) = state.input_divider(80).unwrap();
        assert!(row.line.contains("subtask of \"parent\": "), "{}", row.line);
        let (row, _) = state.input_divider(24).unwrap();
        assert!(row.line.contains("subtask: "), "{}", row.line);
        state.todo_key(bare(BareKey::Esc));
        state.todo_key(key('a'));
        assert_eq!(state.todo_cursor(), None);
        assert!(state
            .input_divider(80)
            .unwrap()
            .0
            .line
            .contains("new top-level todo: "));
        state.todo_key(bare(BareKey::Esc));
        state.todo_key(key('e'));
        assert_eq!(state.todo_cursor(), Some(2));
    }

    /// Success notes last until the next key - not until the next change to the list, which
    /// for this pane's own write is the echo of it a moment later.
    #[test]
    fn a_success_note_outlives_the_watch_echo_and_goes_with_the_next_key() {
        let (backend, mut state) = stored(|backend| {
            put(backend, "one", None);
            put(backend, "two", None);
        });
        state.todo_key_with(&backend, key(' '));
        assert!(state.todos.note.is_some());
        let echo = backend
            .fetch(&FetchRequest::entity("todo").filter(Pred::eq("root", "/work")))
            .unwrap();
        assert!(state.todo_rows_changed(store::LOCAL_WATCH_ID, echo));
        assert!(state.todos.note.is_some());
        assert!(state.todo_key_with(&backend, key('j')));
        assert!(state.todos.note.is_none());
    }

    /// A note or a problem leads the hint line in its own colour and leaves the keys after
    /// it, rather than taking the whole row.
    #[test]
    fn an_error_leaves_the_keys_on_show() {
        let mut state = two_todos();
        state.todos.note = Some(Note::error("boom"));
        let row = state.todo_divider(80, 0, 0);
        assert!(row.error);
        let bright = row.bright.clone().unwrap();
        assert_eq!(
            row.line
                .chars()
                .skip(bright.start)
                .take(bright.len())
                .collect::<String>(),
            "boom"
        );
        assert!(row.line.contains("boom · Space done"), "{}", row.line);
        state.todos.note = None;
        state.todos.status = "the store is down".into();
        let row = state.todo_divider(80, 0, 0);
        assert!(row.error);
        assert!(
            row.line
                .contains("the store is down · Ctrl-r retry · Space done"),
            "{}",
            row.line
        );
        state.todos.status.clear();
        let row = state.todo_divider(80, 0, 0);
        assert!(row.bright.is_none() && !row.error);
    }

    /// With no project the list says how to get one, the hint line offers nothing that needs
    /// one, and `a` says why it cannot add rather than doing nothing.
    #[test]
    fn the_empty_list_says_what_to_do() {
        let mut state = State {
            surface: Surface::Todos,
            ..Default::default()
        };
        let joined = |state: &State| {
            state
                .todo_empty_text()
                .into_iter()
                .map(|(line, _)| line)
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert!(joined(&state).contains("zellij action project-remember NAME --root DIR"));
        let hints = state.todo_divider(80, 0, 0).line;
        assert!(
            !hints.contains("a add") && hints.contains("Ctrl-r reload"),
            "{hints}"
        );
        assert!(state.todo_key(key('a')));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert!(state
            .todos
            .note
            .as_ref()
            .unwrap()
            .lead
            .contains("no projects"));

        state.todos.projects = projects::Registry::from([("api".into(), project("api", "/api"))]);
        assert!(joined(&state).contains("p to pick one"));
        // Not a folder the user cannot see: the one the panel happened to be opened in.
        assert!(joined(&state).contains("no project picked"));
        assert!(
            !joined(&state).contains("this folder"),
            "{}",
            joined(&state)
        );
        assert!(state
            .todo_divider(80, 0, 0)
            .line
            .contains("p pick a project"));

        state.todos.project_root = Some("/api".into());
        assert!(joined(&state).contains("a to add one"));
        state.todos.list.items = vec![item(1, "done", true, vec![item(2, "done", true, vec![])])];
        assert!(
            joined(&state).contains("2 done todos hidden"),
            "{}",
            joined(&state)
        );
        assert!(joined(&state).contains("v to show"));
    }

    /// The empty list's lines wrap to the pane rather than being cut at its edge, where the
    /// command to run and the key to press are: every word is on screen, and none is split.
    #[test]
    fn the_empty_text_wraps_to_the_pane_rather_than_cutting() {
        let none = State {
            surface: Surface::Todos,
            ..Default::default()
        };
        let mut unpicked = State {
            surface: Surface::Todos,
            ..Default::default()
        };
        unpicked.todos.projects =
            projects::Registry::from([("api".into(), project("api", "/api"))]);
        let mut hidden = two_todos();
        hidden.todos.list.items = vec![item(1, "one", true, vec![]), item(2, "two", true, vec![])];
        let mut empty = two_todos();
        empty.todos.list.items.clear();
        empty.todos.selected = None;
        let words = |lines: &[(String, bool)]| -> Vec<String> {
            lines
                .iter()
                .flat_map(|(line, _)| line.split_whitespace().map(str::to_owned))
                .collect()
        };
        for mut state in [none, unpicked, hidden, empty] {
            let lines = state.todo_empty_text();
            for cols in [16, 22, 24, 34, 46] {
                let rows = empty_text_rows(&lines, cols);
                assert!(
                    rows.iter().all(|(row, _)| row.width() <= cols),
                    "{cols}: {rows:?}"
                );
                assert_eq!(words(&rows), words(&lines), "{cols}: {rows:?}");
            }
            state.render_todos(12, 22);
        }
        let none = State::default();
        let rows = empty_text_rows(&none.todo_empty_text(), 22);
        assert_eq!(
            rows,
            [
                ("no projects yet".to_owned(), false),
                ("start an agent in a".to_owned(), true),
                ("git checkout,".to_owned(), true),
                ("or: zellij action".to_owned(), true),
                ("project-remember NAME".to_owned(), true),
                ("--root DIR".to_owned(), true),
            ]
        );
        for cols in [16, 22, 24, 34, 46] {
            let joined = empty_text_rows(&none.todo_empty_text(), cols)
                .into_iter()
                .map(|(row, _)| row)
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                joined.contains("zellij action project-remember NAME --root DIR"),
                "{cols}: {joined}"
            );
        }
    }

    /// A panel opened outside every project opens on none, rather than on whichever project's
    /// name sorts first; the project shown is named above the list, where nothing replaces it.
    #[test]
    fn a_folder_outside_every_project_opens_on_none_and_a_project_is_named_above_its_list() {
        let mut state = State {
            surface: Surface::Todos,
            ..Default::default()
        };
        state.todos.initial_cwd = "/elsewhere".into();
        state.take_todo_projects(
            &Host::shared(),
            projects::Registry::from([
                ("alpha".into(), project("alpha", "/alpha")),
                ("beta".into(), project("beta", "/beta")),
            ]),
        );
        assert_eq!(state.todos.project_root, None);
        assert!(state.todo_header(40).is_none());
        state.todo_key(key('a'));
        assert!(state
            .todos
            .note
            .as_ref()
            .unwrap()
            .hint
            .contains("p to pick"));

        state.todos.project_root = Some("/beta".into());
        state.todos.status = "boom".into();
        let header = state.todo_header(40).unwrap();
        assert_eq!(header.line, "Project: beta · p switch");
        assert_eq!(header.bright, Some(0..13));
        state.render_todos(12, 40);
        assert_eq!(state.todos.last_header_rows, 1);
        assert_eq!(state.todos.last_list_rows, 10);
        // Too short to spare the row: the list keeps it.
        state.render_todos(2, 40);
        assert_eq!(state.todos.last_header_rows, 0);

        // `p` switches only from the list: in a box it is a letter, in a move or a question a
        // key that does something else, and the header says so by not offering it.
        let plain = |state: &State| {
            let header = state.todo_header(40).unwrap();
            assert_eq!(header.line, "Project: beta");
            assert_eq!(header.bright, Some(0..13));
        };
        state.todos.status.clear();
        state.todo_key(key('a'));
        assert!(matches!(state.todos.mode, Mode::Input { .. }));
        plain(&state);
        state.todo_key(bare(BareKey::Esc));
        state.todos.mode = Mode::Move {
            id: 1,
            rev: DataRevision::new(1),
            target: None,
        };
        plain(&state);
        state.todos.mode = Mode::Delete {
            id: 1,
            plan: Vec::new(),
            descendants: 0,
        };
        plain(&state);
        state.todos.mode = Mode::Normal;
        assert_eq!(
            state.todo_header(40).unwrap().line,
            "Project: beta · p switch"
        );
    }

    /// Ctrl-r reads the list again in place: the cursor stays on its todo, or goes to the
    /// nearest one left, rather than back to the top.
    #[test]
    fn ctrl_r_reloads_in_place_and_the_hint_line_names_it() {
        let (backend, mut state) = stored(|backend| {
            for at in 0..25 {
                put(backend, &format!("todo {at}"), None);
            }
        });
        let twentieth = state.todos.list.items[19].id;
        state.todos.selected = Some(twentieth);
        assert!(state.todo_divider(200, 0, 0).line.contains("Ctrl-r reload"));
        let reload = key('r').with_ctrl_modifier();
        assert!(state.todo_key_with(&backend, reload.clone()));
        assert_eq!(state.todos.selected, Some(twentieth));
        backend
            .apply(vec![Mutation::delete("todo", Pred::eq("id", twentieth))])
            .unwrap();
        state.todo_key_with(&backend, reload);
        assert_eq!(state.todos.selected, Some(state.todos.list.items[19].id));
        assert_eq!(state.todos.list.items.len(), 24);
    }

    #[test]
    fn a_failed_reload_keeps_the_rows_on_screen() {
        let mut state = two_todos();
        state.todos.selected = Some(2);
        let before = state.todos.list.clone();
        assert!(state.todo_key(key('r').with_ctrl_modifier()));
        assert_eq!(state.todos.list, before);
        assert_eq!(state.todos.selected, Some(2));
        assert!(!state.todos.status.is_empty());
        assert!(state.todo_divider(80, 0, 0).line.contains("Ctrl-r retry"));
    }

    /// With no registry read - the store refused it as the panel opened - Ctrl-r tries the
    /// registry again rather than reloading a list no project has, which would only clear the
    /// error saying why; and the list says the projects could not be read, not that there are
    /// none.
    #[test]
    fn ctrl_r_after_a_registry_error_retries_the_registry() {
        let mut state = State {
            surface: Surface::Todos,
            ..Default::default()
        };
        let joined = |state: &State| {
            state
                .todo_empty_text()
                .into_iter()
                .map(|(line, _)| line)
                .collect::<Vec<_>>()
                .join(" ")
        };
        state.open_todos("/work".into());
        assert!(!state.todos.status.is_empty());
        assert!(state.todos.project_watch.is_none());
        assert_eq!(state.todos.project_root, None);
        assert!(
            !joined(&state).contains("no projects yet"),
            "{}",
            joined(&state)
        );
        state.todo_key(key('a'));
        assert_eq!(
            state.todos.note.as_ref().unwrap().lead,
            "projects not loaded"
        );
        let reload = key('r').with_ctrl_modifier();
        assert!(state.todo_key_with(&Host::shared(), reload.clone()));
        assert!(!state.todos.status.is_empty());
        assert!(state.todos.project_watch.is_none());

        let backend = store::Local::shared("hocus-agents");
        backend.define_all(&store::shared_defs()).unwrap();
        backend
            .apply(vec![projects::to_mutation(&project("work", "/work"))])
            .unwrap();
        assert!(state.todo_key_with(&backend, reload));
        assert!(state.todos.project_watch.is_some());
        assert_eq!(state.todos.projects.len(), 1);
        assert_eq!(state.todos.project_root.as_deref(), Some("/work"));
        assert!(state.todos.status.is_empty(), "{}", state.todos.status);
        assert!(
            !joined(&state).contains("no projects yet"),
            "{}",
            joined(&state)
        );
    }

    /// The todo being moved keeps a mark while the cursor is off choosing its parent; the hint
    /// line names it by its text; a todo with nowhere to go does not open a move at all; and
    /// a move to where it already is writes nothing.
    #[test]
    fn a_move_marks_its_source_and_a_lone_todo_has_nowhere_to_go() {
        let mut state = two_todos();
        state.todo_key(key('m'));
        assert!(matches!(
            state.todos.mode,
            Mode::Move {
                id: 1,
                target: Some(2),
                ..
            }
        ));
        let hints = state.todo_divider(80, 0, 0).line;
        assert!(
            hints.starts_with("─ Enter nest · t top · Esc cancel · moving \"one\""),
            "{hints}"
        );
        let lines = todo_lines(&todo_rows(&state.todos.list, false), 40);
        assert!(line_body(&lines[0], false, true)
            .text
            .starts_with("↕[ ] one"));
        assert!(line_body(&lines[1], true, false)
            .text
            .starts_with(">[ ] two"));
        state.todo_key(key('t'));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert_eq!(state.todos.note.as_ref().unwrap().lead, "already top level");

        state.todos.list.items.pop();
        assert!(state.todo_key(key('m')));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert!(state
            .todos
            .note
            .as_ref()
            .unwrap()
            .lead
            .starts_with("nowhere to move it"));
    }

    /// A move's ways out fit the narrowest half, `t` - the only way to the top level - always
    /// among them, and the scroll readout gives way to them rather than crowding them out.
    #[test]
    fn a_move_keeps_its_top_level_key_on_the_narrowest_half() {
        let mut state = two_todos();
        state.todo_key(key('m'));
        for cols in 22..=60 {
            for (above, below) in [(0, 0), (3, 0), (12, 40)] {
                let line = state.todo_divider(cols, above, below).line;
                assert_eq!(line.width(), cols, "{cols}: {line}");
                assert!(line.starts_with("─ Enter nest · t top"), "{cols}: {line}");
                if cols >= 35 {
                    assert!(line.contains("t top · Esc cancel"), "{cols}: {line}");
                }
                if cols == 60 {
                    assert!(
                        line.ends_with(&scroll_readout(above, below)),
                        "{cols}: {line}"
                    );
                }
            }
        }
    }

    /// Every note the tab says fits the narrowest half whole: the item's excerpt goes before a
    /// word is cut, then the count is shortened, and no row ends in a dangling `+`.
    #[test]
    fn every_note_fits_the_narrowest_half_whole() {
        let long = "Old idea about the resize loop";
        let mut notes = vec![
            Note::done("deleted", long, 2),
            Note::done(
                "moved",
                "a todo with a name much too long for any narrow pane",
                0,
            ),
            Note::error(GONE),
            Note::error("changed elsewhere").then("list reloaded, try again"),
        ];
        let state = two_todos();
        notes.extend([3, 1, 0].map(|count| state.completed_note(long, count)));
        // Each refusal, as the keys that meet it make it.
        let refused = |mut state: State, keys: &[KeyWithModifier]| -> Note {
            for key in keys {
                state.todo_key(key.clone());
            }
            state.todos.note.take().expect("a refusal")
        };
        let mut unpicked = State::default();
        unpicked.todos.projects =
            projects::Registry::from([("api".into(), project("api", "/api"))]);
        let mut unloaded = State::default();
        unloaded.todos.status = "the store is down".into();
        let mut full = two_todos();
        full.todos.list.items = (1..=todos::MAX_OPEN as u64)
            .map(|id| item(id, "open", false, vec![]))
            .collect();
        let mut deep = two_todos();
        let mut chain = item(todos::MAX_DEPTH as u64, "deepest", false, vec![]);
        for id in (1..todos::MAX_DEPTH as u64).rev() {
            chain = item(id, "level", false, vec![chain]);
        }
        deep.todos.list.items = vec![chain];
        deep.todos.selected = Some(todos::MAX_DEPTH as u64);
        let mut lone = two_todos();
        lone.todos.list.items.pop();
        let mut child = parent_and_child();
        child.todos.selected = Some(2);
        notes.extend([
            refused(State::default(), &[key('a')]),
            refused(unpicked, &[key('a')]),
            refused(unloaded, &[key('a')]),
            refused(full, &[key('a')]),
            refused(deep, &[key('A')]),
            refused(lone, &[key('m')]),
            refused(two_todos(), &[key('m'), key('t')]),
            refused(child, &[key('m'), bare(BareKey::Enter)]),
        ]);
        let mut state = two_todos();
        for note in notes {
            state.todos.note = Some(note);
            for cols in 24..=34 {
                let text = state.todos.note.as_ref().unwrap().text(cols - 4);
                assert!(text.width() <= cols - 4, "{cols}: {text}");
                assert!(!text.ends_with('+'), "{cols}: {text}");
                let line = state.todo_divider(cols, 3, 9).line;
                assert!(line.contains(&text), "{cols}: {text} in {line}");
            }
        }
    }

    #[test]
    fn picker_navigation_restores_cursor_scrolling_even_at_boundaries() {
        for projects in [true, false] {
            for (key, start, expected) in [
                (BareKey::Up, 5, 4),
                (BareKey::Char('k'), 0, 0),
                (BareKey::Down, 5, 6),
                (BareKey::Char('j'), 9, 9),
                (BareKey::Up, 0, 0),
                (BareKey::Char('k'), 5, 4),
                (BareKey::Down, 9, 9),
                (BareKey::Char('j'), 5, 6),
            ] {
                let mut state = State::default();
                state.todos.list.items =
                    (0..=10).map(|id| item(id, "todo", false, vec![])).collect();
                state.todos.projects = (0..10)
                    .map(|id| {
                        let name = format!("project {id}");
                        (name.clone(), project(&name, &format!("/work/{id}")))
                    })
                    .collect();
                state.todos.mode = if projects {
                    Mode::Projects { selected: start }
                } else {
                    Mode::Move {
                        id: 10,
                        rev: DataRevision::new(10),
                        target: Some(start as u64),
                    }
                };
                state.render_todos(4, 80);
                state.todo_mouse(if start == 0 {
                    Mouse::ScrollDown(20)
                } else {
                    Mouse::ScrollUp(20)
                });
                state.render_todos(4, 80);
                assert!(state.todos.list_scroll.is_some());
                assert!(state.todo_key(KeyWithModifier::new(key)));
                assert_eq!(state.todos.list_scroll, None);
                state.render_todos(4, 80);
                if projects {
                    assert!(
                        matches!(state.todos.mode, Mode::Projects { selected } if selected == expected)
                    );
                    assert!(state
                        .todos
                        .rendered_projects
                        .contains(&format!("/work/{expected}")));
                } else {
                    assert!(
                        matches!(state.todos.mode, Mode::Move { target: Some(id), .. } if id == expected as u64)
                    );
                    assert!(state.todos.rendered_ids.contains(&(expected as u64)));
                }
            }
        }
    }

    /// The wheel stops at the end of the list, so turning it back moves the list at once, and
    /// the readout says how much is below as well as above.
    #[test]
    fn a_wheel_past_the_end_banks_no_travel_and_the_divider_counts_both_ends() {
        let mut state = State::default();
        state.todos.list.items = (1..=30).map(|id| item(id, "todo", false, vec![])).collect();
        state.todos.selected = Some(1);
        state.render_todos(10, 40);
        assert_eq!(state.todos.last_list_rows, 9);
        assert!(state.todo_divider(40, 0, 21).line.ends_with(" ↓21 "));
        state.todo_mouse(Mouse::ScrollDown(50));
        state.render_todos(10, 40);
        assert_eq!(state.todos.last_window, 21);
        assert_eq!(state.todos.list_scroll, Some(21));
        state.todo_mouse(Mouse::ScrollUp(1));
        state.render_todos(10, 40);
        assert_eq!(state.todos.last_window, 20);

        state.todos.mode = Mode::Projects { selected: 0 };
        state.todos.projects = (0..30)
            .map(|id| {
                let name = format!("project {id:02}");
                (name.clone(), project(&name, &format!("/work/{id}")))
            })
            .collect();
        state.todos.list_scroll = None;
        state.todo_mouse(Mouse::ScrollDown(99));
        state.render_todos(10, 40);
        assert_eq!(state.todos.list_scroll, Some(30 - 8));
    }

    /// The wheel's place in the picker is not a place in the list: leaving the picker without
    /// a pick, whichever way, puts the list back around its cursor.
    #[test]
    fn leaving_the_picker_drops_its_wheel_scroll() {
        for exit in ["Esc", "Enter", "hide"] {
            let mut state = State {
                surface: Surface::Todos,
                ..Default::default()
            };
            state.todos.project_root = Some("/work".into());
            state.todos.list.items = (1..=100)
                .map(|id| item(id, "todo", false, vec![]))
                .collect();
            state.todos.selected = Some(80);
            // Enter leaves without a pick only where there is nothing to pick.
            if exit != "Enter" {
                state.todos.projects = (0..30)
                    .map(|id| {
                        let name = format!("project {id:02}");
                        (name.clone(), project(&name, &format!("/work/{id}")))
                    })
                    .collect();
            }
            state.render_todos(10, 40);
            state.todo_key(key('p'));
            state.todo_mouse(Mouse::ScrollDown(15));
            state.render_todos(10, 40);
            assert!(state.todos.list_scroll.is_some(), "{exit}");
            match exit {
                "Esc" => assert!(state.todo_key(bare(BareKey::Esc))),
                "Enter" => assert!(state.todo_key(bare(BareKey::Enter))),
                _ => state.hide_the_panel(),
            }
            assert!(matches!(state.todos.mode, Mode::Normal), "{exit}");
            assert_eq!(state.todos.list_scroll, None, "{exit}");
            state.render_todos(10, 40);
            assert!(state.todos.rendered_ids.contains(&80), "{exit}");
        }
    }

    /// Every key a mode answers is on its sheet.
    #[test]
    fn the_key_sheet_names_every_key_each_mode_binds() {
        let names = |state: &State| -> Vec<String> {
            state
                .todo_key_sheet()
                .into_iter()
                .flat_map(|(keys, _)| keys.split(" / ").map(str::to_owned).collect::<Vec<_>>())
                .collect()
        };
        let mut state = parent_and_child();
        let expect = |state: &State, keys: &[&str]| {
            let names = names(state);
            for key in keys {
                assert!(names.iter().any(|name| name == key), "{key}: {names:?}");
            }
        };
        expect(
            &state,
            &[
                "Space",
                "Enter",
                "e",
                "a",
                "A",
                "d",
                "n",
                "m",
                "p",
                "v",
                "j",
                "k",
                "↑",
                "↓",
                "Ctrl-r",
                "Tab",
                "Shift+Tab",
                "Esc",
                // The box's, which has no sheet of its own: `?` is typed there.
                "Alt+Enter",
                "Shift+Enter",
                "Home",
                "End",
                "Backspace",
                "Ctrl-w",
                "Ctrl-u",
                "Ctrl-z",
            ],
        );
        state.todo_key(key('d'));
        expect(&state, &["y", "any other key"]);
        state.todo_key(key('n'));
        state.todo_key(key(' '));
        expect(&state, &["y", "any other key"]);
        state.todo_key(key('n'));
        state.todos.selected = Some(2);
        state.todo_key(key('m'));
        expect(&state, &["Enter", "t", "Esc", "j", "k", "Tab", "Shift+Tab"]);
        state.todo_key(bare(BareKey::Esc));
        state.todo_key(key('p'));
        expect(&state, &["Enter", "Esc", "j", "k", "Tab", "Shift+Tab"]);
    }

    /// `?` is a character in the box - even one too narrow to draw, and even pressed on the
    /// tab bar, which hands the box a key it does not own - and the Todos sheet everywhere else.
    #[test]
    fn a_question_mark_is_typed_into_the_box_and_opens_the_sheet_outside_it() {
        let mut state = two_todos();
        state.render(12, 40);
        assert!(state.on_key(key('?')));
        assert_eq!(state.key_sheet_scroll, Some(0));
        assert!(state
            .key_sheet()
            .iter()
            .any(|(keys, what)| keys == "Space" && what.contains("complete")));
        assert!(state.on_key(bare(BareKey::Esc)));
        assert_eq!(state.key_sheet_scroll, None);

        state.on_key(key('a'));
        state.render(12, 3);
        assert!(state.on_key(key('?')));
        state.on_key(bare(BareKey::Tab).with_shift_modifier());
        assert!(state.tabs_focused);
        assert!(state.on_key(key('?')));
        assert_eq!(draft(&state), Some("??"));
        assert_eq!(state.key_sheet_scroll, None);
    }

    /// A paste on the tab bar over an open box lands in the box, as a key the bar does not own
    /// does, and takes the keys down off the bar with it. With no box open the bar turns it
    /// away, as the list would: a paste is never replayed as its letters' commands.
    #[test]
    fn a_paste_on_the_bar_over_the_box_lands_in_the_box() {
        let mut state = two_todos();
        state.render(12, 40);
        assert!(state.on_key(key('a')));
        for ch in "Fix ".chars() {
            state.on_key(key(ch));
        }
        assert!(state.on_key(bare(BareKey::Tab).with_shift_modifier()));
        assert!(state.tabs_focused);
        assert!(state.on_paste("flaky test"));
        assert_eq!(draft(&state), Some("Fix flaky test"));
        assert!(
            !state.tabs_focused,
            "the paste took the keys down to the box"
        );

        assert!(state.on_key(bare(BareKey::Esc)));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert!(state.on_key(bare(BareKey::Tab).with_shift_modifier()));
        assert!(state.tabs_focused);
        assert!(!state.on_paste("ajk"));
        assert!(state.tabs_focused);
        assert!(matches!(state.todos.mode, Mode::Normal));
    }

    #[test]
    fn a_vanished_selection_moves_to_its_nearest_surviving_neighbour() {
        let list = todos::List {
            items: vec![
                item(1, "first", false, vec![]),
                item(2, "parent", false, vec![item(3, "child", false, vec![])]),
                item(4, "last", false, vec![]),
            ],
            ..Default::default()
        };
        let before = todo_rows(&list, false);
        let without = |hidden: &[u64]| -> Vec<Row> {
            before
                .iter()
                .filter(|row| !hidden.contains(&row.id))
                .cloned()
                .collect()
        };
        assert_eq!(nearest_surviving(&before, &before, Some(2)), Some(2));
        assert_eq!(nearest_surviving(&before, &without(&[3]), Some(3)), Some(4));
        assert_eq!(
            nearest_surviving(&before, &without(&[2, 3]), Some(2)),
            Some(4)
        );
        assert_eq!(nearest_surviving(&before, &without(&[4]), Some(4)), Some(3));
        assert_eq!(nearest_surviving(&before, &before, Some(9)), Some(1));
        assert_eq!(nearest_surviving(&before, &[], Some(2)), None);
    }

    #[test]
    fn completing_the_selected_todo_keeps_the_cursor_in_place() {
        let rows = |done: u64| DataRows {
            columns: ["id", "root", "text", "parent_id", "rev", "done"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            rows: (1..=3)
                .map(|id| {
                    vec![
                        DataValue::Int(id),
                        DataValue::Text("/work".into()),
                        DataValue::Text(format!("todo {id}")),
                        DataValue::Null,
                        DataValue::Int(id),
                        DataValue::Bool(id as u64 == done),
                    ]
                })
                .collect(),
            truncated: false,
            next_cursor: None,
        };
        for (selected, expected) in [(2, 3), (3, 2)] {
            let mut state = State {
                surface: Surface::Todos,
                ..Default::default()
            };
            state.todos.project_root = Some("/work".into());
            state.todos.todo_watch = Some(DataWatchId::new(1));
            state.todo_rows_changed(DataWatchId::new(1), rows(0));
            state.todos.selected = Some(selected);
            state.todo_rows_changed(DataWatchId::new(1), rows(selected));
            assert_eq!(state.todos.selected, Some(expected));
        }
    }

    #[test]
    fn items_render_as_markdown_and_wrap_under_their_text() {
        let list = todos::List {
            items: vec![item(
                1,
                "**bold** and `code`",
                false,
                vec![item(2, "one two three four five six", true, vec![])],
            )],
            ..Default::default()
        };
        let lines = todo_lines(&todo_rows(&list, true), 16);
        let texts: Vec<_> = lines.iter().map(|line| line.line.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "[ ] bold and",
                "    code",
                "  [x] one two",
                "      three",
                "      four five",
                "      six"
            ]
        );
        assert!(lines[0].line.styles[4].bold);
        assert!(!lines[0].line.styles[9].bold);
        assert_eq!(
            lines
                .iter()
                .map(|line| (line.id, line.first))
                .collect::<Vec<_>>(),
            [
                (1, true),
                (1, false),
                (2, true),
                (2, false),
                (2, false),
                (2, false)
            ]
        );
        assert!(lines
            .iter()
            .all(|line| line.line.text.width() < 16 && line.done == (line.id == 2)));
    }

    #[test]
    fn wrapped_items_scroll_by_row_and_click_through_continuations() {
        let mut state = State::default();
        state.todos.list.items = vec![
            item(1, "short", false, vec![]),
            item(2, "one two three four five six seven eight", false, vec![]),
            item(3, "last", false, vec![]),
        ];
        state.todos.selected = Some(1);
        state.render_todos(20, 16);
        assert_eq!(state.todos.rendered_ids, [1, 2, 2, 2, 2, 3]);

        state.todo_mouse(Mouse::LeftClick(3, 0));
        assert_eq!(state.todos.selected, Some(2));

        // Three list rows: the tall item shows its head rather than being centred past it.
        state.render_todos(4, 16);
        assert_eq!(state.todos.last_window, 1);
        assert_eq!(state.todos.rendered_ids, [2, 2, 2]);

        state.todos.selected = Some(3);
        state.render_todos(4, 16);
        assert_eq!(state.todos.rendered_ids, [2, 2, 3]);

        // With the project's row above the list, a click is a row lower, and one on the
        // project's row is nobody's.
        state.todos.project_root = Some("/work".into());
        state.render_todos(20, 16);
        assert_eq!(state.todos.last_header_rows, 1);
        assert!(!state.todo_mouse(Mouse::LeftClick(0, 0)));
        state.todo_mouse(Mouse::LeftClick(1, 0));
        assert_eq!(state.todos.selected, Some(1));
    }

    #[test]
    fn project_picker_preserves_a_root_across_rename() {
        let mut state = State::default();
        state.todos.initial_cwd = "/work/nested/src".into();
        state.todos.projects = projects::Registry::from([
            ("outer".into(), project("outer", "/work")),
            ("nested".into(), project("nested", "/work/nested")),
        ]);
        let selected = projects::for_cwd(
            &state.todos.projects,
            &Location::local(&state.todos.initial_cwd),
        )
        .unwrap();
        assert_eq!(selected.name, "nested");
        state.todos.project_root = Some(selected.root.clone());
        state.todos.projects =
            projects::Registry::from([("renamed".into(), project("renamed", "/work/nested"))]);
        assert_eq!(state.todo_project_name(), "renamed");
    }
}

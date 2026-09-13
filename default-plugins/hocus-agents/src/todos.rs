use super::*;
use hocus_proto::location::Location;
use hocus_proto::store::{self, Backend, Host};
use hocus_proto::{projects, todos};
use zellij_tile::prelude::{
    DataRevision, DataRows, DataWatchId, FetchRequest, Mutation, Pred, SortDir, WatchMode,
};

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
    mode: Mode,
    status: String,
    list_scroll: Option<usize>,
    last_window: usize,
    last_list_rows: usize,
    rendered_ids: Vec<u64>,
    rendered_projects: Vec<String>,
    /// The visible items wrapped and styled for a width. Parsing markdown is the expensive
    /// part of a render and the pane redraws on a timer, so this lives until the rows or the
    /// width change; the cursor is painted on top and never invalidates it.
    line_cache: Option<(usize, Vec<TodoLine>)>,
}

#[derive(Default)]
enum Mode {
    #[default]
    Normal,
    Projects {
        selected: usize,
    },
    Input {
        action: InputAction,
        text: String,
    },
    Delete {
        plan: Vec<Mutation>,
        descendants: usize,
    },
    Move {
        id: u64,
        rev: DataRevision,
        target: Option<u64>,
    },
}

enum InputAction {
    Add { parent_id: Option<u64> },
    Edit { id: u64, rev: DataRevision },
}

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
        self.rendered_ids.clear();
        self.rendered_projects.clear();
        self.line_cache = None;
    }
}

impl State {
    pub(super) fn open_todos(&mut self, initial_cwd: String) {
        self.todos.initial_cwd = initial_cwd;
        let backend = Host::shared();
        if let Err(error) = backend.define_all(&store::shared_defs()) {
            self.todos.status = error;
            return;
        }
        match backend.watch(
            &FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT),
            WatchMode::Rows,
        ) {
            Ok(handle) => {
                self.todos.project_watch = Some(handle.watch_id);
                self.take_todo_projects(projects::from_rows(&handle.rows));
            },
            Err(error) => self.todos.status = error,
        }
    }

    pub(super) fn todo_rows_changed(&mut self, watch_id: DataWatchId, rows: DataRows) -> bool {
        if self.todos.project_watch == Some(watch_id) {
            self.take_todo_projects(projects::from_rows(&rows));
            return self.surface == Surface::Todos;
        }
        if self.todos.todo_watch != Some(watch_id) {
            return false;
        }
        self.take_todo_rows(&rows);
        self.surface == Surface::Todos
    }

    fn take_todo_projects(&mut self, registry: projects::Registry) {
        let previous = self.todos.project_root.clone();
        let root = previous
            .filter(|root| registry.values().any(|project| &project.root == root))
            .or_else(|| {
                projects::for_cwd(&registry, &Location::local(&self.todos.initial_cwd))
                    .map(|project| project.root.clone())
            })
            .or_else(|| registry.values().next().map(|project| project.root.clone()));
        self.todos.projects = registry;
        if self.todos.project_root != root {
            self.watch_todo_root(root);
        }
    }

    fn watch_todo_root(&mut self, root: Option<String>) {
        if let Some(watch_id) = self.todos.todo_watch.take() {
            let _ = data_unwatch(watch_id);
        }
        self.todos.project_root = root.clone();
        self.todos.list = Default::default();
        self.todos.line_cache = None;
        self.todos.selected = None;
        self.todos.mode = Mode::Normal;
        self.todos.list_scroll = None;
        let Some(root) = root else {
            self.todos.status = "no remembered projects".into();
            return;
        };
        match Host::shared().watch(
            &FetchRequest::entity("todo")
                .filter(Pred::eq("root", root.as_str()))
                .sort_by("id", SortDir::Asc)
                .limit(todos::MAX_ITEMS as u32),
            WatchMode::Rows,
        ) {
            Ok(handle) => {
                self.todos.todo_watch = Some(handle.watch_id);
                self.todos.status.clear();
                self.take_todo_rows(&handle.rows);
            },
            Err(error) => self.todos.status = error,
        }
    }

    fn take_todo_rows(&mut self, rows: &DataRows) {
        let root = self.todos.project_root.as_deref().unwrap_or_default();
        let before = todo_rows(&self.todos.list, self.todos.show_done);
        self.todos.list = todos::from_rows(rows).remove(root).unwrap_or_default();
        self.todos.line_cache = None;
        let visible = todo_rows(&self.todos.list, self.todos.show_done);
        self.todos.selected = nearest_surviving(&before, &visible, self.todos.selected);
        self.todos.list_scroll = None;
        self.todos.status.clear();
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
        match &self.todos.mode {
            Mode::Input { .. } => return self.todo_input_key(key),
            Mode::Delete { .. } => return self.todo_delete_key(key),
            Mode::Move { .. } => return self.todo_move_key(key),
            Mode::Projects { .. } => return self.todo_project_key(key),
            Mode::Normal => {},
        }
        if key.bare_key == BareKey::Tab {
            if key.has_modifiers(&[KeyModifier::Shift]) {
                self.tabs_focused = true;
            } else {
                self.yield_to_chat(wire::STOP_TABS);
            }
            return true;
        }
        if key.has_modifiers(&[KeyModifier::Ctrl]) {
            match key.bare_key {
                BareKey::Char('o') => {
                    pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_REPOSITION));
                },
                BareKey::Char('r') => self.watch_todo_root(self.todos.project_root.clone()),
                _ => return false,
            }
            return true;
        }
        if key.bare_key == BareKey::Char('A')
            && (key.has_no_modifiers() || key.has_modifiers(&[KeyModifier::Shift]))
        {
            let Some(parent_id) = self.todos.selected else {
                return false;
            };
            return self.begin_todo_input(
                InputAction::Add {
                    parent_id: Some(parent_id),
                },
                "",
            );
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
            BareKey::Char('a') if self.todos.project_root.is_some() => {
                self.begin_todo_input(InputAction::Add { parent_id: None }, "")
            },
            BareKey::Char('e') => {
                let Some(item) = self.selected_todo().cloned() else {
                    return false;
                };
                self.begin_todo_input(
                    InputAction::Edit {
                        id: item.id,
                        rev: item.rev,
                    },
                    &item.text,
                )
            },
            BareKey::Enter | BareKey::Char(' ') => self.toggle_todo(),
            BareKey::Char('d') => self.arm_todo_delete(),
            BareKey::Char('m') => self.arm_todo_move(),
            _ => false,
        }
    }

    fn todo_projects(&self) -> Vec<String> {
        self.todos
            .projects
            .values()
            .map(|project| project.root.clone())
            .collect()
    }

    fn todo_project_key(&mut self, key: KeyWithModifier) -> bool {
        if !key.has_no_modifiers() {
            return true;
        }
        let roots = self.todo_projects();
        let Mode::Projects { selected } = &mut self.todos.mode else {
            return false;
        };
        match key.bare_key {
            BareKey::Esc => self.todos.mode = Mode::Normal,
            BareKey::Up | BareKey::Char('k') => {
                *selected = selected.saturating_sub(1);
                self.todos.list_scroll = None;
            },
            BareKey::Down | BareKey::Char('j') => {
                *selected = (*selected + 1).min(roots.len().saturating_sub(1));
                self.todos.list_scroll = None;
            },
            BareKey::Enter => {
                let root = roots.get(*selected).cloned();
                self.watch_todo_root(root);
            },
            _ => {},
        }
        true
    }

    fn begin_todo_input(&mut self, action: InputAction, text: &str) -> bool {
        self.todos.mode = Mode::Input {
            action,
            text: text.to_owned(),
        };
        true
    }

    fn todo_input_key(&mut self, key: KeyWithModifier) -> bool {
        if key.bare_key == BareKey::Esc {
            self.todos.mode = Mode::Normal;
            return true;
        }
        let Mode::Input { action, text } = &mut self.todos.mode else {
            return false;
        };
        match key.bare_key {
            BareKey::Enter => {
                let text = text.clone();
                let action = std::mem::replace(action, InputAction::Add { parent_id: None });
                self.todos.mode = Mode::Normal;
                self.commit_todo_input(action, text);
            },
            BareKey::Backspace if key.has_no_modifiers() => {
                text.pop();
            },
            BareKey::Char(ch)
                if key.has_no_modifiers() || key.has_modifiers(&[KeyModifier::Shift]) =>
            {
                text.push(ch)
            },
            _ => {},
        }
        true
    }

    fn commit_todo_input(&mut self, action: InputAction, text: String) {
        let Some(root) = self.todos.project_root.clone() else {
            return;
        };
        let planned = match action {
            InputAction::Add { parent_id } => {
                todos::plan_add(&self.todos.list, &root, &text, parent_id).map(|(plan, _)| plan)
            },
            InputAction::Edit { id, rev } => {
                todos::plan_edit(&self.todos.list, id, &text, Some(rev)).map(|(plan, _)| plan)
            },
        };
        self.apply_todo_plan(planned, "saved todo");
    }

    fn toggle_todo(&mut self) -> bool {
        let (Some(root), Some(item)) = (self.todos.project_root.clone(), self.selected_todo())
        else {
            return false;
        };
        let message = if item.done {
            "reopened todo"
        } else {
            "completed todo"
        };
        let planned = todos::plan_set_done(&self.todos.list, &root, item.id, !item.done)
            .map(|(plan, _)| plan);
        self.apply_todo_plan(planned, message);
        true
    }

    fn arm_todo_delete(&mut self) -> bool {
        let (Some(root), Some(id)) = (&self.todos.project_root, self.todos.selected) else {
            return false;
        };
        match todos::plan_delete(&self.todos.list, root, id, true) {
            Ok((plan, ids)) => {
                self.todos.mode = Mode::Delete {
                    plan,
                    descendants: ids.len() - 1,
                };
            },
            Err(error) => self.todos.status = error,
        }
        true
    }

    fn todo_delete_key(&mut self, key: KeyWithModifier) -> bool {
        let mode = std::mem::take(&mut self.todos.mode);
        let Mode::Delete { plan, .. } = mode else {
            return false;
        };
        if key.bare_key == BareKey::Char('y') && key.has_no_modifiers() {
            self.apply_todo_plan(Ok(plan), "deleted todo");
        }
        true
    }

    fn arm_todo_move(&mut self) -> bool {
        let Some(item) = self.selected_todo() else {
            return false;
        };
        let id = item.id;
        let rev = item.rev;
        let target = self.move_targets(id).first().copied();
        self.todos.mode = Mode::Move { id, rev, target };
        true
    }

    fn todo_move_key(&mut self, key: KeyWithModifier) -> bool {
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
                self.commit_todo_move(id, rev, current);
            },
            BareKey::Char('t') if key.has_no_modifiers() => {
                self.commit_todo_move(id, rev, None);
            },
            _ => {},
        }
        true
    }

    fn commit_todo_move(&mut self, id: u64, rev: DataRevision, parent_id: Option<u64>) {
        self.todos.mode = Mode::Normal;
        let Some(root) = self.todos.project_root.clone() else {
            return;
        };
        let planned = todos::plan_move(&self.todos.list, &root, id, parent_id, Some(rev))
            .map(|(plan, _)| plan);
        self.apply_todo_plan(planned, "moved todo");
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
        planned: Result<Vec<zellij_tile::prelude::Mutation>, String>,
        success: &str,
    ) {
        self.todos.status = match planned {
            Err(error) => error,
            Ok(plan) => match Host::shared().apply(plan) {
                Ok(_) => success.to_owned(),
                Err(error) => todos::topology_error(error),
            },
        };
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
            Mouse::LeftClick(line, _)
                if line >= 0 && (line as usize) < self.todos.last_list_rows =>
            {
                let line = line as usize;
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

    pub(super) fn render_todos(&mut self, rows: usize, cols: usize) {
        self.todos.rendered_ids.clear();
        self.todos.rendered_projects.clear();
        if rows == 0 || cols == 0 {
            self.todos.clear_dimensions();
            return;
        }
        let list_rows = rows.saturating_sub(1);
        self.todos.last_list_rows = list_rows;
        if let Mode::Projects { selected } = self.todos.mode {
            let projects: Vec<_> = self.todos.projects.values().collect();
            let start = self
                .todos
                .list_scroll
                .unwrap_or_else(|| selected.saturating_sub(list_rows / 2))
                .min(projects.len().saturating_sub(list_rows));
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
                    line,
                    Some(cols),
                    None,
                );
            }
            self.render_todo_footer(list_rows, cols, "projects · Enter open · Esc cancel");
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
        let cursor = match self.todos.mode {
            Mode::Move { target, .. } => target,
            _ => self.todos.selected,
        };
        // Scrolling is in screen rows, so a tall item is a tall stretch of the list.
        let first = cursor
            .and_then(|id| lines.iter().position(|line| line.id == id))
            .unwrap_or(0);
        let end = lines[first..]
            .iter()
            .position(|line| Some(line.id) != cursor)
            .map_or(lines.len(), |offset| first + offset);
        let start = self
            .todos
            .list_scroll
            .unwrap_or_else(|| {
                let start = first.saturating_sub(list_rows / 2);
                // An item taller than what is left below shows its head.
                match end > start + list_rows {
                    true => first.min(end.saturating_sub(list_rows)),
                    false => start,
                }
            })
            .min(lines.len().saturating_sub(list_rows));
        self.todos.last_window = start;
        for (row, line) in lines.iter().skip(start).take(list_rows).enumerate() {
            self.todos.rendered_ids.push(line.id);
            let selected = cursor == Some(line.id);
            let body = match selected && line.first {
                true => line.line.clone().prepend(">", markdown::MdStyle::default()),
                false => line.line.clone().indent(1),
            };
            let text = markdown::style_text(&body.text, &body.styles, line.done);
            paint(
                if selected { text.selected() } else { text },
                0,
                row,
                Some(cols),
                None,
            );
        }
        if lines.is_empty() {
            paint(
                Text::new("nothing on the list").dim_range(..),
                0,
                0,
                Some(cols),
                None,
            );
        }
        let label = match &self.todos.mode {
            Mode::Input { action, text } => format!(
                "{}: {}_ · Enter save · Esc cancel",
                match action {
                    InputAction::Add { .. } => "new",
                    InputAction::Edit { .. } => "edit",
                },
                text
            ),
            Mode::Delete { descendants, .. } => match descendants {
                0 => "Delete this todo? y/n".into(),
                count => format!("Delete this todo and {count} descendants? y/n"),
            },
            Mode::Move { id, .. } => {
                format!("Move todo {id}: pick parent + Enter · t top-level · Esc cancel")
            },
            Mode::Normal if !self.todos.status.is_empty() => self.todos.status.clone(),
            Mode::Normal => format!(
                "{} · a add · A subtask · e edit · Space done · d delete · m move · v {}",
                self.todo_project_name(),
                if self.todos.show_done {
                    "hide done"
                } else {
                    "show done"
                },
            ),
            Mode::Projects { .. } => unreachable!(),
        };
        self.render_todo_footer(list_rows, cols, &label);
    }

    fn render_todo_footer(&self, top: usize, cols: usize, label: &str) {
        paint(
            Text::new(divider_line(label, self.todos.last_window, 0, cols)).dim_range(..),
            0,
            top,
            Some(cols),
            None,
        );
    }
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

    fn item(id: u64, text: &str, done: bool, children: Vec<todos::Item>) -> todos::Item {
        todos::Item {
            id,
            text: text.into(),
            done,
            rev: DataRevision::new(id as i64),
            children,
        }
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

    #[test]
    fn todo_modes_capture_the_item_and_accept_shifted_text() {
        let mut state = State::default();
        state.todos.project_root = Some("/work".into());
        state.todos.list.items = vec![item(
            1,
            "parent",
            false,
            vec![item(2, "child", false, vec![])],
        )];
        state.todos.selected = Some(1);

        assert!(state.todo_key(KeyWithModifier::new(BareKey::Char('A'))));
        assert!(matches!(
            state.todos.mode,
            Mode::Input {
                action: InputAction::Add { parent_id: Some(1) },
                ..
            }
        ));
        state.todo_key(KeyWithModifier::new(BareKey::Char('X')).with_shift_modifier());
        assert!(matches!(&state.todos.mode, Mode::Input { text, .. } if text == "X"));
        state.todo_key(KeyWithModifier::new(BareKey::Esc));

        state.todo_key(KeyWithModifier::new(BareKey::Char('d')));
        assert!(matches!(
            state.todos.mode,
            Mode::Delete { descendants: 1, .. }
        ));
        state.todo_key(KeyWithModifier::new(BareKey::Char('n')));
        assert!(matches!(state.todos.mode, Mode::Normal));

        state.todo_key(KeyWithModifier::new(BareKey::Char('m')));
        assert!(matches!(
            state.todos.mode,
            Mode::Move {
                id: 1,
                rev,
                target: None
            } if rev == DataRevision::new(1)
        ));
    }

    #[test]
    fn delete_confirmation_keeps_its_plan_across_watch_updates() {
        use zellij_tile::prelude::DataValue;

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
        let expected = todos::plan_delete(&state.todos.list, "/work", 1, true)
            .unwrap()
            .0;
        state.todo_key(KeyWithModifier::new(BareKey::Char('d')));
        assert!(state.todo_rows_changed(
            DataWatchId::new(1),
            DataRows {
                columns: ["id", "root", "text", "parent_id", "rev"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                rows: (1..=3)
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
            },
        ));
        assert_eq!(state.todos.list.items[0].children.len(), 2);
        let Mode::Delete { plan, descendants } = &state.todos.mode else {
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
        use zellij_tile::prelude::DataValue;

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
    fn delete_planning_errors_do_not_open_confirmation() {
        let mut state = State::default();
        state.todos.project_root = Some("/work".into());
        state.todos.selected = Some(1);
        assert!(state.todo_key(KeyWithModifier::new(BareKey::Char('d'))));
        assert!(matches!(state.todos.mode, Mode::Normal));
        assert!(state.todos.status.contains("no todo item 1"));
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
                        (
                            name.clone(),
                            projects::Project {
                                name,
                                root: format!("/work/{id}"),
                                firecracker_image: None,
                                remote: None,
                                worktree_status: None,
                                common_dir: None,
                                discovered: false,
                            },
                        )
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
    }

    #[test]
    fn project_picker_preserves_a_root_across_rename() {
        let project = |name: &str, root: &str| projects::Project {
            name: name.into(),
            root: root.into(),
            firecracker_image: None,
            remote: None,
            worktree_status: None,
            common_dir: None,
            discovered: false,
        };
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

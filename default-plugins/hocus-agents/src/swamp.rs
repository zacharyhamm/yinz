//! Read-only issue browsing through the host's authenticated Swamp CLI.
use super::*;
use chrono::{DateTime, FixedOffset};
use serde::Deserialize;
use zellij_tile::ui_components::forms::{Combo, Form, FormEvent, Rect, RenderBuffer, Select};

const REFRESH_MS: i64 = 60_000;
const REQUEST_ID: &str = "swamp_issue_request";
const ISSUE_NUMBER: &str = "swamp_issue_number";
const TYPES: [&str; 4] = ["", "bug", "feature", "security"];
const STATUSES: [&str; 6] = ["", "open", "triaged", "in_progress", "shipped", "closed"];
const SORTS: [&str; 4] = [
    "Updated newest first",
    "Updated oldest first",
    "Created newest first",
    "Created oldest first",
];

#[derive(Debug, Deserialize)]
struct Issue {
    number: u64,
    title: String,
    status: String,
    #[serde(rename = "type")]
    kind: String,
    author: String,
    url: String,
    #[serde(rename = "createdAt", default, deserialize_with = "timestamp")]
    created_at: Option<DateTime<FixedOffset>>,
    #[serde(rename = "updatedAt", default, deserialize_with = "timestamp")]
    updated_at: Option<DateTime<FixedOffset>>,
}

fn timestamp<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<DateTime<FixedOffset>>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_str()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok()))
}

#[derive(Debug, Deserialize)]
struct Search {
    issues: Vec<Issue>,
    total: usize,
}

#[derive(Debug, Deserialize)]
struct Detail {
    #[serde(flatten)]
    issue: Issue,
    body: String,
    assignees: Vec<String>,
    comments: Vec<Comment>,
}

#[derive(Debug, Deserialize)]
struct Comment {
    author: String,
    body: String,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Default, PartialEq)]
enum Permission {
    #[default]
    Unrequested,
    Waiting,
    Granted,
    Denied,
}

#[derive(Clone, Debug, PartialEq)]
enum Target {
    Search(u64),
    Detail(u64, u64), // issue number, opening generation
}

struct Request {
    id: String,
    target: Target,
}

struct FilterEditor {
    shortcut: char,
    form: Form,
    row: Option<Rect>,
}

#[derive(Default)]
pub(super) struct View {
    // The surface starts available; a PaneUpdate must confirm the pane is shown.
    surface_hidden: bool,
    pub(super) pane_visible: bool,
    permission: Permission,
    query: String,
    editing: Option<String>,
    status: usize,
    kind: usize,
    sort: usize,
    source: String,
    filter: Option<FilterEditor>,
    source_tags: Vec<String>,
    filter_cells: Vec<(usize, usize, usize, char)>, // row, column, width, shortcut
    search: Option<Search>,
    selected: usize,
    list_scroll: Option<usize>,
    list_rows: Vec<(usize, markdown::MdLine)>,
    list_error: Option<String>,
    opened: Option<u64>,
    detail: Option<Detail>,
    detail_scroll: usize,
    detail_error: Option<String>,
    // Only one CLI process at a time. Changes made while it runs coalesce here;
    // generations keep a superseded search or a closed reader from taking its reply.
    pending: Option<Request>,
    search_wanted: bool,
    detail_wanted: bool,
    search_generation: u64,
    detail_generation: u64,
    sequence: u64,
    last_refresh: Option<i64>,
    last_body_top: usize,
    last_body_rows: usize,
    window: usize,
    max_scroll: usize,
    detail_cache: Option<(usize, Vec<markdown::MdLine>)>,
    #[cfg(test)]
    commands: Vec<(Vec<String>, BTreeMap<String, String>)>,
}

impl View {
    fn layout_filters(&mut self, rows: usize, cols: usize) -> (usize, Vec<String>) {
        let mut top = 1 + usize::from(self.note().is_some() && rows > 1);
        let mut labels = Vec::new();
        self.filter_cells.clear();
        if self.opened.is_none() {
            let mut x = 0;
            for (label, value, shortcut) in [
                ("Status", status_label(STATUSES[self.status]), 's'),
                (
                    "Type",
                    if self.kind == 0 {
                        "All"
                    } else {
                        TYPES[self.kind]
                    },
                    't',
                ),
                (
                    "Source",
                    if self.source.is_empty() {
                        "All"
                    } else {
                        &self.source
                    },
                    'f',
                ),
                ("Sort", SORTS[self.sort], 'o'),
            ] {
                let text = fit(&format!("[{label}: {value} ({shortcut})]"), cols);
                let width = text.width();
                if x > 0 && x + width > cols {
                    top += 1;
                    x = 0;
                }
                if top >= rows {
                    break;
                }
                labels.push(text);
                self.filter_cells.push((top, x, width, shortcut));
                x += width + 1;
            }
            top = (top + 1).min(rows);
        }
        (top, labels)
    }

    fn visible(&self) -> bool {
        !self.surface_hidden && self.pane_visible
    }

    pub(super) fn is_editing(&self) -> bool {
        self.editing.is_some() || self.filter.is_some()
    }

    pub(super) fn cancel_filter(&mut self) {
        self.filter = None;
    }

    pub(super) fn has_filter_editor(&self) -> bool {
        self.filter.is_some()
    }

    pub(super) fn clear_dimensions(&mut self) {
        if let Some(editor) = &mut self.filter {
            editor.row = None;
            editor.form.render_buffer_in_viewport(0, 0, 0, 0);
        }
        self.filter_cells.clear();
        self.last_body_rows = 0;
        self.last_body_top = 0;
        self.window = 0;
        self.max_scroll = 0;
    }

    pub(super) fn has_error(&self) -> bool {
        self.list_error.is_some()
            || self.detail_error.is_some()
            || self.permission == Permission::Denied
    }

    fn issues(&self) -> &[Issue] {
        self.search.as_ref().map_or(&[], |search| &search.issues)
    }

    fn selected_number(&self) -> Option<u64> {
        self.issues().get(self.selected).map(|issue| issue.number)
    }

    fn sort_issues(&mut self, selected: Option<u64>) {
        let Some(search) = &mut self.search else {
            return;
        };
        search.issues.sort_by(|a, b| {
            let (a_time, b_time) = if self.sort < 2 {
                (a.updated_at, b.updated_at)
            } else {
                (a.created_at, b.created_at)
            };
            match (a_time, b_time) {
                (Some(a), Some(b)) if self.sort == 0 || self.sort == 2 => b.cmp(&a),
                (Some(a), Some(b)) => a.cmp(&b),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
            .then_with(|| b.number.cmp(&a.number))
        });
        self.selected = search
            .issues
            .iter()
            .position(|issue| Some(issue.number) == selected)
            .unwrap_or(self.selected.min(search.issues.len().saturating_sub(1)));
        self.list_scroll = None;
    }

    pub(super) fn shows_an_age(&self) -> bool {
        self.visible()
            && self.opened.is_none()
            && self.last_body_rows > 0
            && self
                .issues()
                .iter()
                .any(|issue| issue.created_at.is_some() || issue.updated_at.is_some())
    }

    fn note(&self) -> Option<String> {
        match self.permission {
            Permission::Unrequested | Permission::Waiting => {
                return Some("Waiting for permission to run Swamp commands".into());
            },
            Permission::Denied => {
                return Some("Permission denied: r to request command permission again".into());
            },
            Permission::Granted => {},
        }
        let error = if self.opened.is_some() {
            self.detail_error.as_ref().or(self.list_error.as_ref())
        } else {
            self.list_error.as_ref()
        };
        if let Some(error) = error {
            return Some(format!("Error: {error} · r retry"));
        }
        (self.pending.is_some() || self.search_wanted || self.detail_wanted)
            .then(|| "Loading…".into())
    }

    fn header(&self) -> String {
        if let Some(query) = &self.editing {
            return format!("/{query} · Enter apply · Esc cancel");
        }
        if let Some(number) = self.opened {
            return format!("#{number} · Esc back · r refresh");
        }
        let count = self.search.as_ref().map_or(String::new(), |search| {
            if search.total > search.issues.len() {
                format!(" · {} of {}", search.issues.len(), search.total)
            } else {
                format!(" · {} issues", search.issues.len())
            }
        });
        let query = if self.query.is_empty() {
            String::new()
        } else {
            format!(" · {}", self.query)
        };
        format!("Issues{count}{query} · / search · r refresh")
    }
}

impl State {
    pub(super) fn swamp_panes(&mut self, manifest: &PaneManifest) -> bool {
        // PaneUpdate reaches the active tab only. It also reports hidden overlays,
        // which Visible (the tab/floating-surface event) does not report on hide/show.
        let Some(pane) = manifest
            .panes
            .values()
            .flatten()
            .find(|pane| pane.id == PaneId::Plugin(self.own_plugin_id))
        else {
            return false;
        };
        let was_visible = self.swamp.visible();
        self.swamp.pane_visible = !pane.is_suppressed;
        self.swamp_visibility_changed(was_visible)
    }

    pub(super) fn swamp_visibility(&mut self, visible: bool) -> bool {
        let was_visible = self.swamp.visible();
        self.swamp.surface_hidden = !visible;
        self.swamp_visibility_changed(was_visible)
    }

    fn swamp_visibility_changed(&mut self, was_visible: bool) -> bool {
        let visible = self.swamp.visible();
        if !visible {
            self.swamp.cancel_filter();
        }
        if was_visible == visible {
            return false;
        }
        if visible {
            self.poll_swamp(now_ms(), true);
        }
        self.surface == Surface::Swamp
    }

    pub(super) fn swamp_permission(&mut self, status: PermissionStatus) -> bool {
        if self.swamp.permission != Permission::Waiting {
            return false;
        }
        self.swamp.permission = match status {
            PermissionStatus::Granted => Permission::Granted,
            PermissionStatus::Denied => Permission::Denied,
        };
        self.dispatch_swamp();
        self.surface == Surface::Swamp && self.swamp.visible()
    }

    pub(super) fn poll_swamp(&mut self, now: i64, force: bool) -> bool {
        if self.surface != Surface::Swamp || !self.swamp.visible() {
            return false;
        }
        if self.swamp.pending.is_some() {
            return false;
        }
        if !force
            && self
                .swamp
                .last_refresh
                .is_some_and(|last| now.saturating_sub(last) < REFRESH_MS)
        {
            return false;
        }
        self.swamp.last_refresh = Some(now);
        self.swamp.search_wanted = true;
        self.swamp.detail_wanted = self.swamp.opened.is_some();
        self.dispatch_swamp();
        true
    }

    fn dispatch_swamp(&mut self) {
        if self.surface != Surface::Swamp || !self.swamp.visible() || self.swamp.pending.is_some() {
            return;
        }
        match self.swamp.permission {
            Permission::Unrequested => {
                self.swamp.permission = Permission::Waiting;
                request_permission(&[PermissionType::RunCommands]);
                return;
            },
            Permission::Granted => {},
            _ => return,
        }
        let (target, args) = if self.swamp.search_wanted {
            self.swamp.search_wanted = false;
            let args = match search_args(
                &self.swamp.query,
                STATUSES[self.swamp.status],
                TYPES[self.swamp.kind],
                &self.swamp.source,
            ) {
                Ok(args) => args,
                Err(error) => {
                    self.swamp.list_error = Some(error);
                    return;
                },
            };
            (Target::Search(self.swamp.search_generation), args)
        } else if self.swamp.detail_wanted {
            self.swamp.detail_wanted = false;
            let Some(number) = self.swamp.opened else {
                return;
            };
            (
                Target::Detail(number, self.swamp.detail_generation),
                vec![
                    "swamp".into(),
                    "issue".into(),
                    "get".into(),
                    number.to_string(),
                    "--json".into(),
                ],
            )
        } else {
            return;
        };
        self.swamp.sequence = self.swamp.sequence.max(now_ms().max(0) as u64) + 1;
        let id = self.swamp.sequence.to_string();
        let mut context = BTreeMap::from([(REQUEST_ID.into(), id.clone())]);
        if let Target::Detail(number, _) = target {
            context.insert(ISSUE_NUMBER.into(), number.to_string());
        }
        #[cfg(test)]
        self.swamp.commands.push((args.clone(), context.clone()));
        run_command(
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            context,
        );
        self.swamp.pending = Some(Request { id, target });
    }

    pub(super) fn swamp_result(
        &mut self,
        code: Option<i32>,
        stdout: &[u8],
        stderr: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        let Some(request) = self.swamp.pending.as_ref() else {
            return false;
        };
        if context.get(REQUEST_ID) != Some(&request.id) {
            return false;
        }
        if let Target::Detail(number, _) = request.target {
            if context.get(ISSUE_NUMBER) != Some(&number.to_string()) {
                return false;
            }
        }
        let target = request.target.clone();
        self.swamp.pending = None;
        match target {
            Target::Search(generation) if generation == self.swamp.search_generation => {
                match decode::<Search>(code, stdout, stderr) {
                    Ok(search) => {
                        let selected = self.swamp.selected_number();
                        self.swamp.search = Some(search);
                        self.swamp.sort_issues(selected);
                        self.swamp.list_error = None;
                    },
                    Err(error) => self.swamp.list_error = Some(error),
                }
            },
            Target::Detail(number, generation)
                if self.swamp.opened == Some(number)
                    && generation == self.swamp.detail_generation =>
            {
                match decode::<Detail>(code, stdout, stderr) {
                    Ok(detail) if detail.issue.number == number => {
                        self.swamp.detail = Some(detail);
                        self.swamp.detail_error = None;
                        self.swamp.detail_cache = None;
                    },
                    Ok(_) => {
                        self.swamp.detail_error =
                            Some("Malformed Swamp response: wrong issue number".into())
                    },
                    Err(error) => self.swamp.detail_error = Some(error),
                }
            },
            _ => {},
        }
        self.dispatch_swamp();
        self.surface == Surface::Swamp && self.swamp.visible()
    }

    fn open_swamp_issue(&mut self) {
        let Some(number) = self.swamp.selected_number() else {
            return;
        };
        self.swamp.opened = Some(number);
        self.swamp.detail = None;
        self.swamp.detail_cache = None;
        self.swamp.detail_error = None;
        self.swamp.detail_scroll = 0;
        self.swamp.detail_generation += 1;
        self.swamp.detail_wanted = true;
        self.dispatch_swamp();
    }

    fn search_swamp(&mut self) {
        self.swamp.search_generation += 1;
        self.swamp.search_wanted = true;
        self.swamp.list_scroll = None;
        self.swamp.last_refresh = Some(now_ms());
        self.dispatch_swamp();
    }

    pub(super) fn swamp_key(&mut self, key: KeyWithModifier) -> bool {
        if let Some(editor) = &mut self.swamp.filter {
            if key.bare_key == BareKey::Esc || key.bare_key == BareKey::Tab {
                self.swamp.cancel_filter();
                return true;
            }
            let enter = key == KeyWithModifier::new(BareKey::Enter);
            let event = editor.form.handle_key(key);
            if event == FormEvent::Cancel {
                self.swamp.cancel_filter();
            } else if enter
                && editor
                    .form
                    .select_by_id("value")
                    .is_none_or(|select| !select.is_open())
            {
                self.commit_swamp_filter();
            }
            return true;
        }
        if let Some(query) = self.swamp.editing.as_mut() {
            match key.bare_key {
                BareKey::Esc => self.swamp.editing = None,
                BareKey::Enter => {
                    if let Err(error) = search_args(
                        query.trim(),
                        STATUSES[self.swamp.status],
                        TYPES[self.swamp.kind],
                        &self.swamp.source,
                    ) {
                        self.swamp.list_error = Some(error);
                        return true;
                    }
                    self.swamp.query = query.trim().to_owned();
                    self.swamp.editing = None;
                    self.search_swamp();
                },
                BareKey::Backspace => {
                    query.pop();
                },
                BareKey::Char(ch) if key.has_no_modifiers() => query.push(ch),
                _ => return false,
            }
            return true;
        }
        if key.bare_key == BareKey::Tab {
            if key.has_modifiers(&[KeyModifier::Shift]) {
                self.tabs_focused = true;
            } else {
                self.yield_to_chat(wire::STOP_TABS);
            }
            return true;
        }
        if key.bare_key == BareKey::Char('o') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_REPOSITION));
            return true;
        }
        if !key.has_no_modifiers() {
            return false;
        }
        let page = self.swamp.last_body_rows.saturating_sub(1).max(1) as isize;
        match key.bare_key {
            BareKey::Esc if self.swamp.opened.is_some() => {
                self.swamp.opened = None;
                self.swamp.detail = None;
                self.swamp.detail_cache = None;
                self.swamp.detail_error = None;
                self.swamp.detail_generation += 1;
                self.swamp.detail_wanted = false;
            },
            BareKey::Esc => self.hide_the_panel(),
            BareKey::Char('r') => {
                if self.swamp.permission == Permission::Denied {
                    self.swamp.permission = Permission::Unrequested;
                }
                self.poll_swamp(now_ms(), true);
            },
            BareKey::Char('/') if self.swamp.opened.is_none() => {
                self.swamp.editing = Some(self.swamp.query.clone());
            },
            BareKey::Char(shortcut @ ('s' | 't' | 'f' | 'o')) if self.swamp.opened.is_none() => {
                self.open_swamp_filter(shortcut);
            },
            BareKey::Enter if self.swamp.opened.is_none() => self.open_swamp_issue(),
            BareKey::Up | BareKey::Char('k') => self.swamp_step(-1),
            BareKey::Down | BareKey::Char('j') => self.swamp_step(1),
            BareKey::PageUp => self.swamp_page(-page),
            BareKey::PageDown => self.swamp_page(page),
            BareKey::Home => self.swamp_step(-isize::MAX),
            BareKey::End => self.swamp_step(isize::MAX),
            _ => return false,
        }
        true
    }

    fn open_swamp_filter(&mut self, shortcut: char) {
        let mut form = match shortcut {
            's' => Form::new().select(
                "value",
                "Status",
                Select::new(STATUSES.map(status_label)).with_selected(self.swamp.status),
            ),
            't' => Form::new().select(
                "value",
                "Type",
                Select::new(TYPES.map(|kind| if kind.is_empty() { "All" } else { kind }))
                    .with_selected(self.swamp.kind),
            ),
            'o' => Form::new().select(
                "value",
                "Sort",
                Select::new(SORTS).with_selected(self.swamp.sort),
            ),
            _ => Form::new().combo(
                "value",
                "Source",
                Combo::new(
                    std::iter::once("swamp")
                        .chain(self.swamp.source_tags.iter().map(String::as_str)),
                )
                .with_text(&self.swamp.source),
            ),
        };
        form.handle_key(KeyWithModifier::new(BareKey::Enter));
        self.swamp.filter = Some(FilterEditor {
            shortcut,
            form,
            row: None,
        });
    }

    fn commit_swamp_filter(&mut self) {
        let Some(editor) = &mut self.swamp.filter else {
            return;
        };
        let sorting = editor.shortcut == 'o';
        let changed = if let Some(select) = editor.form.select_by_id("value") {
            let Some(index) = select.selected_index() else {
                return;
            };
            let value = match editor.shortcut {
                's' => &mut self.swamp.status,
                'o' => &mut self.swamp.sort,
                _ => &mut self.swamp.kind,
            };
            let changed = *value != index;
            *value = index;
            changed
        } else {
            let source = editor.form.text("value").trim().to_owned();
            if let Err(error) = search_args(
                &self.swamp.query,
                STATUSES[self.swamp.status],
                TYPES[self.swamp.kind],
                &source,
            ) {
                self.swamp.list_error = Some(error);
                editor.form.handle_key(KeyWithModifier::new(BareKey::Enter));
                return;
            }
            if !source.is_empty() && source != "swamp" && !self.swamp.source_tags.contains(&source)
            {
                self.swamp.source_tags.push(source.clone());
            }
            let changed = self.swamp.source != source;
            self.swamp.source = source;
            changed
        };
        self.swamp.cancel_filter();
        if changed {
            if sorting {
                self.swamp.sort_issues(self.swamp.selected_number());
            } else {
                self.search_swamp();
            }
        }
    }

    // Forms use pane coordinates, including the surface-tab row. Consume every
    // event while editing so an overlay click cannot also open an issue below it.
    pub(super) fn swamp_filter_mouse(&mut self, mouse: Mouse) -> bool {
        let Some(editor) = &mut self.swamp.filter else {
            return false;
        };
        let overlay = editor.form.overlay_rect();
        if let Mouse::LeftClick(line, column) = mouse {
            if line < 0
                || !editor
                    .row
                    .into_iter()
                    .chain(overlay)
                    .any(|rect| rect.contains(column, line as usize))
            {
                self.swamp.cancel_filter();
                return true;
            }
        }
        let event = editor.form.handle_mouse(mouse);
        if let Mouse::Release(line, column) = mouse {
            if line >= 0
                && overlay.is_some_and(|rect| rect.contains(column, line as usize))
                && event != FormEvent::Ignored
                && editor.form.overlay_rect().is_none()
            {
                self.commit_swamp_filter();
            }
        }
        true
    }

    fn swamp_page(&mut self, delta: isize) {
        if self.swamp.opened.is_some() {
            self.swamp_step(delta);
            return;
        }
        let first = self
            .swamp
            .list_rows
            .iter()
            .position(|(index, _)| *index == self.swamp.selected)
            .unwrap_or(0);
        let target = first
            .saturating_add_signed(delta)
            .min(self.swamp.list_rows.len().saturating_sub(1));
        let index = self
            .swamp
            .list_rows
            .get(target)
            .map_or(self.swamp.selected, |(index, _)| *index);
        let step = index as isize - self.swamp.selected as isize;
        self.swamp_step(if step == 0 { delta.signum() } else { step });
    }

    fn swamp_step(&mut self, delta: isize) {
        if self.swamp.opened.is_some() {
            self.swamp.detail_scroll = self
                .swamp
                .detail_scroll
                .saturating_add_signed(delta)
                .min(self.swamp.max_scroll);
        } else {
            self.swamp.selected = self
                .swamp
                .selected
                .saturating_add_signed(delta)
                .min(self.swamp.issues().len().saturating_sub(1));
            self.swamp.list_scroll = None;
        }
    }

    pub(super) fn swamp_mouse(&mut self, mouse: Mouse) -> bool {
        if self.swamp.opened.is_none() && self.swamp.editing.is_none() {
            if let Mouse::LeftClick(line, column) = mouse {
                if let Some((_, _, _, shortcut)) =
                    self.swamp.filter_cells.iter().find(|(row, x, width, _)| {
                        line == *row as isize && column >= *x && column < x + width
                    })
                {
                    return self.swamp_key(KeyWithModifier::new(BareKey::Char(*shortcut)));
                }
            }
        }
        if self.swamp.last_body_rows == 0 {
            return false;
        }
        match mouse {
            Mouse::ScrollUp(lines) | Mouse::ScrollDown(lines) => {
                let delta = lines.max(1).min(isize::MAX as usize) as isize;
                let delta = if matches!(mouse, Mouse::ScrollUp(_)) {
                    -delta
                } else {
                    delta
                };
                if self.swamp.opened.is_some() {
                    self.swamp_step(delta);
                } else {
                    self.swamp.list_scroll = Some(
                        self.swamp
                            .window
                            .saturating_add_signed(delta)
                            .min(self.swamp.max_scroll),
                    );
                }
                true
            },
            Mouse::LeftClick(line, _)
                if self.swamp.opened.is_none() && self.swamp.editing.is_none() =>
            {
                let Some(row) = (line as usize).checked_sub(self.swamp.last_body_top) else {
                    return false;
                };
                let index = self.swamp.window + row;
                if row >= self.swamp.last_body_rows {
                    return false;
                }
                let Some((issue, _)) = self.swamp.list_rows.get(index) else {
                    return false;
                };
                self.swamp.selected = *issue;
                self.open_swamp_issue();
                true
            },
            _ => false,
        }
    }

    pub(super) fn swamp_filter_buffer(&mut self, rows: usize, cols: usize) -> Option<RenderBuffer> {
        if rows == 0 || cols == 0 || self.swamp.filter.is_none() {
            return None;
        }
        self.swamp.layout_filters(rows, cols);
        if let Some(editor) = &mut self.swamp.filter {
            let (row, x) = self
                .swamp
                .filter_cells
                .iter()
                .find(|(_, _, _, shortcut)| *shortcut == editor.shortcut)
                .map_or((rows - 1, 0), |(row, x, _, _)| (*row, *x));
            let width = cols.min(32);
            let x = x.min(cols - width);
            let y = row + 1;
            editor.row = Some(Rect::new(x, y, width, 1));
            return Some(
                editor
                    .form
                    .render_buffer_with_overlay(x, y, width, 1, rows + 1),
            );
        }
        None
    }

    pub(super) fn render_swamp(&mut self, rows: usize, cols: usize) {
        if rows == 0 || cols == 0 {
            self.swamp.clear_dimensions();
            return;
        }
        let note = self.swamp.note();
        let (top, labels) = self.swamp.layout_filters(rows, cols);
        for ((row, x, width, _), text) in self.swamp.filter_cells.iter().zip(labels) {
            paint(
                Text::new(text).color_range(2, ..),
                *x,
                *row,
                Some(*width),
                None,
            );
        }
        let room = rows.saturating_sub(top);
        self.swamp.last_body_top = top;
        self.swamp.last_body_rows = room;
        if self.swamp.opened.is_some() {
            if let Some(detail) = &self.swamp.detail {
                if self
                    .swamp
                    .detail_cache
                    .as_ref()
                    .is_none_or(|(width, _)| *width != cols)
                {
                    self.swamp.detail_cache =
                        Some((cols, detail_lines(detail, cols.saturating_sub(2).max(1))));
                }
            }
            let lines = self
                .swamp
                .detail_cache
                .as_ref()
                .map_or(&[][..], |(_, lines)| lines.as_slice());
            self.swamp.max_scroll = lines.len().saturating_sub(room);
            if room > 0 {
                self.swamp.detail_scroll = self.swamp.detail_scroll.min(self.swamp.max_scroll);
            }
            let scroll = self.swamp.detail_scroll.min(lines.len());
            let end = scroll.saturating_add(room).min(lines.len());
            render_divider(
                &self.swamp.header(),
                scroll,
                lines.len() - end,
                !self.tabs_focused,
                0,
                cols,
            );
            for (row, line) in lines[scroll..end].iter().enumerate() {
                let text = markdown::style_text(&line.text, &line.styles, false);
                paint(
                    text,
                    usize::from(cols > 1),
                    top + row,
                    Some(cols.saturating_sub(1).max(1)),
                    None,
                );
            }
        } else {
            paint(
                Text::new(fit(&self.swamp.header(), cols)).color_range(2, ..),
                0,
                0,
                Some(cols),
                None,
            );
            let now = now_ms();
            self.swamp.list_rows = self
                .swamp
                .issues()
                .iter()
                .enumerate()
                .flat_map(|(index, issue)| {
                    issue_lines(issue, index == self.swamp.selected, cols, now)
                        .into_iter()
                        .map(move |line| (index, line))
                })
                .collect();
            let lines = &self.swamp.list_rows;
            self.swamp.max_scroll = lines.len().saturating_sub(room);
            let first = lines
                .iter()
                .position(|(index, _)| *index == self.swamp.selected)
                .unwrap_or(0);
            let end = lines
                .iter()
                .rposition(|(index, _)| *index == self.swamp.selected)
                .map_or(0, |row| row + 1);
            let start = self
                .swamp
                .list_scroll
                .unwrap_or_else(|| {
                    if end - first >= room {
                        first
                    } else {
                        self.swamp.window.min(first).max(end.saturating_sub(room))
                    }
                })
                .min(self.swamp.max_scroll);
            self.swamp.window = start;
            if room > 0 {
                if self.swamp.list_scroll.is_some() {
                    self.swamp.list_scroll = Some(start);
                }
                if self.swamp.issues().is_empty() && note.is_none() {
                    paint(
                        Text::new(fit("No issues found", cols)).dim_range(..),
                        0,
                        top,
                        Some(cols),
                        None,
                    );
                }
                for (row, (index, line)) in lines.iter().skip(start).take(room).enumerate() {
                    let text = list_text(line, *index == self.swamp.selected);
                    paint(text, 0, top + row, Some(cols), None);
                }
            }
        }
        if let Some(note) = note {
            if rows > 1 {
                let text = Text::new(fit(&note, cols));
                let text = if self.swamp.has_error() {
                    text.error_color_range(..)
                } else {
                    text.dim_range(..)
                };
                paint(text, 0, 1, Some(cols), None);
            }
        }
    }
}

fn search_args(query: &str, status: &str, kind: &str, source: &str) -> Result<Vec<String>, String> {
    // The installed CLI ignores positionals after `--`. Reject an option-shaped
    // keyword rather than silently changing the meaning of the search.
    if query.starts_with('-') {
        return Err("Search must start with a keyword, not '-'".into());
    }
    if source.starts_with('-') {
        return Err("Source must start with a tag, not '-'".into());
    }
    let mut args: Vec<String> = ["swamp", "issue", "search", "--json", "--limit", "100"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    for (flag, value) in [("--status", status), ("--type", kind), ("--source", source)] {
        if !value.is_empty() {
            args.extend([flag.into(), value.into()]);
        }
    }
    if !query.is_empty() {
        args.push(query.into());
    }
    Ok(args)
}

fn decode<T: serde::de::DeserializeOwned>(
    code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<T, String> {
    let value = serde_json::from_slice::<serde_json::Value>(stdout);
    if let Ok(value) = &value {
        if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
            return Err(error.into());
        }
    }
    if code != Some(0) {
        let stderr = String::from_utf8_lossy(stderr);
        if code == Some(127) || (stdout.is_empty() && stderr.contains("No such file or directory"))
        {
            return Err("Swamp command not found on the server PATH".into());
        }
        return Err(if stderr.trim().is_empty() {
            format!("Swamp command failed (exit {code:?})")
        } else {
            stderr.trim().into()
        });
    }
    value
        .and_then(serde_json::from_value)
        .map_err(|error| format!("Malformed Swamp response: {error}"))
}

fn status_label(status: &str) -> &str {
    match status {
        "" => "All",
        "open" => "Open",
        "triaged" => "Triaged",
        "in_progress" => "In progress",
        "shipped" => "Shipped",
        "closed" => "Closed",
        other => other,
    }
}

fn fit(text: &str, cols: usize) -> String {
    let text = zellij_tile::text::sanitize(text).replace('\n', " ");
    agents::cut_to_width(text, cols)
}

const METADATA: markdown::MdStyle = markdown::MdStyle {
    dim: true,
    bold: false,
    color: None,
    error: false,
    strike: false,
};
const TITLE: markdown::MdStyle = markdown::MdStyle {
    bold: true,
    dim: false,
    color: Some(1),
    ..METADATA
};

fn status_style(status: &str) -> markdown::MdStyle {
    markdown::MdStyle {
        color: match status {
            "open" => Some(1),
            "triaged" => Some(0),
            "in_progress" => Some(3),
            "shipped" => Some(2),
            _ => None,
        },
        ..Default::default()
    }
}

fn styled_line(runs: &[(&str, markdown::MdStyle)]) -> markdown::MdLine {
    runs.iter()
        .rev()
        .fold(markdown::MdLine::blank(), |line, (text, style)| {
            line.prepend(&zellij_tile::text::sanitize(text), *style)
        })
}

fn clip_line(mut line: markdown::MdLine, cols: usize) -> markdown::MdLine {
    line.text = fit(&line.text, cols);
    line.styles.truncate(line.text.chars().count());
    line
}

fn wrap_line(line: markdown::MdLine, cols: usize) -> Vec<markdown::MdLine> {
    let chars = line
        .text
        .chars()
        .zip(line.styles)
        .map(|(ch, style)| markdown::StyledChar { ch, style })
        .collect::<Vec<_>>();
    markdown::word_wrap_styled(&chars, cols)
        .into_iter()
        .map(|chars| {
            clip_line(
                markdown::MdLine {
                    text: chars.iter().map(|ch| ch.ch).collect(),
                    styles: chars.iter().map(|ch| ch.style).collect(),
                },
                cols,
            )
        })
        .collect()
}

fn list_text(line: &markdown::MdLine, selected: bool) -> Text {
    let text = markdown::style_text(&line.text, &line.styles, false);
    if selected {
        text.selected()
    } else {
        text
    }
}

fn issue_lines(issue: &Issue, selected: bool, cols: usize, now: i64) -> Vec<markdown::MdLine> {
    let mut lines = vec![clip_line(
        styled_line(&[
            (
                if selected { ">" } else { " " },
                markdown::MdStyle::default(),
            ),
            (&format!(" #{} ", issue.number), METADATA),
            (status_label(&issue.status), status_style(&issue.status)),
            (&format!(" · {}", issue.kind), METADATA),
        ]),
        cols,
    )];
    // Leave room for a wide Unicode character before adding the indentation.
    let indent = if cols >= 4 { "  " } else { "" };
    let age = |timestamp: Option<DateTime<FixedOffset>>| {
        timestamp.map_or_else(
            || "unknown".into(),
            |timestamp| match agents::short_age(now, timestamp.timestamp_millis()) {
                age if age == "now" => age,
                age => format!("{age} ago"),
            },
        )
    };
    let timestamps = format!(
        "Created {} · Updated {}",
        age(issue.created_at),
        age(issue.updated_at),
    );
    for (text, style) in [(&issue.title, TITLE), (&timestamps, METADATA)] {
        lines.extend(
            wrap_line(styled_line(&[(text, style)]), cols - indent.len())
                .into_iter()
                .map(|line| line.indent(indent.len())),
        );
    }
    lines
}

fn detail_lines(detail: &Detail, cols: usize) -> Vec<markdown::MdLine> {
    let issue = &detail.issue;
    let assignees = if detail.assignees.is_empty() {
        "none".into()
    } else {
        detail.assignees.join(", ")
    };
    let mut lines = Vec::new();
    let precise = |timestamp: Option<DateTime<FixedOffset>>| {
        timestamp.map_or_else(|| "unknown".into(), |timestamp| timestamp.to_rfc3339())
    };
    for line in [
        styled_line(&[
            (&format!("#{} ", issue.number), METADATA),
            (&issue.title, TITLE),
        ]),
        styled_line(&[
            (status_label(&issue.status), status_style(&issue.status)),
            (
                &format!(" · {} · by {}", issue.kind, issue.author),
                METADATA,
            ),
        ]),
        styled_line(&[(&format!("Created: {}", precise(issue.created_at)), METADATA)]),
        styled_line(&[(&format!("Updated: {}", precise(issue.updated_at)), METADATA)]),
        styled_line(&[(&format!("Assignees: {assignees}"), METADATA)]),
        styled_line(&[(&issue.url, METADATA)]),
    ] {
        lines.extend(wrap_line(line, cols));
    }
    lines.push(markdown::MdLine::plain(""));
    lines.extend(markdown::render(
        &detail.body,
        cols,
        markdown::Options::compact(),
    ));
    lines.extend(
        zellij_tile::text::wrap(&format!("Comments ({})", detail.comments.len()), cols)
            .into_iter()
            .map(markdown::MdLine::plain),
    );
    for comment in &detail.comments {
        lines.extend(wrap_line(
            styled_line(&[(
                &format!("{} · {}", comment.author, comment.created_at),
                METADATA,
            )]),
            cols,
        ));
        lines.extend(markdown::render(
            &comment.body,
            cols,
            markdown::Options::compact(),
        ));
        lines.push(markdown::MdLine::plain(""));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // CLI shapes verified with `swamp issue search/get --json`, including a
    // shipped issue with a comment (#2180). Bodies are shortened for the fixture.
    const SEARCH: &str = r#"{"issues":[
        {"number":2254,"title":"Invite output","type":"bug","status":"triaged","author":"skunk-ape","url":"https://swamp-club.com/lab/2254"},
        {"number":2180,"title":"CLI referral link","type":"platform","status":"shipped","author":"skunk-ape","url":"https://swamp-club.com/lab/2180"}
    ],"total":2219}"#;
    const DETAIL: &str = r###"{
        "number":2180,"title":"CLI referral link","type":"platform","status":"shipped",
        "author":"skunk-ape","body":"## Referral link\n\nPrint **one URL**.",
        "assignees":["skunk-ape"],"commentCount":1,
        "comments":[{"author":"hammz","body":"Depends on **#2179**.","createdAt":"2026-09-16T16:20:51.953Z"}],
        "url":"https://swamp-club.com/lab/2180"
    }"###;

    fn ready() -> State {
        let mut state = State {
            surface: Surface::Swamp,
            ..Default::default()
        };
        state.swamp.pane_visible = true;
        state.swamp.permission = Permission::Granted;
        state
    }

    fn context(state: &State) -> BTreeMap<String, String> {
        state.swamp.commands.last().expect("a command").1.clone()
    }

    fn reply(state: &mut State, json: &str) {
        let context = context(state);
        state.swamp_result(Some(0), json.as_bytes(), &[], &context);
    }

    fn pane_update(plugin_id: PluginId, suppressed: bool) -> Event {
        Event::PaneUpdate(PaneManifest {
            panes: std::collections::HashMap::from([(
                TabPosition::new(0),
                vec![PaneInfo {
                    id: PaneId::Plugin(plugin_id),
                    is_suppressed: suppressed,
                    ..Default::default()
                }],
            )]),
        })
    }

    fn loaded() -> State {
        let mut state = ready();
        state.poll_swamp(0, true);
        reply(&mut state, SEARCH);
        state.render(10, 80);
        state
    }

    fn key(state: &mut State, key: BareKey) {
        state.on_key(KeyWithModifier::new(key));
    }

    fn reader() -> State {
        let mut state = loaded();
        key(&mut state, BareKey::Char('j'));
        key(&mut state, BareKey::Enter);
        reply(&mut state, DETAIL);
        state.render(10, 80);
        state
    }

    #[test]
    fn cli_shapes_include_comments_empty_results_and_unknown_statuses() {
        let search = decode::<Search>(Some(0), SEARCH.as_bytes(), &[]).unwrap();
        assert_eq!((search.issues.len(), search.total), (2, 2219));
        let detail = decode::<Detail>(Some(0), DETAIL.as_bytes(), &[]).unwrap();
        let lines = detail_lines(&detail, 100);
        let text = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "#2180 CLI referral link",
            "Shipped · platform · by skunk-ape",
            "Assignees: skunk-ape",
            "https://swamp-club.com/lab/2180",
            "Print one URL.",
            "Comments (1)",
            "hammz · 2026-09-16T16:20:51.953Z",
            "Depends on #2179.",
        ] {
            assert!(text.contains(expected), "{expected}: {text}");
        }
        assert!(lines
            .iter()
            .flat_map(|line| &line.styles)
            .any(|style| style.bold));
        let empty = decode::<Search>(Some(0), br#"{"issues":[],"total":0}"#, &[]).unwrap();
        assert!(empty.issues.is_empty());
        let unknown = SEARCH.replace("triaged", "reviewing");
        let search = decode::<Search>(Some(0), unknown.as_bytes(), &[]).unwrap();
        assert!(issue_lines(&search.issues[0], true, 100, now_ms())[0]
            .text
            .contains("reviewing"));
        let empty = DETAIL.replace(r#"["skunk-ape"]"#, "[]");
        let mut value: serde_json::Value = serde_json::from_str(&empty).unwrap();
        value["comments"] = json!([]);
        let detail = decode::<Detail>(Some(0), value.to_string().as_bytes(), &[]).unwrap();
        assert!(detail.assignees.is_empty() && detail.comments.is_empty());
    }

    #[test]
    fn timestamps_decode_and_render_relative_ages_and_precise_offsets() {
        let mut value: serde_json::Value = serde_json::from_str(DETAIL).unwrap();
        value["createdAt"] = json!("2026-09-18T05:00:00-05:00");
        value["updatedAt"] = json!("2026-09-18T13:55:00.123+02:00");
        let detail = decode::<Detail>(Some(0), value.to_string().as_bytes(), &[]).unwrap();
        let now = DateTime::parse_from_rfc3339("2026-09-18T12:00:00.123Z")
            .unwrap()
            .timestamp_millis();
        let lines = issue_lines(&detail.issue, true, 100, now);
        assert_eq!(
            lines.last().unwrap().text,
            "  Created 2h ago · Updated 5m ago"
        );
        assert!(issue_lines(&detail.issue, true, 100, now + REFRESH_MS)
            .last()
            .unwrap()
            .text
            .contains("Updated 6m ago"));
        let precise = detail_lines(&detail, 100)
            .into_iter()
            .map(|line| line.text)
            .collect::<Vec<_>>();
        assert!(precise.contains(&"Created: 2026-09-18T05:00:00-05:00".into()));
        assert!(precise.contains(&"Updated: 2026-09-18T13:55:00.123+02:00".into()));
        for cols in 1..50 {
            assert!(issue_lines(&detail.issue, true, cols, now)
                .iter()
                .all(|line| line.text.width() <= cols));
            assert!(detail_lines(&detail, cols)
                .iter()
                .all(|line| line.text.width() <= cols));
        }
        assert!(issue_lines(&detail.issue, false, 100, 0)
            .last()
            .unwrap()
            .text
            .contains("Created now · Updated now"));
    }

    #[test]
    fn missing_null_and_invalid_timestamps_are_unknown() {
        let mut value: serde_json::Value = serde_json::from_str(DETAIL).unwrap();
        for invalid in [
            None,
            Some(json!(null)),
            Some(json!("not a timestamp")),
            Some(json!("2026-09-18T10:00:00")), // no timezone
            Some(json!("2026-02-30T10:00:00Z")),
            Some(json!(42)),
            Some(json!(false)),
            Some(json!({})),
            Some(json!([])),
        ] {
            if let Some(invalid) = invalid {
                value["createdAt"] = invalid.clone();
                value["updatedAt"] = invalid;
            }
            let detail = decode::<Detail>(Some(0), value.to_string().as_bytes(), &[]).unwrap();
            assert!(detail.issue.created_at.is_none() && detail.issue.updated_at.is_none());
            assert_eq!(
                issue_lines(&detail.issue, false, 100, now_ms())
                    .last()
                    .unwrap()
                    .text,
                "  Created unknown · Updated unknown"
            );
            let precise = detail_lines(&detail, 100);
            for label in ["Created: unknown", "Updated: unknown"] {
                assert!(precise.iter().any(|line| line.text == label));
            }
        }
    }

    fn timestamped_search() -> String {
        let mut value: serde_json::Value = serde_json::from_str(SEARCH).unwrap();
        let template = value["issues"][0].clone();
        value["issues"] = json!([]);
        for (number, created, updated) in [
            (
                1,
                Some("2026-09-18T10:00:00Z"),
                Some("2026-09-18T10:00:00Z"),
            ),
            (
                2,
                Some("2026-09-18T09:00:00Z"),
                Some("2026-09-18T12:00:00Z"),
            ),
            (
                3,
                Some("2026-09-18T11:00:00+01:00"),
                Some("2026-09-18T13:00:00+01:00"),
            ),
            (4, None, Some("invalid")),
            (5, Some("invalid"), None),
            (6, None, None),
        ] {
            let mut issue = template.clone();
            issue["number"] = json!(number);
            if number != 6 {
                issue["createdAt"] = json!(created);
                issue["updatedAt"] = json!(updated);
            }
            value["issues"].as_array_mut().unwrap().push(issue);
        }
        value.to_string()
    }

    #[test]
    fn all_sort_orders_use_instants_then_descending_numbers_with_unknowns_last() {
        let mut state = ready();
        state.poll_swamp(0, true);
        reply(&mut state, &timestamped_search());
        assert_eq!(state.swamp.sort, 0);
        assert_eq!(state.swamp.selected_number(), Some(3));
        let count = state.swamp.commands.len();
        for (sort, expected) in [
            [3, 2, 1, 6, 5, 4],
            [1, 3, 2, 6, 5, 4],
            [3, 1, 2, 6, 5, 4],
            [2, 3, 1, 6, 5, 4],
        ]
        .into_iter()
        .enumerate()
        {
            choose(&mut state, 'o', SORTS[sort]);
            assert!(!state.swamp.is_editing());
            assert_eq!(state.swamp.sort, sort);
            assert_eq!(
                state
                    .swamp
                    .issues()
                    .iter()
                    .map(|issue| issue.number)
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(state.swamp.selected_number(), Some(3));
            assert_eq!(state.swamp.commands.len(), count, "sorting stays local");
            assert!(state.swamp.header().contains("6 of 2219"));
        }
        key(&mut state, BareKey::Char('o'));
        key(&mut state, BareKey::Up);
        key(&mut state, BareKey::Esc);
        assert_eq!(state.swamp.sort, 3, "cancel keeps the committed sort");
        key(&mut state, BareKey::Char('o'));
        key(&mut state, BareKey::Up);
        key(&mut state, BareKey::Enter);
        assert_eq!(state.swamp.sort, 2);
        assert_eq!(state.swamp.commands.len(), count);
    }

    #[test]
    fn sorting_and_refresh_reveal_selection_and_keep_sort_across_surfaces() {
        let mut state = ready();
        state.poll_swamp(0, true);
        reply(&mut state, &timestamped_search());
        state.render(9, 12);
        state.swamp.list_scroll = Some(usize::MAX);
        choose(&mut state, 'o', "Created oldest first");
        assert_eq!(state.swamp.list_scroll, None);
        state.render(9, 12);
        assert_eq!(state.swamp.selected_number(), Some(3));
        assert_eq!(
            state.swamp.list_rows[state.swamp.window].0,
            state.swamp.selected
        );
        let mut refresh: serde_json::Value = serde_json::from_str(&timestamped_search()).unwrap();
        refresh["issues"][2]["createdAt"] = json!("2026-09-18T08:00:00Z");
        refresh["issues"].as_array_mut().unwrap().reverse();
        state.poll_swamp(REFRESH_MS, true);
        state.swamp.list_scroll = Some(usize::MAX);
        reply(&mut state, &refresh.to_string());
        assert_eq!(state.swamp.selected_number(), Some(3));
        assert_eq!(state.swamp.selected, 0);
        assert_eq!(state.swamp.list_scroll, None);
        state.render(9, 12);
        assert_eq!(state.swamp.window, 0);
        state.switch_surface(Surface::Agents);
        state.switch_surface(Surface::Swamp);
        reply(&mut state, &refresh.to_string());
        assert_eq!(state.swamp.sort, 3);
        assert_eq!(state.swamp.selected_number(), Some(3));
        assert_eq!(state.swamp.selected, 0);
    }

    #[test]
    fn minute_ticks_redraw_issue_ages_while_a_refresh_is_pending() {
        let mut state = loaded();
        assert!(!state.shows_an_age(), "legacy issues have no ages to tick");
        state.poll_swamp(REFRESH_MS, true);
        reply(&mut state, &timestamped_search());
        state.render(12, 80);
        assert!(state.shows_an_age());
        state.poll_swamp(2 * REFRESH_MS, true);
        let count = state.swamp.commands.len();
        state.age_minute = now_ms() / REFRESH_MS - 1;
        assert!(state.update(Event::Timer(FractionalSeconds::new(0.0))));
        assert_eq!(state.swamp.commands.len(), count);
        assert!(!state.update(Event::Timer(FractionalSeconds::new(0.0))));
        key(&mut state, BareKey::Enter);
        assert!(!state.shows_an_age(), "details use precise timestamps");
        key(&mut state, BareKey::Esc);
        state.render(1, 80);
        assert!(!state.shows_an_age());
        state.render(12, 80);
        state.swamp_visibility(false);
        assert!(!state.shows_an_age());
    }

    fn type_text(state: &mut State, text: &str) {
        for ch in text.chars() {
            key(state, BareKey::Char(ch));
        }
    }

    fn choose(state: &mut State, shortcut: char, text: &str) {
        key(state, BareKey::Char(shortcut));
        type_text(state, text);
        key(state, BareKey::Enter);
    }

    fn clear_source(state: &mut State) {
        key(state, BareKey::Home);
        state.on_key(KeyWithModifier::new(BareKey::Char('k')).with_ctrl_modifier());
    }

    #[test]
    fn filters_combine_coalesce_and_source_edits_apply_cancel_clear() {
        let mut state = loaded();
        choose(&mut state, 's', "open");
        let stale = context(&state);
        choose(&mut state, 't', "bug");
        choose(&mut state, 'f', "  cli tag  ");
        assert_eq!(state.swamp.source, "cli tag");
        choose(&mut state, '/', "two words");
        assert_eq!(state.swamp.commands.len(), 2);
        state.swamp_result(Some(0), br#"{"issues":[],"total":0}"#, &[], &stale);
        assert_eq!(state.swamp.issues().len(), 2, "superseded result discarded");
        assert_eq!(
            state.swamp.commands.last().unwrap().0,
            [
                "swamp",
                "issue",
                "search",
                "--json",
                "--limit",
                "100",
                "--status",
                "open",
                "--type",
                "bug",
                "--source",
                "cli tag",
                "two words"
            ]
        );
        reply(&mut state, SEARCH);
        key(&mut state, BareKey::Char('f'));
        key(&mut state, BareKey::Backspace);
        key(&mut state, BareKey::Esc);
        assert_eq!(state.swamp.source, "cli tag");
        key(&mut state, BareKey::Char('f'));
        clear_source(&mut state);
        type_text(&mut state, "--help");
        let count = state.swamp.commands.len();
        key(&mut state, BareKey::Enter);
        assert!(state.swamp.is_editing());
        assert_eq!(state.swamp.commands.len(), count);
        assert_eq!(state.swamp.source, "cli tag");
        assert!(state
            .swamp
            .list_error
            .as_ref()
            .unwrap()
            .contains("Source must"));
        clear_source(&mut state);
        key(&mut state, BareKey::Enter);
        assert!(state.swamp.source.is_empty());
        assert!(!state.swamp.is_editing());
        choose(&mut state, 's', "all");
        choose(&mut state, 't', "all");
        assert_eq!((state.swamp.status, state.swamp.kind), (0, 0));
        reply(&mut state, SEARCH);
        assert_eq!(
            state.swamp.commands.last().unwrap().0,
            search_args("two words", "", "", "").unwrap()
        );
    }

    #[test]
    fn select_typeahead_is_provisional_and_unmatched_input_cannot_commit() {
        let mut state = loaded();
        let count = state.swamp.commands.len();
        key(&mut state, BareKey::Char('s'));
        type_text(&mut state, "iprg");
        assert_eq!(state.swamp.status, 0);
        assert_eq!(state.swamp.commands.len(), count);
        key(&mut state, BareKey::Enter);
        assert_eq!(STATUSES[state.swamp.status], "in_progress");
        assert!(!state.swamp.is_editing());
        reply(&mut state, SEARCH);
        let count = state.swamp.commands.len();
        choose(&mut state, 's', "iprg");
        assert_eq!(
            state.swamp.commands.len(),
            count,
            "unchanged commit does not search"
        );
        key(&mut state, BareKey::Char('t'));
        type_text(&mut state, "zzzz");
        key(&mut state, BareKey::Enter);
        assert!(state.swamp.is_editing());
        assert_eq!(state.swamp.kind, 0);
        assert_eq!(state.swamp.commands.len(), count);
        key(&mut state, BareKey::Esc);
        key(&mut state, BareKey::Char('t'));
        key(&mut state, BareKey::Down);
        key(&mut state, BareKey::Esc);
        assert_eq!(state.swamp.kind, 0);
        assert_eq!(state.swamp.commands.len(), count);
        key(&mut state, BareKey::Char('t'));
        key(&mut state, BareKey::Down);
        key(&mut state, BareKey::Enter);
        assert_eq!(TYPES[state.swamp.kind], "bug");
    }

    #[test]
    fn source_suggestions_remember_only_committed_unique_tags() {
        let mut state = loaded();
        choose(&mut state, 'f', "custom");
        reply(&mut state, SEARCH);
        choose(&mut state, 'f', "");
        assert_eq!(
            state.swamp.commands.len(),
            2,
            "unchanged source does not search"
        );
        key(&mut state, BareKey::Char('f'));
        clear_source(&mut state);
        type_text(&mut state, "cancelled");
        key(&mut state, BareKey::Esc);
        key(&mut state, BareKey::Char('f'));
        assert_eq!(
            state
                .swamp
                .filter
                .as_ref()
                .unwrap()
                .form
                .combo_by_id("value")
                .unwrap()
                .suggestions(),
            ["swamp", "custom"]
        );
        clear_source(&mut state);
        key(&mut state, BareKey::Down);
        key(&mut state, BareKey::Enter);
        assert_eq!(state.swamp.source, "swamp");
        assert_eq!(state.swamp.source_tags, ["custom"]);
        reply(&mut state, SEARCH);
        key(&mut state, BareKey::Char('f'));
        clear_source(&mut state);
        type_text(&mut state, "cstm");
        key(&mut state, BareKey::Down);
        key(&mut state, BareKey::Enter);
        assert_eq!(state.swamp.source, "custom");
        assert_eq!(state.swamp.source_tags, ["custom"]);
    }

    #[test]
    fn filter_mouse_selects_on_release_and_intercepts_issue_rows() {
        for shortcut in ['s', 't', 'f', 'o'] {
            let mut state = loaded();
            let (row, x, _, _) = *state
                .swamp
                .filter_cells
                .iter()
                .find(|cell| cell.3 == shortcut)
                .unwrap();
            state.on_mouse(Mouse::LeftClick(row as isize + 1, x));
            state.render(14, 80);
            state.on_mouse(Mouse::Release(row as isize + 1, x));
            assert!(state.swamp.is_editing(), "opening release must not commit");
            let rect = state
                .swamp
                .filter
                .as_ref()
                .unwrap()
                .form
                .overlay_rect()
                .unwrap();
            let y = rect.y as isize + if shortcut == 'f' { 0 } else { 1 };
            let count = state.swamp.commands.len();
            state.on_mouse(Mouse::LeftClick(y, rect.x));
            state.render(14, 80);
            assert_eq!(state.swamp.commands.len(), count);
            assert!(state.swamp.opened.is_none());
            state.on_mouse(Mouse::Release(y + 1, rect.x));
            assert!(
                state.swamp.is_editing(),
                "release off the pressed option cancels the click"
            );
            state.on_mouse(Mouse::LeftClick(y, rect.x));
            state.on_mouse(Mouse::Release(y, rect.x));
            assert!(!state.swamp.is_editing());
            assert!(state.swamp.opened.is_none());
            let expected_count = count + usize::from(shortcut != 'o');
            assert_eq!(state.swamp.commands.len(), expected_count);
            match shortcut {
                's' => assert_eq!(STATUSES[state.swamp.status], "open"),
                't' => assert_eq!(TYPES[state.swamp.kind], "bug"),
                'o' => assert_eq!(state.swamp.sort, 1),
                _ => assert_eq!(state.swamp.source, "swamp"),
            }
            reply(&mut state, SEARCH);
            key(&mut state, BareKey::Char(shortcut));
            state.render(14, 80);
            state.on_mouse(Mouse::LeftClick(state.swamp.last_body_top as isize + 1, 79));
            assert!(!state.swamp.is_editing());
            assert!(state.swamp.opened.is_none());
            assert_eq!(state.swamp.commands.len(), expected_count);
        }
    }

    #[test]
    fn filter_chips_and_active_forms_fit_resized_panes() {
        let mut state = loaded();
        for shortcut in ['s', 't', 'f', 'o'] {
            key(&mut state, BareKey::Char(shortcut));
            for cols in 1..80 {
                for rows in [2, 4, 12] {
                    state.render(rows, cols);
                    for (row, x, width, _) in &state.swamp.filter_cells {
                        assert!(x + width <= cols);
                        assert!(*row < state.swamp.last_body_top);
                    }
                    let editor = state.swamp.filter.as_ref().unwrap();
                    for rect in editor.row.into_iter().chain(editor.form.overlay_rect()) {
                        assert!(rect.x + rect.columns <= cols);
                        assert!(rect.y + rect.rows <= rows);
                    }
                }
            }
            state.render(1, 1);
            let editor = state.swamp.filter.as_ref().unwrap();
            assert!(editor.row.is_none() && editor.form.overlay_rect().is_none());
            key(&mut state, BareKey::Esc);
        }
        state.render(12, 20);
        assert_eq!(state.swamp.filter_cells.len(), 4);
        assert!(state
            .swamp
            .filter_cells
            .windows(2)
            .all(|pair| pair[0].0 < pair[1].0));
        assert!(state.on_mouse(Mouse::LeftClick(state.swamp.last_body_top as isize + 1, 0)));
        assert_eq!(state.swamp.opened, Some(2254));
    }

    #[test]
    fn leaving_the_surface_cancels_filter_edits_and_keeps_committed_values() {
        let mut state = loaded();
        choose(&mut state, 't', "security");
        reply(&mut state, SEARCH);
        for leave in 0..4 {
            key(&mut state, BareKey::Char('f'));
            type_text(&mut state, "provisional");
            match leave {
                0 => state.switch_surface(Surface::Agents),
                1 => {
                    state.swamp_visibility(false);
                },
                2 => {
                    state.update(pane_update(state.own_plugin_id, true));
                },
                _ => state.hide_the_panel(),
            }
            assert!(!state.swamp.is_editing());
            assert!(state.swamp.source.is_empty());
            assert_eq!(TYPES[state.swamp.kind], "security");
            state.switch_surface(Surface::Swamp);
            state.swamp_visibility(true);
            state.update(pane_update(state.own_plugin_id, false));
            reply(&mut state, SEARCH);
        }
    }

    #[test]
    fn malformed_output_and_command_errors_are_explained() {
        for bytes in [
            &b""[..],
            b"not JSON",
            b"{}",
            br#"{"issues":{},"total":1}"#,
            br#"{"issues":[],"total":"1"}"#,
            b"\xff",
        ] {
            assert!(decode::<Search>(Some(0), bytes, &[])
                .unwrap_err()
                .contains("Malformed Swamp response"));
        }
        let bad = DETAIL.replace(
            r#""createdAt":"2026-09-16T16:20:51.953Z""#,
            r#""createdAt":42"#,
        );
        assert!(decode::<Detail>(Some(0), bad.as_bytes(), &[]).is_err());
        assert_eq!(
            decode::<Search>(
                Some(1),
                br#"{"error":"Could not connect: fetch failed"}"#,
                &[]
            )
            .unwrap_err(),
            "Could not connect: fetch failed"
        );
        assert!(
            decode::<Search>(Some(2), &[], b"No such file or directory (os error 2)")
                .unwrap_err()
                .contains("command not found")
        );
        assert!(decode::<Search>(Some(127), &[], &[])
            .unwrap_err()
            .contains("command not found"));
        assert_eq!(
            decode::<Search>(Some(2), &[], b"Permission denied (os error 13)").unwrap_err(),
            "Permission denied (os error 13)"
        );
        assert!(decode::<Search>(None, &[], &[])
            .unwrap_err()
            .contains("command failed"));
    }

    #[test]
    fn keywords_are_single_arguments_and_statuses_use_the_cli_spelling() {
        assert_eq!(
            search_args("", "", "", "").unwrap(),
            ["swamp", "issue", "search", "--json", "--limit", "100"]
        );
        let query = "two words; $(touch /tmp/no) `echo no`";
        assert_eq!(
            search_args(query, "in_progress", "", "").unwrap(),
            [
                "swamp",
                "issue",
                "search",
                "--json",
                "--limit",
                "100",
                "--status",
                "in_progress",
                query
            ]
        );
        assert!(search_args("--help", "", "", "").is_err());
        let mut state = loaded();
        for status in STATUSES.iter().skip(1).chain([&STATUSES[0]]) {
            choose(&mut state, 's', status_label(status));
            let args = &state.swamp.commands.last().unwrap().0;
            assert_eq!(*args, search_args("", status, "", "").unwrap());
            reply(&mut state, SEARCH);
        }
        assert_eq!(state.swamp.status, 0);
        key(&mut state, BareKey::Char('/'));
        for ch in "--help".chars() {
            key(&mut state, BareKey::Char(ch));
        }
        let commands = state.swamp.commands.len();
        key(&mut state, BareKey::Enter);
        assert!(state.swamp.is_editing());
        assert_eq!(state.swamp.query, "");
        assert_eq!(state.swamp.commands.len(), commands);
    }

    #[test]
    fn search_edits_apply_or_cancel_and_superseded_replies_cannot_win() {
        let mut state = loaded();
        key(&mut state, BareKey::Char('/'));
        for ch in "first".chars() {
            key(&mut state, BareKey::Char(ch));
        }
        key(&mut state, BareKey::Esc);
        assert_eq!(state.swamp.query, "");
        assert_eq!(state.swamp.commands.len(), 1);
        key(&mut state, BareKey::Char('/'));
        key(&mut state, BareKey::Char('a'));
        key(&mut state, BareKey::Enter);
        let old = context(&state);
        key(&mut state, BareKey::Char('/'));
        key(&mut state, BareKey::Backspace);
        key(&mut state, BareKey::Char('b'));
        key(&mut state, BareKey::Enter);
        assert_eq!(state.swamp.commands.len(), 2, "wait for the first process");
        assert_eq!(state.swamp.query, "b");
        reply(&mut state, r#"{"issues":[],"total":0}"#);
        assert_eq!(
            state.swamp.issues().len(),
            2,
            "ignore old query's empty results"
        );
        assert_eq!(state.swamp.commands.len(), 3);
        assert_eq!(state.swamp.commands.last().unwrap().0.last().unwrap(), "b");
        assert!(!state.swamp_result(Some(0), SEARCH.as_bytes(), &[], &old));
        assert!(state.swamp.pending.is_some());
        reply(&mut state, r#"{"issues":[],"total":0}"#);
        assert!(state.swamp.issues().is_empty());
        assert!(state.swamp.pending.is_none());
        key(&mut state, BareKey::Char('/'));
        key(&mut state, BareKey::Backspace);
        key(&mut state, BareKey::Enter);
        assert_eq!(
            state.swamp.commands.last().unwrap().0,
            search_args("", "", "", "").unwrap()
        );
    }

    #[test]
    fn refreshing_preserves_selection_and_content_after_failure() {
        let mut state = loaded();
        state.swamp.selected = 1;
        state.swamp.list_scroll = Some(1);
        state.poll_swamp(REFRESH_MS, false);
        let mut reordered: serde_json::Value = serde_json::from_str(SEARCH).unwrap();
        reordered["issues"].as_array_mut().unwrap().reverse();
        reply(&mut state, &reordered.to_string());
        assert_eq!(state.swamp.selected, 1);
        assert_eq!(state.swamp.list_scroll, None);
        state.poll_swamp(2 * REFRESH_MS, false);
        reply(&mut state, r#"{"error":"Network unavailable"}"#);
        assert_eq!(state.swamp.issues()[state.swamp.selected].number, 2180);
        assert!(state.swamp.note().unwrap().contains("Network unavailable"));
        state.poll_swamp(3 * REFRESH_MS, false);
        reply(&mut state, SEARCH);
        assert_eq!(state.swamp.selected, 1);
        assert!(!state.swamp.has_error());
        state.poll_swamp(4 * REFRESH_MS, false);
        reply(&mut state, r#"{"issues":[],"total":0}"#);
        assert_eq!(state.swamp.selected, 0);
        key(&mut state, BareKey::Enter);
        assert_eq!(state.swamp.opened, None);
    }

    #[test]
    fn refreshes_only_when_visible_and_never_overlap() {
        let mut state = reader();
        let count = state.swamp.commands.len();
        state.swamp.last_refresh = Some(0);
        assert!(!state.poll_swamp(REFRESH_MS - 1, false));
        assert!(state.poll_swamp(REFRESH_MS, false));
        assert_eq!(state.swamp.commands.len(), count + 1);
        assert!(!state.poll_swamp(REFRESH_MS * 2, true));
        assert!(!state.poll_swamp(REFRESH_MS * 2, false));
        state.switch_surface(Surface::Agents);
        reply(&mut state, SEARCH);
        assert!(state.swamp.detail_wanted);
        assert!(!state.poll_swamp(REFRESH_MS * 3, true));
        assert_eq!(state.swamp.commands.len(), count + 1);
        state.switch_surface(Surface::Swamp);
        reply(&mut state, SEARCH);
        assert_eq!(
            state.swamp.commands.last().unwrap().0,
            ["swamp", "issue", "get", "2180", "--json"]
        );
        reply(&mut state, DETAIL);
        let count = state.swamp.commands.len();
        state.swamp_visibility(false);
        assert!(!state.poll_swamp(i64::MAX, true));
        assert_eq!(state.swamp.commands.len(), count);
        state.swamp_visibility(true);
        assert_eq!(state.swamp.commands.len(), count + 1);
        reply(&mut state, SEARCH);
        reply(&mut state, DETAIL);
        assert_eq!(
            state.swamp.commands.len(),
            count + 2,
            "reopening refreshes both list and reader"
        );
    }

    #[test]
    fn polling_waits_for_pane_visibility_confirmation() {
        for surface_event in [false, true] {
            let mut state = State {
                surface: Surface::Swamp,
                ..Default::default()
            };
            state.swamp.permission = Permission::Granted;
            assert!(!state.poll_swamp(0, true));
            if surface_event {
                assert!(!state.update(Event::Visible(true)));
            }
            assert!(!state.update(Event::PaneUpdate(PaneManifest::default())));
            assert!(!state.poll_swamp(REFRESH_MS, true));
            assert!(state.swamp.commands.is_empty());
            assert!(state.update(pane_update(state.own_plugin_id, false)));
            assert_eq!(state.swamp.commands.len(), 1);
        }
    }

    #[test]
    fn pane_suppression_and_visibility_events_control_polling() {
        for surface_first in [false, true] {
            let mut state = ready();
            state.update(Event::Visible(false));
            state.update(pane_update(state.own_plugin_id, true));
            let mut restore = [
                Event::Visible(true),
                pane_update(state.own_plugin_id, false),
            ];
            if !surface_first {
                restore.reverse();
            }
            assert!(!state.update(restore[0].clone()));
            assert!(!state.update(restore[0].clone()));
            assert!(!state.update(Event::PaneUpdate(PaneManifest::default())));
            assert!(!state.swamp.visible());
            assert!(!state.poll_swamp(i64::MAX, true));
            assert!(state.swamp.commands.is_empty());

            assert!(state.update(restore[1].clone()));
            assert!(state.swamp.visible());
            assert_eq!(state.swamp.commands.len(), 1);
            reply(&mut state, SEARCH);
            for event in restore {
                assert!(!state.update(event));
            }
            assert!(!state.update(Event::PaneUpdate(PaneManifest::default())));
            assert!(state.swamp.visible());
            assert_eq!(state.swamp.commands.len(), 1, "refresh only on reopening");
        }
    }

    #[test]
    fn dismissal_preserves_surface_state_and_hidden_results_wait_for_reopening() {
        for surface_hidden in [false, true] {
            let mut state = reader();
            state.poll_swamp(REFRESH_MS, true);
            let count = state.swamp.commands.len();
            let pending = context(&state);
            if surface_hidden {
                state.update(Event::Visible(false));
            }
            state.hide_the_panel();
            assert_eq!(state.swamp.surface_hidden, surface_hidden);
            assert!(!state.swamp.pane_visible);
            assert_eq!(
                state.swamp.pending.as_ref().unwrap().id,
                pending[REQUEST_ID]
            );
            if surface_hidden {
                assert!(!state.update(pane_update(state.own_plugin_id, false)));
            } else {
                assert!(!state.update(Event::Visible(true)));
            }
            assert!(!state.poll_swamp(i64::MAX, true));
            assert!(!state.update(Event::RunCommandResult(
                Some(0),
                SEARCH.as_bytes().to_vec(),
                vec![],
                pending,
            )));
            assert!(state.swamp.pending.is_none());
            assert!(state.swamp.detail_wanted);
            assert_eq!(state.swamp.commands.len(), count);

            if surface_hidden {
                assert!(state.update(Event::Visible(true)));
            } else {
                assert!(state.update(pane_update(state.own_plugin_id, false)));
            }
            assert_eq!(state.swamp.commands.len(), count + 1);
            reply(&mut state, SEARCH);
            reply(&mut state, DETAIL);
            assert!(!state.update(pane_update(state.own_plugin_id, false)));
            assert!(!state.update(Event::Visible(true)));
            assert_eq!(state.swamp.commands.len(), count + 2);
        }
    }

    #[test]
    fn permission_denial_can_be_retried_and_grants_while_hidden_do_not_poll() {
        for surface_hidden in [false, true] {
            let mut state = ready();
            state.swamp.permission = Permission::Unrequested;
            state.poll_swamp(0, true);
            assert!(state.swamp.commands.is_empty());
            assert!(state
                .swamp
                .note()
                .unwrap()
                .contains("Waiting for permission"));
            state.update(Event::PermissionRequestResult(PermissionStatus::Denied));
            assert!(state.swamp.note().unwrap().contains("Permission denied"));
            state.poll_swamp(REFRESH_MS, false);
            assert!(state.swamp.permission == Permission::Denied);
            key(&mut state, BareKey::Char('r'));
            assert!(state.swamp.permission == Permission::Waiting);
            if surface_hidden {
                state.update(Event::Visible(false));
                state.update(pane_update(state.own_plugin_id, false));
            } else {
                state.hide_the_panel();
                state.update(Event::Visible(true));
            }
            assert!(!state.update(Event::PermissionRequestResult(PermissionStatus::Granted)));
            assert!(state.swamp.commands.is_empty());
            if surface_hidden {
                state.update(Event::Visible(true));
            } else {
                state.update(pane_update(state.own_plugin_id, false));
            }
            assert_eq!(state.swamp.commands.len(), 1);
        }
    }

    #[test]
    fn detail_replies_match_both_request_and_issue_and_survive_refresh_failure() {
        let mut state = reader();
        state.swamp.detail_scroll = 4;
        state.poll_swamp(REFRESH_MS, true);
        reply(&mut state, SEARCH);
        let mut wrong_context = context(&state);
        wrong_context.insert(ISSUE_NUMBER.into(), "2254".into());
        assert!(!state.swamp_result(Some(0), DETAIL.as_bytes(), &[], &wrong_context));
        assert!(state.swamp.pending.is_some());
        reply(&mut state, &DETAIL.replace("2180", "2254"));
        assert!(state
            .swamp
            .detail_error
            .as_ref()
            .unwrap()
            .contains("wrong issue number"));
        assert_eq!(state.swamp.detail.as_ref().unwrap().issue.number, 2180);
        assert_eq!(state.swamp.detail_scroll, 4);
        state.poll_swamp(2 * REFRESH_MS, true);
        reply(&mut state, SEARCH);
        reply(&mut state, DETAIL);
        assert_eq!(state.swamp.detail_scroll, 4);
        assert!(!state.swamp.has_error());

        state.poll_swamp(3 * REFRESH_MS, true);
        reply(&mut state, SEARCH);
        let old = context(&state);
        key(&mut state, BareKey::Esc);
        key(&mut state, BareKey::Up);
        key(&mut state, BareKey::Enter);
        assert_eq!(state.swamp.opened, Some(2254));
        state.swamp_result(Some(0), DETAIL.as_bytes(), &[], &old);
        assert!(state.swamp.detail.is_none());
        assert_eq!(context(&state).get(ISSUE_NUMBER).unwrap(), "2254");
        assert!(!state.swamp_result(Some(0), DETAIL.as_bytes(), &[], &old));
        reply(&mut state, &DETAIL.replace("2180", "2254"));
        assert_eq!(state.swamp.detail.as_ref().unwrap().issue.number, 2254);
    }

    #[test]
    fn fourth_tab_keyboard_mouse_and_escape_keep_panel_conventions() {
        let mut state = loaded();
        state.surface = Surface::Agents;
        state.tabs_focused = true;
        for surface in [Surface::Todos, Surface::Vms, Surface::Swamp, Surface::Swamp] {
            key(&mut state, BareKey::Right);
            assert_eq!(state.surface, surface);
        }
        key(&mut state, BareKey::Down);
        assert!(!state.tabs_focused);
        key(&mut state, BareKey::Char('/'));
        key(&mut state, BareKey::Esc);
        assert_eq!(state.panel_hides, 0);
        state.render(12, 80);
        let line = state.swamp.last_body_top + 4; // second metadata row plus tab bar
        assert!(state.on_mouse(Mouse::LeftClick(line as isize, 4)));
        assert_eq!(state.swamp.opened, Some(2180));
        key(&mut state, BareKey::Esc);
        assert_eq!(state.swamp.opened, None);
        assert_eq!(state.panel_hides, 0);
        state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier());
        assert!(state.tabs_focused);
        key(&mut state, BareKey::Left);
        assert_eq!(state.surface, Surface::Vms);
        state.render(12, 80);
        let x = state
            .tab_cells
            .iter()
            .find(|cell| cell.target == Surface::Swamp)
            .unwrap()
            .x;
        state.on_mouse(Mouse::LeftClick(0, x));
        assert_eq!(state.surface, Surface::Swamp);
        key(&mut state, BareKey::Enter);
        key(&mut state, BareKey::Tab);
        assert_eq!(state.ring_yields, [wire::STOP_TABS]);
        key(&mut state, BareKey::Esc);
        assert_eq!(state.panel_hides, 1);
        assert!(!state.swamp.visible());
    }

    #[test]
    fn reader_scrolls_with_keys_and_wheel_and_narrow_renders_stay_bounded() {
        let mut state = reader();
        key(&mut state, BareKey::Down);
        assert_eq!(state.swamp.detail_scroll, 1);
        state.render(10, 80);
        key(&mut state, BareKey::PageDown);
        assert_eq!(state.swamp.detail_scroll, state.swamp.max_scroll);
        state.on_mouse(Mouse::Hover(3, 0));
        state.on_mouse(Mouse::ScrollUp(1));
        assert_eq!(state.swamp.detail_scroll, state.swamp.max_scroll - 1);
        key(&mut state, BareKey::Home);
        assert_eq!(state.swamp.detail_scroll, 0);
        key(&mut state, BareKey::End);
        assert_eq!(state.swamp.detail_scroll, state.swamp.max_scroll);
        state.render(1, 80);
        let scroll = state.swamp.detail_scroll;
        assert!(!state.on_mouse(Mouse::ScrollDown(2)));
        assert_eq!(state.swamp.detail_scroll, scroll);
        for cols in 0..40 {
            for rows in [0, 1, 2, 3, 10] {
                state.render(rows, cols);
                assert!(
                    state
                        .tab_cells
                        .iter()
                        .all(|cell| cell.x + cell.width <= cols)
                        || rows == 0
                        || cols == 0
                );
                assert!(state.swamp.last_body_rows <= rows.saturating_sub(1));
            }
            if cols > 0 {
                let detail = decode::<Detail>(Some(0), DETAIL.as_bytes(), &[]).unwrap();
                assert!(detail_lines(&detail, cols)
                    .iter()
                    .all(|line| line.text.width() <= cols));
            }
            assert!(issue_lines(&state.swamp.issues()[0], true, cols, now_ms())
                .iter()
                .all(|line| line.text.width() <= cols));
        }
        key(&mut state, BareKey::Esc);
        state.render(8, 4);
        assert_eq!(
            state
                .tab_cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<Vec<_>>(),
            ["A", "T", "V", "S"]
        );
        assert!(state.swamp.header().contains("2 of 2219"));
    }

    #[test]
    fn titles_wrap_without_losing_words_and_stay_bounded() {
        let mut state = loaded();
        let issue = &mut state.swamp.search.as_mut().unwrap().issues[0];
        issue.title = "alpha beta\ngamma\nabcdefghijk\n界面 é".into();
        let lines = issue_lines(issue, true, 8, now_ms());
        assert_eq!(
            lines[1..7]
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            &[
                "  alpha",
                "  beta",
                "  gamma",
                "  abcdef",
                "  ghijk",
                "  界面 é"
            ]
        );
        assert!(!lines[0].text.contains("alpha"));
        for line in &lines[1..7] {
            assert_eq!(line.styles.len(), line.text.chars().count());
            assert!(line.styles[2..].iter().all(|style| *style == TITLE));
        }
        for cols in 0..20 {
            assert!(issue_lines(issue, false, cols, now_ms())
                .iter()
                .all(|line| line.text.width() <= cols));
        }
        for selected in [false, true] {
            for line in &lines {
                assert_eq!(
                    list_text(line, selected).serialize().starts_with('x'),
                    selected
                );
            }
        }
    }

    #[test]
    fn issue_colors_survive_selection_wrapping_and_clipping() {
        let mut detail = decode::<Detail>(Some(0), DETAIL.as_bytes(), &[]).unwrap();
        detail.issue.title = "界面 é title".into();
        detail.issue.kind = "種類".into();
        for (status, color) in [
            ("open", Some(1)),
            ("triaged", Some(0)),
            ("in_progress", Some(3)),
            ("shipped", Some(2)),
            ("closed", None),
            ("未知", None),
        ] {
            detail.issue.status = status.into();
            let expected_status = markdown::MdStyle {
                color,
                ..Default::default()
            };
            for selected in [false, true] {
                let lines = issue_lines(&detail.issue, selected, 100, 0);
                assert_eq!(lines[0].text.starts_with('>'), selected);
                let start = format!("  #{} ", detail.issue.number).chars().count();
                let end = start + status_label(status).chars().count();
                assert!(lines[0].styles[1..start].iter().all(|s| *s == METADATA));
                assert!(lines[0].styles[start..end]
                    .iter()
                    .all(|s| *s == expected_status));
                assert!(lines[0].styles[end..].iter().all(|s| *s == METADATA));
                assert!(lines[1].styles[2..].iter().all(|s| *s == TITLE));
                assert!(lines[2].styles[2..].iter().all(|s| *s == METADATA));
                for cols in 0..30 {
                    let clipped = issue_lines(&detail.issue, selected, cols, 0).remove(0);
                    assert_eq!(
                        clipped.styles,
                        lines[0].styles[..clipped.text.chars().count()]
                    );
                }
                for line in &lines {
                    assert_eq!(
                        list_text(line, true).serialize(),
                        format!("x{}", list_text(line, false).serialize())
                    );
                }
            }
            let headers = detail_lines(&detail, 100);
            let title_start = format!("#{} ", detail.issue.number).chars().count();
            assert!(headers[0].styles[..title_start]
                .iter()
                .all(|s| *s == METADATA));
            assert!(headers[0].styles[title_start..].iter().all(|s| *s == TITLE));
            let status_end = status_label(status).chars().count();
            assert!(headers[1].styles[..status_end]
                .iter()
                .all(|s| *s == expected_status));
            assert!(headers[1].styles[status_end..]
                .iter()
                .all(|s| *s == METADATA));
            assert!(headers[2..6]
                .iter()
                .flat_map(|line| &line.styles)
                .all(|s| *s == METADATA));
            for cols in 1..30 {
                let wrapped = detail_lines(&detail, cols);
                // Wrapping may discard spaces, but every visible header character keeps its style.
                let expected = headers[..6]
                    .iter()
                    .flat_map(|line| line.text.chars().zip(&line.styles))
                    .filter(|(ch, _)| !ch.is_whitespace())
                    .collect::<Vec<_>>();
                let actual = wrapped
                    .iter()
                    .take_while(|line| !line.text.is_empty())
                    .flat_map(|line| line.text.chars().zip(&line.styles))
                    .filter(|(ch, _)| !ch.is_whitespace())
                    .collect::<Vec<_>>();
                if cols >= 2 {
                    assert_eq!(actual, expected, "width {cols}");
                }
                for line in wrapped
                    .iter()
                    .chain(issue_lines(&detail.issue, true, cols, 0).iter())
                {
                    assert!(line.text.width() <= cols);
                    assert!(
                        line.styles.is_empty() || line.styles.len() == line.text.chars().count()
                    );
                }
            }
        }
    }

    #[test]
    fn page_navigation_counts_rendered_rows_instead_of_issues() {
        let mut state = loaded();
        let issues = (0..20)
            .map(|_| {
                decode::<Search>(Some(0), SEARCH.as_bytes(), &[])
                    .unwrap()
                    .issues
                    .remove(0)
            })
            .collect();
        state.swamp.search = Some(Search { issues, total: 20 });
        state.render_swamp(8, 80);
        key(&mut state, BareKey::PageDown);
        assert_eq!(state.swamp.selected, 1);
        state.render_swamp(8, 80);
        key(&mut state, BareKey::PageUp);
        assert_eq!(state.swamp.selected, 0);
        key(&mut state, BareKey::End);
        state.render_swamp(8, 80);
        assert_eq!(state.swamp.selected, 19);
        assert_eq!(state.swamp.window, state.swamp.max_scroll);
        key(&mut state, BareKey::Home);
        state.render_swamp(8, 80);
        assert_eq!(state.swamp.window, 0);
    }

    #[test]
    fn wrapped_rows_drive_clicks_navigation_scroll_and_resize() {
        let mut state = loaded();
        state.swamp.search.as_mut().unwrap().issues[0].title = "word ".repeat(40);
        state.render_swamp(10, 12);
        let first_end = state
            .swamp
            .list_rows
            .iter()
            .position(|(index, _)| *index == 1)
            .unwrap();
        assert!(first_end > state.swamp.last_body_rows);
        assert_eq!(state.swamp.window, 0);
        state.swamp_mouse(Mouse::ScrollDown(3));
        state.render_swamp(10, 12);
        assert_eq!(state.swamp.window, 3);
        assert_eq!(state.swamp.selected, 0);
        assert!(state.swamp_mouse(Mouse::LeftClick(state.swamp.last_body_top as isize, 0)));
        assert_eq!(state.swamp.opened, Some(2254));
        key(&mut state, BareKey::Esc);
        state.render_swamp(10, 12);
        key(&mut state, BareKey::Down);
        state.render_swamp(10, 12);
        assert_eq!(state.swamp.selected, 1);
        assert_eq!(state.swamp.window, first_end);
        assert_eq!(state.swamp.list_rows[state.swamp.window].0, 1);
        key(&mut state, BareKey::Up);
        state.render_swamp(10, 12);
        assert_eq!(state.swamp.window, 0);
        key(&mut state, BareKey::PageDown);
        assert_eq!(state.swamp.selected, 1);
        key(&mut state, BareKey::PageUp);
        assert_eq!(state.swamp.selected, 0);
        state.render_swamp(10, 12);
        state.swamp_mouse(Mouse::ScrollDown(usize::MAX));
        state.render_swamp(10, 12);
        assert_eq!(state.swamp.window, state.swamp.max_scroll);
        state.render_swamp(20, 100);
        assert_eq!(state.swamp.window, 0);
        assert_eq!(state.swamp.list_scroll, Some(0));
        let continuation = state
            .swamp
            .list_rows
            .iter()
            .rposition(|(index, _)| *index == 1)
            .unwrap();
        assert!(state.swamp_mouse(Mouse::LeftClick(
            (state.swamp.last_body_top + continuation) as isize,
            0
        )));
        assert_eq!(state.swamp.opened, Some(2180));
    }

    #[test]
    #[ignore = "manual check: requires the installed authenticated Swamp CLI and network"]
    fn installed_swamp_cli() {
        let output = std::process::Command::new("swamp")
            .args(&search_args("", "", "", "").unwrap()[1..])
            .output()
            .unwrap();
        let search =
            decode::<Search>(output.status.code(), &output.stdout, &output.stderr).unwrap();
        assert!(search.issues.len() <= 100);
        if let Some(issue) = search.issues.first() {
            let output = std::process::Command::new("swamp")
                .args(["issue", "get", &issue.number.to_string(), "--json"])
                .output()
                .unwrap();
            let detail =
                decode::<Detail>(output.status.code(), &output.stdout, &output.stderr).unwrap();
            assert_eq!(detail.issue.number, issue.number);
            assert!(!detail_lines(&detail, 80).is_empty());
        }
    }
}

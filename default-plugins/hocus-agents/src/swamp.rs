//! Read-only issue browsing through the host's authenticated Swamp CLI.
use super::*;
use serde::Deserialize;

const REFRESH_MS: i64 = 60_000;
const REQUEST_ID: &str = "swamp_issue_request";
const ISSUE_NUMBER: &str = "swamp_issue_number";
const TYPES: [&str; 4] = ["", "bug", "feature", "security"];
const STATUSES: [&str; 6] = ["", "open", "triaged", "in_progress", "shipped", "closed"];

#[derive(Debug, Deserialize)]
struct Issue {
    number: u64,
    title: String,
    status: String,
    #[serde(rename = "type")]
    kind: String,
    author: String,
    url: String,
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
    source: String,
    editing_source: bool,
    filter_cells: Vec<(usize, usize, usize, char)>, // row, column, width, shortcut
    search: Option<Search>,
    selected: usize,
    list_scroll: Option<usize>,
    list_rows: Vec<(usize, String)>,
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
    fn visible(&self) -> bool {
        !self.surface_hidden && self.pane_visible
    }

    pub(super) fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    pub(super) fn clear_dimensions(&mut self) {
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
            let prefix = if self.editing_source { "Source: " } else { "/" };
            return format!("{prefix}{query} · Enter apply · Esc cancel");
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
                        let selected = self
                            .swamp
                            .issues()
                            .get(self.swamp.selected)
                            .map(|issue| issue.number);
                        self.swamp.selected = search
                            .issues
                            .iter()
                            .position(|issue| Some(issue.number) == selected)
                            .unwrap_or(
                                self.swamp
                                    .selected
                                    .min(search.issues.len().saturating_sub(1)),
                            );
                        self.swamp.search = Some(search);
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
        let Some(number) = self
            .swamp
            .issues()
            .get(self.swamp.selected)
            .map(|issue| issue.number)
        else {
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
        if let Some(query) = self.swamp.editing.as_mut() {
            match key.bare_key {
                BareKey::Esc => self.swamp.editing = None,
                BareKey::Enter => {
                    let (keyword, source) = if self.swamp.editing_source {
                        (self.swamp.query.as_str(), query.trim())
                    } else {
                        (query.trim(), self.swamp.source.as_str())
                    };
                    if let Err(error) = search_args(
                        keyword,
                        STATUSES[self.swamp.status],
                        TYPES[self.swamp.kind],
                        source,
                    ) {
                        self.swamp.list_error = Some(error);
                        return true;
                    }
                    if self.swamp.editing_source {
                        self.swamp.source = query.trim().to_owned();
                    } else {
                        self.swamp.query = query.trim().to_owned();
                    }
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
                self.swamp.editing_source = false;
                self.swamp.editing = Some(self.swamp.query.clone());
            },
            BareKey::Char('s') if self.swamp.opened.is_none() => {
                self.swamp.status = (self.swamp.status + 1) % STATUSES.len();
                self.search_swamp();
            },
            BareKey::Char('t') if self.swamp.opened.is_none() => {
                self.swamp.kind = (self.swamp.kind + 1) % TYPES.len();
                self.search_swamp();
            },
            BareKey::Char('f') if self.swamp.opened.is_none() => {
                self.swamp.editing_source = true;
                self.swamp.editing = Some(self.swamp.source.clone());
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

    pub(super) fn render_swamp(&mut self, rows: usize, cols: usize) {
        if rows == 0 || cols == 0 {
            self.swamp.clear_dimensions();
            return;
        }
        let note = self.swamp.note();
        let mut top = 1 + usize::from(note.is_some() && rows > 1);
        self.swamp.filter_cells.clear();
        if self.swamp.opened.is_none() {
            let mut x = 0;
            for (label, value, shortcut) in [
                ("Status", status_label(STATUSES[self.swamp.status]), 's'),
                (
                    "Type",
                    if self.swamp.kind == 0 {
                        "All"
                    } else {
                        TYPES[self.swamp.kind]
                    },
                    't',
                ),
                (
                    "Source",
                    if self.swamp.source.is_empty() {
                        "All"
                    } else {
                        &self.swamp.source
                    },
                    'f',
                ),
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
                paint(
                    Text::new(text).color_range(2, ..),
                    x,
                    top,
                    Some(width),
                    None,
                );
                self.swamp.filter_cells.push((top, x, width, shortcut));
                x += width + 1;
            }
            top = (top + 1).min(rows);
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
            self.swamp.list_rows = self
                .swamp
                .issues()
                .iter()
                .enumerate()
                .flat_map(|(index, issue)| {
                    issue_lines(issue, index == self.swamp.selected, cols)
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

fn list_text(line: &str, selected: bool) -> Text {
    let text = Text::new(line);
    if selected {
        text.selected().color_range(2, ..)
    } else {
        text
    }
}

fn issue_lines(issue: &Issue, selected: bool, cols: usize) -> Vec<String> {
    let mut lines = vec![fit(
        &format!(
            "{} #{} {} · {}",
            if selected { ">" } else { " " },
            issue.number,
            status_label(&issue.status),
            issue.kind,
        ),
        cols,
    )];
    // Leave room for a wide Unicode character before adding the indentation.
    let indent = if cols >= 4 { "  " } else { "" };
    lines.extend(
        zellij_tile::text::wrap(&issue.title, cols - indent.len())
            .into_iter()
            .map(|line| fit(&format!("{indent}{line}"), cols)),
    );
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
    for text in [
        format!("#{} {}", issue.number, issue.title),
        format!(
            "{} · {} · by {}",
            status_label(&issue.status),
            issue.kind,
            issue.author
        ),
        format!("Assignees: {assignees}"),
        issue.url.clone(),
    ] {
        lines.extend(
            zellij_tile::text::wrap(&text, cols)
                .into_iter()
                .map(markdown::MdLine::plain),
        );
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
        lines.extend(
            zellij_tile::text::wrap(
                &format!("{} · {}", comment.author, comment.created_at),
                cols,
            )
            .into_iter()
            .map(markdown::MdLine::plain),
        );
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
        assert!(issue_lines(&search.issues[0], true, 100)[0].contains("reviewing"));
        let empty = DETAIL.replace(r#"["skunk-ape"]"#, "[]");
        let mut value: serde_json::Value = serde_json::from_str(&empty).unwrap();
        value["comments"] = json!([]);
        let detail = decode::<Detail>(Some(0), value.to_string().as_bytes(), &[]).unwrap();
        assert!(detail.assignees.is_empty() && detail.comments.is_empty());
    }

    #[test]
    fn filters_combine_coalesce_and_source_edits_apply_cancel_clear() {
        let mut state = loaded();
        key(&mut state, BareKey::Char('s'));
        let stale = context(&state);
        key(&mut state, BareKey::Char('t'));
        key(&mut state, BareKey::Char('f'));
        for ch in "  cli tag  ".chars() {
            key(&mut state, BareKey::Char(ch));
        }
        key(&mut state, BareKey::Enter);
        assert_eq!(state.swamp.source, "cli tag");
        key(&mut state, BareKey::Char('/'));
        for ch in "two words".chars() {
            key(&mut state, BareKey::Char(ch));
        }
        key(&mut state, BareKey::Enter);
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
                "two words",
            ]
        );
        reply(&mut state, SEARCH);
        key(&mut state, BareKey::Char('f'));
        key(&mut state, BareKey::Backspace);
        key(&mut state, BareKey::Esc);
        assert_eq!(state.swamp.source, "cli tag");
        key(&mut state, BareKey::Char('f'));
        state.swamp.editing = Some("--help".into());
        let count = state.swamp.commands.len();
        key(&mut state, BareKey::Enter);
        assert!(state.swamp.is_editing());
        assert_eq!(state.swamp.commands.len(), count);
        assert_eq!(state.swamp.source, "cli tag");
        assert!(search_args("", "", "", "--help").is_err());
        state.swamp.editing = Some("   ".into());
        key(&mut state, BareKey::Enter);
        assert!(state.swamp.source.is_empty());
        assert!(!state.swamp.is_editing());
        for _ in 1..STATUSES.len() {
            key(&mut state, BareKey::Char('s'));
        }
        for _ in 1..TYPES.len() {
            key(&mut state, BareKey::Char('t'));
        }
        assert_eq!((state.swamp.status, state.swamp.kind), (0, 0));
        reply(&mut state, SEARCH);
        assert_eq!(
            state.swamp.commands.last().unwrap().0,
            search_args("two words", "", "", "").unwrap()
        );
    }

    #[test]
    fn filter_chips_wrap_and_mouse_uses_rendered_positions() {
        let mut state = loaded();
        for cols in 1..80 {
            state.render(12, cols);
            assert_eq!(state.swamp.filter_cells.len(), 3);
            for (row, x, width, _) in &state.swamp.filter_cells {
                assert!(x + width <= cols);
                assert!(*row < state.swamp.last_body_top);
            }
        }
        state.render(12, 20);
        let cells = state.swamp.filter_cells.clone();
        assert!(cells.windows(2).all(|pair| pair[0].0 < pair[1].0));
        for (row, x, _, _) in cells {
            assert!(state.on_mouse(Mouse::LeftClick(row as isize + 1, x)));
        }
        assert_eq!((state.swamp.status, state.swamp.kind), (1, 1));
        assert!(state.swamp.editing_source && state.swamp.is_editing());
        key(&mut state, BareKey::Esc);
        reply(&mut state, SEARCH);
        reply(&mut state, SEARCH);
        state.render(12, 20);
        assert!(state.on_mouse(Mouse::LeftClick(state.swamp.last_body_top as isize + 1, 0)));
        assert_eq!(state.swamp.opened, Some(2254));
        assert_eq!((state.swamp.status, state.swamp.kind), (1, 1));
        state.render(12, 20);
        assert!(state.swamp.filter_cells.is_empty());
        key(&mut state, BareKey::Esc);
        state.switch_surface(Surface::Agents);
        state.switch_surface(Surface::Swamp);
        assert_eq!((state.swamp.status, state.swamp.kind), (1, 1));
        state.render_swamp(2, 80);
        assert_eq!(state.swamp.last_body_rows, 0);
        state.swamp.clear_dimensions();
        assert!(state.swamp.filter_cells.is_empty());
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
            key(&mut state, BareKey::Char('s'));
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
    fn refreshing_preserves_selection_scroll_and_content_after_failure() {
        let mut state = loaded();
        state.swamp.selected = 1;
        state.swamp.list_scroll = Some(1);
        state.poll_swamp(REFRESH_MS, false);
        let mut reordered: serde_json::Value = serde_json::from_str(SEARCH).unwrap();
        reordered["issues"].as_array_mut().unwrap().reverse();
        reply(&mut state, &reordered.to_string());
        assert_eq!(state.swamp.selected, 0);
        assert_eq!(state.swamp.list_scroll, Some(1));
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
        let line = state.swamp.last_body_top + 3; // second metadata row plus tab bar
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
            assert!(issue_lines(&state.swamp.issues()[0], true, cols)
                .iter()
                .all(|line| line.width() <= cols));
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
        let lines = issue_lines(issue, true, 8);
        assert_eq!(
            &lines[1..],
            &[
                "  alpha",
                "  beta",
                "  gamma",
                "  abcdef",
                "  ghijk",
                "  界面 é"
            ]
        );
        assert!(!lines[0].contains("alpha"));
        for cols in 0..20 {
            assert!(issue_lines(issue, false, cols)
                .iter()
                .all(|line| line.width() <= cols));
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
        assert_eq!(state.swamp.selected, 2);
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
        assert!(state.swamp.window <= first_end);
        assert!(state.swamp.window + state.swamp.last_body_rows >= state.swamp.list_rows.len());
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

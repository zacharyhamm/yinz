//! Agent-only presentation; ordinary command rows and plugin palettes remain single-line.
use super::*;
use std::ops::Range;
use yinz_bus::command_palette::{HistoryEvent, HistoryHeader};
use yinz_tile_markdown::{self as md, MdBlock, MdLine, MdStyle, Options};

struct Document {
    blocks: Vec<MdBlock>,
    width: Option<usize>,
    lines: Vec<MdLine>,
}
impl Document {
    fn markdown(text: &str) -> Self {
        Self {
            blocks: md::parse(text),
            width: None,
            lines: vec![],
        }
    }
    fn literal(text: &str, code: bool) -> Self {
        Self {
            blocks: vec![MdBlock {
                chars: text
                    .chars()
                    .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
                    .map(|ch| md::StyledChar {
                        ch,
                        style: MdStyle {
                            color: code.then_some(0),
                            ..Default::default()
                        },
                    })
                    .collect(),
                hard_wrap: code,
                ..Default::default()
            }],
            width: None,
            lines: vec![],
        }
    }
    fn lines(&mut self, width: usize) -> &[MdLine] {
        if self.width != Some(width) {
            self.lines = if width == 0 {
                vec![]
            } else {
                md::wrap(&self.blocks, width, Options::default())
            };
            self.width = Some(width);
        }
        &self.lines
    }
}
struct Event {
    data: HistoryEvent,
    document: agent_proto::agents::EntryPresentation,
}
struct Reader {
    header: String,
    document: Document,
    offset: usize,
    sections: Vec<agent_proto::envelope::Section>,
    section_blocks: Vec<Vec<MdBlock>>,
    fold: usize,
    width: Option<usize>,
    lines: Vec<MdLine>,
    starts: Vec<usize>,
    raw: bool,
}
impl Reader {
    fn new(
        header: String,
        document: Document,
        sections: Vec<agent_proto::envelope::Section>,
    ) -> Self {
        let section_blocks = sections
            .iter()
            .map(agent_proto::envelope::section_blocks)
            .collect();
        Self {
            header,
            document,
            offset: 0,
            sections,
            section_blocks,
            fold: 0,
            width: None,
            lines: vec![],
            starts: vec![],
            raw: false,
        }
    }
    fn toggle_fold(&mut self, index: usize) {
        self.sections[index].open = !self.sections[index].open;
        self.section_blocks[index] = agent_proto::envelope::section_blocks(&self.sections[index]);
        self.width = None;
    }
}

struct BrowsePosition {
    revision: Option<u64>,
    anchor: Option<(u64, usize)>,
    offset: usize,
    following: bool,
}

pub(super) struct AgentView {
    pub(super) header: HistoryHeader,
    events: Vec<Event>,
    actions: Vec<Entry>,
    status: String,
    selected: usize,
    action: usize,
    actions_focused: bool,
    pub following: bool,
    pub newest: bool,
    anchor: Option<(u64, usize)>,
    reveal: bool,
    reader: Option<Reader>,
    spans: Vec<Range<usize>>,
    offset: usize,
    total: usize,
    page: usize,
    searching: bool,
    saved_position: Option<BrowsePosition>,
}

pub(super) struct AgentLayout {
    pub(super) lines: Vec<MdLine>,
    hits: Vec<Option<Hit>>,
    selected: Vec<bool>,
    pub(super) padding: usize,
    title: String,
    pub(super) quota: Option<super::QuotaPlacement>,
}
#[derive(Clone, Copy)]
enum Hit {
    Event(usize),
    Action(usize),
    Fold(usize),
}

impl AgentView {
    pub fn new(header: HistoryHeader) -> Self {
        Self {
            header,
            events: vec![],
            actions: vec![],
            status: String::new(),
            selected: 0,
            action: 0,
            actions_focused: false,
            following: true,
            newest: true,
            anchor: None,
            reveal: true,
            reader: None,
            spans: vec![],
            offset: 0,
            total: 0,
            page: 1,
            searching: false,
            saved_position: None,
        }
    }
    pub fn search_mode(&mut self, searching: bool) {
        if searching && !self.searching {
            self.saved_position = Some(BrowsePosition {
                revision: self.events.get(self.selected).map(|e| e.data.revision),
                anchor: self.anchor,
                offset: self.offset,
                following: self.following,
            });
        }
        self.searching = searching;
    }
    pub fn update(
        &mut self,
        header: HistoryHeader,
        events: Vec<HistoryEvent>,
        actions: Vec<Entry>,
        status: String,
    ) {
        let revision = self.events.get(self.selected).map(|e| e.data.revision);
        let mut old = std::mem::take(&mut self.events);
        self.events = events
            .into_iter()
            .map(|data| {
                if let Some(i) = old.iter().position(|e| e.data == data) {
                    old.remove(i)
                } else {
                    let document = agent_proto::agents::EntryPresentation::new(
                        &data.kind,
                        &data.markdown,
                        data.truncated,
                    );
                    Event { data, document }
                }
            })
            .collect();
        self.selected = if self.following {
            self.events.len().saturating_sub(1)
        } else {
            revision
                .and_then(|r| self.events.iter().position(|e| e.data.revision == r))
                .unwrap_or_else(|| {
                    self.reveal = true;
                    0
                })
        };
        if !self.searching
            && let Some(saved) = self.saved_position.take()
        {
            self.following = saved.following;
            self.anchor = saved.anchor;
            self.offset = saved.offset;
            self.reveal = false;
            self.selected = if self.following {
                self.events.len().saturating_sub(1)
            } else {
                saved
                    .revision
                    .and_then(|revision| {
                        self.events.iter().position(|e| {
                            e.data.revision == revision || e.data.revisions.contains(&revision)
                        })
                    })
                    .unwrap_or_else(|| {
                        self.anchor = None;
                        self.reveal = true;
                        0
                    })
            };
        }
        self.header = header;
        self.action = self
            .actions
            .get(self.action)
            .and_then(|old| actions.iter().position(|new| new.id == old.id))
            .unwrap_or(0);
        self.actions = actions;
        self.status = status;
    }
    pub fn is_reader(&self) -> bool {
        self.reader.is_some()
    }
    pub fn first_revision(&self) -> Option<u64> {
        self.events.first().map(|e| e.data.revision)
    }
    pub fn select_revision(&mut self, revision: u64) {
        self.following = false;
        if let Some(i) = self
            .events
            .iter()
            .position(|e| e.data.revision == revision || e.data.revisions.contains(&revision))
        {
            self.selected = i;
            self.reveal = true;
        }
    }
    pub fn page_request(&mut self, following: bool) {
        self.saved_position = None;
        self.following = following;
        self.anchor = None;
        self.reveal = true;
        self.actions_focused = false;
    }
    pub fn reader(&mut self, header: String, markdown: String, tool: String) {
        let sections = if header == "prompt" {
            agent_proto::envelope::sections(&markdown)
        } else {
            vec![]
        };
        let mut document = if matches!(header.as_str(), "tool_call" | "tool_result") {
            Document::literal(&markdown, false)
        } else {
            Document::markdown(&markdown)
        };
        if !tool.is_empty() {
            document
                .blocks
                .extend(Document::literal(&tool, true).blocks);
        }
        self.reader = Some(Reader::new(header, document, sections));
    }
    pub fn back(&mut self) -> bool {
        // Details stays in this view; event readers have their own palette history entry.
        if self.reader.as_ref().is_some_and(|r| r.header == "Details") {
            self.reader = None;
            return true;
        }
        false
    }
    fn remember_anchor(&mut self) {
        self.anchor = self
            .spans
            .iter()
            .position(|s| s.contains(&self.offset))
            .and_then(|i| {
                self.events
                    .get(i)
                    .map(|e| (e.data.revision, self.offset - self.spans[i].start))
            });
    }
    pub fn scroll(&mut self, down: bool, amount: usize) {
        let end = self.total.saturating_sub(self.page);
        if let Some(reader) = &mut self.reader {
            reader.offset = if down {
                reader.offset.saturating_add(amount).min(end)
            } else {
                reader.offset.saturating_sub(amount)
            };
        } else {
            self.offset = if down {
                self.offset.saturating_add(amount).min(end)
            } else {
                self.offset.saturating_sub(amount)
            };
            self.following = self.newest && down && self.offset == end;
            if self.following {
                self.selected = self.events.len().saturating_sub(1);
            }
            self.reveal = false;
            self.remember_anchor();
        }
    }
    pub fn key(&mut self, key: &KeyWithModifier, view: u64) -> Option<Outcome> {
        let bare = key.bare_key;
        if let Some(reader) = &mut self.reader {
            if key.has_no_modifiers() && !reader.sections.is_empty() {
                match bare {
                    BareKey::Char('j') => {
                        reader.fold = (reader.fold + 1).min(reader.sections.len() - 1)
                    },
                    BareKey::Char('k') => reader.fold = reader.fold.saturating_sub(1),
                    BareKey::Char('v') => {
                        reader.raw = !reader.raw;
                        reader.offset = 0;
                    },
                    BareKey::Enter => {
                        reader.toggle_fold(reader.fold);
                    },
                    _ => {},
                }
                if matches!(bare, BareKey::Char('j' | 'k') | BareKey::Enter) {
                    reader.offset = reader.starts.get(reader.fold).copied().unwrap_or(0);
                    return Some(Outcome::None);
                }
            }
            match bare {
                BareKey::Up | BareKey::Left => self.scroll(false, 1),
                BareKey::Down | BareKey::Right => self.scroll(true, 1),
                BareKey::PageUp => self.scroll(false, self.page.max(1)),
                BareKey::PageDown => self.scroll(true, self.page.max(1)),
                BareKey::Home => self.scroll(false, usize::MAX),
                BareKey::End => self.scroll(true, usize::MAX),
                _ => {},
            }
            return Some(Outcome::None);
        }
        match bare {
            BareKey::Tab => self.actions_focused = !self.actions_focused,
            BareKey::Enter if self.actions_focused => {
                if let Some(action) = self.actions.get(self.action) {
                    if action.id == "history-details" {
                        self.reader = Some(Reader::new(
                            "Details".into(),
                            Document::literal(&self.header.details, false),
                            vec![],
                        ));
                    } else {
                        return Some(Outcome::Activate(action.id.clone()));
                    }
                }
            },
            BareKey::Enter => {
                if let Some(event) = self.events.get(self.selected) {
                    return Some(Outcome::Activate(format!(
                        "history:{view}:event:{}",
                        event.data.revision
                    )));
                }
            },
            BareKey::Up | BareKey::Down | BareKey::Home | BareKey::End if !self.actions_focused => {
                self.selected = match bare {
                    BareKey::Up => self.selected.saturating_sub(1),
                    BareKey::Down => self
                        .selected
                        .saturating_add(1)
                        .min(self.events.len().saturating_sub(1)),
                    BareKey::Home => 0,
                    _ => self.events.len().saturating_sub(1),
                };
                self.following = self.newest
                    && matches!(bare, BareKey::Down | BareKey::End)
                    && self.selected + 1 == self.events.len();
                self.reveal = true;
            },
            BareKey::Up | BareKey::Left if self.actions_focused => {
                self.action = self.action.saturating_sub(1)
            },
            BareKey::Down | BareKey::Right if self.actions_focused => {
                self.action = self
                    .action
                    .saturating_add(1)
                    .min(self.actions.len().saturating_sub(1))
            },
            BareKey::Home if self.actions_focused => self.action = 0,
            BareKey::End if self.actions_focused => {
                self.action = self.actions.len().saturating_sub(1)
            },
            BareKey::PageUp => self.scroll(false, self.page.max(1)),
            BareKey::PageDown => self.scroll(true, self.page.max(1)),
            _ => return None,
        }
        Some(Outcome::None)
    }
    pub fn click(&mut self, layout: &AgentLayout, row: usize) {
        match layout.hits.get(row).copied().flatten() {
            Some(Hit::Event(i)) => {
                self.selected = i;
                self.actions_focused = false;
                self.following = false;
            },
            Some(Hit::Action(i)) => {
                self.action = i;
                self.actions_focused = true;
            },
            Some(Hit::Fold(i)) => {
                if let Some(reader) = &mut self.reader {
                    reader.fold = i;
                    reader.toggle_fold(i);
                }
            },
            None => {},
        }
    }
    pub fn layout(
        &mut self,
        rect: CellRect,
        border: usize,
        input: &TextInput,
        error: Option<&str>,
        busy: bool,
    ) -> AgentLayout {
        let query = input.get_text();
        let height = rect.rows.saturating_sub(border * 2);
        let inner = rect.cols.saturating_sub(border * 2);
        let padding = usize::from(inner >= 6);
        let width = inner.saturating_sub(padding * 2);
        let mut layout = AgentLayout {
            lines: vec![],
            hits: vec![],
            selected: vec![],
            padding,
            quota: None,
            title: format!(
                " Agent info · {} ",
                if self.reader.is_some() {
                    "Reader"
                } else if self.actions_focused {
                    "Actions"
                } else {
                    "Timeline"
                }
            ),
        };
        if height == 0 || width == 0 {
            return layout;
        }
        let add = |layout: &mut AgentLayout, line: MdLine, hit: Option<Hit>, selected: bool| {
            layout.lines.push(line);
            layout.hits.push(hit);
            layout.selected.push(selected);
        };
        let reader = self.reader.is_some();
        if reader && height >= 3 {
            add(
                &mut layout,
                MdLine::plain(
                    self.reader
                        .as_ref()
                        .map(|r| r.header.clone())
                        .unwrap_or_default(),
                ),
                None,
                false,
            );
        } else if !reader {
            if let Some(mut presentation) = self
                .header
                .live
                .as_ref()
                .and_then(AgentPresentation::from_pane)
            {
                presentation.project.clone_from(&self.header.project);
                presentation.branch.clone_from(&self.header.branch);
                for line in presentation.lines(
                    width.saturating_sub(2),
                    height.saturating_sub(8).clamp(1, 3),
                ) {
                    add(
                        &mut layout,
                        line.prepend("┃ ", MdStyle::default()),
                        None,
                        true,
                    );
                }
            }
            let top = layout.lines.len();
            let available = height.saturating_sub(top + 4);
            let full = width >= 44 && available.saturating_sub(1) >= 10;
            let budget = if full {
                available.saturating_sub(3)
            } else {
                available.saturating_sub(3).min(height / 2)
            };
            for line in header_lines(&self.header, width, budget, full) {
                layout.lines.push(line);
                layout.hits.push(None);
                layout.selected.push(false);
            }
            if full
                && let Some(quota) = self.header.usage.as_ref().map(|usage| {
                    agent_proto::quota::Display::new(
                        usage,
                        agent_proto::agents::now_ms().max(0) as u64,
                    )
                })
                && quota.value.is_some()
                && budget >= 5
            {
                layout.quota = Some(super::QuotaPlacement {
                    rect: CellRect {
                        x: rect.x + border + padding + width - 18,
                        y: rect.y + border + top + 1,
                        cols: 18,
                        rows: 2,
                    },
                    display: quota,
                    line: top + 1,
                });
            }
        }
        if height >= 8 && !reader {
            add(
                &mut layout,
                dim(if query.is_empty() {
                    "> Type to search this run".into()
                } else {
                    viewport("> ", query, input.get_cursor_position(), width).0
                }),
                None,
                false,
            );
        }
        if height >= 10 {
            add(&mut layout, dim("─".repeat(width)), None, false);
        }
        let footer = usize::from(height >= 5);
        let action_rows = if reader {
            0
        } else {
            self.actions
                .len()
                .min(if height >= 16 { 3 } else { 1 })
                .min(height.saturating_sub(layout.lines.len() + footer + 1))
        };
        let page = height.saturating_sub(layout.lines.len() + footer + action_rows);
        self.page = page;
        let mut lines = vec![];
        self.spans.clear();
        if let Some(reader) = &mut self.reader {
            let wrapped = if !reader.raw && !reader.sections.is_empty() {
                if reader.width != Some(width) {
                    let folded = agent_proto::envelope::wrap_sections(
                        reader.section_blocks.iter().map(Vec::as_slice),
                        width,
                    );
                    reader.starts = folded.starts;
                    reader.lines = folded.lines;
                    reader.width = Some(width);
                }
                &reader.lines
            } else {
                reader.document.lines(width)
            };
            lines.extend(wrapped.iter().cloned().enumerate().map(|(row, l)| {
                let fold = (!reader.raw && !reader.starts.is_empty()).then(|| {
                    reader
                        .starts
                        .partition_point(|start| *start <= row)
                        .saturating_sub(1)
                });
                (
                    l,
                    fold.map(Hit::Fold),
                    fold == Some(reader.fold) && reader.starts.get(reader.fold) == Some(&row),
                )
            }));
            self.total = lines.len();
            reader.offset = reader.offset.min(self.total.saturating_sub(page));
            self.offset = reader.offset;
        } else {
            for (i, event) in self.events.iter_mut().enumerate() {
                let start = lines.len();
                let selected = i == self.selected && !self.actions_focused;
                let hit = Some(Hit::Event(i));
                let preview = if !self.searching && event.data.kind == "tool_call" {
                    vec![dim(format!(
                        "{}tool call x{}",
                        agent_proto::agents::age_gutter(
                            agent_proto::agents::now_ms(),
                            event.data.created_at_ms
                        ),
                        event.data.tool_calls.max(1)
                    ))]
                } else {
                    event.document.lines(
                        &event.data.kind,
                        width,
                        &agent_proto::agents::age_gutter(
                            agent_proto::agents::now_ms(),
                            event.data.created_at_ms,
                        ),
                    )
                };
                lines.extend(preview.into_iter().map(|l| (l, hit, selected)));
                lines.push((MdLine::blank(), hit, false));
                self.spans.push(start..lines.len());
            }
            if let Some(last) = self.spans.last_mut() {
                last.end -= 1;
                lines.truncate(last.end);
            }
            self.total = lines.len();
            if self.following {
                self.offset = self.total.saturating_sub(page);
            } else if let Some((revision, line)) = self.anchor
                && let Some(i) = self.events.iter().position(|e| {
                    e.data.revision == revision || e.data.revisions.contains(&revision)
                })
            {
                self.offset = self.spans[i].start + line.min(self.spans[i].len().saturating_sub(1));
            }
            if self.reveal
                && !self.following
                && let Some(span) = self.spans.get(self.selected)
            {
                if span.start < self.offset || span.start >= self.offset + page {
                    self.offset = span.start;
                } else if span.end > self.offset + page {
                    self.offset = span.end.saturating_sub(page).min(span.start);
                }
            }
            self.offset = self.offset.min(self.total.saturating_sub(page));
            self.reveal = false;
            self.remember_anchor();
            if lines.is_empty() {
                lines.push((
                    dim(if self.status.is_empty() {
                        "No retained events"
                    } else {
                        &self.status
                    }),
                    None,
                    false,
                ));
            }
        }
        for (line, hit, selected) in lines.into_iter().skip(self.offset).take(page) {
            layout.lines.push(line);
            layout.hits.push(hit);
            layout.selected.push(selected);
        }
        while layout.lines.len() < height.saturating_sub(footer + action_rows) {
            layout.lines.push(MdLine::blank());
            layout.hits.push(None);
            layout.selected.push(false);
        }
        let first = self.action.saturating_add(1).saturating_sub(action_rows);
        for (i, action) in self
            .actions
            .iter()
            .enumerate()
            .skip(first)
            .take(action_rows)
        {
            let selected = self.actions_focused && i == self.action;
            let mut label = super::inline(&action.label)
                .prepend(if selected { "› " } else { "· " }, MdStyle::default());
            for style in &mut label.styles {
                style.dim = true;
            }
            layout.lines.push(label);
            layout.hits.push(Some(Hit::Action(i)));
            layout.selected.push(selected);
        }
        if footer > 0 {
            layout.lines.push(dim(error.unwrap_or(if busy {
                "Loading… · Esc Cancel"
            } else if reader {
                "↑↓ Scroll · j/k Folds · Enter Toggle · v Raw · Esc Back"
            } else if self.actions_focused {
                "↑↓ Actions · Enter Choose · Tab Timeline · Esc Back"
            } else {
                if width >= 60 {
                    "↑↓ Events · Enter Read · PgUp/Dn Scroll · Tab Actions · Esc Back"
                } else {
                    "↑↓ · Enter Read · Tab · Esc"
                }
            })));
            layout.hits.push(None);
            layout.selected.push(false);
        }
        layout
    }
}

pub(super) fn clipped(line: &MdLine, width: usize) -> MdLine {
    let text = take_columns(&line.text, width);
    let styles = line
        .styles
        .iter()
        .copied()
        .take(text.chars().count())
        .collect();
    MdLine { text, styles }
}

fn header_lines(header: &HistoryHeader, width: usize, budget: usize, full: bool) -> Vec<MdLine> {
    use unicode_width::UnicodeWidthStr;
    if budget == 0 {
        return vec![];
    }
    let quota = header.usage.as_ref().map(|usage| {
        agent_proto::quota::Display::new(usage, agent_proto::agents::now_ms().max(0) as u64)
    });
    let full = full
        && quota
            .as_ref()
            .is_some_and(|q| q.value.is_some() || !q.short.is_empty());
    let left_width = width.saturating_sub(if full { 20 } else { 0 });
    let normal = |text: String, color| {
        let styles = vec![
            MdStyle {
                color,
                ..Default::default()
            };
            text.chars().count()
        ];
        MdLine { text, styles }
    };
    let mut identity = normal(header.status.clone(), Some(3));
    if let Some(agent) = header.live.as_ref().and_then(|pane| pane.agent.as_ref()) {
        let status_start = header
            .status
            .rfind(" · ")
            .map(|at| header.status[..at + " · ".len()].chars().count())
            .unwrap_or(0);
        for (index, style) in identity.styles.iter_mut().enumerate() {
            style.bold = index < status_start;
            style.color = if index < status_start {
                Some(0)
            } else if agent.state == yinz_types::agent::AgentState::Waiting {
                Some(1)
            } else {
                None
            };
        }
    }
    let mut details = vec![identity];
    if !header.task.is_empty() {
        let mut line = MdLine::plain(format!("Task: {}", header.task));
        line.styles = vec![
            MdStyle {
                bold: true,
                ..Default::default()
            };
            line.text.chars().count()
        ];
        details.push(line);
    }
    if !header.project.is_empty() {
        details.push(normal(
            format!(
                "{}{}{}",
                header.project,
                if header.branch.is_empty() { "" } else { " · " },
                header.branch
            ),
            Some(2),
        ));
    }
    if !header.location.is_empty() {
        details.push(normal(header.location.clone(), Some(3)));
    }
    if !header.started.is_empty() {
        details.push(normal(format!("Started {}", header.started), Some(3)));
    }
    let bottom =
        !header.session_id.is_empty() || quota.as_ref().is_some_and(|q| !q.reset.is_empty());
    let room = budget.saturating_sub(usize::from(bottom));
    let other_rows = quota
        .as_ref()
        .filter(|_| full)
        .map(|q| {
            q.other.len().min(room.saturating_sub(if q.value.is_some() {
                4
            } else {
                1 + usize::from(q.stale)
            }))
        })
        .unwrap_or(0);
    let detail_room = room.saturating_sub(other_rows);
    details.truncate(detail_room.saturating_sub(usize::from(quota.is_some() && !full)));
    for line in &mut details {
        *line = clipped(line, left_width);
    }
    if let Some(quota) = &quota {
        if full {
            let right = if let Some(value) = quota.value {
                vec![
                    quota.weekly_label.clone(),
                    format!("{value}%"),
                    String::new(),
                    quota.short.clone(),
                ]
            } else {
                let mut right = vec![quota.short.clone()];
                if quota.stale {
                    right.push("stale".into());
                }
                right
            };
            for (i, text) in right.into_iter().take(detail_room).enumerate() {
                while details.len() <= i {
                    details.push(MdLine::blank());
                }
                let text = take_columns(&text, 18);
                let line = &mut details[i];
                append(
                    line,
                    &MdLine::plain(
                        " ".repeat(width.saturating_sub(line.text.width() + text.width())),
                    ),
                );
                append(line, &normal(text, None));
            }
            details.extend(
                quota
                    .other
                    .iter()
                    .take(other_rows)
                    .map(|text| clipped(&normal(text.clone(), None), width)),
            );
        } else if room > 0 {
            details.push(clipped(&normal(quota.compact.clone(), None), width));
        }
    }
    if bottom {
        let reset = quota
            .as_ref()
            .map(|q| take_columns(&q.reset, width))
            .unwrap_or_default();
        let mut line = clipped(
            &normal(header.session_id.clone(), Some(3)),
            width.saturating_sub(reset.width() + usize::from(!reset.is_empty())),
        );
        let label_end = if header.session_id.starts_with("session ") {
            7
        } else {
            4
        };
        for style in line.styles.iter_mut().skip(label_end) {
            style.color = None;
        }
        let gap = width.saturating_sub(line.text.width() + reset.width());
        append(&mut line, &MdLine::plain(" ".repeat(gap)));
        append(&mut line, &normal(reset, None));
        details.push(line);
    }
    details.truncate(budget);
    details
}

fn dim(text: impl Into<String>) -> MdLine {
    let text = text.into();
    let styles = vec![
        MdStyle {
            dim: true,
            ..Default::default()
        };
        text.chars().count()
    ];
    MdLine { text, styles }
}

pub(super) fn chunks(
    layout: &CommandPaletteLayout,
    agent: &AgentLayout,
    style: &Style,
) -> Vec<CharacterChunk> {
    let rect = layout.rect;
    if rect.is_empty() {
        return vec![];
    }
    let colors = style.colors.text_unselected;
    let border: RcCharacterStyles = RESET_STYLES
        .foreground(Some(colors.emphasis_0.into()))
        .into();
    (0..rect.rows)
        .map(|row| {
            let mut chars = vec![];
            if layout.border == 1 && (row == 0 || row + 1 == rect.rows) {
                let text = if row == 0 {
                    format!("╭{}╮", fit(&agent.title, rect.cols - 2, '─'))
                } else {
                    format!("╰{}╯", "─".repeat(rect.cols - 2))
                };
                chars.extend(
                    text.chars()
                        .map(|c| TerminalCharacter::new_styled(c, border.clone())),
                );
            } else {
                if layout.border == 1 {
                    chars.push(TerminalCharacter::new_styled('│', border.clone()));
                }
                let index = row.saturating_sub(layout.border);
                let selected = agent.selected.get(index).copied().unwrap_or(false);
                let base = if selected {
                    RESET_STYLES
                        .foreground(Some(style.colors.text_selected.base.into()))
                        .background(Some(style.colors.text_selected.background.into()))
                } else {
                    RESET_STYLES.foreground(Some(colors.base.into()))
                };
                let line = agent.lines.get(index);
                let width = rect.cols.saturating_sub(layout.border * 2);
                let content = format!(
                    "{}{}",
                    " ".repeat(agent.padding),
                    line.map(|l| l.text.as_str()).unwrap_or("")
                );
                let text = fit(
                    &content
                        .chars()
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .collect::<String>(),
                    width.saturating_sub(agent.padding),
                    ' ',
                ) + &" ".repeat(agent.padding);
                for (i, c) in text.chars().enumerate() {
                    let md = i
                        .checked_sub(agent.padding)
                        .and_then(|i| line.and_then(|l| l.styles.get(i)))
                        .copied()
                        .unwrap_or_default();
                    chars.push(TerminalCharacter::new_styled(
                        c,
                        super::paint_markdown_with_selection(base.into(), md, style, selected),
                    ));
                }
                if layout.border == 1 {
                    chars.push(TerminalCharacter::new_styled('│', border.clone()));
                }
            }
            CharacterChunk::new(chars, rect.x, rect.y + row)
        })
        .collect()
}

#[cfg(test)]
#[path = "unit/command_palette_agent_tests.rs"]
mod tests;

//! Agent-only presentation; ordinary command rows and plugin palettes remain single-line.
use super::*;
use std::ops::Range;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Search,
    Messages,
    Prompt,
    Action(usize),
}

pub(super) struct AgentView {
    pub(super) header: HistoryHeader,
    pub(super) live_id: Option<String>,
    pub(super) hook_flash: bool,
    events: Vec<Event>,
    actions: Vec<Entry>,
    status: String,
    selected: usize,
    focus: Focus,
    search_expanded: bool,
    prompt: TextInput,
    prompt_available: bool,
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
    widgets: Vec<Widget>,
    arrow_fonts: bool,
    pub(super) cursor: Option<(usize, usize)>,
    pub(super) padding: usize,
    title: String,
    pub(super) quota: Option<super::QuotaPlacement>,
    pub(super) graphical_header: Option<Vec<MdLine>>,
}
struct Widget {
    rect: CellRect,
    hit: Hit,
    focused: bool,
}
#[derive(Clone, Copy)]
enum Hit {
    Search,
    Prompt,
    Event(usize),
    Action(usize),
    Fold(usize),
}

impl AgentView {
    pub(super) fn shows_header(&self) -> bool {
        self.reader.is_none()
    }

    pub fn new(header: HistoryHeader) -> Self {
        Self {
            header,
            live_id: None,
            hook_flash: false,
            events: vec![],
            actions: vec![],
            status: String::new(),
            selected: 0,
            focus: Focus::Search,
            search_expanded: false,
            prompt: TextInput::default(),
            prompt_available: false,
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
        let mut old: std::collections::BTreeMap<_, _> = std::mem::take(&mut self.events)
            .into_iter()
            .map(|event| (event.data.revision, event))
            .collect();
        self.events = events
            .into_iter()
            .map(|data| {
                if let Some(event) = old.remove(&data.revision).filter(|e| e.data == data) {
                    event
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
                .and_then(|r| {
                    self.events
                        .iter()
                        .position(|e| e.data.revision == r || e.data.revisions.contains(&r))
                })
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
        self.prompt_available = actions.iter().any(|a| a.id == "history-prompt");
        let actions: Vec<_> = actions
            .into_iter()
            .filter(|a| a.id != "history-prompt")
            .collect();
        if let Focus::Action(index) = self.focus {
            self.focus = self
                .actions
                .get(index)
                .and_then(|old| actions.iter().position(|new| new.id == old.id))
                .or_else(|| (!actions.is_empty()).then_some(0))
                .map(Focus::Action)
                .unwrap_or(Focus::Search);
        } else if self.focus == Focus::Prompt && !self.prompt_available {
            self.focus = Focus::Search;
        }
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
        self.reset_page(following);
        self.focus = Focus::Messages;
        self.search_expanded = false;
    }
    pub fn retarget_run(&mut self) {
        self.reset_page(true);
    }
    fn reset_page(&mut self, following: bool) {
        self.saved_position = None;
        self.following = following;
        self.anchor = None;
        self.reveal = true;
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
    pub fn back(&mut self, input: &mut TextInput) -> bool {
        // Details stays in this view; event readers have their own palette history entry.
        if self.reader.as_ref().is_some_and(|r| r.header == "Details") {
            self.reader = None;
            return true;
        }
        if self.reader.is_none() && self.search_expanded {
            input.clear();
            self.search_expanded = false;
            self.focus = Focus::Search;
            return true;
        }
        false
    }
    pub fn prompt_sent(&mut self) {
        self.prompt.clear();
    }

    pub fn insert_text(&mut self, input: &mut TextInput, text: &str) {
        if self.reader.is_some() {
            return;
        }
        let text: String = text
            .chars()
            .map(|c| if c.is_whitespace() { ' ' } else { c })
            .collect();
        match self.focus {
            Focus::Search if self.search_expanded => input.insert_text(&text),
            Focus::Prompt if self.prompt_available => self.prompt.insert_text(&text),
            _ => {},
        }
    }

    fn tab(&mut self, reverse: bool) {
        let mut targets = vec![Focus::Search, Focus::Messages];
        if self.prompt_available {
            targets.push(Focus::Prompt);
        }
        targets.extend((0..self.actions.len()).map(Focus::Action));
        let current = targets.iter().position(|f| *f == self.focus).unwrap_or(0);
        let next = if reverse {
            current + targets.len() - 1
        } else {
            current + 1
        };
        self.focus = targets[next % targets.len()];
    }

    fn activate_action(&mut self, index: usize) -> Outcome {
        if let Some(action) = self.actions.get(index) {
            if action.id == "history-details" {
                self.reader = Some(Reader::new(
                    "Details".into(),
                    Document::literal(&self.header.details, false),
                    vec![],
                ));
            } else {
                return Outcome::Activate(action.id.clone());
            }
        }
        Outcome::None
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
    pub fn key(&mut self, key: &KeyWithModifier, view: u64, input: &mut TextInput) -> Outcome {
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
                    return Outcome::None;
                }
            }
            let bare = match bare {
                BareKey::Char('p') if key.has_only_modifiers(&[KeyModifier::Ctrl]) => BareKey::Up,
                BareKey::Char('n') if key.has_only_modifiers(&[KeyModifier::Ctrl]) => BareKey::Down,
                _ => bare,
            };
            match bare {
                BareKey::Up | BareKey::Left => self.scroll(false, 1),
                BareKey::Down | BareKey::Right => self.scroll(true, 1),
                BareKey::PageUp => self.scroll(false, self.page.max(1)),
                BareKey::PageDown => self.scroll(true, self.page.max(1)),
                BareKey::Home => self.scroll(false, usize::MAX),
                BareKey::End => self.scroll(true, usize::MAX),
                _ => {},
            }
            return Outcome::None;
        }
        if bare == BareKey::Tab {
            self.tab(key.has_modifiers(&[KeyModifier::Shift]));
            return Outcome::None;
        }
        match self.focus {
            Focus::Search => {
                if self.search_expanded {
                    input.handle_key(key.clone());
                } else if key.has_no_modifiers()
                    && matches!(bare, BareKey::Enter | BareKey::Char(' '))
                {
                    self.search_expanded = true;
                }
            },
            Focus::Prompt => {
                if bare == BareKey::Enter {
                    if self.prompt_available && !self.prompt.get_text().trim().is_empty() {
                        return Outcome::Submit {
                            id: "history-prompt".into(),
                            values: vec![self.prompt.get_text().into()],
                        };
                    }
                } else {
                    self.prompt.handle_key(key.clone());
                }
            },
            Focus::Action(index) => match bare {
                BareKey::Enter | BareKey::Char(' ') if key.has_no_modifiers() => {
                    return self.activate_action(index)
                },
                BareKey::Left | BareKey::Up => self.focus = Focus::Action(index.saturating_sub(1)),
                BareKey::Right | BareKey::Down => {
                    self.focus =
                        Focus::Action((index + 1).min(self.actions.len().saturating_sub(1)))
                },
                BareKey::Home => self.focus = Focus::Action(0),
                BareKey::End => self.focus = Focus::Action(self.actions.len().saturating_sub(1)),
                _ => {},
            },
            Focus::Messages => {
                let navigation = match bare {
                    BareKey::Char('k') if key.has_no_modifiers() => BareKey::Up,
                    BareKey::Char('j') if key.has_no_modifiers() => BareKey::Down,
                    BareKey::Char('p') if key.has_only_modifiers(&[KeyModifier::Ctrl]) => {
                        BareKey::Up
                    },
                    BareKey::Char('n') if key.has_only_modifiers(&[KeyModifier::Ctrl]) => {
                        BareKey::Down
                    },
                    _ => bare,
                };
                match navigation {
                    BareKey::Enter => {
                        if let Some(event) = self.events.get(self.selected) {
                            return Outcome::Activate(format!(
                                "history:{view}:event:{}",
                                event.data.revision
                            ));
                        }
                    },
                    BareKey::Up | BareKey::Down | BareKey::Home | BareKey::End => {
                        self.selected = match navigation {
                            BareKey::Up => self.selected.saturating_sub(1),
                            BareKey::Down => {
                                (self.selected + 1).min(self.events.len().saturating_sub(1))
                            },
                            BareKey::Home => 0,
                            _ => self.events.len().saturating_sub(1),
                        };
                        self.following = self.newest
                            && matches!(navigation, BareKey::Down | BareKey::End)
                            && self.selected + 1 == self.events.len();
                        self.reveal = true;
                    },
                    BareKey::PageUp => self.scroll(false, self.page.max(1)),
                    BareKey::PageDown => self.scroll(true, self.page.max(1)),
                    _ => {},
                }
            },
        }
        Outcome::None
    }
    pub fn click(
        &mut self,
        layout: &AgentLayout,
        column: usize,
        row: usize,
        input: &mut TextInput,
    ) -> Outcome {
        let widget = layout.widgets.iter().find(|w| w.rect.contains(column, row));
        let hit = widget
            .map(|w| w.hit)
            .or_else(|| layout.hits.get(row).copied().flatten());
        match hit {
            Some(Hit::Search) => {
                self.focus = Focus::Search;
                self.search_expanded = true;
                if let Some(widget) = widget {
                    input.set_cursor_position(yinz_types::text_input::viewport::column_to_cursor(
                        "Search: [ ",
                        input.get_text(),
                        input.get_cursor_position(),
                        widget.rect.cols.saturating_sub(2),
                        column.saturating_sub(widget.rect.x),
                    ));
                }
            },
            Some(Hit::Prompt) if self.prompt_available => {
                self.focus = Focus::Prompt;
                if let Some(widget) = widget {
                    self.prompt.set_cursor_position(
                        yinz_types::text_input::viewport::column_to_cursor(
                            "Prompt: [ ",
                            self.prompt.get_text(),
                            self.prompt.get_cursor_position(),
                            widget.rect.cols.saturating_sub(2),
                            column.saturating_sub(widget.rect.x),
                        ),
                    );
                }
            },
            Some(Hit::Event(i)) => {
                self.following = false;
                self.selected = i;
                self.focus = Focus::Messages;
            },
            Some(Hit::Action(i)) => {
                self.focus = Focus::Action(i);
                return self.activate_action(i);
            },
            Some(Hit::Fold(i)) => {
                if let Some(reader) = &mut self.reader {
                    reader.fold = i;
                    reader.toggle_fold(i);
                }
            },
            _ => {},
        }
        Outcome::None
    }
    pub fn layout(
        &mut self,
        rect: CellRect,
        border: usize,
        input: &TextInput,
        error: Option<&str>,
        busy: bool,
        arrow_fonts: bool,
    ) -> AgentLayout {
        let height = rect.rows.saturating_sub(border * 2);
        let inner = rect.cols.saturating_sub(border * 2);
        let padding = usize::from(inner >= 6);
        let width = inner.saturating_sub(padding * 2);
        let mut layout = AgentLayout {
            lines: vec![],
            hits: vec![],
            selected: vec![],
            widgets: vec![],
            arrow_fonts,
            cursor: None,
            padding,
            quota: None,
            graphical_header: None,
            title: if self.reader.is_some() {
                " Agent info · Reader "
            } else {
                " Agent info "
            }
            .into(),
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
        let footer = usize::from(height >= 5);
        let search_rows = usize::from(!reader && height >= 4);
        let prompt_rows = usize::from(!reader && height >= 5);
        let separator = usize::from(height >= 10);
        let buttons = if reader {
            vec![]
        } else {
            button_rows(&self.actions, width, arrow_fonts)
        };
        // Even constrained windows keep a message visible; larger windows reserve three.
        let message_rows = if height >= 10 { 3 } else { 1 };
        let action_rows = buttons.len().min(
            height
                .saturating_sub(footer + search_rows + prompt_rows + separator + message_rows + 1),
        );
        let bottom_rows = footer + prompt_rows + action_rows;

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
            let available = height.saturating_sub(bottom_rows + search_rows + separator);
            let full = width >= 44 && available.saturating_sub(1) >= 10;
            let budget = if full {
                available.saturating_sub(3)
            } else {
                available.saturating_sub(message_rows).min(height / 2)
            };
            let header = agent_header(&self.header, width, budget, full, self.hook_flash);
            for line in header.lines {
                add(&mut layout, line, None, false);
            }
            layout.graphical_header = header.graphical;
            if let Some(display) = header.quota {
                layout.quota = Some(super::QuotaPlacement {
                    rect: CellRect {
                        x: rect.x + border + padding + width - 18,
                        y: rect.y + border + 1,
                        cols: 18,
                        rows: 2,
                    },
                    display,
                });
            }
        }

        if search_rows > 0 {
            let focused = self.focus == Focus::Search;
            let row = layout.lines.len();
            let (line, cursor) = if self.search_expanded {
                input_line("Search", input, "Search this run…", width)
            } else {
                (MdLine::plain("[ Search ]"), None)
            };
            let cols = if self.search_expanded {
                width
            } else {
                10.min(width)
            };
            layout.widgets.push(Widget {
                rect: CellRect {
                    x: 0,
                    y: row,
                    cols,
                    rows: 1,
                },
                hit: Hit::Search,
                focused,
            });
            if focused {
                layout.cursor = cursor.map(|column| (column, row));
            }
            add(&mut layout, line, None, false);
        }
        if separator > 0 {
            add(&mut layout, dim("─".repeat(width)), None, false);
        }
        let page = height.saturating_sub(layout.lines.len() + bottom_rows);
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
            let now = agent_proto::agents::now_ms();
            for (i, event) in self.events.iter_mut().enumerate() {
                let start = lines.len();
                let selected = i == self.selected && self.focus == Focus::Messages;
                let hit = Some(Hit::Event(i));
                let preview = if event.data.kind == "tools" {
                    let count = if event.data.tool_calls == 0 {
                        event.data.revisions.len()
                    } else {
                        event.data.tool_calls
                    };
                    vec![clipped(
                        &dim(format!("tools x{count}")),
                        width.saturating_sub(1),
                    )]
                } else {
                    event.document.lines(
                        &event.data.kind,
                        width.saturating_sub(1),
                        &agent_proto::agents::age_gutter(now, event.data.created_at_ms),
                    )
                };
                lines.extend(preview.into_iter().map(|line| {
                    (
                        line.prepend(if selected { "┃" } else { " " }, MdStyle::default()),
                        hit,
                        selected,
                    )
                }));
                self.spans.push(start..lines.len());
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
        while layout.lines.len() < height.saturating_sub(bottom_rows) {
            add(&mut layout, MdLine::blank(), None, false);
        }
        if prompt_rows > 0 {
            let row = layout.lines.len();
            let focused = self.focus == Focus::Prompt;
            let (line, cursor) = if self.prompt_available {
                input_line("Prompt", &self.prompt, "Type a message…", width)
            } else {
                (dim("Prompt unavailable"), None)
            };
            if self.prompt_available {
                layout.widgets.push(Widget {
                    rect: CellRect {
                        x: 0,
                        y: row,
                        cols: width,
                        rows: 1,
                    },
                    hit: Hit::Prompt,
                    focused,
                });
            }
            if focused {
                layout.cursor = cursor.map(|column| (column, row));
            }
            add(&mut layout, line, None, false);
        }
        let focused_row = if let Focus::Action(index) = self.focus {
            buttons
                .iter()
                .position(|(_, spans)| spans.iter().any(|(_, _, i)| *i == index))
                .unwrap_or(0)
        } else {
            0
        };
        let first = (focused_row + 1).saturating_sub(action_rows);
        for (line, spans) in buttons.into_iter().skip(first).take(action_rows) {
            let row = layout.lines.len();
            for (x, cols, index) in spans {
                layout.widgets.push(Widget {
                    rect: CellRect {
                        x,
                        y: row,
                        cols,
                        rows: 1,
                    },
                    hit: Hit::Action(index),
                    focused: self.focus == Focus::Action(index),
                });
            }
            add(&mut layout, line, None, false);
        }
        if footer > 0 {
            layout.lines.push(dim(error.unwrap_or(if busy {
                "Loading… · Esc Cancel"
            } else if reader {
                "↑↓ Scroll · j/k Folds · Enter Toggle · v Raw · Esc Back"
            } else {
                match self.focus {
                    Focus::Prompt if width < 50 => "Tab · Enter Send · Esc",
                    Focus::Prompt => "Tab Next · Enter Send · Esc Back",
                    Focus::Action(_) if width < 50 => "Tab · ←/→ · Enter · Esc",
                    Focus::Action(_) => "Tab Next · ←/→ Move · Enter Choose · Esc Back",
                    Focus::Search if self.search_expanded => "Tab Messages · Esc Clear search",
                    _ if width >= 50 => "Tab Next · j/k Move · Enter Read · Esc Back",
                    _ => "Tab · j/k · Enter · Esc",
                }
            })));
            layout.hits.push(None);
            layout.selected.push(false);
        }
        layout
    }
}

fn input_line(
    label: &str,
    input: &TextInput,
    placeholder: &str,
    width: usize,
) -> (MdLine, Option<usize>) {
    let prefix = format!("{label}: [ ");
    let (text, cursor) = viewport(
        &prefix,
        input.get_text(),
        input.get_cursor_position(),
        width.saturating_sub(2),
    );
    let mut line = MdLine::plain(text);
    if input.is_empty() {
        let room = width.saturating_sub(2 + line.text.width());
        append(&mut line, &dim(take_columns(placeholder, room)));
    }
    line.text = fit(&line.text, width.saturating_sub(2), ' ');
    line.styles
        .resize(line.text.chars().count(), MdStyle::default());
    let room = width.saturating_sub(line.text.width());
    append(&mut line, &MdLine::plain(take_columns(" ]", room)));
    (
        line,
        (width > 0).then_some(cursor.min(width.saturating_sub(1))),
    )
}

type ButtonRow = (MdLine, Vec<(usize, usize, usize)>);

fn button_rows(actions: &[Entry], width: usize, arrow_fonts: bool) -> Vec<ButtonRow> {
    let mut rows: Vec<ButtonRow> = vec![];
    for (index, action) in actions.iter().enumerate() {
        let chrome = if arrow_fonts { 4 } else { 2 };
        let label = plain_text(&action.label);
        let button = if width < chrome {
            MdLine::plain(take_columns(&label, width))
        } else {
            let label = take_columns(&label, width - chrome);
            let arrow = if arrow_fonts { "" } else { "" };
            MdLine::plain(format!("{arrow} {label} {arrow}"))
        };
        let cols = button.text.width();
        if rows
            .last()
            .is_none_or(|(line, _)| line.text.width() + 2 + cols > width)
        {
            rows.push((MdLine::blank(), vec![]));
        }
        if let Some((line, spans)) = rows.last_mut() {
            if !spans.is_empty() {
                append(line, &MdLine::plain("  "));
            }
            spans.push((line.text.width(), cols, index));
            append(line, &button);
        }
    }
    rows
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

struct HeaderLayout {
    lines: Vec<MdLine>,
    graphical: Option<Vec<MdLine>>,
    quota: Option<agent_proto::quota::Display>,
}

/// Both render modes share one grid, so changing graphics support cannot move hit targets.
fn agent_header(
    header: &HistoryHeader,
    width: usize,
    budget: usize,
    full: bool,
    hook_flash: bool,
) -> HeaderLayout {
    let now = agent_proto::agents::now_ms().max(0) as u64;
    let quota = header
        .usage
        .as_ref()
        .map(|usage| agent_proto::quota::Display::new(usage, now));
    let full = full && width >= 44 && budget >= 6;
    let left_width = if full { width - 21 } else { width };
    let live = header
        .live
        .as_ref()
        .and_then(AgentPresentation::from_pane)
        .map(|mut live| {
            live.hook_flash = hook_flash;
            live
        });
    let field = |label: &str, value: &str| {
        let value = value.trim().replace(char::is_control, " ");
        MdLine::plain(format!(
            "{label}: {}",
            if value.is_empty() { "—" } else { &value }
        ))
    };
    let identity = live
        .as_ref()
        .map(|p| p.lines(left_width, 1).remove(0))
        .unwrap_or_else(|| {
            MdLine::plain(if header.status.is_empty() {
                "Agent · —".into()
            } else {
                header.status.clone()
            })
        });
    let activity = live
        .as_ref()
        .map(|p| p.activity.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&header.task);
    let branch = if header.branch == "no branch" {
        ""
    } else {
        &header.branch
    };
    let project = [header.project.as_str(), branch]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let (id_label, id) = header
        .session_id
        .strip_prefix("run ")
        .map(|id| ("Run", id))
        .unwrap_or((
            "Session",
            header
                .session_id
                .strip_prefix("session ")
                .unwrap_or(&header.session_id),
        ));
    let mut details = vec![
        identity,
        field("Task", activity),
        field("Project", &project),
        field("Location", &header.location),
        field("Started", &header.started),
        field(id_label, id),
    ];
    details[1].styles = vec![
        MdStyle {
            bold: true,
            ..Default::default()
        };
        details[1].text.chars().count()
    ];
    if !full {
        let usage_rows = usize::from(quota.is_some())
            + usize::from(quota.as_ref().is_some_and(|q| !q.reset.is_empty()));
        details.truncate(budget.saturating_sub(usage_rows).max(1).min(budget));
        if let Some(q) = quota {
            for text in [&q.compact, &q.reset].into_iter().filter(|s| !s.is_empty()) {
                if details.len() < budget {
                    let mut line = ellipsized(&MdLine::plain(text), width);
                    let gap = width.saturating_sub(line.text.width());
                    line = line.prepend(&" ".repeat(gap), MdStyle::default());
                    details.push(line);
                }
            }
        }
        return HeaderLayout {
            lines: details.iter().map(|line| ellipsized(line, width)).collect(),
            graphical: None,
            quota: None,
        };
    }

    let mut right = vec![];
    let mut graphical_right = None;
    if let Some(q) = &quota {
        right.push(q.weekly_label.clone());
        right.push(
            q.value
                .map(|v| format!("{v}%"))
                .unwrap_or_else(|| "—".into()),
        );
        let mut tail = vec![if q.short.is_empty() {
            "5hr · —".into()
        } else {
            q.short.clone()
        }];
        // Leave room for reset even with many custom windows.
        tail.extend(q.other.iter().take(budget.saturating_sub(5)).cloned());
        tail.push(if q.reset.is_empty() {
            "Reset —".into()
        } else {
            q.reset.clone()
        });
        if q.value.is_some() {
            let mut graphical = right.clone();
            graphical[1].clear();
            graphical.push(String::new()); // The raster covers both of these cells' rows.
            graphical.extend(tail.clone());
            graphical_right = Some(graphical);
        }
        right.extend(tail);
        if q.value.is_none() && q.short.is_empty() && q.other.is_empty() {
            right[0] = "Usage unavailable".into();
            right[1] = q.compact.clone();
        }
    } else {
        right = vec![
            "Usage unavailable".into(),
            "—".into(),
            "5hr · —".into(),
            "Reset —".into(),
        ];
    }
    let height = graphical_right
        .as_ref()
        .map_or(right.len(), Vec::len)
        .max(6)
        .min(budget);
    // Extra windows can use extra detail rows for long tasks and locations.
    let mut extra = height - 6;
    for index in [3, 1] {
        if extra == 0 {
            break;
        }
        let wrapped = wrap_inline(&details[index], left_width);
        let count = wrapped.len().min(extra + 1);
        if count > 1 {
            let mut lines = wrapped[..count].to_vec();
            if wrapped.len() > count {
                lines[count - 1] = ellipsized(
                    &MdLine::plain(format!("{}…", lines[count - 1].text)),
                    left_width,
                );
            }
            details.splice(index..=index, lines);
            extra -= count - 1;
        }
    }
    let compose = |right: &[String]| {
        (0..height)
            .map(|row| {
                let mut line = details
                    .get(row)
                    .map(|line| ellipsized(line, left_width))
                    .unwrap_or_else(|| MdLine::plain("—"));
                let gap = left_width.saturating_sub(line.text.width());
                append(&mut line, &MdLine::plain(format!("{} │ ", " ".repeat(gap))));
                let text = right.get(row).map(String::as_str).unwrap_or("");
                // Preserve percentages when custom window names exceed the usage column.
                let text = if let Some((label, value)) = text.rsplit_once(" · ") {
                    let suffix = format!(" · {value}");
                    format!(
                        "{}{suffix}",
                        take_columns(label, 18usize.saturating_sub(suffix.width()))
                    )
                } else {
                    ellipsized(&MdLine::plain(text), 18).text
                };
                append(
                    &mut line,
                    &MdLine::plain(format!(
                        "{}{}",
                        " ".repeat(18usize.saturating_sub(text.width())),
                        text
                    )),
                );
                line
            })
            .collect()
    };
    HeaderLayout {
        lines: compose(&right),
        graphical: graphical_right.as_ref().map(|right| compose(right)),
        quota: quota.filter(|q| q.value.is_some()),
    }
}

fn ellipsized(line: &MdLine, width: usize) -> MdLine {
    if line.text.width() <= width {
        let mut line = line.clone();
        line.styles
            .resize(line.text.chars().count(), MdStyle::default());
        return line;
    }
    if width == 0 {
        return MdLine::blank();
    }
    let mut line = clipped(line, width - 1);
    append(&mut line, &MdLine::plain("…"));
    line
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
                let base = |selected| {
                    if selected {
                        RESET_STYLES
                            .foreground(Some(style.colors.text_selected.base.into()))
                            .background(Some(style.colors.text_selected.background.into()))
                    } else {
                        RESET_STYLES.foreground(Some(colors.base.into()))
                    }
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
                let mut column: usize = 0;
                for (i, c) in text.chars().enumerate() {
                    let widget = column.checked_sub(agent.padding).and_then(|x| {
                        agent
                            .widgets
                            .iter()
                            .find(|widget| widget.rect.contains(x, index))
                    });
                    let selected = selected || widget.is_some_and(|widget| widget.focused);
                    if let Some(widget) =
                        widget.filter(|widget| matches!(widget.hit, Hit::Action(_)))
                    {
                        let declaration = if widget.focused {
                            style.colors.ribbon_selected
                        } else {
                            style.colors.ribbon_unselected
                        };
                        let x = column - agent.padding - widget.rect.x;
                        let arrows = agent.arrow_fonts && widget.rect.cols >= 4;
                        let (foreground, background) = if arrows && x == 0 {
                            (colors.background, declaration.background)
                        } else if arrows && x + 1 == widget.rect.cols {
                            (declaration.background, colors.background)
                        } else {
                            (declaration.base, declaration.background)
                        };
                        chars.push(TerminalCharacter::new_styled(
                            c,
                            RESET_STYLES
                                .foreground(Some(foreground.into()))
                                .background(Some(background.into()))
                                .bold(Some(AnsiCode::On))
                                .into(),
                        ));
                        column += c.width().unwrap_or(0);
                        continue;
                    }
                    let mut md = i
                        .checked_sub(agent.padding)
                        .and_then(|i| line.and_then(|l| l.styles.get(i)))
                        .copied()
                        .unwrap_or_default();
                    // The agents panel normalizes Markdown dim to ordinary, unbold text.
                    if matches!(agent.hits.get(index), Some(Some(Hit::Event(_)))) {
                        md.dim = false;
                    }
                    chars.push(TerminalCharacter::new_styled(
                        c,
                        super::paint_markdown_with_selection(
                            base(selected).into(),
                            md,
                            style,
                            selected,
                        ),
                    ));
                    column += c.width().unwrap_or(0);
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

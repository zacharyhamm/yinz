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
    document: Document,
}
struct Reader {
    header: String,
    document: Document,
    offset: usize,
}

pub(super) struct AgentView {
    header: HistoryHeader,
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
}

pub(super) struct AgentLayout {
    lines: Vec<MdLine>,
    hits: Vec<Option<Hit>>,
    selected: Vec<bool>,
    padding: usize,
    title: String,
}
#[derive(Clone, Copy)]
enum Hit {
    Event(usize),
    Action(usize),
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
        }
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
                    let document = Document::markdown(&data.markdown);
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
        if let Some(i) = self.events.iter().position(|e| e.data.revision == revision) {
            self.selected = i;
            self.reveal = true;
        }
    }
    pub fn page_request(&mut self, following: bool) {
        self.following = following;
        self.anchor = None;
        self.reveal = true;
        self.actions_focused = false;
    }
    pub fn reader(&mut self, header: String, markdown: String, tool: String) {
        let mut document = Document::markdown(&markdown);
        if !tool.is_empty() {
            document
                .blocks
                .extend(Document::literal(&tool, true).blocks);
        }
        self.reader = Some(Reader {
            header,
            document,
            offset: 0,
        });
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
        if self.reader.is_some() {
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
                        self.reader = Some(Reader {
                            header: "Details".into(),
                            document: Document::literal(&self.header.details, false),
                            offset: 0,
                        });
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
        let mut add = |line: MdLine, hit: Option<Hit>, selected: bool| {
            layout.lines.push(line);
            layout.hits.push(hit);
            layout.selected.push(selected);
        };
        let reader = self.reader.is_some();
        if height >= 3 {
            add(
                MdLine::plain(
                    self.reader
                        .as_ref()
                        .map(|r| r.header.clone())
                        .unwrap_or_else(|| {
                            use unicode_width::UnicodeWidthStr;
                            format!(
                                "{} · {}",
                                take_columns(
                                    &self.header.name,
                                    width
                                        .saturating_sub(self.header.status.width() + 3)
                                        .max(width / 3)
                                ),
                                self.header.status
                            )
                        }),
                ),
                None,
                false,
            );
        }
        if height >= 12 && !reader {
            add(
                dim(format!(
                    "{} · Started {}",
                    take_columns(
                        &self.header.location,
                        width.saturating_sub(self.header.started.len() + 11)
                    ),
                    self.header.started
                )),
                None,
                false,
            );
        }
        if height >= 8 && !reader {
            add(
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
            add(dim("─".repeat(width)), None, false);
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
            lines.extend(
                reader
                    .document
                    .lines(width)
                    .iter()
                    .cloned()
                    .map(|l| (l, None, false)),
            );
            self.total = lines.len();
            reader.offset = reader.offset.min(self.total.saturating_sub(page));
            self.offset = reader.offset;
        } else {
            for (i, event) in self.events.iter_mut().enumerate() {
                let start = lines.len();
                let selected = i == self.selected && !self.actions_focused;
                let hit = Some(Hit::Event(i));
                lines.push((
                    dim(format!(
                        "{} {} · {}",
                        if selected { "›" } else { " " },
                        event.data.kind,
                        event.data.time
                    )),
                    hit,
                    selected,
                ));
                let preview = event.document.lines(width);
                let more = preview.len() > 4 || event.data.truncated;
                let keep = if more { 3 } else { 4 };
                lines.extend(
                    preview
                        .iter()
                        .take(keep)
                        .cloned()
                        .map(|l| (l, hit, selected)),
                );
                if more {
                    lines.push((dim("… more"), hit, selected));
                }
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
            } else if let Some((revision, line)) = self.anchor {
                if let Some(i) = self.events.iter().position(|e| e.data.revision == revision) {
                    self.offset =
                        self.spans[i].start + line.min(self.spans[i].len().saturating_sub(1));
                }
            }
            if self.reveal && !self.following {
                if let Some(span) = self.spans.get(self.selected) {
                    if span.start < self.offset || span.start >= self.offset + page {
                        self.offset = span.start;
                    } else if span.end > self.offset + page {
                        self.offset = span.end.saturating_sub(page).min(span.start);
                    }
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
                "↑↓ Scroll · PgUp/PgDn · Home/End · Esc Back"
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
                        super::paint_markdown(base.into(), md, style),
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

//! Text wrapping and screen layout.
//!
//! Zellij wipes the plugin's grid and repaints it fully on every render, and
//! gives plugins no scrollback of their own, so everything on screen is
//! recomputed here from `State` each time and all scroll state is ours.

use hocus_proto::text::{sanitize, wrap, wrap_input, WrappedLine};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zellij_tile::prelude::*;

use crate::api::ConfigError;
use crate::{Picker, PickerKind, Role, RowKind, State, Status};

/// Below this we can't lay out a title, a transcript row and an input row,
/// so we say so instead of rendering nonsense.
pub(crate) const MIN_ROWS: usize = 6;
pub(crate) const MIN_COLS: usize = 24;
/// Rows the input box may grow to before it starts scrolling internally.
const INPUT_MAX_ROWS: usize = 5;

const PROMPT: &str = "> ";
/// The input box's prompt, which says which mode you are typing into.
///
/// Read by `chat_layout` and `place_cursor`, which is what stops the box and the
/// cursor disagreeing about how wide it is — so both must go through here.
fn prompt(state: &State) -> &'static str {
    if state.plan_mode_on() {
        "plan> "
    } else {
        PROMPT
    }
}

// ---------------------------------------------------------------------------
// Transcript
// ---------------------------------------------------------------------------

/// One rendered line of the transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct VLine {
    pub text: String,
    pub role: Role,
    styles: Vec<MdStyle>,
}

impl VLine {
    fn blank() -> Self {
        VLine {
            text: String::new(),
            role: Role::Notice,
            styles: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct MdStyle {
    bold: bool,
    dim: bool,
    color: Option<usize>,
}

#[derive(Debug, Clone)]
struct StyledChar {
    ch: char,
    style: MdStyle,
}

#[derive(Default)]
struct MdBlock {
    chars: Vec<StyledChar>,
    prefix: String,
    continuation: String,
    compact: bool,
    hard_wrap: bool,
}

struct ListState {
    next: Option<u64>,
}

struct LinkState {
    start: usize,
    destination: String,
}

#[derive(Default)]
struct MarkdownRenderer {
    blocks: Vec<MdBlock>,
    current: Option<MdBlock>,
    styles: Vec<MdStyle>,
    lists: Vec<ListState>,
    links: Vec<LinkState>,
    quote_depth: usize,
}

impl MarkdownRenderer {
    fn start_block(&mut self, prefix: String, continuation: String, compact: bool, style: MdStyle) {
        if self.current.is_some() {
            self.finish_block();
        }
        self.current = Some(MdBlock {
            prefix,
            continuation,
            compact,
            ..MdBlock::default()
        });
        self.styles.clear();
        self.styles.push(style);
    }

    fn start_paragraph(&mut self) {
        if self.current.is_some() {
            return;
        }
        let prefix = "│ ".repeat(self.quote_depth);
        self.start_block(
            prefix.clone(),
            prefix,
            !self.lists.is_empty(),
            MdStyle::default(),
        );
    }

    fn finish_block(&mut self) {
        if let Some(block) = self.current.take() {
            self.blocks.push(block);
        }
        self.styles.clear();
        self.links.clear();
    }

    fn style(&self) -> MdStyle {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, update: impl FnOnce(&mut MdStyle)) {
        let mut style = self.style();
        update(&mut style);
        self.styles.push(style);
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn append(&mut self, text: &str) {
        if self.current.is_none() {
            self.start_paragraph();
        }
        let style = self.style();
        self.current
            .as_mut()
            .expect("started above")
            .chars
            .extend(text.chars().map(|ch| StyledChar { ch, style }));
    }

    fn append_with_style(&mut self, text: &str, style: MdStyle) {
        if self.current.is_none() {
            self.start_paragraph();
        }
        self.current
            .as_mut()
            .expect("started above")
            .chars
            .extend(text.chars().map(|ch| StyledChar { ch, style }));
    }

    fn start_item(&mut self) {
        self.finish_block();
        let depth = self.lists.len().max(1);
        let list = self.lists.last_mut().expect("item belongs to a list");
        let marker = match &mut list.next {
            Some(next) => {
                let marker = format!("{next}. ");
                *next += 1;
                marker
            },
            None => "• ".to_owned(),
        };
        let prefix = format!(
            "{}{}{}",
            "│ ".repeat(self.quote_depth),
            "  ".repeat(depth - 1),
            marker
        );
        self.start_block(
            prefix.clone(),
            " ".repeat(prefix.width()),
            true,
            MdStyle::default(),
        );
    }

    fn start_link(&mut self, destination: &str) {
        if self.current.is_none() {
            self.start_paragraph();
        }
        let start = self.current.as_ref().expect("started above").chars.len();
        self.links.push(LinkState {
            start,
            destination: destination.to_owned(),
        });
        self.push_style(|style| style.color = Some(2));
    }

    fn end_link(&mut self) {
        self.pop_style();
        let Some(link) = self.links.pop() else {
            return;
        };
        let label: String = self
            .current
            .as_ref()
            .map(|block| {
                block.chars[link.start..]
                    .iter()
                    .map(|styled| styled.ch)
                    .collect()
            })
            .unwrap_or_default();
        if label != link.destination {
            let suffix = if label.is_empty() {
                link.destination
            } else {
                format!(" ({})", link.destination)
            };
            self.append_with_style(
                &suffix,
                MdStyle {
                    color: Some(2),
                    ..MdStyle::default()
                },
            );
        }
    }

    fn event(&mut self, event: Event<'_>, cols: usize) {
        match event {
            Event::Start(Tag::Paragraph) => self.start_paragraph(),
            Event::End(TagEnd::Paragraph) => self.finish_block(),
            Event::Start(Tag::Heading { .. }) => self.start_block(
                "│ ".repeat(self.quote_depth),
                "│ ".repeat(self.quote_depth),
                false,
                MdStyle {
                    bold: true,
                    color: Some(0),
                    dim: false,
                },
            ),
            Event::End(TagEnd::Heading(_)) => self.finish_block(),
            Event::Start(Tag::BlockQuote(_)) => {
                self.finish_block();
                self.quote_depth += 1;
            },
            Event::End(TagEnd::BlockQuote(_)) => {
                self.finish_block();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            },
            Event::Start(Tag::List(start)) => {
                if self
                    .current
                    .as_ref()
                    .is_some_and(|block| !block.chars.is_empty())
                {
                    self.finish_block();
                }
                self.lists.push(ListState { next: start });
            },
            Event::End(TagEnd::List(_)) => {
                self.finish_block();
                self.lists.pop();
            },
            Event::Start(Tag::Item) => self.start_item(),
            Event::End(TagEnd::Item) => self.finish_block(),
            Event::Start(Tag::CodeBlock(_)) => {
                self.finish_block();
                let prefix = "│ ".repeat(self.quote_depth);
                self.start_block(
                    prefix.clone(),
                    prefix,
                    false,
                    MdStyle {
                        color: Some(3),
                        ..MdStyle::default()
                    },
                );
                self.current.as_mut().expect("just started").hard_wrap = true;
            },
            Event::End(TagEnd::CodeBlock) => {
                if self
                    .current
                    .as_mut()
                    .and_then(|block| block.chars.last())
                    .is_some_and(|styled| styled.ch == '\n')
                {
                    self.current.as_mut().expect("checked above").chars.pop();
                }
                self.finish_block();
            },
            Event::Start(Tag::HtmlBlock) => self.start_paragraph(),
            Event::End(TagEnd::HtmlBlock) => {
                if self
                    .current
                    .as_mut()
                    .and_then(|block| block.chars.last())
                    .is_some_and(|styled| styled.ch == '\n')
                {
                    self.current.as_mut().expect("checked above").chars.pop();
                }
                self.finish_block();
            },
            Event::Start(Tag::Emphasis) => self.push_style(|style| style.color = Some(1)),
            Event::End(TagEnd::Emphasis) => self.pop_style(),
            Event::Start(Tag::Strong) => self.push_style(|style| style.bold = true),
            Event::End(TagEnd::Strong) => self.pop_style(),
            Event::Start(Tag::Link { dest_url, .. })
            | Event::Start(Tag::Image { dest_url, .. }) => self.start_link(&dest_url),
            Event::End(TagEnd::Link) | Event::End(TagEnd::Image) => self.end_link(),
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => self.append(&text),
            Event::Code(text) => self.append_with_style(
                &text,
                MdStyle {
                    color: Some(3),
                    ..MdStyle::default()
                },
            ),
            Event::SoftBreak => self.append(" "),
            Event::HardBreak => self.append("\n"),
            Event::Rule => {
                self.finish_block();
                self.start_block(String::new(), String::new(), false, MdStyle::default());
                self.append_with_style(
                    &"─".repeat(cols),
                    MdStyle {
                        dim: true,
                        ..MdStyle::default()
                    },
                );
                self.current.as_mut().expect("just started").hard_wrap = true;
                self.finish_block();
            },
            _ => {},
        }
    }
}

/// The two conversation roles carry no label: a question sits against the right
/// edge and a reply against the left, which says the same thing without saying
/// "AI". The rest are asides, and the label is what tells you which kind.
fn prefix_for(role: Role) -> &'static str {
    match role {
        Role::User | Role::Assistant => "",
        Role::Tool => "fn:  ",
        Role::Error => "err: ",
        Role::Notice => "···  ",
    }
}

/// Displayed lines of a tool result before it is elided. A session listing is
/// hundreds of lines of JSON that the model needs and the user does not — it
/// would push the entire conversation off screen.
///
/// Not applied to a result carrying `Message::full_output`: a tool the user
/// typed was run *for* its output, so eliding it would defeat the command.
const TOOL_RESULT_ROWS: usize = 3;

fn styled_width(chars: &[StyledChar]) -> usize {
    chars
        .iter()
        .map(|styled| styled.ch.width().unwrap_or(0))
        .sum()
}

fn hard_wrap_styled(chars: &[StyledChar], width: usize) -> Vec<Vec<StyledChar>> {
    if width == 0 {
        return vec![Vec::new()];
    }
    let mut lines = Vec::new();
    let mut line = Vec::new();
    let mut line_width = 0;
    for styled in chars {
        if styled.ch == '\n' {
            lines.push(std::mem::take(&mut line));
            line_width = 0;
            continue;
        }
        let ch_width = styled.ch.width().unwrap_or(0);
        if line_width + ch_width > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            line_width = 0;
        }
        line.push(styled.clone());
        line_width += ch_width;
    }
    lines.push(line);
    lines
}

fn word_wrap_styled(chars: &[StyledChar], width: usize) -> Vec<Vec<StyledChar>> {
    if width == 0 {
        return vec![Vec::new()];
    }
    let mut source_lines = vec![Vec::new()];
    for styled in chars {
        if styled.ch == '\n' {
            source_lines.push(Vec::new());
        } else {
            source_lines
                .last_mut()
                .expect("seeded above")
                .push(styled.clone());
        }
    }

    let mut lines = Vec::new();
    for source in source_lines {
        let mut line = Vec::new();
        let mut pending_spaces = Vec::new();
        let mut index = 0;
        while index < source.len() {
            let spaces = source[index].ch == ' ';
            let end = (index + 1..source.len())
                .find(|next| (source[*next].ch == ' ') != spaces)
                .unwrap_or(source.len());
            let token = &source[index..end];
            index = end;
            if spaces {
                pending_spaces.extend_from_slice(token);
                continue;
            }

            let word_width = styled_width(token);
            if styled_width(&line) + styled_width(&pending_spaces) + word_width <= width {
                line.append(&mut pending_spaces);
                line.extend_from_slice(token);
            } else if word_width <= width {
                lines.push(std::mem::take(&mut line));
                pending_spaces.clear();
                line.extend_from_slice(token);
            } else {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                pending_spaces.clear();
                let mut pieces = hard_wrap_styled(token, width);
                line = pieces.pop().unwrap_or_default();
                lines.extend(pieces);
            }
        }
        lines.push(line);
    }
    lines
}

fn render_markdown_block(block: &MdBlock, cols: usize, role: Role) -> Vec<VLine> {
    let body_width = cols.saturating_sub(block.prefix.width()).max(1);
    let bodies = if block.hard_wrap {
        hard_wrap_styled(&block.chars, body_width)
    } else {
        word_wrap_styled(&block.chars, body_width)
    };

    bodies
        .into_iter()
        .enumerate()
        .map(|(index, body)| {
            let prefix = if index == 0 {
                &block.prefix
            } else {
                &block.continuation
            };
            let prefix_style = MdStyle {
                dim: true,
                ..MdStyle::default()
            };
            let mut text = prefix.clone();
            let mut styles = vec![prefix_style; prefix.chars().count()];
            text.extend(body.iter().map(|styled| styled.ch));
            styles.extend(body.into_iter().map(|styled| styled.style));
            VLine { text, role, styles }
        })
        .collect()
}

fn markdown_lines(text: &str, cols: usize) -> Vec<VLine> {
    markdown_lines_as(text, cols, Role::Assistant)
}

/// `markdown_lines`, for a message that is not the assistant speaking.
///
/// The role rides through to every line rather than being patched on
/// afterwards, because `ends_with_empty_reply` reads it: an empty
/// `Role::Assistant` line at the tail *is* the streaming placeholder, so a
/// presented result that rendered to nothing would otherwise be given a spinner.
fn markdown_lines_as(text: &str, cols: usize, role: Role) -> Vec<VLine> {
    let sanitized = sanitize(text);
    let mut renderer = MarkdownRenderer::default();
    for event in Parser::new_ext(&sanitized, Options::empty()) {
        renderer.event(event, cols);
    }
    renderer.finish_block();

    if renderer.blocks.is_empty() {
        return vec![VLine {
            text: String::new(),
            role,
            styles: Vec::new(),
        }];
    }

    // ponytail: style metadata is one tiny Copy value per displayed character.
    // Replace it with ranges only if transcript memory shows up in a profile.
    let mut lines = Vec::new();
    for (index, block) in renderer.blocks.iter().enumerate() {
        if index > 0 && !(renderer.blocks[index - 1].compact && block.compact) {
            lines.push(VLine::blank());
        }
        lines.extend(render_markdown_block(block, cols, role));
    }
    lines
}

// ---------------------------------------------------------------------------
// The waiting indicator
// ---------------------------------------------------------------------------

/// Braille spinner frames. Every frame is one column wide, so the text beside it
/// never shifts.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Timer ticks a phrase survives before the next one takes over.
pub const TICKS_PER_PHRASE: u64 = 30;

const PHRASES: [&str; 18] = [
    "Rummaging through your panes…",
    "Consulting the layout…",
    "Counting your tabs…",
    "Negotiating with the multiplexer…",
    "Warming up the wasm…",
    "Reticulating panes…",
    "Grepping the void…",
    "Asking around…",
    "Tailing the logs…",
    "Waking the daemon…",
    "Bribing the scheduler…",
    "Untangling the layout…",
    "Reading the room…",
    "Consulting the manual…",
    "Aligning the columns…",
    "Chasing a pointer…",
    "Blaming the terminal…",
    "Herding the panes…",
];

pub fn spinner(tick: u64) -> char {
    SPINNER[(tick % SPINNER.len() as u64) as usize]
}

/// The phrase for this moment of this wait.
///
/// Cycled rather than random: wasm has no cheap RNG, and walking the pool in
/// order gives variety *without* repeats, which reads better than chance does.
/// `seed` is the request number, so consecutive requests don't open on the same
/// phrase.
pub fn phrase(tick: u64, seed: u64) -> &'static str {
    let step = seed.wrapping_add(tick / TICKS_PER_PHRASE);
    PHRASES[(step % PHRASES.len() as u64) as usize]
}

/// Pad `line` to sit flush against the right edge.
///
/// Measured in columns rather than chars, so `format!("{:>width$}")` won't do:
/// it counts chars, and a CJK line would come out a column short per character.
fn right_align(line: &str, cols: usize) -> String {
    format!("{}{line}", " ".repeat(cols.saturating_sub(line.width())))
}

/// The transcript as it is drawn: every message wrapped into display lines, and
/// where in that list each message began.
pub struct Transcript {
    pub lines: Vec<VLine>,
    /// The first display line of each message, by message index — its first
    /// *content* line, not the blank separator pushed ahead of it.
    ///
    /// Always one entry per message, which is what lets the viewport hold a
    /// message on the top row by index and re-resolve it after a re-wrap.
    pub starts: Vec<usize>,
}

/// Flatten the transcript into wrapped display lines, with a blank line between
/// messages and a hanging indent on continuations.
///
/// User turns are wrapped narrow and pushed against the right edge; replies run
/// the full width from the left. That contrast is what tells them apart.
pub fn transcript(messages: &[crate::Message], cols: usize) -> Transcript {
    let mut lines = Vec::new();
    let mut starts = Vec::with_capacity(messages.len());
    for (index, message) in messages.iter().enumerate() {
        if index > 0 {
            lines.push(VLine::blank());
        }
        starts.push(lines.len());
        if message.role == Role::Assistant && message.tool_calls.is_empty() {
            lines.extend(markdown_lines(&message.content, cols));
            continue;
        }
        // A tool the user typed, drawn as the answer they asked for rather than
        // as context for the model. `content` is untouched and is still what
        // gets sent; this is only what the pane shows.
        //
        // No `fn:` prefix: the presenter opens with a heading, which says the
        // same thing, and a five-column prefix would fight the markdown
        // renderer's own indents and narrow every code fence.
        if let Some(display) = message.display.as_deref() {
            lines.extend(markdown_lines_as(display, cols, message.role));
            continue;
        }
        let prefix = prefix_for(message.role);
        let indent = " ".repeat(prefix.chars().count());
        let body_width = match message.role {
            // Two thirds: a full-width block against the right edge reads as
            // justified text rather than as a turn of its own.
            Role::User => (cols * 2 / 3).max(1),
            _ => cols.saturating_sub(prefix.chars().count()).max(1),
        };

        let mut wrapped = wrap(&message.content, body_width);
        // Truncation is display-only; `content` still goes to the model whole.
        if message.role == Role::Tool && !message.full_output && wrapped.len() > TOOL_RESULT_ROWS {
            let hidden = wrapped.len() - TOOL_RESULT_ROWS;
            wrapped.truncate(TOOL_RESULT_ROWS);
            wrapped.push(format!("… {hidden} more lines"));
        }

        for (n, line) in wrapped.into_iter().enumerate() {
            let text = match message.role {
                Role::User => right_align(&line, cols),
                _ if n == 0 => format!("{prefix}{line}"),
                _ => format!("{indent}{line}"),
            };
            lines.push(VLine {
                text,
                role: message.role,
                styles: Vec::new(),
            });
        }
    }
    Transcript { lines, starts }
}

/// What `transcript`'s output depends on: the width it was wrapped for,
/// the number of messages, and the length of the one message that can grow
/// without the count moving.
///
/// That last field is what makes the count sufficient. Only the streaming
/// placeholder is ever mutated in place (`State::append_to_reply`), and
/// `State::mutable_len` is which message that is; every other change to the
/// transcript is a push, a pop or a clear, all of which move the count.
///
/// `Message::display` needs no field of its own here for the same reason: it is
/// set once, when the message is pushed, and a push moves the count. The
/// placeholder never carries one.
pub type TranscriptKey = (usize, usize, usize);

/// `transcript`, memoised on the state. Wrapping the whole transcript is the
/// single most expensive thing a render does, and typing does not change any of
/// it.
fn cached_transcript(state: &mut State, cols: usize) -> &Transcript {
    let key: TranscriptKey = (cols, state.messages.len(), state.mutable_len());
    if state.transcript_cache.as_ref().map(|(k, _)| *k) != Some(key) {
        state.transcript_cache = Some((key, transcript(&state.messages, cols)));
    }
    &state.transcript_cache.as_ref().expect("just populated").1
}

/// Whether the transcript ends with the streaming placeholder — an assistant
/// message with nothing in it yet. The blank line between messages can't be
/// mistaken for one: it carries `Role::Notice`.
fn ends_with_empty_reply(lines: &[VLine]) -> bool {
    lines
        .last()
        .is_some_and(|line| line.role == Role::Assistant && line.text.is_empty())
}

fn styled(line: &VLine) -> Text {
    let text = Text::new(&line.text);
    let mut text = match line.role {
        Role::Error => text.error_color_range(..),
        Role::Notice => text.dim_range(..),
        // A presented result carries its own styling, one entry per character;
        // dimming the whole line would flatten every colour it asked for. A raw
        // result has no styles at all — the plain `wrap` path leaves the vector
        // empty — and still reads as the aside it is.
        Role::Tool if line.styles.is_empty() => text.dim_range(..),
        Role::Tool => text,
        // Colour levels 0-3 resolve against the user's theme host-side, so this
        // follows whatever palette they've configured. Colouring the pad along
        // with the text is free: a foreground colour on a space shows nothing.
        Role::User => text.color_range(2, ..),
        Role::Assistant => text,
    };
    let mut start = 0;
    while start < line.styles.len() {
        let style = line.styles[start];
        let end = (start + 1..line.styles.len())
            .find(|index| line.styles[*index] != style)
            .unwrap_or(line.styles.len());
        if !style.bold {
            text = text.unbold_range(start..end);
        }
        if let Some(color) = style.color {
            text = text.color_range(color, start..end);
        }
        if style.dim {
            text = text.dim_range(start..end);
        }
        start = end;
    }
    text
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

pub fn render(state: &mut State, rows: usize, cols: usize) {
    if rows < MIN_ROWS || cols < MIN_COLS {
        show_cursor(None);
        print_text_with_coordinates(Text::new("pane too small"), 0, 0, Some(cols), None);
        return;
    }

    if !state.perms {
        show_cursor(None);
        print_text_with_coordinates(
            Text::new("waiting for permissions…").dim_range(..),
            0,
            0,
            Some(cols),
            None,
        );
        return;
    }

    // The agent strip that used to own the top third is its own pane now
    // (hocus-agents, split off above this one); the conversation has the whole
    // body below the title.
    let body_y = 1;
    let body_rows = rows.saturating_sub(body_y + 1);
    let layout = state
        .picker
        .is_none()
        .then(|| chat_layout(state, body_rows, cols));

    if state.picker.is_some() {
        show_cursor(None);
    } else if let Some(layout) = &layout {
        // Must precede any print_*: the host reads the cursor position out of
        // the render output stream in order.
        place_cursor(state, layout, layout.input_y(body_y));
    }
    render_title(state, cols);

    // History and models take the top half and leave the conversation visible.
    if let Some(picker) = &state.picker {
        let picker_rows = (body_rows / 2).max(1);
        let ruled = picker_rows > 1;
        render_picker(picker, body_y, picker_rows - usize::from(ruled), cols, true);
        if ruled {
            print_text_with_coordinates(
                Text::new("─".repeat(cols)).dim_range(..),
                0,
                body_y + picker_rows - 1,
                Some(cols),
                None,
            );
        }

        let chat_y = body_y + picker_rows;
        let layout = chat_layout(state, body_rows.saturating_sub(picker_rows), cols);
        state.transcript_rows = layout.transcript_rows;
        render_chat(state, chat_y, cols, &layout);

        print_text_with_coordinates(
            Text::new(help_line(state)).dim_range(..),
            0,
            rows - 1,
            Some(cols),
            None,
        );
        return;
    }

    let layout = layout.expect("non-picker layout");
    state.transcript_rows = layout.transcript_rows;

    render_chat(state, body_y, cols, &layout);
}

/// How the conversation lays out in the rows below the title: the transcript on
/// top, a blank row, and the input box at the bottom.
struct ChatLayout {
    /// The tail of the wrapped input, one entry per row of the box. Each row
    /// carries where it starts in the wrapped string, which is how the cursor
    /// finds the cell it belongs in.
    input_lines: Vec<WrappedLine>,
    transcript_rows: usize,
}

impl ChatLayout {
    /// The first row of the input box, given the first row of the whole body.
    fn input_y(&self, y: usize) -> usize {
        y + self.transcript_rows + 1
    }
}

fn chat_layout(state: &State, height: usize, cols: usize) -> ChatLayout {
    let input_text = format!("{}{}", prompt(state), state.input.get_text());
    let all_input_lines = wrap_input(&input_text, cols);
    // Cap against the rows actually available, not just INPUT_MAX_ROWS: in a
    // minimum-height body a tall input would otherwise be laid out past the
    // last row. The row held back is the blank one above the box.
    let room_for_input = height.saturating_sub(1).max(1);
    let input_rows = all_input_lines
        .len()
        .clamp(1, INPUT_MAX_ROWS.min(room_for_input));
    ChatLayout {
        // Show the tail so the box follows the cursor on a long multi-line
        // message.
        input_lines: all_input_lines[all_input_lines.len() - input_rows..].to_vec(),
        transcript_rows: height.saturating_sub(1 + input_rows),
    }
}

/// The transcript and the input box, in the rows `layout` was measured for,
/// starting at `y`. The caller places the cursor, because that has to happen
/// before anything is printed.
fn render_chat(state: &mut State, y: usize, cols: usize, layout: &ChatLayout) {
    if let Err(error) = &state.cfg {
        render_config_banner(error, y, layout.transcript_rows, cols);
    } else {
        render_transcript(state, y, layout.transcript_rows, cols);
    }

    let input_y = layout.input_y(y);
    for (index, line) in layout.input_lines.iter().enumerate() {
        // `opaque()` paints the theme's unselected background across the whole
        // width the coordinates give, which is what makes the box read as a box
        // - and it names no colour, so it follows whatever theme is configured.
        print_text_with_coordinates(
            Text::new(&line.text).opaque(),
            0,
            input_y + index,
            Some(cols),
            None,
        );
    }
}

fn render_title(state: &mut State, cols: usize) {
    let model = match &state.cfg {
        Ok(cfg) => cfg.model.clone(),
        Err(error) => error.model.clone(),
    };
    // "hocus — {model}" lives in the real window title bar (set below) rather
    // than drawn into the pane, so it survives even when the pane is scrolled
    // or covered, and doesn't compete with the right-hand status slot for
    // space.
    let title = if state.plan_mode_on() {
        format!("hocus (plan) — {model}")
    } else {
        format!("hocus — {model}")
    };
    // Placement can rename the pane after our previous render. Reassert the
    // title rather than caching what we asked the host to do.
    rename_plugin_pane(state.own_plugin_id, &title);

    // One slot at the right edge: what is happening now beats what is pending.
    // A count rides along when other chats are working too, so a background
    // reply is visible without opening `/history` to look for it.
    let elsewhere = match state.parked_count() {
        0 => String::new(),
        n => format!(" +{n}"),
    };
    let note = if state.is_streaming() {
        format!("streaming…{elsewhere}")
    } else if !elsewhere.is_empty() {
        format!("working{elsewhere}")
    } else if !state.jobs.is_empty() {
        format!("⏲ {}", state.jobs.len())
    } else {
        return;
    };
    let x = cols.saturating_sub(note.width());
    print_text_with_coordinates(
        Text::new(&note).dim_range(..),
        x,
        0,
        Some(note.width()),
        None,
    );
}

/// The slice of transcript lines a render draws, and whether the anchor is what
/// put it there.
struct Window {
    start: usize,
    end: usize,
    /// Lines hidden *below* the viewport — the number `State`'s scroll map holds.
    offset: usize,
    /// The top row is the anchored line, so the viewport has stopped following
    /// the tail.
    frozen: bool,
}

/// Which lines to draw out of `total`, in a viewport `height` rows tall.
///
/// `offset` counts lines hidden *below* the viewport, so 0 means pinned to the
/// bottom and a growing transcript follows itself with no extra state.
///
/// `anchor_line` is the first line of the reply this client is still being held
/// to. Following the tail past it would scroll the opening of the reply off the
/// pane, so once the reply outgrows the viewport the anchor sits on the top row
/// instead and everything after a screenful of it is hidden below. `requested`
/// is ignored while anchored: a client that scrolled for itself was released
/// from the anchor by the same keystroke, and honouring a stale count here would
/// break the anchor the next time `height` changed under it.
///
/// Clamping is done here rather than by the scroll keys, which know neither the
/// line count nor the height, and it absorbs resizes and a growing transcript
/// for free.
fn transcript_window(
    total: usize,
    height: usize,
    requested: usize,
    anchor_line: Option<usize>,
) -> Window {
    let max_offset = total.saturating_sub(height);
    let offset = anchor_line
        .map_or(requested, |line| total.saturating_sub(line + height))
        .min(max_offset);
    let end = total - offset;
    let start = end.saturating_sub(height);
    Window {
        start,
        end,
        offset,
        frozen: anchor_line == Some(start),
    }
}

fn render_transcript(state: &mut State, y: usize, height: usize, cols: usize) {
    if height == 0 {
        return;
    }
    // Read before the cache borrow: these all want `&State`.
    let loader = state.loader();
    let prompt = state.confirmation_prompt();
    // The caret is appended to the last rendered line, so it is only honest
    // while the placeholder really is last. A tool typed mid-reply pushes its
    // call and result behind it; no caret then rather than one on the wrong line.
    let streaming = state.is_receiving() && state.reply_is_last();
    let scroll_offset = state.scroll_offset();
    // Never while a tool is waiting on a y/n: that prompt renders into the tail,
    // and an anchor holding an older line on the top row would leave it off the
    // bottom of the pane, unanswerable and unexplained.
    let anchor = state.anchor().filter(|_| prompt.is_none());

    let transcript = cached_transcript(state, cols);
    let lines = &transcript.lines;
    // A stale index cannot happen — `pop_empty_placeholder` drops the anchor
    // with the message — but resolving it fallibly costs nothing and keeps a
    // future edit that moves indices from indexing out of bounds.
    let anchor_line = anchor.and_then(|at| transcript.starts.get(at).copied());

    // Every decoration lands on the tail, so the cached lines are never mutated
    // and at most one of them is ever copied.
    let mut tail: Vec<VLine> = Vec::new();
    // Whether `tail` opens with a decorated copy of the last cached line.
    let mut replaced_last = false;

    match loader {
        // Waiting on a reply: the transcript already ends with the empty
        // assistant placeholder, which renders as an empty line. That line is
        // the loader's home — the spinner sits where the reply will start.
        // Waiting on a command instead, the last message is the tool-call
        // summary, so this check fails and the loader gets a line of its own.
        Some(loader) if ends_with_empty_reply(lines) => {
            let mut last = lines.last().expect("checked non-empty").clone();
            last.text.push_str(&loader);
            tail.push(last);
            replaced_last = true;
        },
        Some(loader) => {
            tail.push(VLine::blank());
            tail.push(VLine {
                text: loader,
                role: Role::Notice,
                styles: Vec::new(),
            });
        },
        // Text is arriving, so it is its own proof of life; the caret just marks
        // where the next token lands.
        None => {
            if streaming {
                if let Some(last) = lines.last() {
                    let mut last = last.clone();
                    last.text.push('▌');
                    tail.push(last);
                    replaced_last = true;
                }
            }
        },
    }

    // Name the call awaiting a y/n. The assistant message above lists every call
    // it asked for, which doesn't say which one this prompt is about.
    //
    // Wrapped here rather than left to the host: the tail skips the wrapping the
    // cached body went through, and `print_text_with_coordinates` *truncates* what
    // it is given. Unwrapped, a long argument runs off the pane edge and the user
    // agrees to something they were never shown.
    if let Some(prompt) = prompt {
        tail.push(VLine::blank());
        for line in prompt {
            for row in wrap(&line, cols) {
                tail.push(VLine {
                    text: row,
                    role: Role::Error,
                    styles: Vec::new(),
                });
            }
        }
    }

    let body = &lines[..lines.len() - usize::from(replaced_last)];
    let total = body.len() + tail.len();

    if total == 0 {
        print_text_with_coordinates(
            Text::new("Ask something.").dim_range(..),
            0,
            y,
            Some(cols),
            None,
        );
        return;
    }

    let Window {
        start,
        end,
        offset,
        frozen,
    } = transcript_window(total, height, scroll_offset, anchor_line);

    for (index, position) in (start..end).enumerate() {
        let line = match body.get(position) {
            Some(line) => line,
            None => &tail[position - body.len()],
        };
        print_text_with_coordinates(styled(line), 0, y + index, Some(cols), None);
    }

    // `body` is done with, so the cache's borrow of `state` is over and the
    // window can be written back. The render is the only thing that knows where
    // a client actually ended up, and the key handler has to start from there.
    state.set_scroll_offset(offset);

    // The badge prints over the top row, and while frozen that row is the first
    // line of the reply — the one line the anchor exists to keep on screen.
    if start > 0 && !frozen {
        print_text_with_coordinates(
            Text::new(format!("↑ {start} more")).dim_range(..),
            0,
            y,
            Some(cols),
            None,
        );
    }
    if offset > 0 {
        print_text_with_coordinates(
            Text::new(format!("↓ {offset} more")).dim_range(..),
            0,
            y + height - 1,
            Some(cols),
            None,
        );
    }
}

/// The archive (one chat per row, titled by its opening question) or the model
/// list (one id per row).
fn render_picker(picker: &Picker, y: usize, height: usize, cols: usize, active: bool) {
    if picker.entries.is_empty() {
        // An empty model list is the fetch still being in flight — `finish_models`
        // either fills it or closes the picker.
        let empty = match picker.kind {
            PickerKind::Chats(_) => "No saved chats yet.",
            PickerKind::Models => "fetching models…",
        };
        print_text_with_coordinates(Text::new(empty).dim_range(..), 0, y, Some(cols), None);
        return;
    }

    // Window the list around the selection rather than tracking a scroll offset:
    // keeping it near the middle means it is always visible and the list moves
    // by one row per keypress in both directions.
    let start = picker
        .selected
        .saturating_sub(height / 2)
        .min(picker.entries.len().saturating_sub(height));

    for (index, entry) in picker.entries.iter().skip(start).take(height).enumerate() {
        // Titles are already stripped of control characters by `history::title`,
        // and the host clips to `cols`, so neither is repeated here.
        let selected = active && start + index == picker.selected;
        let text = match entry.kind {
            RowKind::Item => {
                let line = Text::new(format!(
                    "{} {}",
                    if selected { ">" } else { " " },
                    entry.text
                ));
                match selected {
                    true => line.color_range(2, ..),
                    false => line.dim_range(..),
                }
            },
        };
        print_text_with_coordinates(text, 0, y + index, Some(cols), None);
    }
}

fn render_config_banner(error: &ConfigError, y: usize, height: usize, cols: usize) {
    // `message` carries the whole explanation (including which environment we
    // actually read), so this only has to frame it and show what we resolved.
    let body = format!(
        "hocus is not configured\n\n{}\n\nbase_url : {}\nmodel    : {}\nkey env  : {}",
        error.message, error.base_url, error.model, error.api_key_env
    );

    for (index, line) in wrap(&body, cols).into_iter().take(height).enumerate() {
        let text = if index == 0 {
            Text::new(&line).error_color_range(..)
        } else {
            Text::new(&line)
        };
        print_text_with_coordinates(text, 0, y + index, Some(cols), None);
    }
}

fn help_line(state: &State) -> &'static str {
    match state.picker.as_ref().map(|picker| &picker.kind) {
        Some(PickerKind::Chats(_)) => return "<↑↓> choose  <Enter> open  <Esc> back",
        Some(PickerKind::Models) => return "<↑↓> choose  <Enter> use  <Esc> back",
        None => {},
    }
    // Ahead of the `Status` match: a plan waits on the user rather than on the
    // tool queue, so the two are orthogonal and this is the nearer question.
    if state.awaiting_plan() {
        return "plan ready  /execute carries it out  <n> keep planning  <Esc> hide";
    }
    // Esc hides from every one of these; Ctrl-c is what interrupts, and when a tool
    // and a stream are both outstanding it takes the tool first.
    match state.status {
        Status::ConfirmingTool => "run this?  <y> yes  <n> no  <Ctrl c> abort  <Esc> hide",
        Status::AwaitingCommand => "running…  <Ctrl c> abort  <Esc> hide  <↑↓> scroll",
        Status::AwaitingSearch => "searching…  <Ctrl c> abort  <Esc> hide  <↑↓> scroll",
        Status::AwaitingLinear => "reading linear…  <Ctrl c> abort  <Esc> hide  <↑↓> scroll",
        Status::AwaitingPluginLoad => "loading plugin…  <Ctrl c> abort  <Esc> hide  <↑↓> scroll",
        Status::Idle if state.is_streaming() => {
            "streaming…  <Ctrl c> cancel  <Esc> hide  <↑↓> scroll"
        },
        Status::Idle if state.is_planning() => "planning  read-only tools  <Esc> hide",
        Status::Idle if !state.jobs.is_empty() => "<Ctrl l> new chat  <Esc> hide",
        Status::Idle => "<Ctrl l> new chat  <Esc> hide",
    }
}

/// Place the terminal cursor at the text cursor's position inside the input box.
///
/// The cursor is read out of the very rows the box is about to draw, so the two
/// cannot disagree. Deriving it instead by wrapping the text *before* it would
/// be wrong now the box wraps on words: characters typed after the cursor can
/// push the word it sits in onto the next row.
fn place_cursor(state: &State, layout: &ChatLayout, input_y: usize) {
    // get_cursor_position() is a char index, not a byte offset, and the rows
    // were wrapped from the prompt and the text together.
    let cursor = prompt(state).chars().count() + state.input.get_cursor_position();

    match cursor_cell(&layout.input_lines, cursor) {
        Some((column, row)) => show_cursor(Some((column, input_y + row))),
        // Scrolled off the top of the box: nowhere on screen to put it.
        None => show_cursor(None),
    }
}

/// The (column, row) `cursor` occupies in `lines`, or `None` when it falls above
/// them - the box shows only its last `INPUT_MAX_ROWS` rows.
///
/// The row is the last one starting at or before the cursor: one sitting exactly
/// on a boundary belongs to the later row, which is where the next keystroke
/// lands.
fn cursor_cell(lines: &[WrappedLine], cursor: usize) -> Option<(usize, usize)> {
    let row = lines.iter().rposition(|line| line.start <= cursor)?;
    let column = lines[row]
        .text
        .chars()
        .take(cursor - lines[row].start)
        .map(|ch| ch.width().unwrap_or(0))
        .sum();
    Some((column, row))
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    /// Most of these care only about the wrapping, not about where each message
    /// starts in it.
    fn transcript_lines(messages: &[Message], cols: usize) -> Vec<VLine> {
        transcript(messages, cols).lines
    }

    // -- the input box's wrapping -------------------------------------------

    /// The bug the trailing empty row exists for: a full row put the cursor at
    /// column `cols`, one past the last column, and zellij hides a plugin cursor
    /// that sits outside the pane. The wrapper itself lives in `hocus_proto::text`
    /// now; the prompt it has to leave room for is still ours.
    #[test]
    fn the_cursor_stays_inside_the_pane_when_a_row_is_full() {
        let cols = 24;
        let full = format!("{PROMPT}{}", "a".repeat(cols - PROMPT.width()));
        let lines = wrap_input(&full, cols);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            cursor_cell(&lines, full.chars().count()),
            Some((0, 1)),
            "cursor lands at column 0 of the next row"
        );
    }

    #[test]
    fn the_input_box_breaks_between_words() {
        let cols = 24;
        let text = format!("{PROMPT}some reasonably long words here");
        let rows: Vec<String> = wrap_input(&text, cols)
            .into_iter()
            .map(|line| line.text)
            .collect();
        assert_eq!(rows, vec!["> some reasonably long ", "words here"]);
    }

    /// The cursor has to follow the word the wrap moved down, which is why it is
    /// read out of the box's own rows rather than from a wrap of the text before
    /// it: "words" only left the first row because of what was typed after it.
    #[test]
    fn the_cursor_follows_a_word_the_wrap_moved_down() {
        let cols = 24;
        let text = format!("{PROMPT}some reasonably long words here");
        let lines = wrap_input(&text, cols);
        // Char index of "words", which is where the second row begins.
        let at_words = text[..text.find("words").expect("present")].chars().count();
        // "words" starts the second row, so the cursor just before it is at its
        // column 0 and the cursor after its first character is at column 1.
        assert_eq!(cursor_cell(&lines, at_words), Some((0, 1)));
        assert_eq!(cursor_cell(&lines, at_words + 1), Some((1, 1)));
    }

    #[test]
    fn the_cursor_hides_when_it_scrolls_off_the_top_of_the_box() {
        let lines = wrap_input("> aaa bbb ccc", 8);
        // Only the tail is drawn, so a cursor in a row above it has no cell.
        let tail = &lines[lines.len() - 1..];
        assert_eq!(cursor_cell(tail, 0), None);
    }

    // -- transcript_lines ---------------------------------------------------

    #[test]
    fn transcript_right_aligns_user_turns() {
        // Wrapped to two thirds of 12, then padded out to the right edge.
        let lines = transcript_lines(&[Message::user("aaaa bbbb")], 12);
        assert_eq!(
            lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            vec!["        aaaa", "        bbbb"]
        );
        assert!(lines.iter().all(|l| l.text.width() == 12));
    }

    #[test]
    fn transcript_leaves_replies_flush_left() {
        let lines = transcript_lines(&[Message::assistant("aaaa bbbb")], 6);
        assert_eq!(
            lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            vec!["aaaa", "bbbb"]
        );
    }

    #[test]
    fn assistant_markdown_renders_as_terminal_blocks() {
        let markdown = "# Heading\n\nA **bold** and *slanted* `value`.\n\n- one\n- two\n\n> quoted\n\n[site](https://example.com)\n\n---";
        let lines = transcript_lines(&[Message::assistant(markdown)], 80);
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Heading",
                "",
                "A bold and slanted value.",
                "",
                "• one",
                "• two",
                "",
                "│ quoted",
                "",
                "site (https://example.com)",
                "",
                &"─".repeat(80),
            ]
        );
    }

    #[test]
    fn fenced_code_preserves_layout_and_hard_wraps() {
        let lines = transcript_lines(&[Message::assistant("```rust\n  abcdef\nxy\n```")], 5);
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            vec!["  abc", "def", "xy"]
        );
    }

    #[test]
    fn markdown_is_not_applied_to_non_reply_messages_or_tool_summaries() {
        let mut tool_summary = Message::assistant("**literal**");
        tool_summary.tool_calls.push(crate::ToolCall {
            id: "call-1".to_owned(),
            name: "list_session".to_owned(),
            arguments: "{}".to_owned(),
        });
        let lines = transcript_lines(
            &[
                Message::user("**user**"),
                tool_summary,
                Message::tool_result("call-1", "**tool**"),
                Message::error("**error**"),
            ],
            80,
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| !line.text.is_empty())
                .map(|line| line.text.trim_start())
                .collect::<Vec<_>>(),
            vec!["**user**", "**literal**", "fn:  **tool**", "err: **error**"]
        );
    }

    #[test]
    fn a_long_tool_result_is_elided_to_a_few_lines() {
        let content = (1..=10)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let lines = transcript_lines(&[Message::tool_result("call-1", content)], 80);
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.trim())
                .collect::<Vec<_>>(),
            vec!["fn:  1", "2", "3", "… 7 more lines"]
        );
    }

    /// A presented result is drawn as markdown, with the heading styled and no
    /// `fn:` prefix competing with the renderer's own indents.
    #[test]
    fn a_presented_result_is_rendered_as_markdown() {
        let message =
            Message::tool_result_shown("manual-1", "{}", Some("## agents\n\n- one\n- two".into()));
        let lines = transcript_lines(&[message], 80);
        let texts: Vec<&str> = lines.iter().map(|line| line.text.trim_end()).collect();
        assert_eq!(texts, vec!["agents", "", "• one", "• two"]);
        // The heading is bold; nothing carries the `fn:` prefix.
        assert!(lines[0].styles.iter().all(|style| style.bold));
        assert!(!texts.iter().any(|text| text.starts_with("fn:")));
    }

    /// `styled` dims a whole tool line, which would flatten every colour a
    /// presented result asked for — so it must not, when there is styling.
    #[test]
    fn a_presented_result_keeps_its_own_styling() {
        let plain = VLine {
            text: "raw".to_owned(),
            role: Role::Tool,
            styles: Vec::new(),
        };
        let presented = VLine {
            text: "shown".to_owned(),
            role: Role::Tool,
            styles: vec![MdStyle::default(); 5],
        };
        // A Text carries its styling opaquely, so compare against the two
        // renderings we know are meant to differ.
        assert_eq!(
            format!("{:?}", styled(&plain)),
            format!("{:?}", Text::new("raw").dim_range(..))
        );
        assert_ne!(
            format!("{:?}", styled(&presented)),
            format!("{:?}", Text::new("shown").dim_range(..))
        );
    }

    /// An empty `Role::Assistant` line at the tail is how the renderer spots the
    /// streaming placeholder. A presented result must never be mistaken for one.
    #[test]
    fn a_presented_result_is_not_mistaken_for_the_streaming_placeholder() {
        let lines = transcript_lines(
            &[Message::tool_result_shown(
                "manual-1",
                "{}",
                Some(String::new()),
            )],
            80,
        );
        assert!(!ends_with_empty_reply(&lines));
    }

    /// The output of a tool the user typed is the reason they typed it, so none
    /// of it is hidden.
    #[test]
    fn a_typed_tools_result_is_rendered_whole() {
        let content = (1..=10)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let lines = transcript_lines(&[Message::tool_result_full("manual-1", content)], 80);
        let mut expected = vec!["fn:  1".to_owned()];
        expected.extend((2..=10).map(|n| n.to_string()));
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.trim())
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn markdown_styles_survive_marker_removal() {
        let lines = transcript_lines(&[Message::assistant("plain **bold** *slanted* `code`")], 80);
        let line = &lines[0];
        let style_at = |needle: &str| {
            let byte = line.text.find(needle).unwrap();
            line.styles[line.text[..byte].chars().count()]
        };
        assert_eq!(style_at("plain"), MdStyle::default());
        assert!(style_at("bold").bold);
        assert_eq!(style_at("slanted").color, Some(1));
        assert_eq!(style_at("code").color, Some(3));
    }

    #[test]
    fn lists_keep_markers_and_hanging_indents_inside_the_pane() {
        let lines = transcript_lines(
            &[Message::assistant("3. abcdefgh\n4. 日本語\n   - nested")],
            8,
        );
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            vec![
                "3. abcde",
                "   fgh",
                "4. 日本",
                "   語",
                "  • nest",
                "    ed"
            ]
        );
        assert!(lines.iter().all(|line| line.text.width() <= 8));
    }

    #[test]
    fn links_images_html_and_tables_have_readable_fallbacks() {
        let lines = transcript_lines(
            &[Message::assistant(
                "[site](https://example.com) ![logo](logo.png) <kbd>x</kbd>\n\n| a | b |",
            )],
            100,
        );
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            vec![
                "site (https://example.com) logo (logo.png) <kbd>x</kbd>",
                "",
                "| a | b |",
            ]
        );
    }

    #[test]
    fn raw_html_blocks_remain_separate_literal_blocks() {
        let lines = transcript_lines(&[Message::assistant("<div>\nraw\n</div>\n\nafter")], 80);
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            vec!["<div>", "raw", "</div>", "", "after"]
        );
    }

    #[test]
    fn markdown_sanitizes_control_characters_before_parsing() {
        let lines = transcript_lines(
            &[Message::assistant("[safe](https://exa\u{1b}mple.com)\u{7}")],
            80,
        );
        assert_eq!(lines[0].text, "safe (https://example.com)");
        assert!(!lines[0].text.chars().any(char::is_control));
    }

    #[test]
    fn incomplete_streaming_markdown_stays_renderable() {
        let lines = transcript_lines(
            &[Message::assistant(
                "An **unfinished emphasis\n\n```\npartial",
            )],
            80,
        );
        assert!(!lines.is_empty());
        assert!(lines.iter().any(|line| line.text.contains("unfinished")));
        assert!(lines.iter().any(|line| line.text.contains("partial")));
    }

    #[test]
    fn transcript_still_labels_the_asides() {
        let lines = transcript_lines(&[Message::error("boom")], 40);
        assert_eq!(lines[0].text, "err: boom");
    }

    #[test]
    fn transcript_puts_a_blank_line_between_messages() {
        let lines = transcript_lines(&[Message::user("hi"), Message::assistant("yo")], 40);
        assert_eq!(
            lines
                .iter()
                .map(|l| l.text.trim_start())
                .collect::<Vec<_>>(),
            vec!["hi", "", "yo"]
        );
    }

    #[test]
    fn transcript_of_nothing_is_nothing() {
        assert!(transcript_lines(&[], 40).is_empty());
    }

    #[test]
    fn transcript_survives_a_pane_too_narrow_to_wrap_into() {
        // body_width would underflow to 0 without the saturating_sub/max guard.
        let lines = transcript_lines(&[Message::user("hello"), Message::error("boom")], 3);
        assert!(!lines.is_empty());
    }

    // -- the transcript cache -----------------------------------------------

    /// The one case the message count can't catch: a streamed reply grows in
    /// place, so the key has to notice the last message getting longer.
    #[test]
    fn the_transcript_cache_notices_text_appended_to_the_last_message() {
        let mut state = State::default();
        state.messages.push(Message::assistant("Hel"));
        assert_eq!(cached_transcript(&mut state, 40).lines[0].text, "Hel");

        state.messages.last_mut().unwrap().content.push_str("lo");
        assert_eq!(cached_transcript(&mut state, 40).lines[0].text, "Hello");
    }

    #[test]
    fn the_transcript_cache_rewraps_when_the_pane_is_resized() {
        let mut state = State::default();
        state.messages.push(Message::user("aaaa bbbb"));
        assert_eq!(cached_transcript(&mut state, 40).lines.len(), 1);
        assert_eq!(cached_transcript(&mut state, 9).lines.len(), 2);
    }

    #[test]
    fn transcript_keeps_the_role_on_continuation_lines() {
        let lines = transcript_lines(&[Message::error("aaaa bbbb")], 9);
        assert!(lines.iter().all(|l| l.role == Role::Error));
    }

    // -- where each message starts ------------------------------------------

    #[test]
    fn every_message_gets_a_start_line() {
        let transcript = transcript(
            &[
                Message::user("aaaa bbbb cccc"),
                Message::assistant("dddd"),
                Message::error("eeee"),
            ],
            9,
        );
        assert_eq!(transcript.starts.len(), 3);
        assert_eq!(transcript.starts[0], 0);
        assert!(transcript.starts.windows(2).all(|pair| pair[0] < pair[1]));
    }

    /// The anchor puts this line on the top row, so it has to be the reply
    /// itself rather than the blank line separating it from the question.
    #[test]
    fn a_start_line_is_the_first_content_line_not_the_separator() {
        let transcript = transcript(&[Message::user("hi"), Message::assistant("yo")], 40);
        let start = transcript.starts[1];
        assert_eq!(transcript.lines[start].text, "yo");
        assert!(transcript.lines[start - 1].text.is_empty());
    }

    /// The placeholder a reply streams into is empty, and the spinner renders on
    /// it. Its start is that line, which is where the first token lands.
    #[test]
    fn the_streaming_placeholder_still_gets_a_start_line() {
        let transcript = transcript(&[Message::user("hi"), Message::assistant("")], 40);
        assert_eq!(transcript.starts.len(), 2);
        assert_eq!(transcript.starts[1], transcript.lines.len() - 1);
    }

    // -- the transcript window ----------------------------------------------

    #[test]
    fn without_an_anchor_the_window_is_the_requested_offset_clamped() {
        assert_eq!(transcript_window(100, 10, 0, None).offset, 0);
        assert_eq!(transcript_window(100, 10, 25, None).offset, 25);
        assert_eq!(transcript_window(100, 10, 999, None).offset, 90);
        assert_eq!(transcript_window(4, 10, 3, None).offset, 0);
    }

    /// Until the reply outgrows the pane there is nothing to protect, so the
    /// viewport goes on following the tail exactly as it always did.
    #[test]
    fn a_reply_that_fits_the_pane_still_follows_the_tail() {
        // 30 lines, a 10-row pane, the reply starting at line 25: five lines of
        // it so far, so the whole thing is on screen with room to spare.
        let window = transcript_window(30, 10, 0, Some(25));
        assert_eq!(window.offset, 0);
        assert_eq!(window.start, 20);
        assert!(!window.frozen);
    }

    /// The frame the reply fills the pane exactly: its first line has just
    /// reached the top row, and the next line is what would push it off.
    #[test]
    fn a_reply_exactly_as_tall_as_the_pane_is_pinned_and_frozen() {
        let window = transcript_window(30, 10, 0, Some(20));
        assert_eq!(window.offset, 0);
        assert_eq!(window.start, 20);
        assert!(window.frozen);
    }

    /// The whole point: the reply goes on growing and its opening line does not
    /// move off the top row.
    #[test]
    fn a_growing_reply_stops_at_the_line_it_started_on() {
        for grown in 0..40 {
            let window = transcript_window(30 + grown, 10, 0, Some(20));
            assert_eq!(window.start, 20, "reply grown by {grown} lines");
            assert_eq!(window.end - window.start, 10);
            assert_eq!(window.offset, grown);
            assert!(window.frozen);
        }
    }

    /// The offset that would hold the anchor depends on the height, so a stored
    /// one goes stale the moment the picker opens or the input box grows. The
    /// anchor is re-resolved instead, which is why `requested` is ignored.
    #[test]
    fn the_anchor_survives_the_viewport_changing_height() {
        assert_eq!(transcript_window(60, 10, 0, Some(20)).start, 20);
        let shrunk = transcript_window(60, 5, 30, Some(20));
        assert_eq!(shrunk.start, 20);
        assert_eq!(transcript_window(60, 10, shrunk.offset, Some(20)).start, 20);
    }

    /// A transcript that is all reply: nothing to hide above it, everything to
    /// hide below.
    #[test]
    fn an_anchor_on_the_first_line_hides_only_what_is_below() {
        let window = transcript_window(100, 10, 0, Some(0));
        assert_eq!(window.start, 0);
        assert_eq!(window.offset, 90);
        assert!(window.frozen);
    }

    #[test]
    fn an_anchor_past_the_end_cannot_scroll_past_the_bottom() {
        let window = transcript_window(30, 10, 0, Some(999));
        assert_eq!(window.offset, 0);
        assert_eq!(window.end, 30);
        assert!(!window.frozen);
    }

    /// A pane too short to hold the transcript's first line still has to draw
    /// something rather than underflow.
    #[test]
    fn a_viewport_taller_than_the_transcript_shows_all_of_it() {
        let window = transcript_window(4, 10, 0, Some(2));
        assert_eq!((window.start, window.end, window.offset), (0, 4, 0));
    }

    /// The two halves together, over a reply arriving a word at a time: the
    /// anchored message index resolves through `starts` to a line that stays on
    /// the top row however much more text lands under it.
    #[test]
    fn a_streamed_reply_keeps_its_first_line_on_the_top_row() {
        const HEIGHT: usize = 6;
        let mut messages = vec![Message::user("count"), Message::assistant("")];
        let mut tops = Vec::new();

        for word in 1..20 {
            messages[1].content.push_str(&format!("line {word}\n\n"));
            let transcript = transcript(&messages, 40);
            let window = transcript_window(
                transcript.lines.len(),
                HEIGHT,
                0,
                Some(transcript.starts[1]),
            );
            tops.push(transcript.lines[window.start].text.clone());
        }

        // Followed the tail while the reply was short — the question was still
        // on screen above it — then stopped dead on the reply's first line
        // rather than scrolling it away.
        assert!(tops[0].trim_end().ends_with("count"), "{:?}", tops[0]);
        assert_eq!(tops.last().unwrap(), "line 1");
        assert!(tops.iter().rev().take(12).all(|top| top == "line 1"));
    }

    // -- rendering the window -----------------------------------------------

    /// `render_transcript` only ever prints, so a test can drive the real thing
    /// and read the viewport it settled on back off the state.
    const RENDER_HEIGHT: usize = 6;
    const RENDER_COLS: usize = 40;

    /// A question, then a reply four times taller than the pane.
    fn state_mid_reply() -> State {
        let mut state = State {
            messages: vec![
                Message::user("count"),
                Message::assistant(
                    (1..=12)
                        .map(|n| format!("line {n}\n\n"))
                        .collect::<String>(),
                ),
            ],
            ..Default::default()
        };
        state.view.anchor = Some(1);
        state
    }

    /// Where the reply starts, which is where the anchor should hold the pane.
    fn reply_start(state: &State) -> usize {
        transcript(&state.messages, RENDER_COLS).starts[1]
    }

    fn render(state: &mut State) {
        render_transcript(state, 0, RENDER_HEIGHT, RENDER_COLS);
    }

    /// The offset the render settled on is the one the scroll keys then work
    /// from. Nothing else knows it: the anchor and the clamp are applied here.
    #[test]
    fn rendering_records_the_viewport_it_settled_on() {
        let mut state = state_mid_reply();
        let total = transcript(&state.messages, RENDER_COLS).lines.len();

        render(&mut state);

        assert_eq!(
            total - state.scroll_offset() - RENDER_HEIGHT,
            reply_start(&state),
            "the reply's first line is on the top row"
        );
    }

    /// The reply goes on arriving and the pane does not move.
    #[test]
    fn a_reply_growing_under_a_frozen_pane_does_not_move_it() {
        let mut state = state_mid_reply();
        render(&mut state);
        let before = state.scroll_offset();

        state.messages[1].content.push_str("and another\n\n");
        render(&mut state);

        let total = transcript(&state.messages, RENDER_COLS).lines.len();
        assert!(state.scroll_offset() > before, "more is hidden below");
        assert_eq!(
            total - state.scroll_offset() - RENDER_HEIGHT,
            reply_start(&state),
            "but the top row is the same line it was"
        );
    }

    /// What the write-back is for. Without it the client reads as pinned to the
    /// bottom, and this PageDown would saturate at 0 and drop them at the end of
    /// the reply — the trip back up the anchor exists to save.
    #[test]
    fn the_first_page_down_after_a_freeze_moves_one_page() {
        let mut state = state_mid_reply();
        render(&mut state);
        let frozen = state.scroll_offset();
        assert!(frozen > 0, "the pane is frozen to begin with");

        state.scroll_down(RENDER_HEIGHT);

        assert_eq!(state.scroll_offset(), frozen - RENDER_HEIGHT);
    }

    /// Scrolling is how you say you don't want the anchor. It does not come back
    /// and take the pane again on the next frame.
    #[test]
    fn a_client_that_scrolled_is_not_dragged_back_by_the_anchor() {
        let mut state = state_mid_reply();
        state.scroll_up(3);

        render(&mut state);

        assert_eq!(state.scroll_offset(), 3);
    }

    /// The `run? …` prompt renders below the transcript. Freezing an older line
    /// on the top row would leave it off the bottom of the pane, and the tool
    /// waits on a keypress nothing on screen has asked for.
    #[test]
    fn a_pending_confirmation_is_never_frozen_off_screen() {
        let mut state = state_mid_reply();
        state.status = Status::ConfirmingTool;
        state.pending = vec![crate::ToolCall {
            id: "a".into(),
            name: "close_pane".into(),
            arguments: "{}".into(),
        }]
        .into();
        assert!(state.confirmation_prompt().is_some(), "the prompt is up");

        render(&mut state);

        assert_eq!(state.scroll_offset(), 0, "pinned to the bottom instead");
    }

    /// Every other client goes on being held by it.
    #[test]
    fn one_client_scrolling_does_not_release_another() {
        let mut state = state_mid_reply();
        state.client = 1;
        state.scroll_up(3);
        render(&mut state);
        assert_eq!(state.scroll_offset(), 3);

        state.client = 2;
        render(&mut state);

        let total = transcript(&state.messages, RENDER_COLS).lines.len();
        assert_eq!(
            total - state.scroll_offset() - RENDER_HEIGHT,
            reply_start(&state)
        );
    }

    /// The pane the anchor was measured against is gone, and the offset that
    /// held it is wrong for the new one. Re-resolving the message index is what
    /// makes that not matter.
    #[test]
    fn the_anchor_survives_the_pane_being_resized_under_it() {
        let mut state = state_mid_reply();
        render(&mut state);

        render_transcript(&mut state, 0, RENDER_HEIGHT * 2, RENDER_COLS);

        let total = transcript(&state.messages, RENDER_COLS).lines.len();
        assert_eq!(
            total - state.scroll_offset() - RENDER_HEIGHT * 2,
            reply_start(&state)
        );
    }

    /// A reply still short enough to fit is nothing to protect, and the pane
    /// goes on following the tail exactly as it always did.
    #[test]
    fn an_anchored_reply_that_fits_still_follows_the_tail() {
        let mut state = State {
            messages: vec![Message::user("hi"), Message::assistant("yo")],
            ..Default::default()
        };
        state.view.anchor = Some(1);

        render(&mut state);

        assert_eq!(state.scroll_offset(), 0);
    }

    // -- the waiting indicator ----------------------------------------------

    #[test]
    fn the_spinner_walks_every_frame_and_wraps() {
        let frames: Vec<char> = (0..SPINNER.len() as u64).map(spinner).collect();
        assert_eq!(frames, SPINNER);
        assert_eq!(spinner(SPINNER.len() as u64), SPINNER[0]);
    }

    /// Every frame must be one column wide or the text beside it jitters.
    #[test]
    fn every_spinner_frame_is_one_column_wide() {
        for frame in SPINNER {
            assert_eq!(frame.width(), Some(1), "{frame:?} is not one column");
        }
    }

    #[test]
    fn a_phrase_holds_until_its_ticks_run_out() {
        let first = phrase(0, 0);
        assert_eq!(phrase(TICKS_PER_PHRASE - 1, 0), first, "still the same");
        assert_ne!(phrase(TICKS_PER_PHRASE, 0), first, "now the next one");
    }

    #[test]
    fn phrases_cycle_through_the_whole_pool_and_wrap() {
        let seen: Vec<&str> = (0..PHRASES.len() as u64)
            .map(|step| phrase(step * TICKS_PER_PHRASE, 0))
            .collect();
        assert_eq!(seen, PHRASES, "in order, no repeats");
        assert_eq!(
            phrase(PHRASES.len() as u64 * TICKS_PER_PHRASE, 0),
            PHRASES[0]
        );
    }

    #[test]
    fn consecutive_requests_open_on_different_phrases() {
        assert_ne!(phrase(0, 1), phrase(0, 2));
    }

    /// Control characters are stripped during wrapping, so a phrase containing
    /// one would silently lose it.
    #[test]
    fn no_phrase_contains_a_control_character() {
        for text in PHRASES {
            assert!(!text.chars().any(char::is_control), "{text:?}");
        }
    }

    /// The loader is appended after wrapping, so it is truncated rather than
    /// wrapped if it overruns the pane. This is the width budget that keeps it
    /// whole in a 40-column pane: the spinner and its space.
    #[test]
    fn every_phrase_fits_beside_the_spinner() {
        let budget = 40 - 2;
        for text in PHRASES {
            assert!(text.width() <= budget, "{text:?} is {} wide", text.width());
        }
    }

    // -- loader placement ---------------------------------------------------

    #[test]
    fn the_empty_placeholder_is_recognised_as_the_loaders_home() {
        let lines = transcript_lines(&[Message::user("hi"), Message::assistant("")], 40);
        assert!(ends_with_empty_reply(&lines));
    }

    #[test]
    fn a_reply_with_text_is_not_the_loaders_home() {
        let lines = transcript_lines(&[Message::user("hi"), Message::assistant("word")], 40);
        assert!(!ends_with_empty_reply(&lines));
    }

    /// What the transcript looks like while a tool runs: the last message is the
    /// assistant's tool-call summary, so the loader needs its own line.
    #[test]
    fn a_tool_call_summary_is_not_the_loaders_home() {
        let lines = transcript_lines(&[Message::assistant("list_session {}")], 40);
        assert!(!ends_with_empty_reply(&lines));
    }
}

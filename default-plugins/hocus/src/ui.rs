//! Text wrapping and screen layout.
//!
//! Zellij wipes the plugin's grid and repaints it fully on every render, and
//! gives plugins no scrollback of their own, so everything on screen is
//! recomputed here from `State` each time and all scroll state is ours.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zellij_tile::prelude::*;

use crate::api::ConfigError;
use crate::{Picker, PickerKind, Role, State, Status};

/// Below this we can't lay out a title, a transcript row and an input row,
/// so we say so instead of rendering nonsense.
const MIN_ROWS: usize = 6;
const MIN_COLS: usize = 24;
/// Rows the input box may grow to before it starts scrolling internally.
const INPUT_MAX_ROWS: usize = 5;
/// Rows consumed by the chrome: title, blank separator.
const CHROME_ROWS: usize = 2;

const PROMPT: &str = "> ";

// ---------------------------------------------------------------------------
// Wrapping
// ---------------------------------------------------------------------------

/// Wrap `text` to `width` display columns.
///
/// Newlines are preserved (each one starts a new visual line, and blank lines
/// survive), widths are measured in terminal columns rather than bytes, and a
/// word too long to ever fit is hard-split at a column boundary rather than
/// truncated.
///
/// Control characters are dropped: model output is rendered as plain text, so an
/// escape sequence in a reply must not be able to repaint the pane. Tabs become
/// four spaces, since a tab measures zero columns here but eight on screen.
///
/// Never returns an empty `Vec` - empty input is one empty line.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let sanitized = sanitize(text);

    if width == 0 {
        // Nothing can fit; still honour the "one visual line per source line"
        // contract so callers can't divide by zero or loop forever.
        return sanitized.split('\n').map(|_| String::new()).collect();
    }

    let mut lines = Vec::new();
    for source_line in sanitized.split('\n') {
        wrap_line(source_line, width, &mut lines);
    }
    lines
}

fn sanitize(text: &str) -> String {
    // Pushed into one buffer rather than flat_mapped: a Vec per character is a
    // heap allocation per character, and the host interprets this wasm rather
    // than compiling it, so that constant lands on every render.
    let mut sanitized = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\t' => sanitized.push_str("    "),
            '\n' => sanitized.push('\n'),
            ch if ch.is_control() => {},
            ch => sanitized.push(ch),
        }
    }
    sanitized
}

/// Wrap `text` to `width` columns for the input box: break mid-word, exactly at
/// the edge, the way a terminal input does.
///
/// Word wrap is wrong here. It holds the word being typed whole, so the break
/// lands nowhere near the keystroke that caused it, and it discards the spaces
/// that caused a break, so trailing spaces are invisible and the cursor stalls
/// on them.
///
/// A line that is exactly full is followed by an empty one so the cursor has a
/// row to land on: that empty line is what makes the break happen *at* the edge
/// rather than one keystroke later.
fn wrap_hard(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return text.split('\n').map(|_| String::new()).collect();
    }

    let mut lines = Vec::new();
    for source_line in text.split('\n') {
        lines.extend(hard_split(source_line, width));
    }
    if lines.last().is_some_and(|line| line.width() == width) {
        lines.push(String::new());
    }
    lines
}

fn wrap_line(line: &str, width: usize, out: &mut Vec<String>) {
    let mut current = String::new();
    let mut current_width = 0usize;
    let mut pending_spaces = String::new();

    for token in tokenize(line) {
        let word = match token {
            // Spaces are held back so a break can discard them: a wrapped line
            // should not start with the space that caused the wrap. Held rather
            // than collapsed so leading indentation survives, which matters for
            // code in model output.
            Token::Spaces(spaces) => {
                pending_spaces.push_str(spaces);
                continue;
            },
            Token::Word(word) => word,
        };

        let spaces_width = pending_spaces.width();
        let word_width = word.width();

        if current_width + spaces_width + word_width <= width {
            current.push_str(&pending_spaces);
            current.push_str(word);
            current_width += spaces_width + word_width;
        } else if word_width <= width {
            out.push(std::mem::take(&mut current));
            current.push_str(word);
            current_width = word_width;
        } else {
            // Wider than the whole line however we break it: flush, then split
            // at column boundaries. No hyphenation - this is a terminal.
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            let mut pieces = hard_split(word, width);
            let last = pieces.pop().unwrap_or_default();
            out.extend(pieces);
            current_width = last.width();
            current = last;
        }
        pending_spaces.clear();
    }

    // Always push, even when empty: a blank source line is a blank visual line.
    out.push(current);
}

fn hard_split(word: &str, width: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut piece = String::new();
    let mut piece_width = 0usize;
    for ch in word.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if piece_width + ch_width > width && !piece.is_empty() {
            pieces.push(std::mem::take(&mut piece));
            piece_width = 0;
        }
        piece.push(ch);
        piece_width += ch_width;
    }
    pieces.push(piece);
    pieces
}

enum Token<'a> {
    Spaces(&'a str),
    Word(&'a str),
}

/// Split into alternating runs of spaces and non-spaces.
fn tokenize(line: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        let is_space = rest.starts_with(' ');
        let end = rest
            .find(|ch: char| (ch == ' ') != is_space)
            .unwrap_or(rest.len());
        let (head, tail) = rest.split_at(end);
        tokens.push(if is_space {
            Token::Spaces(head)
        } else {
            Token::Word(head)
        });
        rest = tail;
    }
    tokens
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

fn render_markdown_block(block: &MdBlock, cols: usize) -> Vec<VLine> {
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
            VLine {
                text,
                role: Role::Assistant,
                styles,
            }
        })
        .collect()
}

fn markdown_lines(text: &str, cols: usize) -> Vec<VLine> {
    let sanitized = sanitize(text);
    let mut renderer = MarkdownRenderer::default();
    for event in Parser::new_ext(&sanitized, Options::empty()) {
        renderer.event(event, cols);
    }
    renderer.finish_block();

    if renderer.blocks.is_empty() {
        return vec![VLine {
            text: String::new(),
            role: Role::Assistant,
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
        lines.extend(render_markdown_block(block, cols));
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

/// Flatten the transcript into wrapped display lines, with a blank line between
/// messages and a hanging indent on continuations.
///
/// User turns are wrapped narrow and pushed against the right edge; replies run
/// the full width from the left. That contrast is what tells them apart.
pub fn transcript_lines(messages: &[crate::Message], cols: usize) -> Vec<VLine> {
    let mut lines = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        if index > 0 {
            lines.push(VLine::blank());
        }
        if message.role == Role::Assistant && message.tool_calls.is_empty() {
            lines.extend(markdown_lines(&message.content, cols));
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
    lines
}

/// What `transcript_lines`' output depends on: the width it was wrapped for,
/// the number of messages, and the length of the one message that can grow
/// without the count moving.
///
/// That last field is what makes the count sufficient. Only the streaming
/// placeholder is ever mutated in place (`State::append_to_reply`), and
/// `State::mutable_len` is which message that is; every other change to the
/// transcript is a push, a pop or a clear, all of which move the count.
pub type TranscriptKey = (usize, usize, usize);

/// `transcript_lines`, memoised on the state. Wrapping the whole transcript is
/// the single most expensive thing a render does, and typing does not change
/// any of it.
fn cached_transcript_lines(state: &mut State, cols: usize) -> &[VLine] {
    let key: TranscriptKey = (cols, state.messages.len(), state.mutable_len());
    if state.transcript_cache.as_ref().map(|(k, _)| *k) != Some(key) {
        state.transcript_cache = Some((key, transcript_lines(&state.messages, cols)));
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
        Role::Notice | Role::Tool => text.dim_range(..),
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

    // The archive replaces the transcript and the input box outright: it is a
    // list you move around in, and nothing below it would accept a keystroke.
    if let Some(picker) = &state.picker {
        show_cursor(None);
        render_title(state, cols);
        render_picker(picker, 1, rows - 2, cols);
        print_text_with_coordinates(
            Text::new(help_line(state)).dim_range(..),
            0,
            rows - 1,
            Some(cols),
            None,
        );
        return;
    }

    let input_text = format!("{PROMPT}{}", state.input.get_text());
    let all_input_lines = wrap_hard(&input_text, cols);
    // Cap against the rows actually available, not just INPUT_MAX_ROWS: in a
    // minimum-height pane a tall input would otherwise be laid out past the
    // last row.
    let room_for_input = rows.saturating_sub(CHROME_ROWS).max(1);
    let input_rows = all_input_lines
        .len()
        .clamp(1, INPUT_MAX_ROWS.min(room_for_input));
    // Show the tail so the box follows the cursor on a long multi-line message.
    let input_lines = &all_input_lines[all_input_lines.len() - input_rows..];
    let skipped_input_rows = all_input_lines.len() - input_rows;

    let transcript_rows = rows.saturating_sub(CHROME_ROWS + input_rows);
    state.transcript_rows = transcript_rows;

    let transcript_y = 1;
    let input_y = transcript_y + transcript_rows + 1;

    // Must precede any print_*: the host reads the cursor position out of the
    // render output stream in order.
    place_cursor(state, cols, input_y, skipped_input_rows);

    render_title(state, cols);

    if let Err(error) = &state.cfg {
        render_config_banner(error, transcript_y, transcript_rows, cols);
    } else {
        render_transcript(state, transcript_y, transcript_rows, cols);
    }

    for (index, line) in input_lines.iter().enumerate() {
        print_text_with_coordinates(Text::new(line), 0, input_y + index, Some(cols), None);
    }
}

fn render_title(state: &State, cols: usize) {
    let model = match &state.cfg {
        Ok(cfg) => cfg.model.clone(),
        Err(error) => error.model.clone(),
    };
    let title = format!("hocus — {model}");
    print_text_with_coordinates(
        Text::new(&title).color_range(0, ..title.chars().count().min(5)),
        0,
        0,
        Some(cols),
        None,
    );

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

    let lines = cached_transcript_lines(state, cols);

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
    if let Some(prompt) = prompt {
        tail.push(VLine::blank());
        tail.push(VLine {
            text: format!("run? {prompt}"),
            role: Role::Error,
            styles: Vec::new(),
        });
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

    // scroll_offset counts lines hidden *below* the viewport, so 0 means pinned
    // to the bottom and a growing transcript follows itself with no extra state.
    // Clamping here also absorbs resizes and message growth for free.
    let max_offset = total.saturating_sub(height);
    let offset = scroll_offset.min(max_offset);
    let end = total - offset;
    let start = end.saturating_sub(height);

    for (index, position) in (start..end).enumerate() {
        let line = match body.get(position) {
            Some(line) => line,
            None => &tail[position - body.len()],
        };
        print_text_with_coordinates(styled(line), 0, y + index, Some(cols), None);
    }

    if start > 0 {
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
fn render_picker(picker: &Picker, y: usize, height: usize, cols: usize) {
    if picker.entries.is_empty() {
        // An empty model list is the fetch still being in flight — `finish_models`
        // either fills it or closes the picker.
        let empty = match picker.kind {
            PickerKind::Chats(_) => "No saved chats yet.",
            PickerKind::Models => "fetching models…",
            PickerKind::Agents(_) => "no agents",
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
        let selected = start + index == picker.selected;
        let text = Text::new(format!("{} {}", if selected { ">" } else { " " }, entry));
        print_text_with_coordinates(
            if selected {
                text.color_range(2, ..)
            } else {
                text.dim_range(..)
            },
            0,
            y + index,
            Some(cols),
            None,
        );
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
        Some(PickerKind::Agents(_)) => return "<↑↓> choose  <Enter> focus  <Esc> back",
        None => {},
    }
    // Tools before the stream, matching what Esc actually does when both are
    // outstanding.
    match state.status {
        Status::ConfirmingTool => "run this?  <y> yes  <n> no  <Esc> abort",
        Status::AwaitingCommand => "running…  <Esc> abort  <↑↓> scroll",
        Status::AwaitingSearch => "searching…  <Esc> abort  <↑↓> scroll",
        Status::AwaitingStore => "saving…  <Esc> abort  <↑↓> scroll",
        Status::Idle if state.is_streaming() => "streaming…  <Esc> cancel  <↑↓> scroll",
        Status::Idle if !state.jobs.is_empty() => "<Ctrl l> new chat  <Esc> hide",
        Status::Idle => "<Ctrl l> new chat  <Ctrl r> agents  <Esc> hide",
    }
}

/// Place the terminal cursor at the text cursor's position inside the input box.
///
/// The cursor's own coordinates are derived by wrapping the text *before* it
/// with the same function that laid out the box, so the two can't disagree.
fn place_cursor(state: &State, cols: usize, input_y: usize, skipped_rows: usize) {
    // get_cursor_position() is a char index, not a byte offset.
    let typed: String = state
        .input
        .get_text()
        .chars()
        .take(state.input.get_cursor_position())
        .collect();
    let lines = wrap_hard(&format!("{PROMPT}{typed}"), cols);
    let row = lines.len().saturating_sub(1);
    let column = lines.last().map(|line| line.width()).unwrap_or(0);

    // The box shows only its last INPUT_MAX_ROWS lines; if the cursor scrolled
    // off the top of that window there is nowhere on screen to put it.
    if row < skipped_rows {
        show_cursor(None);
        return;
    }
    show_cursor(Some((column, input_y + row - skipped_rows)));
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    // -- wrap ---------------------------------------------------------------

    #[test]
    fn wrap_returns_one_empty_line_for_empty_input() {
        assert_eq!(wrap("", 10), vec![""]);
    }

    #[test]
    fn wrap_leaves_a_short_line_alone() {
        assert_eq!(wrap("hello there", 40), vec!["hello there"]);
    }

    #[test]
    fn wrap_breaks_on_word_boundaries() {
        assert_eq!(wrap("aaa bbb ccc", 7), vec!["aaa bbb", "ccc"]);
    }

    #[test]
    fn wrap_preserves_newlines_and_blank_lines() {
        assert_eq!(
            wrap("one\n\ntwo", 20),
            vec!["one".to_string(), String::new(), "two".to_string()]
        );
    }

    #[test]
    fn wrap_preserves_leading_indentation() {
        // Code in model output relies on this surviving.
        assert_eq!(wrap("    indented", 40), vec!["    indented"]);
    }

    #[test]
    fn wrap_drops_the_spaces_that_caused_a_break() {
        assert_eq!(wrap("aaaa bbbb", 4), vec!["aaaa", "bbbb"]);
    }

    #[test]
    fn wrap_measures_in_display_columns_not_bytes() {
        // Each CJK character is two columns wide, so only two fit in four.
        assert_eq!(wrap("日本語", 4), vec!["日本", "語"]);
    }

    #[test]
    fn wrap_hard_splits_a_word_longer_than_the_line() {
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_hard_splits_after_flushing_the_current_line() {
        assert_eq!(wrap("hi abcdefghij", 4), vec!["hi", "abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_terminates_on_zero_width() {
        assert_eq!(wrap("anything at all", 0), vec![""]);
        assert_eq!(wrap("two\nlines", 0), vec!["", ""]);
    }

    #[test]
    fn wrap_expands_tabs_to_spaces() {
        // A tab measures zero columns but prints as several, which would
        // silently desync every width calculation downstream.
        assert_eq!(wrap("a\tb", 20), vec!["a    b"]);
    }

    #[test]
    fn wrap_strips_control_characters() {
        // Model output is rendered as plain text; an escape sequence in a reply
        // must not be able to repaint the pane.
        assert_eq!(wrap("safe\u{1b}[31mtext", 40), vec!["safe[31mtext"]);
    }

    // -- wrap_hard (the input box) ------------------------------------------

    #[test]
    fn input_wrap_breaks_exactly_at_the_edge() {
        assert_eq!(wrap_hard("abcdef", 4), vec!["abcd", "ef"]);
    }

    /// The cursor needs a row to sit on once a row is full, and that empty row
    /// is what makes the break land on the keystroke that filled the line.
    #[test]
    fn input_wrap_adds_a_row_when_the_last_row_is_full() {
        assert_eq!(wrap_hard("abcd", 4), vec!["abcd", ""]);
    }

    #[test]
    fn input_wrap_keeps_trailing_spaces() {
        // wrap() drops these; in an input the cursor has to move as you type them.
        assert_eq!(wrap_hard("ab ", 4), vec!["ab "]);
    }

    #[test]
    fn input_wrap_preserves_newlines_and_blank_lines() {
        assert_eq!(
            wrap_hard("one\n\ntwo", 20),
            vec!["one".to_string(), String::new(), "two".to_string()]
        );
    }

    #[test]
    fn input_wrap_measures_in_display_columns_not_bytes() {
        assert_eq!(wrap_hard("日本語", 4), vec!["日本", "語"]);
    }

    #[test]
    fn input_wrap_terminates_on_zero_width() {
        assert_eq!(wrap_hard("two\nlines", 0), vec!["", ""]);
    }

    /// The bug this replaced word wrap for: a full row put the cursor at column
    /// `cols`, one past the last column, and zellij hides a plugin cursor that
    /// sits outside the pane.
    #[test]
    fn the_cursor_stays_inside_the_pane_when_a_row_is_full() {
        let cols = 24;
        let full = format!("{PROMPT}{}", "a".repeat(cols - PROMPT.width()));
        let lines = wrap_hard(&full, cols);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines.last().unwrap().width(), 0, "cursor lands at column 0");
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
        assert_eq!(cached_transcript_lines(&mut state, 40)[0].text, "Hel");

        state.messages.last_mut().unwrap().content.push_str("lo");
        assert_eq!(cached_transcript_lines(&mut state, 40)[0].text, "Hello");
    }

    #[test]
    fn the_transcript_cache_rewraps_when_the_pane_is_resized() {
        let mut state = State::default();
        state.messages.push(Message::user("aaaa bbbb"));
        assert_eq!(cached_transcript_lines(&mut state, 40).len(), 1);
        assert_eq!(cached_transcript_lines(&mut state, 9).len(), 2);
    }

    #[test]
    fn transcript_keeps_the_role_on_continuation_lines() {
        let lines = transcript_lines(&[Message::error("aaaa bbbb")], 9);
        assert!(lines.iter().all(|l| l.role == Role::Error));
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
        let lines = transcript_lines(&[Message::assistant("run_command {}")], 40);
        assert!(!ends_with_empty_reply(&lines));
    }
}

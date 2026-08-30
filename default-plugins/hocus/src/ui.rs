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
use crate::{
    history, Confirmation, Picker, PickerKind, Plan, PromptKind, Role, RowKind, State, Status,
};

/// What the whole layout costs: the input row, one transcript row, the title
/// bar, the help line and the two rules around the input box. At this many rows
/// and above the conversation is drawn entire; below it `chrome_for` gives the
/// rows away in order of what matters least.
///
/// A comfort threshold rather than a refusal, and the size `overlay` places a
/// pane at (`MIN_PANE_ROWS` is this plus the frame around it).
pub(crate) const MIN_ROWS: usize = 6;
/// The columns the conversation wants: enough for a wrapped sentence and the
/// frames around the boxes. Like `MIN_ROWS`, what `overlay` sizes a pane to
/// rather than a floor anything is refused below.
pub(crate) const MIN_COLS: usize = 24;
/// The one hard floor: a prompt and a character after it. Below this there is
/// nowhere to type, and nothing a conversation could be.
const MIN_INPUT_COLS: usize = 4;
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
    /// Painted in the theme's error colour. A run rather than a whole line,
    /// because a framed line carries its border and its body together: the
    /// border must stay dim while the one sentence saying what a mutating call
    /// will do is the part that goes red.
    error: bool,
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
                    ..MdStyle::default()
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

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// Columns a frame spends on every body row: `│ ` on the left, ` │` on the right.
const FRAME_PAD: usize = 4;
/// Narrower than this and the box costs more room than it earns, so the content
/// is drawn bare instead. `print_text_with_coordinates` truncates rather than
/// wraps, so a border that doesn't fit would eat the text rather than the border.
const MIN_FRAME_COLS: usize = MIN_COLS + 8;

/// One styled stretch of a line, for the lines that are assembled rather than
/// wrapped — a frame's borders, and the key hints written along them.
struct Run {
    text: String,
    style: MdStyle,
}

impl Run {
    fn new(text: impl Into<String>, style: MdStyle) -> Self {
        Run {
            text: text.into(),
            style,
        }
    }
}

fn border_style() -> MdStyle {
    MdStyle {
        dim: true,
        ..MdStyle::default()
    }
}

/// A key hint: the key itself in the theme's accent colour, its label dim.
///
/// Separated by a dim middle dot rather than by spaces, so a row of hints reads
/// as one strip instead of as four unrelated words.
fn key_runs(keys: &[(&str, &str)]) -> Vec<Run> {
    let mut runs = Vec::new();
    for (index, (key, label)) in keys.iter().enumerate() {
        if index > 0 {
            runs.push(Run::new(" · ", border_style()));
        }
        runs.push(Run::new(
            *key,
            MdStyle {
                color: Some(2),
                ..MdStyle::default()
            },
        ));
        runs.push(Run::new(format!(" {label}"), border_style()));
    }
    runs
}

/// Glue runs into a line, keeping `styles` one entry per character — which is
/// what `styled` indexes by.
fn line_from_runs(runs: &[Run], role: Role) -> VLine {
    let mut text = String::new();
    let mut styles = Vec::new();
    for run in runs {
        text.push_str(&run.text);
        styles.extend(std::iter::repeat_n(run.style, run.text.chars().count()));
    }
    VLine { text, role, styles }
}

/// Wrap styled runs without losing the key colours between rows.
fn lines_from_runs(runs: &[Run], width: usize, role: Role) -> Vec<VLine> {
    let chars: Vec<StyledChar> = runs
        .iter()
        .flat_map(|run| {
            run.text
                .chars()
                .map(|ch| StyledChar {
                    ch,
                    style: run.style,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    word_wrap_styled(&chars, width.max(1))
        .into_iter()
        .map(|line| VLine {
            text: line.iter().map(|styled| styled.ch).collect(),
            role,
            styles: line.into_iter().map(|styled| styled.style).collect(),
        })
        .collect()
}

fn runs_width(runs: &[Run]) -> usize {
    runs.iter().map(|run| run.text.width()).sum()
}

/// The longest prefix of `text` that fits in `width` columns.
fn truncate_to_width(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > width {
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out
}

/// Draw a box around `inner`, `cols` wide, with `title` on the top border and
/// `footer` written along the bottom one.
///
/// The caller renders `inner` at `cols - FRAME_PAD`; anything wider is left to
/// overhang rather than being clipped, since clipping a plan mid-word is worse
/// than a ragged right edge.
fn frame(title: &str, footer: &[Run], inner: Vec<VLine>, cols: usize) -> Vec<VLine> {
    // Too narrow to box: the content still has to be readable, and the keys
    // still have to be findable, so both are drawn bare.
    if cols < MIN_FRAME_COLS {
        let mut lines = inner;
        if !footer.is_empty() {
            lines.extend(lines_from_runs(footer, cols, Role::Assistant));
        }
        return lines;
    }

    let inner_width = cols - FRAME_PAD;
    let mut lines = Vec::with_capacity(inner.len() + 2);

    // Top: `┌ TITLE ─────┐`, the title bold so it reads as a label.
    let title = truncate_to_width(title, inner_width);
    let mut top = vec![Run::new("┌ ", border_style())];
    if !title.is_empty() {
        let used = 2 + title.width() + 1;
        top.push(Run::new(
            title,
            MdStyle {
                bold: true,
                ..MdStyle::default()
            },
        ));
        top.push(Run::new(
            format!(" {}┐", "─".repeat(cols.saturating_sub(used + 1))),
            border_style(),
        ));
    } else {
        top.push(Run::new(
            format!("{}┐", "─".repeat(cols.saturating_sub(3))),
            border_style(),
        ));
    }
    lines.push(line_from_runs(&top, Role::Assistant));

    for line in inner {
        let pad = inner_width.saturating_sub(line.text.width());
        let mut text = String::from("│ ");
        let mut styles = vec![border_style(); 2];
        text.push_str(&line.text);
        // A short `styles` is the plain `wrap` path's empty vector, or a line
        // built by hand; the default carries no colour, so filling with it
        // leaves the character exactly as it would have rendered unframed.
        styles.extend(
            line.styles
                .iter()
                .copied()
                .chain(std::iter::repeat(MdStyle::default()))
                .take(line.text.chars().count()),
        );
        text.push_str(&" ".repeat(pad));
        styles.extend(std::iter::repeat_n(MdStyle::default(), pad));
        text.push_str(" │");
        styles.extend(vec![border_style(); 2]);
        lines.push(VLine {
            text,
            role: Role::Assistant,
            styles,
        });
    }

    // Bottom: `└ y run · n skip ────┘`, or a plain rule with the hints on the
    // line under it when they are too wide to sit on the border.
    let footer_width = runs_width(footer);
    if footer.is_empty() || footer_width + 5 > cols {
        lines.push(line_from_runs(
            &[Run::new(
                format!("└{}┘", "─".repeat(cols.saturating_sub(2))),
                border_style(),
            )],
            Role::Assistant,
        ));
        if !footer.is_empty() {
            lines.extend(lines_from_runs(footer, cols, Role::Assistant));
        }
        return lines;
    }
    let mut bottom = vec![Run::new("└ ", border_style())];
    bottom.extend(footer.iter().map(|run| Run::new(&*run.text, run.style)));
    bottom.push(Run::new(
        format!(" {}┘", "─".repeat(cols - footer_width - 4)),
        border_style(),
    ));
    lines.push(line_from_runs(&bottom, Role::Assistant));
    lines
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
    let mut transcript = Transcript {
        lines: Vec::new(),
        starts: Vec::with_capacity(messages.len()),
    };
    append_messages(&mut transcript, messages, cols);
    transcript
}

/// Draw `messages` onto the end of `transcript`, which is assumed to already
/// hold everything before them. The separator logic keys off the flattened
/// output rather than the message index, so this is correct whether it is
/// appending to nothing or to a cached prefix.
fn append_messages(transcript: &mut Transcript, messages: &[crate::Message], cols: usize) {
    for message in messages {
        if !transcript.starts.is_empty() {
            transcript.lines.push(VLine::blank());
        }
        transcript.starts.push(transcript.lines.len());
        transcript.lines.extend(message_lines(message, cols));
    }
}

// How many messages have been drawn since `rebuilds()` was last read. The
// property the transcript cache exists for is "a streaming frame redraws one
// message", and counting is the only way to assert it directly — the output is
// identical either way, which is the whole point.
#[cfg(test)]
thread_local! {
    static REBUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Rebuilds since this was last called, resetting the count.
#[cfg(test)]
fn rebuilds() -> usize {
    REBUILDS.with(|count| count.replace(0))
}

/// One message's display lines, with no separator and no knowledge of what
/// surrounds it. The single renderer behind both `transcript` and the
/// incremental cache, so the two can never drift.
fn message_lines(message: &crate::Message, cols: usize) -> Vec<VLine> {
    #[cfg(test)]
    REBUILDS.with(|count| count.set(count.get() + 1));
    if message.role == Role::Assistant && message.tool_calls.is_empty() {
        return markdown_lines(&message.content, cols);
    }
    // A tool the user typed, drawn as the answer they asked for rather than
    // as context for the model. `content` is untouched and is still what
    // gets sent; this is only what the pane shows.
    //
    // No `fn:` prefix: the presenter opens with a heading, which says the
    // same thing, and a five-column prefix would fight the markdown
    // renderer's own indents and narrow every code fence.
    if let Some(display) = message
        .display
        .as_deref()
        .filter(|_| message.role != Role::User)
    {
        return markdown_lines_as(display, cols, message.role);
    }
    let prefix = prefix_for(message.role);
    let indent = " ".repeat(prefix.chars().count());
    let body_width = match message.role {
        // Two thirds: a full-width block against the right edge reads as
        // justified text rather than as a turn of its own.
        Role::User => (cols * 2 / 3).max(1),
        _ => cols.saturating_sub(prefix.chars().count()).max(1),
    };

    let mut wrapped = wrap(
        message.display.as_deref().unwrap_or(&message.content),
        body_width,
    );
    // Truncation is display-only; `content` still goes to the model whole.
    if message.role == Role::Tool && !message.full_output && wrapped.len() > TOOL_RESULT_ROWS {
        let hidden = wrapped.len() - TOOL_RESULT_ROWS;
        wrapped.truncate(TOOL_RESULT_ROWS);
        wrapped.push(format!("… {hidden} more lines"));
    }

    wrapped
        .into_iter()
        .enumerate()
        .map(|(n, line)| {
            let text = match message.role {
                Role::User => right_align(&line, cols),
                _ if n == 0 => format!("{prefix}{line}"),
                _ => format!("{indent}{line}"),
            };
            VLine {
                text,
                role: message.role,
                styles: Vec::new(),
            }
        })
        .collect()
}

/// What one message's display lines depend on: everything `message_lines`
/// reads out of it.
///
/// Lengths rather than the strings themselves, which is the same bargain the
/// whole-transcript key used to make: only the streaming placeholder is ever
/// mutated in place (`State::append_to_reply`), and that only ever appends. A
/// caller that rewrites a message's text without changing its length owes the
/// cache an `invalidate_from`; `on_chunk`'s prose-swap onto a tool-calls
/// message is the one such site, and it does exactly that.
type MessageKey = (Role, usize, Option<usize>, usize, bool);

fn message_key(message: &crate::Message) -> MessageKey {
    (
        message.role,
        message.content.len(),
        message.display.as_ref().map(String::len),
        message.tool_calls.len(),
        message.full_output,
    )
}

/// The wrapped transcript, kept across renders and rebuilt only from the first
/// message that changed.
///
/// Wrapping the transcript is by far the most expensive thing a render does — a
/// full markdown parse and a per-character style vector for every line — and
/// during a streaming reply it used to happen in its entirety ten times a
/// second, because the cache key carried the growing reply's length. The cost
/// was O(whole conversation) per frame and it queued ahead of the user's
/// keystrokes. Now a frame re-renders only the messages whose `MessageKey`
/// moved, which while streaming is just the last one.
pub struct TranscriptCache {
    cols: usize,
    /// One per message in `transcript.starts`, in the same order.
    keys: Vec<MessageKey>,
    transcript: Transcript,
}

impl TranscriptCache {
    /// How many messages the cache currently covers.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether it covers nothing at all — a fresh cache, or one just cleared by
    /// a resize.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Drop everything from `index` onward, so the next render re-draws it.
    ///
    /// For the callers that change a message's text without changing its
    /// length, which `MessageKey` cannot see.
    pub fn invalidate_from(&mut self, index: usize) {
        if index >= self.keys.len() {
            return;
        }
        self.keys.truncate(index);
        // `starts[index]` is the message's first *content* line; the blank
        // separator that precedes it belongs to this message too, and goes
        // with it. The first message has no separator.
        let line = match self.transcript.starts.get(index) {
            Some(start) if index > 0 => start.saturating_sub(1),
            Some(start) => *start,
            None => return,
        };
        self.transcript.lines.truncate(line);
        self.transcript.starts.truncate(index);
    }
}

/// Pane width, selected plan message, and the only message length that can
/// change without moving the transcript count.
pub type PlanKey = (usize, usize, usize);

/// `transcript`, memoised on the state and rebuilt incrementally. See
/// `TranscriptCache`.
pub fn cached_transcript(state: &mut State, cols: usize) -> &Transcript {
    let messages = &state.messages;
    let cache = state
        .transcript_cache
        .get_or_insert_with(|| TranscriptCache {
            cols,
            keys: Vec::new(),
            transcript: Transcript {
                lines: Vec::new(),
                starts: Vec::new(),
            },
        });
    // A resize re-wraps everything; nothing about the old lines survives it.
    if cache.cols != cols {
        cache.cols = cols;
        cache.invalidate_from(0);
    }
    // Messages that are gone, and then the first surviving one that changed.
    cache.invalidate_from(messages.len());
    let stale = cache
        .keys
        .iter()
        .zip(messages.iter())
        .position(|(cached, message)| *cached != message_key(message));
    if let Some(index) = stale {
        cache.invalidate_from(index);
    }
    let from = cache.keys.len();
    if from < messages.len() {
        append_messages(&mut cache.transcript, &messages[from..], cols);
        cache.keys.extend(messages[from..].iter().map(message_key));
    }
    &cache.transcript
}

fn frame_content_width(cols: usize) -> usize {
    if cols < MIN_FRAME_COLS {
        cols.max(1)
    } else {
        cols.saturating_sub(FRAME_PAD).max(1)
    }
}

fn cached_plan(state: &mut State, cols: usize, plan_at: usize) -> &[VLine] {
    let key: PlanKey = (cols, plan_at, state.mutable_len());
    if state.plan_cache.as_ref().map(|(key, _)| *key) != Some(key) {
        state.plan_cache = Some((
            key,
            markdown_lines(&state.messages[plan_at].content, frame_content_width(cols)),
        ));
    }
    &state.plan_cache.as_ref().expect("just populated").1
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
        if style.error {
            text = text.error_color_range(start..end);
        }
        start = end;
    }
    text
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Which chrome the rows on offer can pay for.
///
/// **The conversation degrades rather than refusing to draw.** A pane hocus can
/// be given is now as small as the terminal is (`overlay::MIN_PANE_ROWS`), and
/// "pane too small" in place of the conversation is worse than a conversation
/// with less around it. What goes first is what matters least: the rules around
/// the input box, then the help line, then the title bar. The input row and one
/// transcript row are what is left, and the input row is never given up - a
/// conversation you cannot type into is not one.
///
/// At `MIN_ROWS` rows and above this is everything, so the layout there is
/// exactly the layout it has always been.
///
/// | rows | drawn |
/// |---|---|
/// | ≥ 6 | everything |
/// | 5 | no rules |
/// | 4 | no rules, no help line |
/// | 3 | and no title bar |
/// | 2 | a transcript row and the input |
/// | 1 | the input |
/// | 0 | nothing |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Chrome {
    title: bool,
    transcript: bool,
    help: bool,
    rules: bool,
}

fn chrome_for(rows: usize) -> Chrome {
    Chrome {
        title: rows >= 4,
        transcript: rows >= 2,
        help: rows >= 5,
        rules: rows >= MIN_ROWS,
    }
}

/// Where the conversation is drawn: the row it starts on, once the title bar has
/// taken its own, and how many rows it has once the help line has taken its.
///
/// One place, because `render` and `chat_layout` have to agree on it - the same
/// rule that keeps `place_cursor` and `render_chat` on `ChatLayout::input_y`.
fn body_of(chrome: Chrome, rows: usize) -> (usize, usize) {
    let y = usize::from(chrome.title);
    (y, rows.saturating_sub(y + usize::from(chrome.help)))
}

pub fn render(state: &mut State, rows: usize, cols: usize) {
    // The only refusal left, and it is about columns: `chrome_for` answers for
    // every height, but a pane too narrow for a prompt and a character has
    // nowhere to put either.
    if rows == 0 || cols < MIN_INPUT_COLS {
        state.chat_tab_width = 0;
        show_cursor(None);
        if rows > 0 {
            print_text_with_coordinates(
                Text::new(truncate_to_width("too small", cols)),
                0,
                0,
                Some(cols),
                None,
            );
        }
        return;
    }

    if !state.perms {
        state.chat_tab_width = 0;
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
    let chrome = chrome_for(rows);
    let (body_y, body_rows) = body_of(chrome, rows);
    let layout = state
        .picker
        .is_none()
        .then(|| chat_layout(state, body_rows, cols, chrome));

    if state.picker.is_some() {
        show_cursor(None);
    } else if let Some(layout) = &layout {
        // Must precede any print_*: the host reads the cursor position out of
        // the render output stream in order.
        place_cursor(state, layout, layout.input_y(body_y));
    }
    match chrome.title {
        true => render_title(state, cols),
        // The bar is not drawn, so it must not go on taking the keys it would
        // have drawn: `chat_tab_width` is what says whether a key is the bar's.
        false => state.chat_tab_width = 0,
    }

    // History and models take the top half and leave the conversation visible.
    if let Some(picker) = &state.picker {
        // Leave the chat enough room for both input rules and one text row -
        // and, in a body too short for that, the one row the input box is on.
        let picker_rows = (body_rows / 2)
            .max(1)
            .min(body_rows.saturating_sub(3).max(1))
            .min(body_rows.saturating_sub(1));
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
        let layout = chat_layout(state, body_rows.saturating_sub(picker_rows), cols, chrome);
        state.transcript_rows = layout.transcript_rows;
        render_chat(state, chat_y, cols, &layout);
    } else {
        let layout = layout.expect("non-picker layout");
        state.transcript_rows = layout.transcript_rows;
        render_chat(state, body_y, cols, &layout);
    }

    if chrome.help {
        print_text_with_coordinates(
            Text::new(help_line(state)).dim_range(..),
            0,
            rows - 1,
            Some(cols),
            None,
        );
    }
}

/// How the conversation lays out in the rows below the title: the transcript on
/// top, and the input box between two rules at the bottom.
struct ChatLayout {
    /// The visible window of the wrapped input, one entry per row of the box.
    /// Each row carries where it starts in the wrapped string, which is how the
    /// cursor finds the cell it belongs in.
    input_lines: Vec<WrappedLine>,
    transcript_rows: usize,
    /// What the pane's height can pay for. Held here rather than passed around
    /// so `input_y` answers with it: the rules are a row above the box, and the
    /// cursor and the box must not disagree about whether it is there.
    chrome: Chrome,
}

impl ChatLayout {
    /// The first row of the input box, given the first row of the whole body.
    fn input_y(&self, y: usize) -> usize {
        y + self.transcript_rows + usize::from(self.chrome.rules)
    }

    /// The rows the box and its rules take together. The tests' way of asking
    /// whether a body was spent exactly; `render_chat` draws from `input_y`.
    #[cfg(test)]
    fn input_rows(&self) -> usize {
        self.input_lines.len() + 2 * usize::from(self.chrome.rules)
    }
}

fn chat_layout(state: &State, height: usize, cols: usize, chrome: Chrome) -> ChatLayout {
    let input_text = format!("{}{}", prompt(state), state.input.get_text());
    let all_input_lines = wrap_input(&input_text, cols);
    // Cap against the rows actually available, not just INPUT_MAX_ROWS: in a
    // minimum-height body a tall input would otherwise be laid out past the
    // last row. The held back rows are the rules around the box, where the pane
    // is tall enough to be drawing them.
    let rules = 2 * usize::from(chrome.rules);
    let room_for_input = height.saturating_sub(rules).max(1);
    let input_rows = all_input_lines
        .len()
        .clamp(1, INPUT_MAX_ROWS.min(room_for_input));
    let cursor = prompt(state).chars().count() + state.input.get_cursor_position();
    let cursor_row = all_input_lines
        .iter()
        .rposition(|line| line.start <= cursor)
        .unwrap_or(0);
    let end = (cursor_row + 1).max(input_rows).min(all_input_lines.len());
    ChatLayout {
        input_lines: all_input_lines[end - input_rows..end].to_vec(),
        transcript_rows: height.saturating_sub(rules + input_rows),
        chrome,
    }
}

/// The transcript and the input box, in the rows `layout` was measured for,
/// starting at `y`. The caller places the cursor, because that has to happen
/// before anything is printed.
fn render_chat(state: &mut State, y: usize, cols: usize, layout: &ChatLayout) {
    if layout.chrome.transcript {
        if let Err(error) = &state.cfg {
            render_config_banner(error, y, layout.transcript_rows, cols);
        } else {
            render_transcript(state, y, layout.transcript_rows, cols);
        }
    }

    let input_y = layout.input_y(y);
    let rule = "─".repeat(cols);
    if layout.chrome.rules {
        print_text_with_coordinates(
            Text::new(&rule).dim_range(..),
            0,
            input_y - 1,
            Some(cols),
            None,
        );
    }
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
    if layout.chrome.rules {
        print_text_with_coordinates(
            Text::new(&rule).dim_range(..),
            0,
            input_y + layout.input_lines.len(),
            Some(cols),
            None,
        );
    }
}

/// The top row: the tab bar on the left, one cell per open chat, and the status
/// note at the right edge.
fn render_title(state: &mut State, cols: usize) {
    let model = match &state.cfg {
        Ok(cfg) => cfg.model.clone(),
        Err(error) => error.model.clone(),
    };
    // "hocus — {model}" lives in the real window title bar (set below) rather
    // than drawn into the pane, so it survives even when the pane is scrolled
    // or covered, and doesn't compete with the tab bar for space.
    let title = if state.plan_mode_on() {
        format!("hocus (plan) — {model}")
    } else {
        format!("hocus — {model}")
    };
    // Only when it actually moved. The host turns this into a
    // `ScreenInstruction::RenamePane`, which re-renders the screen and reports
    // the whole session state back out to every plugin — far too much to spend
    // on restating an unchanged title ten times a second while a reply streams,
    // and on every keystroke besides. `on_panes` clears the memo when the
    // manifest shows some other title, so placement renaming the pane behind our
    // back is still caught.
    if state.last_pane_title.as_deref() != Some(title.as_str()) {
        rename_plugin_pane(state.own_plugin_id, &title);
        state.last_pane_title = Some(title.clone());
    }

    // The note first, so the bar knows how much of the row is its own. A gap
    // of one keeps the last tab from running into it.
    let note = title_note(state);
    let note_width = note.as_ref().map_or(0, |note| note.width() + 1);
    state.chat_tab_width = cols.saturating_sub(note_width);
    for cell in chat_tabs(state, state.chat_tab_width) {
        let text = Text::new(&cell.text).unbold_range(..);
        let text = if cell.highlighted {
            text.selected()
        } else if cell.active {
            text.color_range(2, ..)
        } else {
            text.dim_range(..)
        };
        print_text_with_coordinates(text, cell.x, 0, Some(cell.width), None);
    }
    if let Some(note) = note {
        let x = cols.saturating_sub(note.width());
        print_text_with_coordinates(
            Text::new(&note).dim_range(..),
            x,
            0,
            Some(note.width()),
            None,
        );
    }
}

/// One slot at the right edge: what is happening now beats what is pending.
/// A count rides along when other chats are working too, so a background
/// reply is visible without looking for its `●` in the bar. The job count is
/// every tab's: a job fires and bills whichever tab is on screen.
fn title_note(state: &mut State) -> Option<String> {
    let elsewhere = match state.parked_count() {
        0 => String::new(),
        n => format!(" +{n}"),
    };
    let activity = if state.is_streaming() {
        Some(format!("streaming…{elsewhere}"))
    } else if !elsewhere.is_empty() {
        Some(format!("working{elsewhere}"))
    } else if state.any_jobs() {
        Some(format!("⏲ {}", state.job_count()))
    } else {
        None
    };
    let context = context_note(state);
    match (activity, context) {
        (Some(activity), Some(context)) => Some(format!("{activity} · {context}")),
        (activity, context) => activity.or(context),
    }
}

/// `12.3k tok`, or `~12.3k tok` while the number is a guess. Nothing for a
/// chat with nothing in it, so a fresh pane is as bare as it was.
fn context_note(state: &mut State) -> Option<String> {
    if state.messages.is_empty() && state.usage.is_none() {
        return None;
    }
    let (tokens, estimated) = state.context_tokens();
    let mark = if estimated { "~" } else { "" };
    Some(format!("{mark}{} tok", format_tokens(tokens)))
}

/// A token count at the width a title slot can spare: `842`, `12.3k`, `1.2M`.
pub(crate) fn format_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    // The unit is chosen on the rounded value: from 999_950 up, one decimal
    // of `k` reads `1000.0k`.
    } else if tokens < 999_950 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    }
}

/// The widest a tab's title is drawn. Long enough to tell chats apart by their
/// opening question, short enough that several fit in a narrow overlay.
const TAB_TITLE_COLS: usize = 18;

/// One cell of the tab bar, ready to print at `x` on the top row.
#[derive(Debug, PartialEq)]
pub(crate) struct TabCell {
    pub(crate) x: usize,
    pub(crate) width: usize,
    pub(crate) text: String,
    /// The chat on screen.
    pub(crate) active: bool,
    /// Under the bar's highlight, while Shift+Tab has it focused.
    pub(crate) highlighted: bool,
    pub(crate) target: TabTarget,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TabTarget {
    Tab(usize),
    Reveal(usize),
}

/// What one tab says: a glyph for what the chat is doing, then its title.
fn tab_label(messages: &[crate::Message], live: bool, confirming: bool) -> String {
    let glyph = if confirming {
        "? "
    } else if live {
        "● "
    } else {
        ""
    };
    let title = match messages.is_empty() {
        true => "new chat".to_owned(),
        false => history::title(messages),
    };
    let mut clipped = truncate_to_width(&title, TAB_TITLE_COLS);
    if clipped.len() < title.len() {
        clipped.push('…');
    }
    format!(" {glyph}{clipped} │")
}

/// The tab bar, laid out in `width` columns. Every open chat gets a cell in
/// bar order; when they do not all fit, the window slides so the highlighted
/// tab — or the active one, when the bar is not focused — is always on screen,
/// with an arrow at whichever edge has tabs hidden past it.
pub(crate) fn chat_tabs(state: &State, width: usize) -> Vec<TabCell> {
    let focus = state.chat_tab_focus;
    let keep = focus
        .or(state.chat_tab_view.get(&state.client).copied())
        .unwrap_or(state.chat_tab);
    let labels: Vec<String> = (0..state.chat_tab_count())
        .map(|tab| match state.parked_at_tab(tab) {
            None => tab_label(
                &state.messages,
                state.is_streaming()
                    || !matches!(state.status, Status::Idle)
                    || !matches!(state.plan, Plan::Off),
                matches!(state.status, Status::ConfirmingTool),
            ),
            Some(index) => {
                let chat = &state.parked[index];
                tab_label(
                    &chat.messages,
                    chat.is_live(),
                    matches!(chat.status, Status::ConfirmingTool),
                )
            },
        })
        .collect();
    let widths: Vec<usize> = labels.iter().map(|label| label.width()).collect();
    if widths.iter().sum::<usize>() <= width {
        return lay_out(&labels, 0, labels.len(), width, state.chat_tab, focus);
    }

    // Grow the window outward from `keep` — rightward first, so the tabs after
    // the one you are on are what you see — until nothing more fits, leaving a
    // two columns for each arrow and divider the window will need.
    let mut start = keep;
    let mut end = keep + 1;
    let mut used = widths[keep];
    loop {
        // Counted even for an edge the next step would reach, which only ever
        // leaves a column spare.
        let arrows = usize::from(start > 0) + usize::from(end < labels.len());
        let room = width.saturating_sub(arrows * 2);
        if end < labels.len() && used + widths[end] <= room {
            used += widths[end];
            end += 1;
        } else if start > 0 && used + widths[start - 1] <= room {
            start -= 1;
            used += widths[start];
        } else {
            break;
        }
    }
    lay_out(&labels, start, end, width, state.chat_tab, focus)
}

/// Print-ready cells for the tabs in `start..end`, with an arrow on either
/// side that has tabs hidden past it.
fn lay_out(
    labels: &[String],
    start: usize,
    end: usize,
    width: usize,
    active: usize,
    focus: Option<usize>,
) -> Vec<TabCell> {
    let mut cells = Vec::new();
    let mut x = 0;
    let mut full = false;
    // Stops at the first cell that does not fit, rather than skipping it: a
    // later, narrower one would otherwise be drawn in its place.
    let mut push = |text: String, active: bool, highlighted: bool, target: TabTarget| {
        let w = text.width();
        if full || x + w > width {
            full = true;
            return;
        }
        cells.push(TabCell {
            x,
            width: w,
            text,
            active,
            highlighted,
            target,
        });
        x += w;
    };
    if start > 0 {
        push("←│".to_owned(), false, false, TabTarget::Reveal(start - 1));
    }
    for (tab, label) in labels.iter().enumerate().take(end).skip(start) {
        push(
            label.clone(),
            tab == active,
            focus == Some(tab),
            TabTarget::Tab(tab),
        );
    }
    if end < labels.len() {
        push("→│".to_owned(), false, false, TabTarget::Reveal(end));
    }
    cells
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

/// The box the pending answer is drawn in: the title on the top border, the
/// body inside it, the keys along the bottom one.
///
/// `plan_inner` is the plan's already-rendered lines, and is ignored for a tool
/// confirmation — a tool describes itself in `Confirmation::detail`, while a
/// plan's body is the assistant message the caller lifted out of the transcript.
fn prompt_box(prompt: &Confirmation, plan_inner: Vec<VLine>, cols: usize) -> Vec<VLine> {
    let inner_cols = frame_content_width(cols);
    let inner = match prompt.kind {
        PromptKind::Plan => plan_inner,
        PromptKind::Tool => prompt
            .detail
            .iter()
            .flat_map(|(line, consequence)| {
                // Only the sentence saying what the call will do is red. The
                // arguments beside it are dim, so the one line worth reading
                // twice is the one that stands out — which is what the whole
                // block being red used to cost.
                let style = match consequence {
                    true => MdStyle {
                        error: true,
                        ..MdStyle::default()
                    },
                    false => MdStyle {
                        dim: true,
                        ..MdStyle::default()
                    },
                };
                wrap(line, inner_cols)
                    .into_iter()
                    .map(move |row| line_from_runs(&[Run::new(row, style)], Role::Assistant))
            })
            .collect(),
    };
    frame(&prompt.title, &key_runs(&prompt.keys), inner, cols)
}

fn plan_at(state: &State, prompt: Option<&Confirmation>) -> Option<usize> {
    prompt
        .filter(|prompt| prompt.kind == PromptKind::Plan)
        .and_then(|_| state.plan_index())
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
    // The plan may have notices or manual command output after it. Find the
    // most recent assistant prose rather than requiring it to stay last.
    let plan_at = plan_at(state, prompt.as_ref());
    // Finish the plan-cache borrow before borrowing the transcript cache.
    let prompt_box = prompt.as_ref().map(|prompt| match plan_at {
        Some(at) if prompt.kind == PromptKind::Plan => {
            prompt_box(prompt, cached_plan(state, cols, at).to_vec(), cols)
        },
        _ => prompt_box(prompt, Vec::new(), cols),
    });

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

    // What is waiting on an answer, drawn as a box: the title says which call —
    // the assistant message above lists every call it asked for, which doesn't
    // say which one this prompt is about — and the keys sit on the bottom
    // border.
    //
    // Wrapped here rather than left to the host: the tail skips the wrapping the
    // cached body went through, and `print_text_with_coordinates` *truncates* what
    // it is given. Unwrapped, a long argument runs off the pane edge and the user
    // agrees to something they were never shown.
    //
    // A plan's body is the reply already in `lines`, so the box is drawn around
    // a re-render of that one message and `plan_body` says where to cut the
    // cached lines short. Once the plan is answered it falls back into the body
    // and renders like any other reply.
    let plan_body = plan_at.and_then(|at| {
        let start = transcript.starts.get(at).copied()?;
        let end = transcript
            .starts
            .get(at + 1)
            .map(|next| next.saturating_sub(1))
            .unwrap_or(lines.len());
        Some((start.saturating_sub(1), end))
    });
    if let Some(prompt_box) = prompt_box {
        if let Some((_, plan_end)) = plan_body {
            // Keep messages appended after the plan, then put the still-pending
            // plan box back at the tail where its answer remains visible.
            let suffix_end = lines.len() - usize::from(replaced_last);
            let mut suffix = lines[plan_end..suffix_end].to_vec();
            suffix.append(&mut tail);
            tail = suffix;
        }
        tail.push(VLine::blank());
        tail.extend(prompt_box);
    }

    // The plan moved into the tail, so the blank separator that preceded it goes
    // too — the tail pushes its own.
    let body_end = match plan_body {
        Some((start, _)) => start,
        None => lines.len() - usize::from(replaced_last),
    };
    let body = &lines[..body_end];
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
    if state.chat_tab_focus.is_some() {
        return "<←→> choose  <Enter> switch  <x> close  <Ctrl l> new  <Esc> back";
    }
    match state.picker.as_ref().map(|picker| &picker.kind) {
        Some(PickerKind::Chats(_)) => return "<↑↓> choose  <Enter> open  <Esc> back",
        Some(PickerKind::Models) => return "<↑↓> choose  <Enter> use  <Esc> back",
        None => {},
    }
    // Ahead of the `Status` match: a plan waits on the user rather than on the
    // tool queue, so the two are orthogonal and this is the nearer question.
    if state.awaiting_plan() {
        return "</execute> run  </execute --clear> fresh  <n> refine  <Esc> hide";
    }
    // Esc hides from every one of these; Ctrl-c is what interrupts, and when a tool
    // and a stream are both outstanding it takes the tool first.
    match state.status {
        Status::ConfirmingTool => "<y> yes  <n> no  run this?  <Ctrl c> abort  <Esc> hide",
        Status::AwaitingCommand => "running…  <Ctrl c> abort  <Esc> hide  <↑↓> scroll",
        Status::AwaitingSearch => "searching…  <Ctrl c> abort  <Esc> hide  <↑↓> scroll",
        Status::AwaitingLinear => "reading linear…  <Ctrl c> abort  <Esc> hide  <↑↓> scroll",
        Status::AwaitingExtension => "running extension…  <Ctrl c> abort  <Esc> hide  <↑↓> scroll",
        Status::AwaitingPluginLoad => "loading plugin…  <Ctrl c> abort  <Esc> hide  <↑↓> scroll",
        Status::Idle if state.is_streaming() => {
            "streaming…  <Ctrl c> cancel  <Esc> hide  <↑↓> scroll"
        },
        Status::Idle if state.is_planning() => "planning  read-only tools  <Esc> hide",
        Status::Idle => "<Ctrl l> new chat  <Ctrl o> move  <Esc> hide",
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

/// The (column, row) `cursor` occupies in `lines`, or `None` when it falls outside
/// the visible `INPUT_MAX_ROWS`-row window.
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

    // -- frames ---------------------------------------------------------------

    fn plan_prompt() -> Confirmation {
        Confirmation {
            kind: PromptKind::Plan,
            title: "PLAN".to_owned(),
            detail: Vec::new(),
            keys: vec![("/execute", "run"), ("n", "refine"), ("Esc", "hide")],
        }
    }

    fn tool_prompt() -> Confirmation {
        Confirmation {
            kind: PromptKind::Tool,
            title: "send_input".to_owned(),
            detail: vec![
                ("types into pane 7".to_owned(), true),
                ("text: ls".to_owned(), false),
            ],
            keys: vec![("y", "run"), ("n", "skip")],
        }
    }

    /// Every row of a box is exactly as wide as the pane, borders included —
    /// `print_text_with_coordinates` truncates, so a row a column too wide loses
    /// its right-hand border rather than wrapping.
    #[test]
    fn a_frame_is_exactly_as_wide_as_the_pane() {
        let cols = 40;
        let inner = markdown_lines("ordinary words and 日本語 too", cols - FRAME_PAD);
        let lines = frame("PLAN", &key_runs(&[("n", "keep planning")]), inner, cols);

        for line in &lines {
            assert_eq!(
                line.text.width(),
                cols,
                "row {:?} is the wrong width",
                line.text
            );
            assert_eq!(
                line.styles.len(),
                line.text.chars().count(),
                "row {:?} has a style per character",
                line.text
            );
        }
    }

    /// The title sits on the top border and the keys on the bottom one, which is
    /// what lets the plan be one box rather than a box and a prompt under it.
    #[test]
    fn a_frame_carries_its_title_and_keys_on_its_borders() {
        let lines = prompt_box(&plan_prompt(), markdown_lines("do the thing", 36), 40);

        assert!(lines[0].text.starts_with("┌ PLAN "), "{:?}", lines[0].text);
        assert!(lines[1].text.contains("do the thing"));
        let bottom = &lines.last().expect("a bottom border").text;
        assert!(
            bottom.starts_with("└ /execute run · n refine "),
            "{bottom:?}"
        );
        assert!(bottom.ends_with('┘'));
    }

    /// A pane too narrow for `│ … │` keeps the words and loses the box: the
    /// content is what the user has to read, and the keys are what they have to
    /// press.
    #[test]
    fn a_frame_gives_up_rather_than_clip_a_narrow_pane() {
        for cols in MIN_COLS..MIN_FRAME_COLS {
            let lines = prompt_box(&plan_prompt(), markdown_lines("do the thing", cols), cols);
            let text = lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            assert!(lines.iter().all(|line| !line.text.contains('┌')));
            assert!(lines.iter().all(|line| line.text.width() <= cols));
            assert!(text.contains("do the thing"));
            for key in ["/execute", "n", "Esc"] {
                assert!(text.contains(key), "{key:?} was clipped at {cols} columns");
            }
        }
    }

    /// Keys wider than the border they would sit on move to their own line
    /// instead of being cut in half by it.
    #[test]
    fn keys_too_wide_for_the_border_move_under_the_box() {
        for cols in MIN_FRAME_COLS..=35 {
            let lines = prompt_box(
                &plan_prompt(),
                markdown_lines("do the thing", cols - FRAME_PAD),
                cols,
            );
            let text = lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            assert!(
                lines
                    .iter()
                    .any(|line| line.text.starts_with('└') && line.text.ends_with('┘')),
                "the bottom border is missing at {cols} columns"
            );
            assert!(lines.iter().all(|line| line.text.width() <= cols));
            for key in ["/execute", "n", "Esc"] {
                assert!(text.contains(key), "{key:?} was clipped at {cols} columns");
            }
        }
    }

    #[test]
    fn an_unframed_tool_uses_the_whole_pane_width() {
        let cols = MIN_FRAME_COLS - 1;
        let prompt = Confirmation {
            detail: vec![("1234567890123456789012345678901".to_owned(), false)],
            ..tool_prompt()
        };
        let lines = prompt_box(&prompt, Vec::new(), cols);

        assert!(lines.iter().any(|line| line.text.len() == cols));
    }

    /// The point of the whole change: a confirmation is no longer a block of
    /// red. Only the sentence saying what the call will do carries the error
    /// colour, and the border stays dim.
    #[test]
    fn only_the_consequence_of_a_tool_call_is_red() {
        let lines = prompt_box(&tool_prompt(), Vec::new(), 40);

        assert!(
            lines.iter().all(|line| line.role != Role::Error),
            "no line is red as a whole"
        );
        assert!(lines[0].text.starts_with("┌ send_input "));
        let reddened: Vec<&str> = lines
            .iter()
            .filter(|line| line.styles.iter().any(|style| style.error))
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(
            reddened.len(),
            1,
            "exactly one line is reddened: {reddened:?}"
        );
        assert!(reddened[0].contains("types into pane 7"));
        // The border of that very line is still dim rather than red.
        let consequence = lines
            .iter()
            .find(|line| line.text.contains("types into pane 7"))
            .expect("present");
        assert!(consequence.styles[0].dim && !consequence.styles[0].error);
        assert!(lines
            .last()
            .expect("a bottom border")
            .text
            .contains("y run · n skip"));
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
    fn a_long_input_window_follows_the_cursor() {
        let mut state = State::default();
        state.input.set_text(
            (0..10)
                .map(|index| format!("line {index}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );

        let tail = chat_layout(&state, 8, 24, full_chrome());
        assert_eq!(tail.input_lines.first().unwrap().text, "line 5");
        assert!(cursor_cell(
            &tail.input_lines,
            PROMPT.chars().count() + state.input.get_cursor_position()
        )
        .is_some());

        state.input.move_to_start();
        let head = chat_layout(&state, 8, 24, full_chrome());
        assert_eq!(head.input_lines.first().unwrap().text, "> line 0");
        assert_eq!(
            cursor_cell(&head.input_lines, PROMPT.chars().count()),
            Some((2, 0))
        );
    }

    #[test]
    fn the_input_rules_fit_inside_the_chat_layout() {
        let layout = chat_layout(&State::default(), 4, 24, full_chrome());

        assert_eq!(layout.transcript_rows + layout.input_rows(), 4);
    }

    // -- degrading into a small pane ----------------------------------------
    //
    // The pane hocus is given is as small as the terminal is (see
    // `overlay::MIN_PANE_ROWS`), so "pane too small" in place of the
    // conversation is a refusal the user can do nothing about. What is drawn
    // gets thinner instead.

    /// The layout a pane with rows to spare gets, which is every one of these
    /// tests' starting point.
    fn full_chrome() -> Chrome {
        chrome_for(MIN_ROWS)
    }

    #[test]
    fn chrome_is_given_up_in_order_of_what_matters_least() {
        let all = Chrome {
            title: true,
            transcript: true,
            help: true,
            rules: true,
        };
        assert_eq!(chrome_for(MIN_ROWS), all, "the whole layout at {MIN_ROWS}");
        assert_eq!(chrome_for(40), all, "and at any height above it");
        assert_eq!(
            chrome_for(5),
            Chrome {
                rules: false,
                ..all
            },
            "the rules go first"
        );
        assert_eq!(
            chrome_for(4),
            Chrome {
                rules: false,
                help: false,
                ..all
            },
            "then the help line"
        );
        assert_eq!(
            chrome_for(3),
            Chrome {
                title: false,
                transcript: true,
                help: false,
                rules: false
            },
            "then the title bar"
        );
        assert_eq!(
            chrome_for(2),
            Chrome {
                title: false,
                transcript: true,
                help: false,
                rules: false
            },
            "a transcript row and the input"
        );
        assert_eq!(
            chrome_for(1),
            Chrome {
                title: false,
                transcript: false,
                help: false,
                rules: false
            },
            "the input alone"
        );
        assert_eq!(chrome_for(0), chrome_for(1), "and nothing at all");
    }

    /// Whatever is drawn is drawn *inside* the pane: the input box is the one
    /// thing never given up, so the row it lands on is the assertion that
    /// matters. It is also where the cursor goes.
    #[test]
    fn a_short_pane_drops_chrome_rather_than_refusing() {
        let mut state = State {
            perms: true,
            ..Default::default()
        };
        state.input.set_text("typing".to_owned());
        for rows in 1..=(MIN_ROWS + 2) {
            for cols in [MIN_INPUT_COLS, 12, 40] {
                let chrome = chrome_for(rows);
                let (body_y, body_rows) = body_of(chrome, rows);
                let layout = chat_layout(&state, body_rows, cols, chrome);
                let at = format!("{rows}x{cols}");

                assert!(!layout.input_lines.is_empty(), "nowhere to type at {at}");
                assert_eq!(
                    layout.transcript_rows + layout.input_rows(),
                    body_rows,
                    "the body is spent exactly at {at}"
                );
                assert!(
                    layout.input_y(body_y) + layout.input_lines.len() <= rows,
                    "the input box runs past the last row at {at}"
                );
                assert!(
                    chrome.transcript || layout.transcript_rows == 0,
                    "a transcript row in a pane with no room for one at {at}"
                );
                // and it renders, with a picker over it and without: a pane
                // this small used to say "too small" and draw nothing at all
                super::render(&mut state, rows, cols);
                let picker = state.picker.take();
                state.picker = Some(Picker {
                    entries: vec![crate::PickerRow::item("a model")],
                    selected: 0,
                    kind: PickerKind::Models,
                });
                super::render(&mut state, rows, cols);
                state.picker = picker;
            }
        }
    }

    #[test]
    fn a_one_row_pane_is_the_input_box() {
        let chrome = chrome_for(1);
        let (body_y, body_rows) = body_of(chrome, 1);
        let layout = chat_layout(&State::default(), body_rows, 24, chrome);

        assert_eq!((body_y, body_rows), (0, 1), "no title, no help line");
        assert_eq!(layout.transcript_rows, 0);
        assert_eq!(layout.input_lines.len(), 1);
        assert_eq!(layout.input_y(body_y), 0, "the one row there is");
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

    /// A few turns, ending on the assistant message a reply would be growing.
    fn a_conversation() -> State {
        let mut state = State::default();
        for n in 0..6 {
            state.messages.push(match n % 2 {
                0 => Message::user(format!("question {n}")),
                _ => Message::assistant(format!("## reply {n}\n\nwith a paragraph under it")),
            });
        }
        state
    }

    /// The reason the cache is per-message at all. A streaming reply used to
    /// move the one key the whole transcript was memoised on, so every frame
    /// re-parsed every message — work proportional to the whole conversation,
    /// ten times a second, queued ahead of whatever the user was typing.
    #[test]
    fn a_streaming_frame_redraws_only_the_message_that_grew() {
        let mut state = a_conversation();
        cached_transcript(&mut state, 40);
        assert_eq!(rebuilds(), 6, "the first frame draws all of them");

        state.messages.last_mut().unwrap().content.push_str(" more");
        cached_transcript(&mut state, 40);
        assert_eq!(rebuilds(), 1, "and the next one draws only the reply");
    }

    #[test]
    fn a_frame_with_nothing_new_redraws_nothing() {
        let mut state = a_conversation();
        cached_transcript(&mut state, 40);
        rebuilds();
        cached_transcript(&mut state, 40);
        assert_eq!(rebuilds(), 0);
    }

    #[test]
    fn a_resize_redraws_every_message() {
        let mut state = a_conversation();
        cached_transcript(&mut state, 40);
        rebuilds();
        cached_transcript(&mut state, 60);
        assert_eq!(rebuilds(), 6);
    }

    /// A message going away has to take its lines and its separator with it, or
    /// the cached prefix no longer lines up with `starts`.
    #[test]
    fn popping_a_message_drops_its_lines_and_keeps_the_rest() {
        let mut state = a_conversation();
        cached_transcript(&mut state, 40);
        state.messages.pop();
        rebuilds();

        let popped = cached_transcript(&mut state, 40).lines.clone();
        assert_eq!(state.transcript_cache.as_ref().unwrap().len(), 5);
        assert_eq!(rebuilds(), 0, "the survivors were already drawn");
        assert_eq!(popped, transcript(&state.messages, 40).lines);
    }

    /// Expanding a tool result changes how many of its lines are drawn without
    /// changing a byte of its content, so the key has to carry the flag.
    #[test]
    fn expanding_a_tool_result_redraws_it_and_what_follows() {
        let mut state = State::default();
        state.messages.push(Message::user("run it"));
        let mut result = Message::tool_result("call_1", "a line\n".repeat(40));
        result.full_output = false;
        state.messages.push(result);
        state.messages.push(Message::assistant("done"));
        let elided = cached_transcript(&mut state, 40).lines.len();
        rebuilds();

        state.messages[1].full_output = true;
        let full = cached_transcript(&mut state, 40).lines.clone();
        assert!(full.len() > elided, "every line is drawn now");
        assert_eq!(rebuilds(), 2, "the result and the message after it");
        assert_eq!(full, transcript(&state.messages, 40).lines);
    }

    /// The property that matters most: however the cache got there, what it
    /// holds is what a from-scratch render would have produced.
    #[test]
    fn the_incremental_transcript_matches_a_full_one() {
        let mut state = a_conversation();
        cached_transcript(&mut state, 40);
        for token in ["\n\n- one", ", two", "\n- three", "\n\n```\ncode"] {
            state.messages.last_mut().unwrap().content.push_str(token);
            let incremental = cached_transcript(&mut state, 40);
            let (lines, starts) = (incremental.lines.clone(), incremental.starts.clone());
            let full = transcript(&state.messages, 40);
            assert_eq!(lines, full.lines, "after {token:?}");
            assert_eq!(starts, full.starts, "after {token:?}");
        }
    }

    /// The pane title is asserted to the host with `rename_plugin_pane`, which
    /// costs a screen repaint and a session-state report fanned out to every
    /// plugin. `last_pane_title` is what stops a render restating an unchanged
    /// one — ten times a second while streaming, and once per keystroke.
    #[test]
    fn the_pane_title_is_only_asserted_when_it_changes() {
        let mut state = State::default();
        assert_eq!(state.last_pane_title, None, "nothing asserted yet");

        render_title(&mut state, RENDER_COLS);
        let asserted = state.last_pane_title.clone();
        assert!(asserted.is_some(), "the first render names the pane");

        render_title(&mut state, RENDER_COLS);
        assert_eq!(
            state.last_pane_title, asserted,
            "and the next one has nothing new to say"
        );

        // Plan mode is in the title, so turning it on has to reach the host.
        state.plan = Plan::Planning;
        render_title(&mut state, RENDER_COLS);
        assert_ne!(state.last_pane_title, asserted, "the title moved");
    }

    /// `on_chunk` swaps the streamed prose onto a tool-calls message, which can
    /// leave the count and every length where they were. Nothing but the
    /// explicit invalidation can catch that.
    #[test]
    fn an_invalidated_message_is_redrawn_even_when_its_key_did_not_move() {
        let mut state = a_conversation();
        cached_transcript(&mut state, 40);
        // Same length, different text — invisible to the key by construction.
        let last = state.messages.last_mut().unwrap();
        last.content = "x".repeat(last.content.len());
        rebuilds();

        state.transcript_cache.as_mut().unwrap().invalidate_from(5);
        let redrawn = cached_transcript(&mut state, 40).lines.clone();
        assert_eq!(rebuilds(), 1);
        assert_eq!(redrawn, transcript(&state.messages, 40).lines);
    }

    #[test]
    fn the_plan_cache_reuses_only_the_same_render_key() {
        let mut state = State::default();
        state.messages.push(Message::assistant("the original plan"));
        cached_plan(&mut state, 40, 0);
        state.plan_cache.as_mut().unwrap().1[0].text = "cached".to_owned();

        assert_eq!(cached_plan(&mut state, 40, 0)[0].text, "cached");
        assert_ne!(cached_plan(&mut state, 30, 0)[0].text, "cached");

        state.plan_cache.as_mut().unwrap().1[0].text = "cached".to_owned();
        state.messages.push(Message::notice("x"));
        assert_ne!(cached_plan(&mut state, 30, 0)[0].text, "cached");

        state
            .messages
            .push(Message::assistant("a replacement plan"));
        assert_eq!(cached_plan(&mut state, 30, 2)[0].text, "a replacement plan");
    }

    #[test]
    fn a_notice_after_a_plan_does_not_empty_the_plan_box() {
        let state = State {
            plan: crate::Plan::AwaitingApproval,
            messages: vec![
                Message::assistant("the plan body"),
                Message::notice("the help notice"),
            ],
            ..Default::default()
        };
        let prompt = state.confirmation_prompt().expect("the prompt is up");
        let at = plan_at(&state, Some(&prompt)).expect("the plan is still selected");
        let transcript = transcript(&state.messages, 40);

        assert_eq!(at, 0);
        assert_eq!(
            transcript.lines[transcript.starts[at]].text,
            "the plan body"
        );
        assert!(transcript.lines[transcript.starts[at + 1]..]
            .iter()
            .any(|line| line.text.contains("the help notice")));
    }

    #[test]
    fn confirmation_help_carries_the_answers_without_the_box() {
        let mut state = State {
            status: Status::ConfirmingTool,
            ..Default::default()
        };
        let help = help_line(&state);
        assert!(help.starts_with("<y> yes  <n> no"));

        state.plan = crate::Plan::AwaitingApproval;
        let help = help_line(&state);
        assert!(help.starts_with("</execute> run  </execute --clear> fresh  <n> refine"));
    }

    #[test]
    fn idle_help_names_the_reposition_key() {
        let state = State {
            status: Status::Idle,
            ..Default::default()
        };
        assert!(help_line(&state).contains("<Ctrl o> move"));
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

        state.on_key(KeyWithModifier::new(BareKey::PageUp));
        render(&mut state);
        assert!(
            state.scroll_offset() > 0,
            "the client may read older output"
        );
        assert!(help_line(&state).contains("<y> yes  <n> no"));

        state.on_mouse(Mouse::ScrollUp(1));
        render(&mut state);
        assert!(
            state.scroll_offset() > 0,
            "mouse scrolling behaves the same way"
        );
        assert!(help_line(&state).contains("<y> yes  <n> no"));
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

    // -- the tab bar ----------------------------------------------------------

    fn chat_named(first: &str) -> crate::Chat {
        crate::Chat {
            messages: vec![Message::user(first)],
            ..Default::default()
        }
    }

    /// `n` open chats titled by number, the first on screen.
    fn tabs_state(n: usize) -> State {
        let mut state = State {
            messages: vec![Message::user("chat 0")],
            ..Default::default()
        };
        for i in 1..n {
            state.parked.push(chat_named(&format!("chat {i}")));
        }
        state
    }

    fn texts(cells: &[TabCell]) -> Vec<&str> {
        cells.iter().map(|cell| cell.text.as_str()).collect()
    }

    fn job(id: u64) -> crate::message::Job {
        crate::message::Job {
            id,
            every: 60.0,
            due_in: 60.0,
            prompt: "check".to_owned(),
            show: false,
            auto_confirm: false,
        }
    }

    /// A job in another tab is work happening off-screen, and the note says so
    /// rather than counting only what the foreground armed.
    #[test]
    fn the_title_note_sees_jobs_in_every_tab() {
        let mut state = tabs_state(2);
        assert_eq!(
            title_note(&mut state).as_deref(),
            Some("~1 tok"),
            "nothing happening anywhere: only the size"
        );

        state.jobs.push(job(1));
        assert_eq!(title_note(&mut state).as_deref(), Some("⏲ 1 · ~1 tok"));

        state.parked[0].jobs.push(job(2));
        assert_eq!(
            title_note(&mut state).as_deref(),
            Some("working +1 · ~1 tok"),
            "the other tab is live, and counted"
        );
        assert!(chat_tabs(&state, 80)[1].text.starts_with(" ● "));
    }

    #[test]
    fn a_single_chat_is_one_active_cell() {
        let state = State::default();
        let cells = chat_tabs(&state, 40);
        assert_eq!(texts(&cells), [" new chat │"], "an empty chat is named");
        assert!(cells[0].active);
        assert!(!cells[0].highlighted);
        assert_eq!(cells[0].x, 0);
    }

    #[test]
    fn cells_carry_a_glyph_for_what_the_chat_is_doing() {
        let mut state = tabs_state(3);
        state.stream = Some(crate::StreamState {
            id: "hocus-1".into(),
            finishing: false,
            usage: None,
        });
        state.parked[1].status = Status::ConfirmingTool;
        let cells = chat_tabs(&state, 60);
        assert_eq!(
            texts(&cells),
            [" ● chat 0 │", " chat 1 │", " ? chat 2 │"],
            "streaming, idle, waiting on y/n"
        );
    }

    #[test]
    fn cells_sit_side_by_side_and_never_pass_the_width() {
        let state = tabs_state(4);
        let cells = chat_tabs(&state, 80);
        let mut x = 0;
        for cell in &cells {
            assert_eq!(cell.x, x);
            assert_eq!(cell.width, cell.text.width());
            x += cell.width;
        }
        assert!(x <= 80);

        for width in 0..80 {
            let cells = chat_tabs(&state, width);
            let end = cells.last().map_or(0, |cell| cell.x + cell.width);
            assert!(end <= width, "{width} columns, drawn to {end}");
        }
    }

    #[test]
    fn a_long_title_is_clipped_with_an_ellipsis() {
        let state = State {
            messages: vec![Message::user(
                "a question that goes on for a very long time indeed",
            )],
            ..Default::default()
        };
        let cells = chat_tabs(&state, 80);
        assert_eq!(texts(&cells), [" a question that go… │"]);
        assert!(cells[0].width <= TAB_TITLE_COLS + 4);
    }

    #[test]
    fn the_window_keeps_the_highlighted_tab_on_screen() {
        // Every cell is " chat N │" = 9 columns; room for two and an arrow.
        let mut state = tabs_state(5);
        let cells = chat_tabs(&state, 20);
        assert_eq!(
            texts(&cells),
            [" chat 0 │", " chat 1 │", "→│"],
            "the active tab first"
        );
        assert_eq!(cells[2].target, TabTarget::Reveal(2));

        state.chat_tab_focus = Some(4);
        let cells = chat_tabs(&state, 20);
        assert_eq!(texts(&cells), ["←│", " chat 3 │", " chat 4 │"]);
        assert_eq!(cells[0].target, TabTarget::Reveal(2));
        assert!(cells[2].highlighted);
        assert!(!cells[1].active);

        state.chat_tab_focus = Some(2);
        let cells = chat_tabs(&state, 22);
        assert_eq!(
            texts(&cells),
            ["←│", " chat 2 │", " chat 3 │", "→│"],
            "hidden tabs on both sides, growing rightward first"
        );
        let cells = chat_tabs(&state, 21);
        assert_eq!(
            texts(&cells),
            ["←│", " chat 2 │", "→│"],
            "and with room for only one"
        );
    }

    #[test]
    fn the_active_cell_is_marked_and_the_highlight_can_be_elsewhere() {
        let mut state = tabs_state(3);
        state.chat_tab_focus = Some(2);
        let cells = chat_tabs(&state, 60);
        assert!(cells[0].active && !cells[0].highlighted);
        assert!(!cells[2].active && cells[2].highlighted);
    }

    #[test]
    fn bar_focus_has_its_own_help_line() {
        let state = State {
            chat_tab_focus: Some(0),
            ..Default::default()
        };
        assert!(help_line(&state).starts_with("<←→> choose"));
    }
}

//! Text wrapping and screen layout.
//!
//! Yinz wipes the plugin's grid and repaints it fully on every render, and
//! gives plugins no scrollback of their own, so everything on screen is
//! recomputed here from `State` each time and all scroll state is ours.

use hocus_proto::markdown::{self, MdLine, MdStyle, StyledChar};
use hocus_proto::tabs;
use hocus_proto::text::{wrap, wrap_input, WrappedLine};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use yinz_tile::prelude::*;

use crate::api::ConfigError;
use crate::wand;
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

    /// A rendered markdown line, given the voice it was spoken in.
    ///
    /// The role rides on every line rather than being patched on afterwards, because
    /// `ends_with_empty_reply` reads it: an empty `Role::Assistant` line at the tail *is* the
    /// streaming placeholder, so a presented result that rendered to nothing would otherwise
    /// be given a spinner.
    fn from_md(line: MdLine, role: Role) -> Self {
        VLine {
            text: line.text,
            role,
            styles: line.styles,
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
    // The conversation has the whole pane, so it keeps the blank line between blocks; the
    // agents pane's history half, which asks the same renderer for the same prose, does not.
    markdown::render(text, cols, markdown::Options::default())
        .into_iter()
        .map(|line| VLine::from_md(line, role))
        .collect()
}

// ---------------------------------------------------------------------------
// The waiting indicator
// ---------------------------------------------------------------------------

/// Braille spinner frames. Every frame is one column wide, so the text beside it
/// never shifts.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Timer ticks a phrase survives before the next one takes over.
pub const TICKS_PER_PHRASE: u64 = 30;

const PHRASES: [&str; 32] = [
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
    "Escaping the escape codes…",
    "Repainting the scrollback…",
    "Counting braille dots…",
    "Arguing with the parser…",
    "Rewinding the cursor…",
    "Auditing the ANSI…",
    "Shuffling the tab bar…",
    "Polling an empty socket…",
    "Feeding the pty…",
    "Rehearsing the answer…",
    "Sharpening the diff…",
    "Checking under the status bar…",
    "Waiting on a slow token…",
    "Composing something useful…",
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
    markdown::word_wrap_styled(&chars, width.max(1))
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

// ---------------------------------------------------------------------------
// The empty-chat wand
// ---------------------------------------------------------------------------

/// Which of the wand's two forms fits, if either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wand {
    /// The real thing, over the kitty graphics protocol.
    Image,
    /// Its text stand-in: what a terminal without the protocol gets, and what a pane too short
    /// for the picture gets whether or not the protocol is there.
    Text,
}

/// The largest form that fits, or `None` when even the text one does not. A pane too small for
/// either is left exactly as bare as it used to be.
fn wand_form(state: &State, height: usize, cols: usize) -> Option<Wand> {
    if state.kitty_graphics.contains(&state.client)
        && cols >= wand::MIN_COLS
        && height >= wand::MIN_ROWS
    {
        Some(Wand::Image)
    } else if height >= wand::fallback_lines(0).len() + 2 && cols >= 16 {
        Some(Wand::Text)
    } else {
        None
    }
}

/// How many rows the wand takes above the prompt, including the blank line under it.
fn wand_height(state: &State, height: usize, cols: usize) -> usize {
    match wand_form(state, height, cols) {
        Some(Wand::Image) => wand::WAND_ROWS + 1,
        Some(Wand::Text) => wand::fallback_lines(0).len() + 1,
        None => 0,
    }
}

/// Draw the wand against the left edge, in whichever form fits.
///
/// Sets `wand_drawing`, which is what `wants_frames` reads to keep the animation going. It cannot
/// arm the timer itself: a host call writes to the plugin's stdout and then has the host read
/// that stdout back as its argument, so making one after the frame has started printing hands the
/// host the frame instead. Arming happens in `load` and at the end of `update`.
fn render_wand(state: &mut State, y: usize, height: usize, cols: usize) {
    let form = wand_form(state, height, cols);
    state.wand_drawing = form.is_some();
    if form != Some(Wand::Image) {
        // The picture is not being drawn any more - the host turned out not to speak the
        // protocol, or the pane shrank below `MIN_ROWS`. The pixels are kept alive only by the
        // placeholder cells that refer to them, and there are none now, so release this client's
        // copies. This is also what keeps `wand_transmitted` meaning "clients currently showing
        // the image": without it a later flip back to `Image` would skip the transmit and print
        // placeholders that refer to nothing.
        if state.wand_transmitted.remove(&state.client) {
            print!("{}", wand::delete_bytes());
        }
    }
    let Some(form) = form else {
        return;
    };
    let frame = state.wand_frame;
    if form == Wand::Image {
        // The pixels ride on this client's first frame and are referred to by placeholder cells
        // from then on, which is what makes redrawing at ten frames a second affordable.
        let transmit = state.wand_transmitted.insert(state.client);
        print!("{}", wand::frame_bytes(frame, 0, y, transmit));
        return;
    }
    for (row, line) in wand::fallback_lines(frame).into_iter().enumerate() {
        print_text_with_coordinates(Text::new(line).dim_range(..), 0, y + row, Some(cols), None);
    }
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

    /// Whether the cache describes exactly these messages — nothing appended,
    /// grown, swapped or removed since it was last brought up to date. What a
    /// scroll key checks before trusting the maximum the last render measured.
    pub fn covers(&self, messages: &[crate::Message]) -> bool {
        self.keys.len() == messages.len()
            && self
                .keys
                .iter()
                .zip(messages)
                .all(|(key, message)| *key == message_key(message))
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
    // The role's own colour first, then the per-character styles over it.
    let text = match line.role {
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
    markdown::style_over(text, &line.styles, false)
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
    // Only render_wand sets this again. Every early return therefore stops a wand that is no
    // longer visible instead of leaving its timer chain running forever.
    state.wand_drawing = false;
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
    // (hocus-agents, the left half of the panel this one is the right half
    // of); the conversation has the whole body below the title.
    let chrome = chrome_for(rows);
    let (body_y, body_rows) = body_of(chrome, rows);
    let layout = state
        .picker
        .is_none()
        .then(|| chat_layout(state, body_rows, cols, chrome));

    // The picker's own geometry, wanted before anything is drawn because the
    // cursor sits in its filter row.
    let picker_geometry = state.picker.as_ref().map(|_| {
        // Leave the chat enough room for both input rules and one text row -
        // and, in a body too short for that, the one row the input box is on.
        let rows = (body_rows / 2)
            .max(1)
            .min(body_rows.saturating_sub(3).max(1))
            .min(body_rows.saturating_sub(1));
        let ruled = rows > 1;
        (rows, ruled, rows - usize::from(ruled))
    });

    if let (Some(picker), Some((_, _, drawn))) = (state.picker.as_ref(), picker_geometry) {
        // The typed filter is where the keyboard is, so that is where the
        // cursor goes - unless the list is too short to draw the filter row.
        match filter_row(picker, drawn) {
            true => show_cursor(Some((filter_prompt(picker).width(), body_y))),
            false => show_cursor(None),
        }
    } else if state.chat_input_released {
        // The box let the keyboard go to the ring, and the missing caret is the only thing on
        // screen that says so - the pane frame still reads as focused, because this half *is*
        // the focused one; it is the stop within it that moved. A caret in a box that is not
        // typing would be a lie, and the same lie the picker above refuses to tell.
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
    if let (Some(picker), Some((picker_rows, ruled, drawn))) =
        (state.picker.as_ref(), picker_geometry)
    {
        render_picker(picker, body_y, drawn, cols, true);
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
        let usage = usage_note(state);
        let usage = if usage.is_empty() {
            usage
        } else {
            format!("─ {usage} ")
        };
        let rule = format!(
            "{}{}",
            truncate_to_width(&usage, cols),
            "─".repeat(cols.saturating_sub(usage.width()))
        );
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
        print_text_with_coordinates(tabs::styled(&cell), cell.x, 0, Some(cell.width), None);
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

/// One slot at the right edge, for the work the help line cannot see.
///
/// The foreground chat's own state belongs to the help line at the bottom of
/// the pane, where it sits next to the key that acts on it; repeating it here
/// only costs the tab bar the columns the note takes, for the whole of a reply.
/// What is left for this slot is work happening off screen: other live chats,
/// counted so a background reply is visible without looking for its `●` in the
/// bar, and jobs, which fire and bill whichever tab is on screen.
fn title_note(state: &State) -> Option<String> {
    match state.parked_count() {
        0 => state.any_jobs().then(|| format!("⏲ {}", state.job_count())),
        n => Some(format!("+{n}")),
    }
}

/// Provider-reported last-request input and cumulative totals, never estimates.
fn usage_note(state: &State) -> String {
    let mut fields = Vec::new();
    if let Some(last) = state.last_request_input {
        fields.push(format!("last in {}", format_tokens(last)));
    }
    if let Some(usage) = state.usage {
        fields.push(format!(
            "{} in · {} out",
            format_tokens(usage.input_tokens),
            format_tokens(usage.output_tokens)
        ));
        if let Some(cost) = usage.cost_nanos {
            fields.push(format_cost(cost));
        }
    }
    fields.join(" · ")
}

/// A nano-USD amount at title width: `$0`, `<$0.0001`, `$0.0123`, `$1.23`.
pub(crate) fn format_cost(nanos: u64) -> String {
    let dollars = nanos as f64 / 1e9;
    if nanos == 0 {
        "$0".to_owned()
    // Below half a ten-thousandth, four decimals read as a free request.
    } else if nanos < 50_000 {
        "<$0.0001".to_owned()
    // As with `format_tokens`, the precision is chosen on the rounded value.
    } else if nanos < 999_950_000 {
        format!("${dollars:.4}")
    } else {
        format!("${dollars:.2}")
    }
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

pub(crate) use yinz_types::tab_bar::TabTarget;

pub(crate) type TabCell = tabs::TabCell<TabTarget>;

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
    tabs::label(&format!("{glyph}{clipped}"))
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
    yinz_types::tab_bar::overflow(&labels, width, state.chat_tab, focus, keep)
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
    /// The largest `offset` this transcript and height allow: what the scroll
    /// keys clamp against until the next render measures again.
    max_offset: usize,
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
        max_offset,
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

    // Nothing has arrived yet: the transcript ends with the empty assistant
    // placeholder, which renders as an empty line. That line is the loader's
    // home — the spinner sits where the reply will start, and there is no text
    // for a caret to follow. Taking the loader here leaves `None` for the rest,
    // which is what stops it being drawn a second time on its own line.
    let loader = match loader {
        Some(loader) if ends_with_empty_reply(lines) => {
            let mut last = lines.last().expect("checked non-empty").clone();
            last.text.push_str(&loader);
            tail.push(last);
            replaced_last = true;
            None
        },
        loader => loader,
    };

    // Otherwise the caret marks where the next token lands. Only while the reply
    // really is last — which is what `streaming` says — or it would sit on a
    // line no token is going to extend.
    if streaming {
        if let Some(last) = lines.last() {
            let mut last = last.clone();
            last.text.push('▌');
            tail.push(last);
            replaced_last = true;
        }
    }

    // The phrase gets a line of its own below whatever the last line is: a reply
    // part way in, or a tool-call summary while the tool runs. It is already
    // gone when a tool waits on a y/n — the confirmation box is what should hold
    // the eye then — leaving the caret above as the only mark of the wait.
    if let Some(loader) = loader {
        tail.push(VLine::blank());
        tail.push(VLine {
            text: loader,
            role: Role::Notice,
            styles: Vec::new(),
        });
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
        render_wand(state, y, height, cols);
        print_text_with_coordinates(
            Text::new("Ask something.").dim_range(..),
            0,
            y + wand_height(state, height, cols),
            Some(cols),
            None,
        );
        state.set_max_scroll_offset(0);
        return;
    }

    let Window {
        start,
        end,
        offset,
        frozen,
        max_offset,
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
    state.set_max_scroll_offset(max_offset);
    // The chat has something in it, so the splash is gone: stop its clock and free this client's
    // virtual placements. Every client renders separately and must release its own image store.
    state.wand_drawing = false;
    if state.wand_transmitted.remove(&state.client) {
        print!("{}", wand::delete_bytes());
    }

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

/// Whether the list has room for its filter row: two rows at least, and
/// something to filter. The empty list is the fetch still in flight, and has
/// nothing to type at.
fn filter_row(picker: &Picker, height: usize) -> bool {
    height > 1 && !picker.entries.is_empty()
}

/// The filter row up to the cursor — also what says where the cursor goes.
fn filter_prompt(picker: &Picker) -> String {
    format!("filter: {}", picker.filter)
}

/// The archive (one chat per row, titled by its opening question) or the model
/// list (one id per row), narrowed by whatever has been typed into the filter.
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

    let visible = picker.visible();
    let (list_y, height) = match filter_row(picker, height) {
        true => {
            let prompt = filter_prompt(picker);
            // The count goes at the far end, clear of the cursor: it is what
            // says whether one more character would leave anything at all.
            let count = format!("{}/{}", visible.len(), picker.entries.len());
            let gap = cols
                .saturating_sub(prompt.width() + count.width())
                .max(usize::from(cols > prompt.width()));
            let line = format!("{prompt}{}{count}", " ".repeat(gap));
            print_text_with_coordinates(Text::new(line).dim_range(..), 0, y, Some(cols), None);
            (y + 1, height - 1)
        },
        false => (y, height),
    };

    if visible.is_empty() {
        print_text_with_coordinates(
            Text::new("no matches").dim_range(..),
            0,
            list_y,
            Some(cols),
            None,
        );
        return;
    }

    // Window the list around the selection rather than tracking a scroll offset:
    // keeping it near the middle means it is always visible and the list moves
    // by one row per keypress in both directions.
    let at = visible
        .iter()
        .position(|entry| *entry == picker.selected)
        .unwrap_or(0);
    let start = at
        .saturating_sub(height / 2)
        .min(visible.len().saturating_sub(height));

    for (index, entry) in visible.iter().skip(start).take(height).enumerate() {
        // Titles are already stripped of control characters by `history::title`,
        // and the host clips to `cols`, so neither is repeated here.
        let entry = &picker.entries[*entry];
        let selected = active && start + index == at;
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
        print_text_with_coordinates(text, 0, list_y + index, Some(cols), None);
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
    // Ahead of the picker and the statuses below: a released box is where the keys are, and
    // what they do there is the nearer question than what the conversation is busy with.
    if state.chat_input_released {
        return "<Tab/Shift Tab> move  <Enter> send  <Esc> hide  type to edit";
    }
    match state.picker.as_ref().map(|picker| &picker.kind) {
        Some(PickerKind::Chats(_)) => {
            return "type to filter  <Tab> complete  <↑↓> choose  <Enter> open  <Esc> back"
        },
        Some(PickerKind::Models) => {
            return "type to filter  <Tab> complete  <↑↓> choose  <Enter> use  <Esc> back"
        },
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
        Status::ConfirmingTool => "<y> yes  <n> no  <PgUp/PgDn> scroll  <Ctrl c> abort  <Esc> hide",
        Status::ReviewingTool { .. } => {
            "checking safety…  <y> yes  <n> no  <Ctrl c> abort  <Esc> hide"
        },
        Status::AwaitingCommand => "running…  <Ctrl c> abort  <Esc> hide  <PgUp/PgDn> scroll",
        Status::AwaitingSearch => "searching…  <Ctrl c> abort  <Esc> hide  <PgUp/PgDn> scroll",
        Status::AwaitingDecisions => "evaluating…  <Ctrl c> abort  <Esc> hide  <PgUp/PgDn> scroll",
        Status::AwaitingSwamp { .. } => "running…  <Ctrl c> abort  <Esc> hide  <PgUp/PgDn> scroll",
        Status::AwaitingExtension => {
            "running extension…  <Ctrl c> abort  <Esc> hide  <PgUp/PgDn> scroll"
        },
        Status::AwaitingPluginTest => {
            "testing plugin…  <Ctrl c> abort  <Esc> hide  <PgUp/PgDn> scroll"
        },
        Status::AwaitingPluginLoad => {
            "loading plugin…  <Ctrl c> abort  <Esc> hide  <PgUp/PgDn> scroll"
        },
        Status::AwaitingRetry => "retrying…  <Ctrl c> give up  <Esc> hide  <PgUp/PgDn> scroll",
        Status::Idle if state.is_streaming() => {
            "streaming…  <Ctrl c> cancel  <Esc> hide  <PgUp/PgDn> scroll"
        },
        Status::Idle if state.is_planning() => {
            "planning  read-only tools  </plan off> exit  <Esc> hide"
        },
        // `Ctrl e o` rather than Ctrl-o, which only Locked mode lets through: Normal mode takes it
        // for Session mode before the panel sees it.
        Status::Idle => "<Ctrl l> new chat  <Ctrl e o> move  <Esc> hide",
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
#[path = "unit/ui_tests.rs"]
mod tests;

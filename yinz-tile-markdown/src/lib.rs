// test comment line 1 - this is just harmless placeholder text for a test change
// test comment line 2 - this is just harmless placeholder text for a test change
// test comment line 3 - this is just harmless placeholder text for a test change
// test comment line 4 - this is just harmless placeholder text for a test change
// test comment line 5 - this is just harmless placeholder text for a test change
// test comment line 6 - this is just harmless placeholder text for a test change
// test comment line 7 - this is just harmless placeholder text for a test change
// test comment line 8 - this is just harmless placeholder text for a test change
// test comment line 9 - this is just harmless placeholder text for a test change
// test comment line 10 - this is just harmless placeholder text for a test change
// test comment 1
// test comment 2
// test comment 3
// test comment 4
// test comment 5
//! Markdown to styled terminal lines.
//!
//! Lived in `agent::ui` while the conversation was the only pane that drew a model's prose.
//! The agents pane's history half draws the same material now — prompts, plans and the
//! `last_assistant_message` a run ends on are raw markdown by the time they reach a pane — and
//! one renderer reading one dialect is the whole point of moving it out, the same move
//! [`yinz_tile::text`] records for the wrapper beside it.
//!
//! It is a crate of its own rather than a module of `yinz-tile`, because `yinz-tile` is the
//! base dependency of every builtin plugin and pulldown-cmark is 2.5MB of parser that a tab bar
//! has no use for. A default-off feature would not have confined it: `xtask` builds every plugin
//! in a single cargo invocation, so a feature one plugin turns on is on for all of them. A crate
//! the others never name is the only line that holds.
//!
//! The shape is: parse with pulldown-cmark into [`MdBlock`]s, each a run of styled characters
//! plus the prefix its first and later rows carry; wrap each block to the width on offer; hand
//! back [`MdLine`]s, one [`MdStyle`] per displayed character. [`style_text`] is the last step,
//! collapsing that vector into the range calls the `Text` API takes.
//!
//! The dialect is CommonMark plus the three GFM extensions a model actually emits: tables,
//! strikethrough and task lists. Footnotes, definition lists and the rest stay off - nothing
//! writes them at us, and every extension is a character that stops being literal. Note what
//! that costs the *producers* of markdown: `agent::present::inline` has to escape `|` and `~`
//! now that they mean something.
//!
//! The per-character style vector is deliberate and is the expensive part — see
//! `agent/benches/transcript.rs`, which calls this by far the most expensive thing a render
//! does. Callers that redraw on a timer cache the result rather than parsing again.

use std::collections::HashMap;

use pulldown_cmark::{
    Alignment, BrokenLink, CowStr, Event, Options as ParserOptions, Parser, Tag, TagEnd,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use yinz_tile::prelude::Text;
use yinz_tile::text::sanitize;

/// How one displayed character is painted.
///
/// Colour levels resolve against the user's theme host-side, so a number here follows whatever
/// palette they have configured rather than naming a colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MdStyle {
    pub bold: bool,
    pub dim: bool,
    pub color: Option<usize>,
    /// Painted in the theme's error colour. A run rather than a whole line,
    /// because a framed line carries its border and its body together: the
    /// border must stay dim while the one sentence saying what a mutating call
    /// will do is the part that goes red.
    pub error: bool,
    /// Struck through - `~~like this~~`. Orthogonal to the rest: a struck run keeps
    /// whatever colour and weight it was already given.
    pub strike: bool,
}

/// A character and how to paint it, which is what the wrappers here move around: breaking a
/// line must not lose the emphasis that was spanning the break.
#[derive(Debug, Clone)]
pub struct StyledChar {
    pub ch: char,
    pub style: MdStyle,
}

/// One rendered line: the text as it reads, and one style per character of it.
///
/// Crate-neutral on purpose. `agent` wraps this in a `VLine` that also carries the speaking
/// role; the agents pane has no roles and uses it as it comes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MdLine {
    pub text: String,
    /// One entry per **character** of `text` — the unit the `Text` API indexes ranges by. May
    /// be empty, meaning "unstyled", which is what a line assembled outside the parser has.
    pub styles: Vec<MdStyle>,
}

impl MdLine {
    /// A line with no styling of its own.
    pub fn plain(text: impl Into<String>) -> Self {
        MdLine {
            text: text.into(),
            styles: Vec::new(),
        }
    }

    /// The blank line the renderer puts between two blocks.
    pub fn blank() -> Self {
        MdLine::default()
    }

    /// Move the line right by `columns` spaces, keeping `styles` in step with `text`.
    ///
    /// The margin is part of the string rather than an `x` offset so that a style range covers
    /// the row as it reads; padding the vector too is what keeps that true.
    pub fn indent(mut self, columns: usize) -> Self {
        if columns == 0 {
            return self;
        }
        let mut text = " ".repeat(columns);
        text.push_str(&self.text);
        self.text = text;
        if !self.styles.is_empty() {
            let mut styles = vec![MdStyle::default(); columns];
            styles.append(&mut self.styles);
            self.styles = styles;
        }
        self
    }

    /// Put `runs` in front of this line, keeping the styles aligned.
    ///
    /// Used for the labels the agents pane's timeline carries: `prompt: `, `summary: `. A line
    /// with no styles of its own grows a default one per character first, so the prefix's
    /// styling does not silently apply to the body.
    pub fn prepend(mut self, text: &str, style: MdStyle) -> Self {
        if text.is_empty() {
            return self;
        }
        if self.styles.is_empty() {
            self.styles = vec![MdStyle::default(); self.text.chars().count()];
        }
        let mut styles = vec![style; text.chars().count()];
        styles.append(&mut self.styles);
        self.styles = styles;
        self.text.insert_str(0, text);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// What a render may vary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Drop the blank line the renderer otherwise puts between two blocks.
    ///
    /// For a half-pane of eight rows, a paragraph break costs an eighth of what there is to
    /// read; a conversation with the whole pane can afford the air and leaves this off.
    pub compact_blocks: bool,
}

impl Options {
    /// Every block hard against the last — what a small pane wants.
    pub fn compact() -> Self {
        Options {
            compact_blocks: true,
        }
    }
}

/// Render `text` as markdown, wrapped to `cols` columns.
///
/// Never returns an empty `Vec`: text that renders to nothing is one empty line, so a caller
/// counting lines never has to special-case it.
pub fn render(text: &str, cols: usize, options: Options) -> Vec<MdLine> {
    wrap(&parse(text), cols, options)
}

/// The blocks `text` is made of, before anything knows how wide the rows will be.
///
/// Split out from [`render`] for a caller that has to see the shape of the first block before
/// it can pick a width - the agents pane's timeline decides where an entry's label goes by
/// whether that block carries a marker of its own, and parsing is the expensive half.
pub fn parse(text: &str) -> Vec<MdBlock> {
    parse_head(text, usize::MAX, usize::MAX).0
}

/// The first `max_blocks` of [`parse`]'s blocks, read from no more than the first `max_bytes`
/// of `text`, and whether that was the whole of it.
///
/// For a caller that only ever draws the head of a text - the agents pane's timeline shows a
/// few rows of each entry and opens the rest on demand. Both limits are needed. Stopping at a
/// block count saves the inline parse and the styled characters of everything after, but
/// pulldown-cmark reads the whole input for block structure before it yields a single event,
/// and one block can be the whole text: a 100KB code fence is one block. Cutting the input is
/// what bounds that.
///
/// Uncut, the blocks returned are exactly [`parse`]'s first ones. Cut, they are what the text
/// before the cut parses to, which differs from the whole's parse wherever markdown reads
/// ahead:
///
/// - a block the cut lands inside ends where the cut does;
/// - a table that crosses the cut sizes its columns from the rows before it, and a list that
///   crosses it is loose or tight by the items before it - which can restyle items that ended
///   well before the cut;
/// - a reference link whose definition is past the cut is resolved anyway, from a line scan
///   of the rest of the text rather than a parse of it: see [`tail_reference_definitions`].
///
/// The flag is `false` whenever something was left unread: the input was cut, or another
/// block had begun after the last one kept. A text of exactly `max_blocks` blocks is complete.
pub fn parse_head(text: &str, max_blocks: usize, max_bytes: usize) -> (Vec<MdBlock>, bool) {
    let cut = text.floor_char_boundary(max_bytes);
    let sanitized = sanitize(&text[..cut]);
    let mut renderer = MarkdownRenderer::default();
    let options = ParserOptions::ENABLE_TABLES
        | ParserOptions::ENABLE_STRIKETHROUGH
        | ParserOptions::ENABLE_TASKLISTS;
    // Scanned on the first reference the head cannot resolve, so a text with none - most of
    // them - never reads its tail at all. Uncut, the tail is empty and resolves nothing, as
    // the plain parser would not.
    let tail = &text[cut..];
    let mut definitions: Option<HashMap<String, String>> = None;
    let resolve = |link: BrokenLink<'_>| {
        let destination = definitions
            .get_or_insert_with(|| tail_reference_definitions(tail))
            .get(&normalize_label(&link.reference))?;
        Some((CowStr::from(sanitize(destination)), CowStr::from("")))
    };
    for event in Parser::new_with_broken_link_callback(&sanitized, options, Some(resolve)) {
        renderer.event(event);
        let began_another = renderer.current.is_some() || renderer.table.is_some();
        if renderer.blocks.len() > max_blocks
            || (renderer.blocks.len() == max_blocks && began_another)
        {
            renderer.blocks.truncate(max_blocks);
            return (renderer.blocks, false);
        }
    }
    // In case a stream ever ends inside a table - a reply is rendered while it is still
    // arriving, so half a table is the normal case, not the pathological one.
    renderer.finish_table();
    renderer.finish_block();
    let complete = cut == text.len() && renderer.blocks.len() <= max_blocks;
    renderer.blocks.truncate(max_blocks);
    (renderer.blocks, complete)
}

/// The link reference definitions in `tail` - the part of a text [`parse_head`] cut off - by
/// [`normalize_label`]ed label, the first of each label winning as it does in a parse.
///
/// A line scan, not a parse: `[label]: destination` at the start of a line, indented at most
/// three spaces, the destination either on the same line or alone on the next. Only the
/// destination is kept, since a title is never drawn. A definition written inside a code
/// fence past the cut is taken too, where a parse would ignore it - a line that shape in a
/// fence is rare, and the cost of being wrong is a link where the whole would have shown
/// brackets.
fn tail_reference_definitions(tail: &str) -> HashMap<String, String> {
    let mut definitions = HashMap::new();
    let mut lines = tail.lines().peekable();
    while let Some(line) = lines.next() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let Some(rest) = line[indent..].strip_prefix('[').filter(|_| indent <= 3) else {
            continue;
        };
        let Some((label, after)) = rest.split_once("]:") else {
            continue;
        };
        if label.trim().is_empty() || label.contains(['[', ']']) {
            continue;
        }
        let after = after.trim();
        let after = match after.is_empty() {
            true => match lines.peek() {
                Some(next) => next.trim(),
                None => continue,
            },
            false => after,
        };
        let destination = match after.strip_prefix('<') {
            Some(angled) => angled.split_once('>').map(|(inside, _)| inside),
            None => after.split_whitespace().next(),
        };
        if let Some(destination) = destination.filter(|found| !found.is_empty()) {
            definitions
                .entry(normalize_label(label))
                .or_insert_with(|| destination.to_owned());
        }
    }
    definitions
}

/// A link label as CommonMark matches it: case-insensitive, with runs of whitespace one space
/// and none at either end. Lowercasing stands in for full Unicode case folding.
fn normalize_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// [`parse`]'s blocks, wrapped to `cols` columns.
pub fn wrap(blocks: &[MdBlock], cols: usize, options: Options) -> Vec<MdLine> {
    if blocks.is_empty() {
        return vec![MdLine::blank()];
    }

    // ponytail: style metadata is one tiny Copy value per displayed character.
    // Replace it with ranges only if transcript memory shows up in a profile.
    let mut lines = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let separated = index > 0 && !(blocks[index - 1].compact && block.compact);
        if separated && !options.compact_blocks {
            lines.push(MdLine::blank());
        }
        lines.extend(render_block(block, cols));
    }
    lines
}

/// Apply a per-character style vector to `text`, as the `Text` API's ranges.
///
/// `dim` dims the whole line underneath whatever the styles asked for, which is how a caller
/// says "this is the record, not the conclusion" without flattening the colours markdown chose.
/// Runs are found rather than emitted per character: the API takes ranges, and one call per
/// character of a wrapped paragraph is a great many calls.
pub fn style_text(text: &str, styles: &[MdStyle], dim: bool) -> Text {
    style_over(Text::new(text), styles, dim)
}

/// [`style_text`], onto a `Text` a caller has already painted — the conversation colours a
/// line by who is speaking before markdown gets a say.
pub fn style_over(mut painted: Text, styles: &[MdStyle], dim: bool) -> Text {
    if dim && styles.is_empty() {
        return painted.dim_range(..);
    }
    let mut start = 0;
    while start < styles.len() {
        let style = styles[start];
        let end = (start + 1..styles.len())
            .find(|index| styles[*index] != style)
            .unwrap_or(styles.len());
        if !style.bold {
            painted = painted.unbold_range(start..end);
        }
        if let Some(color) = style.color {
            painted = painted.color_range(color, start..end);
        }
        if style.dim || dim {
            painted = painted.dim_range(start..end);
        }
        if style.error {
            painted = painted.error_color_range(start..end);
        }
        if style.strike {
            painted = painted.strike_range(start..end);
        }
        start = end;
    }
    painted
}

/// `text` as a markdown *value*: every character the renderer acts on backslash-escaped, so a
/// string we did not write cannot restyle the line it lands in.
///
/// Every caller is interpolating something from outside — a pane title, a branch, a todo, an
/// agent's own summary of a background task — into markdown source that this module will then
/// parse. Escaping here is what keeps that string one value rather than markup.
///
/// Not escaped, deliberately: `_`. An underscore inside a word is literal in CommonMark, so
/// escaping it would only turn every `pane_id` and `session_id` into `pane\_id`. The worst it
/// can do is a stray italic, against a backslash on every identifier we print.
///
/// `|` and `~` *are* on the list, because [`parse`] enables tables and strikethrough: a pipe in
/// a pane title is a cell boundary and a pair of tildes strikes out whatever is between them.
/// Both are ASCII punctuation, so `\|` and `\~` read as themselves wherever they land.
///
/// Lived in `agent::present::inline` while agent was the only crate producing markdown. The
/// agents pane's timeline produces it now — `agent_proto::envelope` builds a row out of what a
/// harness wrote at an agent — and one escaper beside the parser it escapes for is the same
/// move [`yinz_tile::text`] and this crate each record above.
pub fn escape_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '\\' | '`' | '*' | '[' | ']' | '<' | '>' | '#' | '|' | '~'
        ) {
            out.push('\\');
        }
        // A newline inside a value would end the list item or the table row it sits in.
        out.push(if ch == '\n' || ch == '\r' { ' ' } else { ch });
    }
    out
}

/// Escape literal palette display text, including block markers and HTML entities.
/// Newlines remain line breaks so callers can also protect plain text documents.
pub fn escape_literal(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    let mut leading = true;
    for ch in text.chars() {
        // Entities preserve indentation without opening a code block that ignores escapes.
        if leading && matches!(ch, ' ' | '\t') {
            escaped.push_str(if ch == ' ' { "&#32;" } else { "&#9;" });
            continue;
        }
        if ch.is_ascii_punctuation() {
            escaped.push('\\');
        }
        if ch == '\n' {
            // Keep blank lines inside one paragraph so Markdown cannot collapse them.
            escaped.push_str("&#10;");
        } else {
            escaped.push(ch);
        }
        leading = ch == '\n';
    }
    escaped
}

/// One block of markdown: a run of styled characters, and the prefixes its first and
/// subsequent rows carry — a list marker, a quote bar, the indent under either.
#[derive(Default)]
pub struct MdBlock {
    pub chars: Vec<StyledChar>,
    pub prefix: String,
    pub continuation: String,
    /// Whether this block sits hard against a neighbouring compact one. List items are
    /// compact; paragraphs and headings are not.
    pub compact: bool,
    /// Break at the column rather than between words — what a code block and a rule want.
    pub hard_wrap: bool,
    /// A horizontal rule: no text of its own, filled with `─` to whatever width it is wrapped
    /// to. Filled at wrap time rather than at parse time so that [`parse`] can run before the
    /// caller has settled on a width.
    pub rule: bool,
    /// A GFM table, laid out into columns at wrap time for the same reason a rule is filled
    /// there: the widths are a function of the width on offer, which [`parse`] does not know.
    /// `chars` is empty for such a block - the cells carry the text.
    ///
    /// Boxed because it is three `Vec`s against `MdBlock`'s two strings, and a transcript
    /// holds thousands of blocks of which approximately none are tables. `benches/transcript.rs`
    /// watches exactly that number.
    pub table: Option<Box<MdTable>>,
}

/// How a column's cells sit in the width the column is given.
///
/// Mirrors pulldown-cmark's `Alignment` rather than re-exporting it: every other public type
/// here is dependency-free, and neither `agent` nor `agents` depends on the parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MdAlign {
    /// No colon in the delimiter row. Reads left, which is what GFM says.
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// A parsed table, before anything knows how wide the columns can be.
///
/// Cells hold styled characters rather than lines, because which characters land on which row
/// is a function of the width - the same deferral [`MdBlock::rule`] makes.
#[derive(Debug, Clone, Default)]
pub struct MdTable {
    /// The header row's cells, empty for a table whose head parsed to nothing.
    pub head: Vec<Vec<StyledChar>>,
    pub rows: Vec<Vec<Vec<StyledChar>>>,
    pub align: Vec<MdAlign>,
}

impl MdTable {
    /// How many columns to draw: the widest of what the delimiter row promised, what the
    /// header carries, and what any body row carries. A ragged row keeps its extra cells -
    /// the cell is content, the header only a label.
    fn columns(&self) -> usize {
        self.rows
            .iter()
            .map(Vec::len)
            .chain(std::iter::once(self.head.len()))
            .chain(std::iter::once(self.align.len()))
            .max()
            .unwrap_or(0)
    }
}

impl From<Alignment> for MdAlign {
    fn from(alignment: Alignment) -> Self {
        match alignment {
            Alignment::None => MdAlign::None,
            Alignment::Left => MdAlign::Left,
            Alignment::Center => MdAlign::Center,
            Alignment::Right => MdAlign::Right,
        }
    }
}

struct ListState {
    next: Option<u64>,
    /// The continuation indent of the item currently open - what a block after the item's
    /// first one has to carry to stay under the bullet. Empty until the first item starts.
    item: String,
}

struct LinkState {
    start: usize,
    destination: String,
}

/// A table under construction. The header is just the first row, so the only real state is
/// the row being filled and the prefix the finished block will carry.
#[derive(Default)]
struct TableState {
    align: Vec<MdAlign>,
    head: Vec<Vec<StyledChar>>,
    rows: Vec<Vec<Vec<StyledChar>>>,
    row: Vec<Vec<StyledChar>>,
    in_head: bool,
    prefix: String,
    continuation: String,
    compact: bool,
}

/// Cells kept per row, and rows kept per table.
///
/// pulldown-cmark auto-completes a short row up to the header's width, capped at its own
/// `1 << 18` cells - so `|a|a|a|…|` with a wide delimiter row hands us a quarter of a million
/// `Vec<StyledChar>` from one line of a reply. These are the sizes past which a table has
/// stopped being something a person reads, and dropping the excess costs one comparison.
const MAX_TABLE_COLUMNS: usize = 32;
const MAX_TABLE_ROWS: usize = 512;

#[derive(Default)]
struct MarkdownRenderer {
    blocks: Vec<MdBlock>,
    current: Option<MdBlock>,
    styles: Vec<MdStyle>,
    lists: Vec<ListState>,
    links: Vec<LinkState>,
    quote_depth: usize,
    table: Option<TableState>,
    /// Where characters go while a table cell is open. The inline arms - emphasis, code,
    /// links - are the same ones prose uses, so redirecting the one place they all funnel
    /// through is what makes markup inside a cell work without a second set of them.
    cell: Option<Vec<StyledChar>>,
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

    /// Where characters go right now: the table cell being filled, otherwise the current
    /// block's run, starting a paragraph if there is not one yet.
    ///
    /// `None` only in the gap between two cells of a table, where stray inline text has
    /// nowhere sensible to land and starting a paragraph would clear the style stack
    /// mid-table.
    fn sink(&mut self) -> Option<&mut Vec<StyledChar>> {
        if self.cell.is_some() {
            return self.cell.as_mut();
        }
        if self.table.is_some() {
            return None;
        }
        if self.current.is_none() {
            self.start_paragraph();
        }
        self.current.as_mut().map(|block| &mut block.chars)
    }

    fn push_chars(&mut self, text: &str, style: MdStyle) {
        if let Some(sink) = self.sink() {
            sink.extend(text.chars().map(|ch| StyledChar { ch, style }));
        }
    }

    fn append(&mut self, text: &str) {
        let style = self.style();
        self.push_chars(text, style);
    }

    fn append_with_style(&mut self, text: &str, style: MdStyle) {
        self.push_chars(text, style);
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
        let continuation = " ".repeat(prefix.width());
        // Remembered for the blocks after this item's first one - a table in a loose item
        // arrives with nothing else left to inherit the indent from.
        if let Some(list) = self.lists.last_mut() {
            list.item = continuation.clone();
        }
        self.start_block(prefix, continuation, true, MdStyle::default());
    }

    /// Open a table, taking over the prefix of an empty block it displaces.
    ///
    /// A table inside a list item is preceded by that item's own block, empty when the item
    /// is nothing but the table; taking its bullet rather than finishing it is what stops a
    /// lone `• ` row appearing above the table. When that block has already been closed - a
    /// loose item whose paragraph came first - the item's own indent is what the table sits
    /// under instead. Otherwise the prefix is the quote bar, or nothing.
    fn start_table(&mut self, align: &[MdAlign]) {
        let inherited = match self.current.take() {
            Some(block) if block.chars.is_empty() => {
                Some((block.prefix, block.continuation, block.compact))
            },
            Some(block) => {
                self.blocks.push(block);
                None
            },
            None => None,
        };
        let indented = self
            .lists
            .last()
            .map(|list| list.item.clone())
            .filter(|item| !item.is_empty());
        let around = indented.unwrap_or_else(|| "│ ".repeat(self.quote_depth));
        let (prefix, continuation, compact) =
            inherited.unwrap_or((around.clone(), around, !self.lists.is_empty()));
        // Seeded here rather than by `start_block`, which a table never calls while it is
        // being filled: a header cell's `push_style`/`pop_style` pair needs something under
        // it, and `pop_style` refuses to empty the stack.
        self.styles.clear();
        self.styles.push(MdStyle::default());
        self.links.clear();
        self.table = Some(TableState {
            align: align.iter().copied().take(MAX_TABLE_COLUMNS).collect(),
            prefix,
            continuation,
            compact,
            ..TableState::default()
        });
    }

    /// Close the table, pushing it as one block.
    ///
    /// A table that parsed to no cells at all pushes nothing: an empty block would still
    /// cost the blank line [`wrap`] puts between two of them.
    fn finish_table(&mut self) {
        let Some(mut state) = self.table.take() else {
            return;
        };
        // A cell or a row still open belongs to the table: `parse` calls this on a stream
        // that ended mid-table, and losing the last row would make a streaming reply flicker
        // a row shorter than the one before it.
        if let Some(cell) = self.cell.take() {
            if state.row.len() < MAX_TABLE_COLUMNS {
                state.row.push(cell);
            }
        }
        if !state.row.is_empty() {
            let row = std::mem::take(&mut state.row);
            if state.in_head {
                state.head = row;
            } else if state.rows.len() < MAX_TABLE_ROWS {
                state.rows.push(row);
            }
        }
        if state.head.is_empty() && state.rows.is_empty() {
            self.styles.clear();
            return;
        }
        self.start_block(
            state.prefix,
            state.continuation,
            state.compact,
            MdStyle::default(),
        );
        self.current.as_mut().expect("just started").table = Some(Box::new(MdTable {
            head: state.head,
            rows: state.rows,
            align: state.align,
        }));
        self.finish_block();
    }

    fn start_link(&mut self, destination: &str) {
        // The sink is resolved before the style stack is touched: it may have to start a
        // paragraph, and starting a block clears the stack - which would drop the colour
        // pushed at the bottom of this.
        let start = self.sink().map(|sink| sink.len()).unwrap_or_default();
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
        // Off whichever sink the label was written to - a link inside a table cell reads
        // its own cell, not whatever block happens to be open.
        let written = match (&self.cell, &self.current) {
            (Some(cell), _) => Some(cell.as_slice()),
            (None, Some(block)) => Some(block.chars.as_slice()),
            (None, None) => None,
        };
        let label: String = written
            .and_then(|chars| chars.get(link.start..))
            .map(|tail| tail.iter().map(|styled| styled.ch).collect())
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

    fn event(&mut self, event: Event<'_>) {
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
                self.lists.push(ListState {
                    next: start,
                    item: String::new(),
                });
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
            Event::Start(Tag::Table(align)) => {
                let align: Vec<MdAlign> = align.iter().map(|a| MdAlign::from(*a)).collect();
                self.start_table(&align);
            },
            Event::End(TagEnd::Table) => self.finish_table(),
            Event::Start(Tag::TableHead) => {
                if let Some(table) = self.table.as_mut() {
                    table.in_head = true;
                    table.row.clear();
                }
            },
            Event::End(TagEnd::TableHead) => {
                if let Some(table) = self.table.as_mut() {
                    table.head = std::mem::take(&mut table.row);
                    table.in_head = false;
                }
            },
            Event::Start(Tag::TableRow) => {
                if let Some(table) = self.table.as_mut() {
                    table.row.clear();
                }
            },
            Event::End(TagEnd::TableRow) => {
                if let Some(table) = self.table.as_mut() {
                    let row = std::mem::take(&mut table.row);
                    if table.rows.len() < MAX_TABLE_ROWS {
                        table.rows.push(row);
                    }
                }
            },
            Event::Start(Tag::TableCell) => {
                self.cell = Some(Vec::new());
                // A header cell is made bold here rather than at layout time, so that
                // emphasis inside it composes and the layout never has to know which band
                // it is padding.
                if self.table.as_ref().is_some_and(|table| table.in_head) {
                    self.push_style(|style| style.bold = true);
                }
            },
            Event::End(TagEnd::TableCell) => {
                if self.table.as_ref().is_some_and(|table| table.in_head) {
                    self.pop_style();
                }
                let cell = self.cell.take().unwrap_or_default();
                if let Some(table) = self.table.as_mut() {
                    if table.row.len() < MAX_TABLE_COLUMNS {
                        table.row.push(cell);
                    }
                }
            },
            Event::Start(Tag::Strikethrough) => self.push_style(|style| style.strike = true),
            Event::End(TagEnd::Strikethrough) => self.pop_style(),
            Event::TaskListMarker(done) => self.append_with_style(
                if done { "☒ " } else { "☐ " },
                MdStyle {
                    dim: true,
                    ..MdStyle::default()
                },
            ),
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
                let block = self.current.as_mut().expect("just started");
                block.hard_wrap = true;
                block.rule = true;
                self.finish_block();
            },
            _ => {},
        }
    }
}

pub fn styled_width(chars: &[StyledChar]) -> usize {
    chars
        .iter()
        .map(|styled| styled.ch.width().unwrap_or(0))
        .sum()
}

pub fn hard_wrap_styled(chars: &[StyledChar], width: usize) -> Vec<Vec<StyledChar>> {
    hard_wrap_head(chars, width, usize::MAX)
}

/// [`hard_wrap_styled`]'s first `max_rows` rows, wrapping nothing past them.
fn hard_wrap_head(chars: &[StyledChar], width: usize, max_rows: usize) -> Vec<Vec<StyledChar>> {
    if max_rows == 0 {
        return Vec::new();
    }
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
            if lines.len() == max_rows {
                return lines;
            }
            continue;
        }
        let ch_width = styled.ch.width().unwrap_or(0);
        if line_width + ch_width > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            line_width = 0;
            if lines.len() == max_rows {
                return lines;
            }
        }
        line.push(styled.clone());
        line_width += ch_width;
    }
    lines.push(line);
    lines
}

pub fn word_wrap_styled(chars: &[StyledChar], width: usize) -> Vec<Vec<StyledChar>> {
    word_wrap_head(chars, width, usize::MAX)
}

/// [`word_wrap_styled`]'s first `max_rows` rows, wrapping nothing past them.
fn word_wrap_head(chars: &[StyledChar], width: usize, max_rows: usize) -> Vec<Vec<StyledChar>> {
    if max_rows == 0 {
        return Vec::new();
    }
    if width == 0 {
        return vec![Vec::new()];
    }
    let mut lines = Vec::new();
    // Split in place rather than copied out line by line first, so that a head of a long
    // block never touches the characters after it.
    for source in chars.split(|styled| styled.ch == '\n') {
        if lines.len() >= max_rows {
            break;
        }
        let mut line = Vec::new();
        let mut pending_spaces = Vec::new();
        let mut index = 0;
        while index < source.len() && lines.len() < max_rows {
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
                // One more than the rows still wanted: the last piece is the row the next word
                // may join, not a finished one.
                let wanted = max_rows.saturating_sub(lines.len()).saturating_add(1);
                let mut pieces = hard_wrap_head(token, width, wanted);
                line = pieces.pop().unwrap_or_default();
                lines.extend(pieces);
            }
        }
        // A row still open when the head filled up is one past it.
        if lines.len() < max_rows {
            lines.push(line);
        }
    }
    lines.truncate(max_rows);
    lines
}

/// How the furniture is painted: the quote bar, a rule, a table's borders. Dim is what says
/// "this is the pane talking, not the model".
const FURNITURE: MdStyle = MdStyle {
    bold: false,
    dim: true,
    color: None,
    error: false,
    strike: false,
};

/// A run of one character, styled — a border segment, a rule, a stretch of padding.
fn run(ch: char, count: usize, style: MdStyle) -> Vec<StyledChar> {
    vec![StyledChar { ch, style }; count]
}

/// Column widths that fit `available`, giving a narrow column all it asks for before a wide
/// one takes a share of what is left.
///
/// Water-filling rather than a flat proportional shrink, because the columns that make a
/// table readable are the narrow ones: squeezing an `id` column of width 2 down to 1 to save
/// a single column, while the description beside it is still fifty wide, helps nobody. Walk
/// the columns narrowest first, hand each `min(what it wants, an equal share of what is
/// left)`, and give the remainder back to whoever is still short. Ties break by index, so the
/// result does not depend on iteration order.
fn fit_columns(natural: &[usize], available: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..natural.len()).collect();
    order.sort_by_key(|index| (natural[*index], *index));

    let mut widths = vec![0; natural.len()];
    let mut budget = available;
    let mut left = natural.len();
    for index in order {
        let share = (budget / left).max(1);
        let width = natural[index].min(share);
        widths[index] = width;
        budget = budget.saturating_sub(width);
        left -= 1;
    }
    // Whatever the flooring left over goes back to the columns still short of what they
    // asked for, left to right - the leftmost column of a table is the one being read down.
    while budget > 0 {
        let starved: Vec<usize> = (0..natural.len())
            .filter(|index| widths[*index] < natural[*index])
            .collect();
        if starved.is_empty() {
            break;
        }
        for index in starved {
            if budget == 0 {
                break;
            }
            widths[index] += 1;
            budget -= 1;
        }
    }
    widths
}

/// One wrapped cell line, padded out to `width` on the side its alignment asks for.
///
/// Padded rather than trimmed: the rectangle is what makes the columns read as columns, and
/// the block is within `cols` by construction. Measured with [`styled_width`], so a line
/// ending on a wide character still lands on the column boundary.
fn pad_cell(line: &[StyledChar], width: usize, align: MdAlign, fill: MdStyle) -> Vec<StyledChar> {
    let pad = width.saturating_sub(styled_width(line));
    let (before, after) = match align {
        MdAlign::Right => (pad, 0),
        MdAlign::Center => (pad / 2, pad - pad / 2),
        MdAlign::None | MdAlign::Left => (0, pad),
    };
    let mut padded = run(' ', before, fill);
    padded.extend_from_slice(line);
    padded.extend(run(' ', after, fill));
    padded
}

/// The `│` between two columns, with a space either side.
const CELL_GAP: usize = 3;

/// A table too narrow for a grid, as one labelled record per row.
///
/// The header is spent on labels rather than drawn as a band of its own: at a width this
/// small a row of headings and the row of values under it share no column boundaries anyway,
/// and a value nothing names is not worth keeping. One cell per line, wrapped whole, with a
/// blank line between records; a table with no header is just its cells.
fn render_records(table: &MdTable, width: usize, max_rows: usize) -> Vec<Vec<StyledChar>> {
    let rows: Vec<&Vec<Vec<StyledChar>>> = if table.rows.is_empty() {
        // Nothing but a header: it is the only content there is, so it is drawn as the record
        // rather than as labels for one.
        std::iter::once(&table.head)
            .filter(|head| !head.is_empty())
            .collect()
    } else {
        table.rows.iter().collect()
    };
    let labels = (!table.rows.is_empty()).then_some(&table.head);

    let mut lines: Vec<Vec<StyledChar>> = Vec::new();
    for row in rows {
        if lines.len() >= max_rows {
            break;
        }
        let mut record: Vec<Vec<StyledChar>> = Vec::new();
        for (column, cell) in row.iter().enumerate() {
            if cell.is_empty() {
                continue;
            }
            // Enough for the head whatever the separator above it costs.
            if lines.len() + record.len() >= max_rows {
                break;
            }
            let mut line = Vec::new();
            if let Some(label) = labels
                .and_then(|head| head.get(column))
                .filter(|label| !label.is_empty())
            {
                line.extend_from_slice(label);
                line.extend(run(':', 1, FURNITURE));
                line.extend(run(' ', 1, FURNITURE));
            }
            line.extend_from_slice(cell);
            record.extend(word_wrap_head(&line, width, max_rows));
        }
        if record.is_empty() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(Vec::new());
        }
        lines.append(&mut record);
    }
    lines.truncate(max_rows);
    lines
}

/// A table's rows as styled characters, laid out to `width` columns.
///
/// Bodies without prefixes, exactly as the wrappers return, so [`render_block`] puts the
/// prefix and the continuation on them the same way it does for prose.
///
/// Only the first `max_rows` rows are laid out. The column widths still read every cell,
/// because a row further down can be the one that makes a column wide.
fn render_table(table: &MdTable, width: usize, max_rows: usize) -> Vec<Vec<StyledChar>> {
    let columns = table.columns();
    if columns == 0 {
        return Vec::new();
    }
    let chrome = CELL_GAP * (columns - 1);
    let available = width.saturating_sub(chrome);
    // Not enough room for even one column per column: the grid is abandoned rather than
    // overflowed, and the rows are read as records instead.
    if available < columns {
        return render_records(table, width.max(1), max_rows);
    }

    let natural: Vec<usize> = (0..columns)
        .map(|column| {
            std::iter::once(&table.head)
                .chain(table.rows.iter())
                .filter_map(|row| row.get(column))
                .map(|cell| styled_width(cell))
                .max()
                .unwrap_or(0)
                .max(1)
        })
        .collect();
    // Only shrunk, never stretched. A two-column table spread across a hundred columns reads
    // worse than a compact one, and nothing else here fills the width except a rule, which
    // does it on purpose.
    let widths = if natural.iter().sum::<usize>() <= available {
        natural
    } else {
        fit_columns(&natural, available)
    };
    let align = |column: usize| table.align.get(column).copied().unwrap_or_default();

    let mut lines = Vec::new();
    let head = (!table.head.is_empty()).then_some(&table.head);
    for (is_head, row) in head
        .map(|head| (true, head))
        .into_iter()
        .chain(table.rows.iter().map(|row| (false, row)))
    {
        if lines.len() >= max_rows {
            break;
        }
        // The header band's padding is bold too, so the row comes out as one style run
        // rather than alternating with the gaps - `style_over` emits a range call per run.
        let fill = MdStyle {
            bold: is_head,
            ..MdStyle::default()
        };
        let wrapped: Vec<Vec<Vec<StyledChar>>> = (0..columns)
            .map(|column| match row.get(column) {
                Some(cell) => word_wrap_head(cell, widths[column], max_rows - lines.len()),
                None => vec![Vec::new()],
            })
            .collect();
        let height = wrapped
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(1)
            .clamp(1, max_rows - lines.len());
        for band in 0..height {
            let mut line = Vec::new();
            for column in 0..columns {
                if column > 0 {
                    line.extend(run(' ', 1, fill));
                    line.extend(run('│', 1, FURNITURE));
                    line.extend(run(' ', 1, fill));
                }
                let empty = Vec::new();
                let cell = wrapped[column].get(band).unwrap_or(&empty);
                line.extend(pad_cell(cell, widths[column], align(column), fill));
            }
            lines.push(line);
        }
        // The one rule the table draws, and only under a header: the columns below it are
        // already told apart by their bars.
        if is_head {
            let mut rule = Vec::new();
            for (column, column_width) in widths.iter().enumerate() {
                if column > 0 {
                    rule.extend(run('─', 1, FURNITURE));
                    rule.extend(run('┼', 1, FURNITURE));
                    rule.extend(run('─', 1, FURNITURE));
                }
                rule.extend(run('─', *column_width, FURNITURE));
            }
            lines.push(rule);
        }
    }
    lines.truncate(max_rows);
    lines
}

/// Wrap one block to `cols`, giving the first row its prefix and the rest the continuation.
pub fn render_block(block: &MdBlock, cols: usize) -> Vec<MdLine> {
    render_block_head(block, cols, usize::MAX)
}

/// [`render_block`]'s first `max_rows` rows, and the same rows it would give: nothing past
/// them is wrapped.
///
/// For a caller that draws only the head of a block. The agents pane's timeline caps an entry
/// at a few rows, and one block can be the whole of a 100KB entry, so wrapping all of it only
/// to keep four rows costs the whole entry on every resize step.
pub fn render_block_head(block: &MdBlock, cols: usize, max_rows: usize) -> Vec<MdLine> {
    if max_rows == 0 {
        return Vec::new();
    }
    if block.rule {
        let text = "─".repeat(cols);
        let styles = vec![FURNITURE; text.chars().count()];
        return vec![MdLine { text, styles }];
    }
    let body_width = cols.saturating_sub(block.prefix.width()).max(1);
    let bodies = match &block.table {
        Some(table) => render_table(table, body_width, max_rows),
        None if block.hard_wrap => hard_wrap_head(&block.chars, body_width, max_rows),
        None => word_wrap_head(&block.chars, body_width, max_rows),
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
            let mut text = prefix.clone();
            let mut styles = vec![FURNITURE; prefix.chars().count()];
            text.extend(body.iter().map(|styled| styled.ch));
            styles.extend(body.into_iter().map(|styled| styled.style));
            MdLine { text, styles }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[MdLine]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    /// The escaper's whole contract in one pass: every character the parser acts on comes
    /// back inert, an identifier comes back intact, and a newline cannot end the row the
    /// value is sitting in. Round-tripped through [`render`] as well as compared, because
    /// what matters is not the backslashes but that nothing survives them as markup.
    #[test]
    fn escape_inline_neutralises_every_character_the_renderer_acts_on() {
        assert_eq!(escape_inline("pane_id"), "pane_id");
        assert_eq!(escape_inline("a|b~~c~~"), r"a\|b\~\~c\~\~");
        assert_eq!(escape_inline("a `b` c"), r"a \`b\` c");
        assert_eq!(escape_inline("one\ntwo"), "one two");

        let hostile = "# **x** ``` | y <b> [z](w) ~~struck~~";
        let lines = render(&escape_inline(hostile), 200, Options::default());
        assert_eq!(texts(&lines), [hostile], "nothing survived as markup");
    }

    /// The whole dialect in one pass: a heading is bold, emphasis and code take colours,
    /// a list gets bullets, a quote gets a bar, a link keeps its destination, and a rule
    /// fills the width.
    #[test]
    fn markdown_renders_as_terminal_blocks() {
        let source = "# Heading\n\nA **bold** and *slanted* `value`.\n\n- one\n- two\n\n> quoted\n\n[site](https://example.com)\n\n---";
        let lines = render(source, 80, Options::default());
        let rendered = texts(&lines).join("\n");

        assert!(rendered.contains("Heading"), "{rendered}");
        assert!(!rendered.contains('#'), "{rendered}");
        assert!(rendered.contains("A bold and slanted value."), "{rendered}");
        assert!(!rendered.contains("**"), "{rendered}");
        assert!(rendered.contains("• one"), "{rendered}");
        assert!(rendered.contains("• two"), "{rendered}");
        assert!(rendered.contains("│ quoted"), "{rendered}");
        assert!(
            rendered.contains("site (https://example.com)"),
            "{rendered}"
        );
        assert!(rendered.contains(&"─".repeat(80)), "{rendered}");

        // and the styles are one per character of the line they belong to
        for line in &lines {
            if line.styles.is_empty() {
                continue;
            }
            assert_eq!(line.styles.len(), line.text.chars().count(), "{line:?}");
        }
    }

    /// The heading is bold and coloured, and the word inside `**` is bold while the prose
    /// around it is not - the thing markdown is actually for.
    #[test]
    fn emphasis_lands_on_the_characters_it_covers() {
        let lines = render("# Title\n\nplain **loud** plain", 40, Options::default());
        let heading = lines.iter().find(|line| line.text == "Title").unwrap();
        assert!(heading.styles.iter().all(|style| style.bold));

        let body = lines
            .iter()
            .find(|line| line.text.starts_with("plain loud"))
            .unwrap();
        let bold: String = body
            .text
            .chars()
            .zip(&body.styles)
            .filter(|(_, style)| style.bold)
            .map(|(ch, _)| ch)
            .collect();
        assert_eq!(bold, "loud");
    }

    /// A small pane pays for a paragraph break in the only rows it has, so it asks for the
    /// blocks hard against each other. The lines themselves are the same either way.
    #[test]
    fn compact_blocks_drops_the_air_between_them() {
        let source = "first para\n\nsecond para";
        let roomy = render(source, 40, Options::default());
        let tight = render(source, 40, Options::compact());

        assert_eq!(texts(&roomy), ["first para", "", "second para"]);
        assert_eq!(texts(&tight), ["first para", "second para"]);
    }

    /// Text that renders to nothing is still one line: a caller counting rows never has to
    /// special-case the empty answer.
    #[test]
    fn empty_input_is_one_empty_line() {
        assert_eq!(render("", 40, Options::default()), vec![MdLine::blank()]);
    }

    /// An escape sequence in a model's reply must not reach the parser, let alone the pane.
    #[test]
    fn control_characters_never_reach_the_parser() {
        let lines = render("a\u{1b}[31mb", 40, Options::default());
        assert_eq!(texts(&lines), ["a[31mb"]);
    }

    /// Wrapping is by column rather than by character, and the emphasis spanning a break
    /// survives it.
    #[test]
    fn a_wrapped_block_keeps_its_styles_on_both_rows() {
        let lines = render("**one two three four five six**", 10, Options::default());
        assert!(lines.len() > 1, "{lines:?}");
        for line in &lines {
            assert!(line.text.width() <= 10, "{line:?}");
            assert!(line.styles.iter().all(|style| style.bold), "{line:?}");
        }
    }

    /// The margin a caller adds is part of the string, so the vector has to grow with it or
    /// every range after it would be off by one.
    #[test]
    fn indent_and_prepend_keep_the_styles_aligned() {
        let line = MdLine {
            text: "body".to_owned(),
            styles: vec![
                MdStyle {
                    bold: true,
                    ..MdStyle::default()
                };
                4
            ],
        }
        .indent(1)
        .prepend(
            "label: ",
            MdStyle {
                dim: true,
                ..MdStyle::default()
            },
        );

        assert_eq!(line.text, "label:  body");
        assert_eq!(line.styles.len(), line.text.chars().count());
        assert!(line.styles[0].dim, "the label");
        assert!(!line.styles[7].bold, "the margin");
        assert!(line.styles[8].bold, "the body");

        // and a line with no styles of its own does not inherit the prefix's
        let plain = MdLine::plain("body").prepend(
            "x",
            MdStyle {
                dim: true,
                ..MdStyle::default()
            },
        );
        assert_eq!(plain.styles.len(), 5);
        assert!(plain.styles[0].dim);
        assert!(!plain.styles[1].dim);
    }
    /// A table's source pipes are gone and its cells sit under their headers, separated by a
    /// bar, with one rule under the header and none between the rows.
    #[test]
    fn a_table_becomes_aligned_columns() {
        let source = "| Pane | Command | Status |\n|---|---|---|\n| 1 | zsh | running |\n| 12 | cargo | exited |";
        let lines = render(source, 60, Options::default());
        assert_eq!(
            texts(&lines),
            [
                "Pane │ Command │ Status ",
                "─────┼─────────┼────────",
                "1    │ zsh     │ running",
                "12   │ cargo   │ exited ",
            ]
        );
        for line in &lines {
            assert_eq!(line.styles.len(), line.text.chars().count(), "{line:?}");
        }
    }

    /// The header is bold and every border character is dim - the same weight the quote bar
    /// and the horizontal rule are drawn at.
    #[test]
    fn the_header_is_bold_and_the_borders_are_dim() {
        let lines = render("| a | b |\n|---|---|\n| 1 | 2 |", 40, Options::default());
        // Bold everywhere but the bar, padding included, so the row is one style run.
        let header = &lines[0];
        assert!(
            header
                .text
                .chars()
                .zip(&header.styles)
                .all(|(ch, style)| style.bold || ch == '│'),
            "{header:?}"
        );
        let rule = &lines[1];
        assert!(rule.styles.iter().all(|style| style.dim), "{rule:?}");

        let body = &lines[2];
        let bars: Vec<MdStyle> = body
            .text
            .chars()
            .zip(&body.styles)
            .filter(|(ch, _)| *ch == '│')
            .map(|(_, style)| *style)
            .collect();
        assert!(!bars.is_empty());
        assert!(
            bars.iter().all(|style| style.dim && !style.bold),
            "{body:?}"
        );
    }

    /// Too wide for the pane: the columns shrink and a long cell wraps inside its own column
    /// rather than being cut off. Every row still ends at the same place.
    #[test]
    fn a_wide_table_shrinks_and_wraps_its_cells() {
        let source =
            "| id | what |\n|---|---|\n| 1 | a rather long description that will not fit |";
        let lines = render(source, 24, Options::default());
        assert!(lines.len() > 3, "{lines:?}");
        for line in &lines {
            assert!(line.text.width() <= 24, "{line:?}");
        }
        // Nothing is lost: every word of the long cell is somewhere in the output.
        let rendered = texts(&lines).join(" ");
        for word in "a rather long description that will not fit".split(' ') {
            assert!(rendered.contains(word), "{word} missing from {rendered}");
        }
        // The narrow column keeps what it asked for rather than being squeezed with the wide
        // one - which is the point of fitting the columns narrowest first.
        assert!(lines[0].text.starts_with("id │"), "{:?}", lines[0]);
    }

    /// The delimiter row's colons decide which side the padding falls on.
    #[test]
    fn alignment_puts_the_pad_where_the_delimiter_asked() {
        let lines = render(
            "| l | c | r |\n|:---|:---:|---:|\n| x | x | x |",
            40,
            Options::default(),
        );
        // Columns are four wide, from the header's `---:` row being three and the cells one.
        assert_eq!(texts(&lines)[2], "x │ x │ x");

        let lines = render(
            "| left | mid | right |\n|:---|:---:|---:|\n| x | x | x |",
            40,
            Options::default(),
        );
        assert_eq!(texts(&lines)[2], "x    │  x  │     x");
    }

    /// A row with fewer cells than the header keeps its separators, and one with more keeps
    /// the extra cell: the cell is content, the header only a label.
    #[test]
    fn a_ragged_row_is_padded_rather_than_dropped() {
        let lines = render(
            "| a | b | c |\n|---|---|---|\n| 1 |\n| 1 | 2 | 3 |",
            40,
            Options::default(),
        );
        let short = texts(&lines)[2];
        assert_eq!(short.matches('│').count(), 2, "{short}");
        for line in &lines {
            assert_eq!(line.text.width(), lines[0].text.width(), "{line:?}");
        }
    }

    /// A header with no rows under it is still a table, and a table with nothing in it is no
    /// block at all - an empty block would still cost the blank line between two of them.
    #[test]
    fn a_header_only_table_draws_and_an_empty_one_does_not() {
        let lines = render("| a | b |\n|---|---|", 40, Options::default());
        assert_eq!(texts(&lines), ["a │ b", "──┼──"]);

        // A delimiter row on its own parses as prose, not as an empty table.
        let lines = render("text\n\n|---|---|\n\ntext", 40, Options::default());
        assert!(
            texts(&lines).iter().all(|text| !text.contains('┼')),
            "{lines:?}"
        );
    }

    /// Narrower than one column per column: the grid is abandoned rather than overflowed, and
    /// each row is read as a record of labelled cells. Nothing runs past the edge, no cell
    /// text is lost, and every value still says which column it came from.
    #[test]
    fn a_table_too_narrow_for_its_columns_falls_back_to_records() {
        let lines = render(
            "| aaa | bbb | ccc |\n|---|---|---|\n| 111 | 222 | 333 |",
            8,
            Options::default(),
        );
        assert_eq!(
            texts(&lines),
            ["aaa: 111", "bbb: 222", "ccc: 333"],
            "{lines:?}"
        );

        // Two records are told apart by a blank line, and a value too wide for the width is
        // wrapped rather than truncated.
        let lines = render(
            "| aaa | bbb |\n|---|---|\n| 111 | 222 |\n| a long value | 333 |",
            4,
            Options::default(),
        );
        let rendered = texts(&lines).join(" ");
        for cell in ["aaa", "bbb", "111", "222", "long", "333"] {
            assert!(rendered.contains(cell), "{cell} missing from {rendered}");
        }
        assert!(texts(&lines).contains(&""), "{lines:?}");
        for line in &lines {
            assert!(line.text.width() <= 4, "{line:?}");
        }
    }

    /// A table inside a quote carries the bar on every row of it, and one inside a list item
    /// takes that item's bullet once rather than leaving an empty marker above itself.
    #[test]
    fn a_table_inherits_the_prefix_of_where_it_sits() {
        let lines = render(
            "> | a | b |\n> |---|---|\n> | 1 | 2 |",
            40,
            Options::default(),
        );
        assert_eq!(texts(&lines), ["│ a │ b", "│ ──┼──", "│ 1 │ 2"]);

        let lines = render(
            "- | a | b |\n  |---|---|\n  | 1 | 2 |",
            40,
            Options::default(),
        );
        assert_eq!(texts(&lines), ["• a │ b", "  ──┼──", "  1 │ 2"]);

        // A table that is not the item's first block has no empty bullet to take over, but it
        // still belongs to the item: it sits under the text rather than back at column zero.
        let lines = render(
            "- text\n\n  | a | b |\n  |---|---|\n  | 1 | 2 |",
            40,
            Options::default(),
        );
        assert_eq!(
            texts(&lines),
            ["• text", "  a │ b", "  ──┼──", "  1 │ 2"],
            "{lines:?}"
        );
    }

    /// A cell is prose: the markup inside it is read the same way it is anywhere else, and
    /// the emphasis lands on the characters it covers rather than on the whole cell.
    #[test]
    fn markup_inside_a_cell_is_read_as_markup() {
        let lines = render(
            "| what | where |\n|---|---|\n| a **loud** word | [site](https://example.com) |",
            60,
            Options::default(),
        );
        let body = &lines[2];
        assert!(!body.text.contains("**"), "{body:?}");
        assert!(body.text.contains("site (https://example.com)"), "{body:?}");
        let bold: String = body
            .text
            .chars()
            .zip(&body.styles)
            .filter(|(_, style)| style.bold)
            .map(|(ch, _)| ch)
            .collect();
        assert_eq!(bold, "loud");
    }

    /// Columns are measured in cells, not characters, so a wide character does not skew every
    /// column to its right.
    #[test]
    fn wide_characters_in_a_cell_do_not_skew_the_column() {
        let lines = render(
            "| a | b |\n|---|---|\n| 日本語 | x |",
            40,
            Options::default(),
        );
        for line in &lines {
            assert_eq!(line.text.width(), lines[0].text.width(), "{line:?}");
        }
    }

    /// A pipe with no delimiter row under it is prose, and stays prose. This is the guard on
    /// turning the table extension on at all.
    #[test]
    fn pipes_outside_a_table_are_still_prose() {
        assert_eq!(texts(&render("a | b", 40, Options::default())), ["a | b"]);
    }

    /// Strikethrough marks the characters it covers and nothing around them, the way every
    /// other span does.
    #[test]
    fn strikethrough_marks_the_characters_it_covers() {
        let lines = render("plain ~~gone~~ plain", 40, Options::default());
        let line = &lines[0];
        assert_eq!(line.text, "plain gone plain");
        let struck: String = line
            .text
            .chars()
            .zip(&line.styles)
            .filter(|(_, style)| style.strike)
            .map(|(ch, _)| ch)
            .collect();
        assert_eq!(struck, "gone");
    }

    /// A task list draws its boxes in the body, so the bullet and the hanging indent the list
    /// already has are left alone.
    #[test]
    fn a_task_list_draws_its_boxes() {
        let lines = render("- [x] done\n- [ ] todo", 40, Options::default());
        assert_eq!(texts(&lines), ["• ☒ done", "• ☐ todo"]);
    }

    /// One of each shape a block can take, and several of them long enough that a head is a
    /// small part of the whole: prose with a word too long for any row, a code block, a table
    /// wide enough to shrink, the same table too narrow for a grid, a quote, a list and a rule.
    fn every_shape() -> String {
        let prose = "word ".repeat(60) + &"x".repeat(90) + " tail";
        let code = (0..30)
            .map(|n| format!("line {n} of the fence"))
            .collect::<Vec<_>>();
        let rows = (0..20)
            .map(|n| format!("| {n} | {} |", "cell text ".repeat(n % 4 + 1)))
            .collect::<Vec<_>>();
        format!(
            "# Heading\n\n{prose}\n\n```\n{}\n```\n\n| id | description |\n|---|---|\n{}\n\n> quoted {prose}\n\n- one\n- {prose}\n\n---\n\nend",
            code.join("\n"),
            rows.join("\n"),
        )
    }

    /// A head is the same rows the whole block starts with, for every shape and at widths from
    /// one too narrow for a table's grid to wider than any row - so a caller that draws only
    /// the head draws exactly what it would have cut the whole down to.
    #[test]
    fn a_blocks_head_is_the_start_of_the_whole_block() {
        let blocks = parse(&every_shape());
        for cols in [1, 3, 7, 20, 41, 200] {
            for block in &blocks {
                let whole = render_block(block, cols);
                for max_rows in [0, 1, 2, 3, 5, 8, whole.len(), whole.len() + 3] {
                    let head = render_block_head(block, cols, max_rows);
                    assert_eq!(
                        head,
                        whole[..max_rows.min(whole.len())],
                        "{max_rows} rows of {:?} at {cols}",
                        whole.first().map(|line| &line.text)
                    );
                }
            }
        }
    }

    /// The public wrappers still wrap the whole of what they are given.
    #[test]
    fn the_wrappers_still_wrap_everything() {
        let chars: Vec<StyledChar> = "ab cd\nef gh"
            .chars()
            .map(|ch| StyledChar {
                ch,
                style: MdStyle::default(),
            })
            .collect();
        let rows = |wrapped: Vec<Vec<StyledChar>>| -> Vec<String> {
            wrapped
                .iter()
                .map(|row| row.iter().map(|styled| styled.ch).collect())
                .collect()
        };
        assert_eq!(rows(word_wrap_styled(&chars, 2)), ["ab", "cd", "ef", "gh"]);
        assert_eq!(
            rows(hard_wrap_styled(&chars, 3)),
            ["ab ", "cd", "ef ", "gh"]
        );
    }

    /// A head of a text is the whole parse's first blocks, and says whether there was more.
    #[test]
    fn a_parsed_head_is_the_start_of_the_whole_parse() {
        let text = every_shape();
        let whole = parse(&text);
        let drawn = |blocks: &[MdBlock]| -> Vec<Vec<MdLine>> {
            blocks.iter().map(|block| render_block(block, 30)).collect()
        };
        for max_blocks in 0..=whole.len() + 1 {
            let (head, complete) = parse_head(&text, max_blocks, usize::MAX);
            let kept = max_blocks.min(whole.len());
            assert_eq!(drawn(&head), drawn(&whole[..kept]), "{max_blocks} blocks");
            assert_eq!(complete, max_blocks >= whole.len(), "{max_blocks} blocks");
        }
    }

    /// The byte cap bounds the parse of a text that is one enormous block, and a text that was
    /// cut is never called complete - even though what was kept is a single block, as the
    /// whole would have been.
    #[test]
    fn a_cut_text_is_parsed_only_as_far_as_the_cut() {
        let fence = format!("```\n{}\n```", "é".repeat(100_000));
        let (blocks, complete) = parse_head(&fence, 5, 1_001);
        assert!(!complete);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].hard_wrap, "still a code block");
        // A cut in the middle of a two-byte character lands before it.
        assert!(blocks[0].chars.len() < 500);

        let (blocks, complete) = parse_head("short", 5, 1_001);
        assert!(complete);
        assert_eq!(blocks.len(), 1);
        // A cap at exactly the length cuts nothing.
        assert!(parse_head("short", 5, 5).1);
    }

    /// A reference link in the head whose definition the cut left behind draws as the whole
    /// parse draws it - full, collapsed and shortcut references, a label matched regardless of
    /// case and spacing, and a destination on the line after its label - while one defined
    /// nowhere still draws as the brackets it was written as.
    #[test]
    fn a_head_resolves_references_defined_past_the_cut() {
        let filler = "filler ".repeat(200);
        for (reference, definition) in [
            ("[docs][1]", "[1]: https://example.com/one"),
            ("[docs][]", "[DOCS]: https://example.com/collapsed"),
            (
                "[The  Docs]",
                "  [the docs]:\n  <https://example.com/shortcut> \"a title\"",
            ),
            ("[nope][9]", "[8]: https://example.com/other"),
        ] {
            let text = format!("See {reference} here.\n\n{filler}\n\n{definition}\n");
            let (head, complete) = parse_head(&text, 5, 64);
            assert!(!complete);
            let whole = parse(&text);
            assert_eq!(
                render_block(&head[0], 80),
                render_block(&whole[0], 80),
                "{reference}"
            );
        }
        let (head, _) = parse_head("See [nope][9].\n\nfiller\n\n[8]: https://x", 5, 20);
        assert!(render_block(&head[0], 80)[0].text.contains("[nope][9]"));
    }
}

#[cfg(test)]
#[path = "unit/literal_tests.rs"]
mod literal_tests;

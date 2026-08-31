//! Markdown to styled terminal lines.
//!
//! Lived in `hocus::ui` while the conversation was the only pane that drew a model's prose.
//! The agents pane's history half draws the same material now — prompts, plans and the
//! `last_assistant_message` a run ends on are raw markdown by the time they reach a pane — and
//! one renderer reading one dialect is the whole point of moving it here, the same move
//! [`crate::text`] records for the wrapper below it.
//!
//! The shape is: parse with pulldown-cmark into [`MdBlock`]s, each a run of styled characters
//! plus the prefix its first and later rows carry; wrap each block to the width on offer; hand
//! back [`MdLine`]s, one [`MdStyle`] per displayed character. [`style_text`] is the last step,
//! collapsing that vector into the range calls the `Text` API takes.
//!
//! The per-character style vector is deliberate and is the expensive part — see
//! `hocus/benches/transcript.rs`, which calls this by far the most expensive thing a render
//! does. Callers that redraw on a timer cache the result rather than parsing again.

use pulldown_cmark::{Event, Options as ParserOptions, Parser, Tag, TagEnd};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zellij_tile::prelude::Text;

use crate::text::sanitize;

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
/// Crate-neutral on purpose. `hocus` wraps this in a `VLine` that also carries the speaking
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
    let sanitized = sanitize(text);
    let mut renderer = MarkdownRenderer::default();
    for event in Parser::new_ext(&sanitized, ParserOptions::empty()) {
        renderer.event(event, cols);
    }
    renderer.finish_block();

    if renderer.blocks.is_empty() {
        return vec![MdLine::blank()];
    }

    // ponytail: style metadata is one tiny Copy value per displayed character.
    // Replace it with ranges only if transcript memory shows up in a profile.
    let mut lines = Vec::new();
    for (index, block) in renderer.blocks.iter().enumerate() {
        let separated = index > 0 && !(renderer.blocks[index - 1].compact && block.compact);
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
        start = end;
    }
    painted
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

pub fn styled_width(chars: &[StyledChar]) -> usize {
    chars
        .iter()
        .map(|styled| styled.ch.width().unwrap_or(0))
        .sum()
}

pub fn hard_wrap_styled(chars: &[StyledChar], width: usize) -> Vec<Vec<StyledChar>> {
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

pub fn word_wrap_styled(chars: &[StyledChar], width: usize) -> Vec<Vec<StyledChar>> {
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

/// Wrap one block to `cols`, giving the first row its prefix and the rest the continuation.
pub fn render_block(block: &MdBlock, cols: usize) -> Vec<MdLine> {
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
}

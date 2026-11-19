//! HTML for untrusted Markdown. Raw markup is text; images never fetch resources.
use std::fmt;

use pulldown_cmark::{html, Alignment, CowStr, Event, Options, Parser, Tag, TagEnd};

/// Conservative browser URL policy. Entity-looking input, controls, whitespace,
/// backslashes and authority-relative URLs are refused rather than normalized twice.
pub fn safe_link(url: &str) -> bool {
    if url.is_empty()
        || url.starts_with("//")
        || url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return false;
    }
    let head = url.split(['/', '?', '#']).next().unwrap_or_default();
    if head.contains('&') {
        return false;
    }
    match head.split_once(':') {
        Some((scheme, _)) => matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "mailto"
        ),
        None => true,
    }
}

struct TableState {
    alignments: Vec<Alignment>,
    in_head: bool,
    in_body: bool,
    col: usize,
}

fn table_cell_open(is_head: bool, alignment: Alignment) -> &'static str {
    match (is_head, alignment) {
        (true, Alignment::None) => "<th scope=\"col\">",
        (true, Alignment::Left) => "<th scope=\"col\" class=\"align-left\">",
        (true, Alignment::Center) => "<th scope=\"col\" class=\"align-center\">",
        (true, Alignment::Right) => "<th scope=\"col\" class=\"align-right\">",
        (false, Alignment::None) => "<td>",
        (false, Alignment::Left) => "<td class=\"align-left\">",
        (false, Alignment::Center) => "<td class=\"align-center\">",
        (false, Alignment::Right) => "<td class=\"align-right\">",
    }
}

/// Extra slack on the pre-parse table HTML lower bound so exact-budget callers
/// cannot trip on non-table lines that interrupt a table without a blank line,
/// while still capping `pulldown-cmark`'s eager first-pass tree allocation.
const TABLE_PRECHECK_SLACK_BYTES: usize = 64 * 1024;
const MARKDOWN_OPTIONS: Options = Options::ENABLE_STRIKETHROUGH.union(Options::ENABLE_TABLES);

fn min_cell_bytes(alignment: Alignment) -> usize {
    match alignment {
        Alignment::None => "<td></td>".len(),
        Alignment::Left => "<td class=\"align-left\"></td>".len(),
        Alignment::Center => "<td class=\"align-center\"></td>".len(),
        Alignment::Right => "<td class=\"align-right\"></td>".len(),
    }
}

fn strip_table_containers_with_depth(mut line: &[u8]) -> (usize, usize, &[u8]) {
    let mut quote_depth = 0usize;
    let mut spaces = 0usize;
    while let Some(&first) = line.first() {
        match first {
            b' ' => {
                spaces = spaces.saturating_add(1);
                line = &line[1..];
            },
            b'\t' => {
                spaces = spaces.saturating_add(4);
                line = &line[1..];
            },
            b'>' => {
                quote_depth = quote_depth.saturating_add(1);
                spaces = 0;
                line = &line[1..];
            },
            _ => break,
        }
    }
    (quote_depth, spaces, line)
}

fn starts_ordered_list_item(spaces: usize, content: &[u8]) -> bool {
    if spaces > 3 {
        return false;
    }
    let digits = content.iter().take_while(|b| b.is_ascii_digit()).count();
    if !(1..=9).contains(&digits) {
        return false;
    }
    matches!(content.get(digits), Some(b'.' | b')'))
        && matches!(content.get(digits + 1), None | Some(b' ' | b'\t'))
}

fn is_thematic_break(spaces: usize, content: &[u8]) -> bool {
    if spaces > 3 {
        return false;
    }
    let Some(&marker) = content.first() else {
        return false;
    };
    if !matches!(marker, b'-' | b'*' | b'_') {
        return false;
    }
    let mut count = 0usize;
    for &byte in content {
        if byte == marker {
            count += 1;
        } else if !matches!(byte, b' ' | b'\t') {
            return false;
        }
    }
    count >= 3
}

/// Mirror `pulldown-cmark`'s `scan_table_head` grammar to count columns and the
/// minimum HTML bytes each row of the table must emit (`<tr></tr>\n` plus empty
/// cells with the column's alignment class).
fn scan_table_delimiter(line: &[u8]) -> Option<(usize, usize)> {
    let mut bytes = line;
    let mut cols = 0usize;
    let mut min_row_bytes = "<tr></tr>\n".len();
    let mut active_col = Alignment::None;
    let mut start_col = true;
    let mut found_pipe = false;
    let mut found_hyphen = false;
    let mut found_hyphen_in_col = false;
    if bytes.first() == Some(&b'|') {
        bytes = &bytes[1..];
        found_pipe = true;
    }
    for &byte in bytes {
        match byte {
            b' ' => {},
            b':' => {
                active_col = match (start_col, active_col) {
                    (true, Alignment::None) => Alignment::Left,
                    (false, Alignment::Left) => Alignment::Center,
                    (false, Alignment::None) => Alignment::Right,
                    _ => active_col,
                };
                start_col = false;
            },
            b'-' => {
                start_col = false;
                found_hyphen = true;
                found_hyphen_in_col = true;
            },
            b'|' => {
                if !found_hyphen_in_col {
                    return None;
                }
                start_col = true;
                found_pipe = true;
                cols = cols.saturating_add(1);
                min_row_bytes = min_row_bytes.saturating_add(min_cell_bytes(active_col));
                active_col = Alignment::None;
                found_hyphen_in_col = false;
            },
            _ => return None,
        }
    }
    if !start_col {
        cols = cols.saturating_add(1);
        min_row_bytes = min_row_bytes.saturating_add(min_cell_bytes(active_col));
    }
    if !found_pipe || !found_hyphen || cols == 0 {
        return None;
    }
    Some((cols, min_row_bytes))
}

/// Split bytes on all CommonMark line endings (`\r\n`, `\n`, and bare `\r`),
/// matching `pulldown-cmark`'s `scan_eol`.
fn commonmark_lines(mut bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::from_fn(move || {
        if bytes.is_empty() {
            return None;
        }
        match bytes.iter().position(|&b| b == b'\n' || b == b'\r') {
            Some(i) => {
                let line = &bytes[..i];
                let eol = if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                bytes = &bytes[i + eol..];
                Some(line)
            },
            None => {
                let line = bytes;
                bytes = &[];
                Some(line)
            },
        }
    })
}

fn scan_block_tables(
    block: &[u8],
    respect_in_block_interrupts: bool,
    estimated_bytes: &mut usize,
    budget: usize,
) -> bool {
    let mut active_row_bytes = 0usize;
    let mut table_quote_depth = 0usize;
    let mut prev_pipes = 0usize;
    for line in commonmark_lines(block) {
        let (quote_depth, spaces, content) = strip_table_containers_with_depth(line);
        if content.is_empty() {
            active_row_bytes = 0;
            prev_pipes = 0;
            continue;
        }
        if respect_in_block_interrupts && active_row_bytes > 0 {
            if is_thematic_break(spaces, content) {
                active_row_bytes = 0;
                prev_pipes = 0;
                continue;
            }
            if quote_depth < table_quote_depth || starts_ordered_list_item(spaces, content) {
                active_row_bytes = 0;
                prev_pipes = content.iter().filter(|&&b| b == b'|').count();
                continue;
            }
        }
        if let Some((cols, min_row_bytes)) = scan_table_delimiter(content)
            && prev_pipes > 0
            && prev_pipes.saturating_add(1) >= cols
        {
            active_row_bytes = active_row_bytes.max(min_row_bytes);
            table_quote_depth = quote_depth;
            *estimated_bytes = estimated_bytes.saturating_add(active_row_bytes);
            if *estimated_bytes > budget {
                return false;
            }
        } else if active_row_bytes > 0 {
            *estimated_bytes = estimated_bytes.saturating_add(active_row_bytes);
            if *estimated_bytes > budget {
                return false;
            }
        }
        prev_pipes = content.iter().filter(|&&b| b == b'|').count();
    }
    true
}

/// Block-aware fallback used when the fast line scan sees a table-shaped region
/// that would exceed the budget. Parses CommonMark blocks with tables disabled
/// (no table-cell auto-completion allocation) so fenced/indented code blocks,
/// HTML blocks, headings, thematic breaks, blockquotes, and list items never
/// masquerade as table rows, while still catching real tables inside paragraphs,
/// setext-like headings (`===` after a table), and tight list items.
fn block_aware_table_html_within_budget(source: &str, budget: usize) -> bool {
    let bytes = source.as_bytes();
    let mut estimated_bytes = 0usize;
    let mut skip_depth = 0usize;
    let mut leaf_block_depth = 0usize;
    let mut tight_span: Option<(usize, usize)> = None;

    let flush_tight = |span: &mut Option<(usize, usize)>, est: &mut usize| -> bool {
        if let Some((start, end)) = span.take() {
            return scan_block_tables(&bytes[start..end], true, est, budget);
        }
        true
    };

    for (event, range) in Parser::new_ext(source, Options::empty()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock) => {
                if !flush_tight(&mut tight_span, &mut estimated_bytes) {
                    return false;
                }
                skip_depth += 1;
            },
            Event::End(TagEnd::CodeBlock | TagEnd::HtmlBlock) => {
                skip_depth = skip_depth.saturating_sub(1);
            },
            Event::Start(Tag::Paragraph | Tag::Heading { .. }) if skip_depth == 0 => {
                if !flush_tight(&mut tight_span, &mut estimated_bytes) {
                    return false;
                }
                leaf_block_depth += 1;
                if !scan_block_tables(&bytes[range], true, &mut estimated_bytes, budget) {
                    return false;
                }
            },
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_)) if skip_depth == 0 => {
                leaf_block_depth = leaf_block_depth.saturating_sub(1);
            },
            Event::Start(Tag::BlockQuote(_) | Tag::List(_) | Tag::Item)
            | Event::End(TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item)
            | Event::Rule
                if skip_depth == 0 =>
            {
                if !flush_tight(&mut tight_span, &mut estimated_bytes) {
                    return false;
                }
            },
            _ if skip_depth == 0
                && leaf_block_depth == 0
                && range.start < range.end
                && range.end <= bytes.len() =>
            {
                tight_span = Some(match tight_span {
                    Some((start, end)) => (start.min(range.start), end.max(range.end)),
                    None => (range.start, range.end),
                });
            },
            _ => {},
        }
    }
    flush_tight(&mut tight_span, &mut estimated_bytes)
}

/// Conservative pre-parse check run before `Parser::new_ext(..., MARKDOWN_OPTIONS)`
/// so `pulldown-cmark`'s eager first-pass table auto-completion (up to `1 << 18`
/// empty cells *per table*) cannot allocate unbounded tree memory on crafted
/// multi-table inputs, while respecting CommonMark block boundaries so fenced
/// code examples and blocks following a table without a blank line still render.
fn table_html_within_budget(source: &str, max_bytes: usize) -> bool {
    let budget = max_bytes.saturating_add(TABLE_PRECHECK_SLACK_BYTES);
    let mut fast_estimated = 0usize;
    if scan_block_tables(source.as_bytes(), false, &mut fast_estimated, budget) {
        return true;
    }
    block_aware_table_html_within_budget(source, budget)
}

struct SafeEvents<I> {
    inner: I,
    image_depth: usize,
    links: Vec<bool>,
    tables: Vec<TableState>,
}

impl<I> SafeEvents<I> {
    fn new(inner: I) -> Self {
        Self {
            inner,
            image_depth: 0,
            links: Vec::new(),
            tables: Vec::new(),
        }
    }
}

impl<'a, I: Iterator<Item = Event<'a>>> Iterator for SafeEvents<I> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        for event in self.inner.by_ref() {
            if self.image_depth > 0 {
                match event {
                    Event::Start(Tag::Image { .. }) => self.image_depth += 1,
                    Event::End(TagEnd::Image) => self.image_depth -= 1,
                    Event::Text(text)
                    | Event::Code(text)
                    | Event::Html(text)
                    | Event::InlineHtml(text) => return Some(Event::Text(text)),
                    Event::SoftBreak | Event::HardBreak => {
                        return Some(Event::Text(" ".into()));
                    },
                    _ => {},
                }
                continue;
            }
            match event {
                Event::Html(text) | Event::InlineHtml(text) => return Some(Event::Text(text)),
                Event::Start(Tag::Image { .. }) => self.image_depth = 1,
                Event::Start(Tag::Link {
                    dest_url, title, ..
                }) => {
                    let safe = safe_link(&dest_url);
                    self.links.push(safe);
                    if safe {
                        let mut opening = String::from("<a href=\"");
                        escape(&mut opening, &dest_url);
                        opening.push_str("\" rel=\"noopener noreferrer\" title=\"");
                        escape(&mut opening, &title);
                        opening.push_str("\">");
                        return Some(Event::InlineHtml(CowStr::from(opening)));
                    }
                },
                Event::End(TagEnd::Link) => {
                    if self.links.pop() == Some(true) {
                        return Some(Event::InlineHtml("</a>".into()));
                    }
                },
                Event::Start(Tag::Table(alignments)) => {
                    self.tables.push(TableState {
                        alignments,
                        in_head: false,
                        in_body: false,
                        col: 0,
                    });
                    return Some(Event::Html("<div class=\"table-wrap\"><table>\n".into()));
                },
                Event::End(TagEnd::Table) => {
                    if let Some(table) = self.tables.pop() {
                        let closing = if table.in_body {
                            "</tbody>\n</table></div>\n"
                        } else {
                            "</table></div>\n"
                        };
                        return Some(Event::Html(closing.into()));
                    }
                },
                Event::Start(Tag::TableHead) => {
                    if let Some(table) = self.tables.last_mut() {
                        table.in_head = true;
                        table.col = 0;
                    }
                    return Some(Event::Html("<thead><tr>".into()));
                },
                Event::End(TagEnd::TableHead) => {
                    if let Some(table) = self.tables.last_mut() {
                        table.in_head = false;
                    }
                    return Some(Event::Html("</tr></thead>\n".into()));
                },
                Event::Start(Tag::TableRow) => {
                    let opening = if let Some(table) = self.tables.last_mut() {
                        table.col = 0;
                        if !table.in_body {
                            table.in_body = true;
                            "<tbody>\n<tr>"
                        } else {
                            "<tr>"
                        }
                    } else {
                        "<tr>"
                    };
                    return Some(Event::Html(opening.into()));
                },
                Event::End(TagEnd::TableRow) => {
                    return Some(Event::Html("</tr>\n".into()));
                },
                Event::Start(Tag::TableCell) => {
                    let (is_head, alignment) =
                        self.tables
                            .last_mut()
                            .map_or((false, Alignment::None), |table| {
                                let alignment = table
                                    .alignments
                                    .get(table.col)
                                    .copied()
                                    .unwrap_or(Alignment::None);
                                table.col += 1;
                                (table.in_head, alignment)
                            });
                    return Some(Event::Html(table_cell_open(is_head, alignment).into()));
                },
                Event::End(TagEnd::TableCell) => {
                    let closing = if self.tables.last().is_some_and(|table| table.in_head) {
                        "</th>"
                    } else {
                        "</td>"
                    };
                    return Some(Event::Html(closing.into()));
                },
                other => return Some(other),
            }
        }
        None
    }
}

struct BoundedString {
    buf: String,
    limit: usize,
}

impl fmt::Write for BoundedString {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.buf.len().saturating_add(s.len()) > self.limit {
            return Err(fmt::Error);
        }
        self.buf.push_str(s);
        Ok(())
    }
}

/// Render CommonMark with GFM strikethrough and tables, without active raw HTML,
/// remote images, unsafe links, or inline `style` attributes.
/// Callers accepting untrusted input should prefer [`render_safe_bounded`].
pub fn render_safe(source: &str) -> String {
    let mut output = String::new();
    html::push_html(
        &mut output,
        SafeEvents::new(Parser::new_ext(source, MARKDOWN_OPTIONS)),
    );
    output
}

/// Render CommonMark with GFM strikethrough and tables into at most `max_bytes`
/// of UTF-8 HTML, returning `None` if table expansion or streamed output would
/// exceed the budget.
pub fn render_safe_bounded(source: &str, max_bytes: usize) -> Option<String> {
    if !table_html_within_budget(source, max_bytes) {
        return None;
    }
    let mut writer = BoundedString {
        buf: String::new(),
        limit: max_bytes,
    };
    html::write_html_fmt(
        &mut writer,
        SafeEvents::new(Parser::new_ext(source, MARKDOWN_OPTIONS)),
    )
    .ok()?;
    Some(writer.buf)
}
fn escape(output: &mut String, source: &str) {
    for c in source.chars() {
        match c {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            c => output.push(c),
        }
    }
}
#[cfg(test)]
#[path = "unit/html.rs"]
mod tests;

//! Pure exact-text edits shared by chat and CLI authoring.
use serde::Deserialize;
use std::ops::Range;

// Edits -------------------------------------------------------------------------------------

/// One replacement of a `plugin_edit`: the single form's `old_string`/`new_string`, or one
/// element of its `edits`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct EditPair {
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

/// One inclusive, 1-based line range of the edited text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LineSpan {
    pub first: usize,
    pub last: usize,
}

/// A source after its edits, and where they landed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edited {
    pub source: String,
    /// Sorted, and merged where two edits touched the same lines.
    pub changed: Vec<LineSpan>,
}

/// What an empty `edits` list is told.
pub const EMPTY_BATCH: &str = "edits is empty; give it at least one {old_string, new_string}";

/// The read advice, for a miss nothing below can place.
const MISSING: &str = "old_string is not in the file. plugin_read it and copy the text to \
                       replace exactly, whitespace included.";

/// `source` with every edit applied in order, each matched against the text the one before it
/// produced, or why the batch does not apply - in which case nothing does. One element is the
/// single `old_string`/`new_string` form, so there is one code path.
///
/// One occurrence per edit unless `replace_all`, which is what makes an edit a safe alternative
/// to re-sending the file: a model that matched the wrong line is told so rather than silently
/// changing the first of several.
pub fn apply_edits(source: &str, edits: &[EditPair]) -> Result<Edited, String> {
    if edits.is_empty() {
        return Err(EMPTY_BATCH.to_owned());
    }
    let mut text = source.to_owned();
    let mut spans: Vec<Range<usize>> = Vec::new();
    for (index, edit) in edits.iter().enumerate() {
        let (next, matches) = apply_one(&text, edit).map_err(|why| match edits.len() {
            1 => why,
            count => format!(
                "edit {} of {count}: {why}\nNothing was written; the file is unchanged.",
                index + 1
            ),
        })?;
        let (old_len, new_len) = (edit.old_string.len(), edit.new_string.len());
        spans = spans
            .into_iter()
            .map(|span| remap(span, &matches, old_len, new_len))
            .collect();
        spans.extend(matches.iter().enumerate().map(|(k, &at)| {
            let start = moved(at, k, old_len, new_len);
            start..start + new_len
        }));
        text = next;
    }
    let changed = line_spans(&text, &spans);
    Ok(Edited {
        source: text,
        changed,
    })
}

/// One edit against `text`: the text after it, and where each replacement began in `text`'s own
/// offsets, ascending.
fn apply_one(text: &str, edit: &EditPair) -> Result<(String, Vec<usize>), String> {
    let old = edit.old_string.as_str();
    if old.is_empty() {
        return Err("old_string is required: the exact text to replace".to_owned());
    }
    if old == edit.new_string {
        return Err("old_string and new_string are the same; nothing to do".to_owned());
    }
    let matches: Vec<usize> = text.match_indices(old).map(|(at, _)| at).collect();
    match matches.len() {
        0 => Err(missing_report(text, old)),
        1 => Ok((text.replacen(old, &edit.new_string, 1), matches)),
        _ if edit.replace_all => Ok((text.replace(old, &edit.new_string), matches)),
        count => Err(ambiguous_report(text, old, count)),
    }
}

/// Where an offset lands once the `k` replacements before it have each swapped `old_len` bytes
/// for `new_len`.
fn moved(offset: usize, k: usize, old_len: usize, new_len: usize) -> usize {
    let delta = new_len as isize - old_len as isize;
    (offset as isize + delta * k as isize).max(0) as usize
}

/// `span`, in the text before an edit, as it lies after that edit replaced `old_len` bytes with
/// `new_len` at each of `matches`: earlier replacements move it by their difference, and one
/// that overlaps it is folded in.
fn remap(span: Range<usize>, matches: &[usize], old_len: usize, new_len: usize) -> Range<usize> {
    let endpoint = |offset, is_end| {
        for (k, &at) in matches.iter().enumerate() {
            if offset <= at {
                return moved(offset, k, old_len, new_len);
            }
            if offset < at + old_len {
                return moved(at, k, old_len, new_len) + if is_end { new_len } else { 0 };
            }
        }
        moved(offset, matches.len(), old_len, new_len)
    };
    endpoint(span.start, false)..endpoint(span.end, true)
}

/// The 1-based line a byte offset of `text` falls on.
fn line_of(text: &str, offset: usize) -> usize {
    text.as_bytes()[..offset.min(text.len())]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

/// Byte spans of `text` as line spans, sorted and merged where they touch. An empty span - a
/// deletion - is the line it was made on.
fn line_spans(text: &str, spans: &[Range<usize>]) -> Vec<LineSpan> {
    let mut spans: Vec<LineSpan> = spans
        .iter()
        .map(|span| {
            let first = line_of(text, span.start);
            let last_byte = span.end.saturating_sub(1).max(span.start);
            LineSpan {
                first,
                last: line_of(text, last_byte).max(first),
            }
        })
        .collect();
    spans.sort();
    let mut merged: Vec<LineSpan> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.first <= last.last + 1 => last.last = last.last.max(span.last),
            _ => merged.push(span),
        }
    }
    merged
}

/// `lines 12-14, line 40`; past five, the rest is counted rather than named.
pub fn span_list(spans: &[LineSpan]) -> String {
    const NAMED: usize = 5;
    let mut parts: Vec<String> = spans
        .iter()
        .take(NAMED)
        .map(|span| match span.first == span.last {
            true => format!("line {}", span.first),
            false => format!("lines {}-{}", span.first, span.last),
        })
        .collect();
    if spans.len() > NAMED {
        parts.push(format!("… and {} more", spans.len() - NAMED));
    }
    parts.join(", ")
}

/// `line 12`, `lines 12 and 40`, `lines 12, 40 and 88`; past eight, the rest is counted.
fn line_list(lines: &[usize]) -> String {
    const NAMED: usize = 8;
    let named: Vec<String> = lines.iter().take(NAMED).map(usize::to_string).collect();
    let more = lines.len().saturating_sub(NAMED);
    match (named.len(), more) {
        (1, 0) => format!("line {}", named[0]),
        (_, 0) => {
            let (last, rest) = named.split_last().expect("at least two");
            format!("lines {} and {last}", rest.join(", "))
        },
        (_, more) => format!("lines {} and {more} more", named.join(", ")),
    }
}

/// `line 41` or `lines 41-43`.
fn range_text(first: usize, last: usize) -> String {
    match first == last {
        true => format!("line {first}"),
        false => format!("lines {first}-{last}"),
    }
}

/// `lines` of `text` from the 1-based `first`, verbatim and unprefixed, so they can be copied
/// into an `old_string` as they are. Capped, since this rides in an error.
fn excerpt(text: &str, first: usize, lines: usize) -> String {
    const MOST: usize = 40;
    text.lines()
        .skip(first.saturating_sub(1))
        .take(lines.min(MOST))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A line with its whitespace made uniform: runs collapsed to one space, the ends trimmed.
fn normalized(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The 1-based line ranges of `text` that read as `old` once whitespace is ignored. The first
/// line of `old` may begin mid-line and its last may end mid-line, as an `old_string` copied from
/// the middle of a line does; a one-line `old` may sit anywhere in its line. At most three.
fn whitespace_matches(text: &str, old: &str) -> Vec<(usize, usize)> {
    let needle: Vec<String> = old.lines().map(normalized).collect();
    let Some((first, rest)) = needle.split_first() else {
        return Vec::new();
    };
    if needle.iter().all(String::is_empty) {
        return Vec::new();
    }
    let lines: Vec<String> = text.lines().map(normalized).collect();
    let mut found = Vec::new();
    for start in 0..lines.len() {
        let end = start + needle.len();
        if end > lines.len() {
            break;
        }
        let window = &lines[start..end];
        let matches = match rest.split_last() {
            None => window[0].contains(first.as_str()),
            Some((last, middle)) => {
                window[0].ends_with(first.as_str())
                    && window[1..window.len() - 1] == *middle
                    && window[window.len() - 1].starts_with(last.as_str())
            },
        };
        if matches {
            found.push((start + 1, end));
            if found.len() == 3 {
                break;
            }
        }
    }
    found
}

/// Why `old` is not in `text`, said so the next call can be right rather than sent to read the
/// page again: the lines that match it apart from whitespace, quoted verbatim; else the line its
/// first line is on, with what follows quoted verbatim; else the read advice.
pub fn missing_report(text: &str, old: &str) -> String {
    let windows = whitespace_matches(text, old);
    if let Some(&(first, last)) = windows.first() {
        let named: Vec<String> = windows
            .iter()
            .map(|(first, last)| range_text(*first, *last))
            .collect();
        let verb = match (windows.len(), first == last) {
            (1, true) => "matches",
            _ => "match",
        };
        return format!(
            "old_string is not in the file exactly, but {} {verb} it apart from whitespace. \
             Copy these lines as they are into old_string:\n{}",
            named.join(" and "),
            excerpt(text, first, last - first + 1)
        );
    }
    if let Some(anchor) = old.lines().map(str::trim).find(|line| !line.is_empty()) {
        let at: Vec<usize> = text
            .lines()
            .enumerate()
            .filter(|(_, line)| line.trim() == anchor)
            .map(|(index, _)| index + 1)
            .take(3)
            .collect();
        if let Some(&first) = at.first() {
            return format!(
                "old_string is not in the file. Its first line is at {}; the file reads from \
                 line {first}:\n{}",
                line_list(&at),
                excerpt(text, first, old.lines().count() + 3)
            );
        }
    }
    MISSING.to_owned()
}

/// Which lines `old` is on when it is on several, so the next `old_string` can carry the lines
/// around the one that was meant.
pub fn ambiguous_report(text: &str, old: &str, count: usize) -> String {
    let at: Vec<usize> = text
        .match_indices(old)
        .map(|(at, _)| line_of(text, at))
        .collect();
    format!(
        "old_string appears {count} times, at {}. Include enough surrounding lines to name one \
         of them, or pass replace_all=true to change every one.",
        line_list(&at)
    )
}

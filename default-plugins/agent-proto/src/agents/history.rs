//! Cached history parsing and terminal rendering.

use super::{short_run_id, AgentRun, RunEntryKind, Summary};
use crate::markdown::{MdLine, MdStyle};
use std::collections::BTreeMap;
use std::sync::Arc;
use unicode_width::UnicodeWidthStr;

/// Below this the text in the history half wraps into a column of single words, and the
/// half draws a placeholder instead.
pub const HISTORY_MIN_COLS: usize = 20;

/// What the history half of the agents pane draws, wrapped to `width` and unframed.
///
/// This used to be a framed box pinned to the foot of the pane, drawn only when there was
/// something to put in it. The half is permanent now and has a divider of its own, so what
/// the caller wants back is content plus enough to label and window it - the frame, the
/// width cap and the row ceiling all belonged to the box rather than to the history.
///
/// The rows are [`MdLine`]s rather than strings: everything in here is markdown by the time
/// it reaches a pane - a prompt is what the user typed, a result is a model's
/// `last_assistant_message` - and drawing the asterisks was never what any of it meant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryLines {
    /// What the divider says - `"claude:9f3c1a7e"`, `"3 · claude !"`. Empty when there is
    /// nothing to name. Uncut: the row it goes on carries the scroll readout too, so only
    /// whoever draws the divider knows the room a label has. See `fit_label`.
    pub label: String,
    /// The timeline, oldest first - summaries among the rest of the events rather than above
    /// them. The caller windows this against the room it has.
    pub entries: Vec<MdLine>,
    /// Which rows of `entries` came from which event, oldest first.
    ///
    /// `entries` alone is a flat run of lines, and a capped event is a lie without the whole
    /// of what it said kept somewhere: this is what lets a caller put a cursor on an event
    /// rather than on a line, and open the uncapped text when the user asks for it. The spans
    /// tile `entries` exactly - every row belongs to one event and no row to two - with one
    /// exception: a run that arrived without its oldest entries opens with a row saying so
    /// (see [`AgentRun::omitted_entries`]), which belongs to no event and which the first
    /// span starts after.
    pub events: Vec<HistoryEvent>,
    /// The rows a reader pins under its divider, above the scrolling timeline: what stays true
    /// of the run however far back it is wound - when it began, and which session it is.
    ///
    /// Held apart from `entries` rather than pushed onto the front of it so that the promise
    /// `events` makes above survives: these rows belong to no event, and a cursor can no more
    /// land on one than it can on the divider.
    ///
    /// Empty for everything but a loaded run: a summary drawn for an agent with no run has
    /// neither of the two facts to state.
    pub header: Vec<MdLine>,
}

/// One event of the timeline: which rows of [`HistoryLines::entries`] it took after the cap,
/// and the whole of what it said, for the reader that shows it uncapped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryEvent {
    /// The recorded entry, or None for the appended rolling summary.
    pub revision: Option<u64>,
    /// First row of this event in `entries`.
    pub start: usize,
    /// How many rows it took - never more than [`MAX_ENTRY_ROWS`].
    pub len: usize,
    /// What a reader's divider calls it: [`RunEntryKind::label`], or `"summary"`.
    pub kind: &'static str,
    /// When it was recorded, so a reader's age stays live rather than freezing at open.
    pub created_at_ms: Option<i64>,
    /// The event's text, uncapped and unwrapped. Shared rather than owned: every history
    /// render hands out every event's text, and a copy of each would cost a pass over the whole
    /// run on the path [`ParsedRun`] exists to make cheap.
    pub text: Arc<str>,
}

impl HistoryEvent {
    /// One past the last row of `entries` this event owns.
    pub fn end(&self) -> usize {
        self.start + self.len
    }
}

/// Nothing selected, or nothing to say about what is - what a caller borrows when its cache is
/// empty.
pub static NO_HISTORY: HistoryLines = HistoryLines {
    label: String::new(),
    entries: Vec::new(),
    events: Vec::new(),
    header: Vec::new(),
};

impl HistoryLines {
    /// Nothing to draw at all - no label, no timeline.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The room a half `width` columns wide has for text, or `None` when there is not enough of it
/// to wrap into anything but a column of single words.
///
/// A margin column at each edge, the same one the list's marker column gives it. The divider is
/// not held to it - a rule wants both ends of the row - so this is the body's width alone.
pub(super) fn wrap_width(width: usize) -> Option<usize> {
    let inner = width.saturating_sub(2);
    (inner >= HISTORY_MIN_COLS).then_some(inner)
}

/// A label cut to the room the divider has, so a long kind cannot push the rule off the row.
///
/// `width` is what is left of the row for the label and its rule: the caller has already
/// taken off anything else sharing it, which for the agents pane is the scroll readout at the
/// right-hand end.
pub fn fit_label(label: String, width: usize) -> String {
    // Two dashes and a space either side of the label is the least a divider is worth
    // drawing; below that there is no label at all.
    cut_to_width(label, width.saturating_sub(4))
}

/// `text` cut from the tail until it fits `width` terminal columns.
///
/// By column rather than by byte or by `char`: a title can hold anything the agent was asked,
/// and both a multi-byte character and a double-width one are drawn wrong by a count of the
/// other. Nothing is appended in place of what was dropped - the columns a table row divides up
/// are all it has, and an ellipsis would cost one of them on every cut cell.
pub fn cut_to_width(mut text: String, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    while text.width() > width {
        text.pop();
    }
    text
}

/// What joins two segments of a hint line.
const HINT_SEPARATOR: &str = " · ";

/// What a hint line ends with when [`fit_hints`] had to leave keys out of it.
const MORE_KEYS: &str = "? keys";

/// A key-hint line: `segments` joined with ` · ` for as long as they fit `width` columns.
///
/// Whole segments are dropped from the end, never cut: half of `Enter focus` names no key, and
/// a hint that reads `Enter fo` is worse than one that stops a key sooner. So callers put the
/// segments most important first - the selection's own verb, then the rest of the keys, then
/// anything that is not a key at all. Where something was dropped and there is still room,
/// the line ends in `? keys` to say there is more - room that is left over, never made:
/// [`fit_hints_marked`] is for a line that would rather give up a key or two for the marker,
/// and [`fit_keys`] for one that must not have it. Only a first segment too wide for the row
/// on its own is cut, by [`cut_to_width`], since an empty hint line helps nobody.
///
/// Not for table cells: those keep [`cut_to_width`], which has no segments to respect.
pub fn fit_hints(segments: &[&str], width: usize) -> String {
    let (mut line, dropped) = fit_segments(segments, width);
    if dropped && !line.is_empty() {
        let marked = line.width() + HINT_SEPARATOR.width() + MORE_KEYS.width();
        if marked <= width {
            line.push_str(HINT_SEPARATOR);
            line.push_str(MORE_KEYS);
        }
    }
    line
}

/// [`fit_hints`] for a line that would rather say `? keys` than show its last key or two.
///
/// [`fit_hints`] adds the marker only where it fits after everything else that did, and on a
/// line of keys alone that is almost never: at the widths a panel is usually drawn, the keys
/// fill the row to within a few columns and `? keys` needs nine. That is the right trade where
/// the line leads with something that is not a key - a status or a note, where the one action
/// after it is the only useful thing left to press - so [`fit_hints`] keeps it. A footer of
/// nothing but keys is the other case: its trailing keys are the least important ones by
/// construction, and the sheet `?` opens lists all of them, so giving up one or two to say so
/// is the better use of the row.
///
/// So this drops whole segments from the end, one at a time, until the marker fits after what
/// is left - but never below the first `keep`, which a caller sets to however many segments
/// name the keys a user cannot do without here. Where no count down to `keep` makes room, or
/// fewer than `keep` fit at all, the line is exactly what [`fit_hints`] would draw.
///
/// Segments are counted rather than found by searching for the separator: a status segment can
/// hold ` · ` of its own, and cutting at the last one would split it. Nothing is ever cut to
/// make room.
pub fn fit_hints_marked(segments: &[&str], width: usize, keep: usize) -> String {
    let unmarked = fit_hints(segments, width);
    if !fit_segments(segments, width).1 || unmarked.ends_with(MORE_KEYS) {
        return unmarked;
    }
    let segments: Vec<&str> = segments
        .iter()
        .copied()
        .filter(|segment| !segment.is_empty())
        .collect();
    // Longest first, and never all of them: something was dropped, so the whole line does not
    // fit even without the marker. Nor does any prefix longer than what `fit_segments` kept, so
    // counting down from the top finds the same line as counting down from what it kept - and a
    // first segment it had to cut fits no count at all, which leaves that cut line standing.
    let marker = HINT_SEPARATOR.width() + MORE_KEYS.width();
    (keep.max(1)..segments.len())
        .rev()
        .map(|n| segments[..n].join(HINT_SEPARATOR))
        .find(|line| line.width() + marker <= width)
        .map(|line| format!("{line}{HINT_SEPARATOR}{MORE_KEYS}"))
        .unwrap_or(unmarked)
}

/// A key-hint line with no `? keys` marker: the whole segments that fit, as [`fit_hints`]
/// drops them.
///
/// For the rows where the marker would be wrong rather than merely short of room: a row drawn
/// while `?` is being typed into a box, where the key types a question mark instead of opening
/// anything, and the rows of the sheet itself, where `?` is what shuts it.
pub fn fit_keys(segments: &[&str], width: usize) -> String {
    fit_segments(segments, width).0
}

/// [`fit_hints`] without the marker: the segments that fit, and whether any were left out.
/// Empty segments are skipped rather than drawn as a doubled separator.
fn fit_segments(segments: &[&str], width: usize) -> (String, bool) {
    let mut segments = segments.iter().filter(|segment| !segment.is_empty());
    let Some(first) = segments.next() else {
        return (String::new(), false);
    };
    if first.width() > width {
        return (cut_to_width((*first).to_owned(), width), true);
    }
    let mut line = (*first).to_owned();
    for segment in segments {
        if line.width() + HINT_SEPARATOR.width() + segment.width() > width {
            return (line, true);
        }
        line.push_str(HINT_SEPARATOR);
        line.push_str(segment);
    }
    (line, false)
}

/// The selected run as the history half draws it: one timeline, summaries among the events
/// rather than above them.
///
/// The filter is the one the box drew: prompts, plans and questions always, and a result
/// only on a run that has stopped and only while the summary has not yet absorbed it. A
/// finished run whose summary is up to date says what it did once, in the summary row,
/// rather than twice.
///
/// The rolling [`AgentRun::summary`] is appended as the newest row, which is where in time it
/// was made. It is dropped when the newest [`RunEntryKind::Summary`] already on the timeline
/// says the same thing - a summary somebody pressed a key for both stays where it was asked
/// for and is the current one, and two identical paragraphs one under the other read as a bug
/// rather than as a history.
pub fn run_history(
    run: &AgentRun,
    width: usize,
    now_ms: i64,
    utc_offset_secs: i32,
) -> HistoryLines {
    ParsedRun::new(run).lines(width, now_ms, utc_offset_secs)
}

/// [`run_history`] with the expensive half kept: every event's markdown parsed once, and its
/// rows wrapped once per width.
///
/// A caller that draws the same run again and again - the agents pane redraws it on every
/// resize step, every minute, and every time agent sends the run with one more entry on it -
/// holds one of these instead of calling `run_history`. The parse depends on nothing but the
/// text, so a resize only re-wraps; the wrap depends on nothing but the text and the width, so
/// a new entry or a new minute wraps nothing but what is new. A run near
/// `rows::RUN_DETAIL_BYTES` is hundreds of kilobytes of markdown, and re-parsing all of it per
/// resize step is what made dragging the pane's edge stall for seconds.
///
/// What an [`ParsedRun::update`] costs is one lookup per entry and the parse of whatever is
/// new: a recorded entry it already holds is recognised without reading its text (see
/// [`ParsedEvent::is`]). The caller has paid for decoding the run by then, which is the one
/// pass over all of it that is left - and [`run_payload_fingerprint`] is how the agents pane
/// avoids even that for a run sent again unchanged.
#[derive(Default)]
pub struct ParsedRun {
    /// The run's own facts, for the label and the pinned header. Cheap, so refreshed whole on
    /// every [`ParsedRun::update`].
    run: RunFacts,
    /// The timeline's events in order, the rolling summary last when it is drawn at all.
    events: Vec<ParsedEvent>,
    /// How many events have been parsed over this one's life.
    ///
    /// Test-only: a kept parse and a fresh one draw the same rows, so nothing else can tell a
    /// cache that works from one that quietly re-parses everything.
    #[cfg(test)]
    pub(super) parses: usize,
}

/// One event of a [`ParsedRun`]: what it said, parsed, and its rows at the last width asked for.
struct ParsedEvent {
    /// The recorded entry's revision, or `None` for the rolling summary.
    revision: Option<u64>,
    kind: &'static str,
    style: MdStyle,
    created_at_ms: Option<i64>,
    /// The whole of what the event said, for [`HistoryEvent::text`].
    raw: Arc<str>,
    /// The head of what it says, parsed: only as much as the timeline can draw of it. See
    /// [`ENTRY_HEAD_BLOCKS`] and [`ENTRY_HEAD_BYTES`]. Shared, so a reader opened on a short
    /// event can draw this parse rather than make its own: see [`ParsedRun::whole_blocks`].
    blocks: Arc<[crate::markdown::MdBlock]>,
    /// Whether `blocks` is the whole of what was parsed. An event that was not parsed whole is
    /// capped however few rows its head came to, so the marker still says there is more.
    complete: bool,
    /// Whether `blocks` is the whole of `raw`: complete, and parsed from the text itself rather
    /// than from a digest of its envelope.
    whole: bool,
    /// The capped rows, without the gutter's text on the first of them, and the `(width,
    /// indent)` they were wrapped for.
    rows: Option<((usize, usize), Vec<MdLine>)>,
    /// The gutter the last [`ParsedRun::lines`] put on the first row, which is all
    /// [`ParsedRun::restamp`] has to change when only the clock moved.
    gutter: String,
}

/// Tool payloads are literal terminal-safe text, never Markdown instructions.
pub fn entry_blocks(kind: &str, text: &str) -> Vec<crate::markdown::MdBlock> {
    if !matches!(kind, "tool_call" | "tool_result") {
        return crate::markdown::parse(text);
    }
    crate::text::sanitize(text)
        .lines()
        .map(|line| crate::markdown::MdBlock {
            chars: line
                .chars()
                .map(|ch| crate::markdown::StyledChar {
                    ch,
                    style: Default::default(),
                })
                .collect(),
            compact: true,
            ..Default::default()
        })
        .collect()
}

impl ParsedEvent {
    fn new(
        revision: Option<u64>,
        kind: &'static str,
        style: MdStyle,
        created_at_ms: Option<i64>,
        digest: Option<&str>,
        raw: &str,
    ) -> Self {
        let (blocks, complete) = if matches!(kind, "tool_call" | "tool_result") {
            let mut end = raw.len().min(ENTRY_HEAD_BYTES);
            while !raw.is_char_boundary(end) {
                end -= 1;
            }
            (entry_blocks(kind, &raw[..end]), end == raw.len())
        } else {
            crate::markdown::parse_head(digest.unwrap_or(raw), ENTRY_HEAD_BLOCKS, ENTRY_HEAD_BYTES)
        };
        Self {
            revision,
            kind,
            style,
            created_at_ms,
            raw: raw.into(),
            blocks: blocks.into(),
            complete,
            whole: complete && digest.is_none(),
            rows: None,
            gutter: String::new(),
        }
    }

    /// Whether the entry held under this one's revision would parse to what this holds, so
    /// the parse can be kept.
    ///
    /// A recorded entry is recognised by its kind and its length, not by reading its text. A
    /// revision names one entry of one run, and an entry is never rewritten under it: the
    /// daemon only ever pushes, and `rows::hydrate_newest_entries` drops whole entries to fit
    /// its budget rather than cutting one. Comparing the text as well would walk every byte
    /// of a long run on each resend to confirm what the revision already says. The length is
    /// kept as a cheap tripwire all the same, and the kind because a prompt is parsed from its
    /// envelope's digest rather than from its text. [`ParsedRun::update`] drops everything
    /// held when the run itself changes, since revisions are only unique within one.
    ///
    /// The rolling summary (`revision` `None`) is compared whole: it is rewritten in place
    /// every time the run is summarised again, often to a paragraph much like the last, and it
    /// is one short paragraph. Not the stamp, which only the gutter reads - an entry whose
    /// stamp was corrected takes the new one and keeps its parse.
    fn is(&self, kind: &str, raw: &str) -> bool {
        self.kind == kind
            && match self.revision {
                Some(_) => self.raw.len() == raw.len(),
                None => *self.raw == *raw,
            }
    }
}

impl ParsedRun {
    pub fn new(run: &AgentRun) -> Self {
        let mut parsed = Self::default();
        parsed.update(run);
        parsed
    }

    /// How many styled characters the held parse keeps, across every event and table cell.
    ///
    /// Test-only, like `parses`: a head that quietly grew back to the whole event draws the
    /// same rows, so only this can tell.
    #[cfg(test)]
    pub(super) fn parsed_chars(&self) -> usize {
        self.events
            .iter()
            .flat_map(|event| event.blocks.iter())
            .map(|block| {
                let cells = block.table.as_ref().map_or(0, |table| {
                    table
                        .head
                        .iter()
                        .chain(table.rows.iter().flatten())
                        .map(Vec::len)
                        .sum()
                });
                block.chars.len() + cells
            })
            .sum()
    }

    /// Take `run` as the one this draws, parsing only the events it does not already hold.
    /// Which events those are, and where the rolling summary goes, is [`run_history`]'s.
    pub fn update(&mut self, run: &AgentRun) {
        // Revisions are counted per run, so another run's revision 3 is not this one's.
        if self.run.id != run.id {
            self.events.clear();
        }
        let mut held: BTreeMap<Option<u64>, ParsedEvent> = std::mem::take(&mut self.events)
            .into_iter()
            .map(|event| (event.revision, event))
            .collect();
        let mut take = |revision: Option<u64>,
                        kind: &'static str,
                        style: MdStyle,
                        created_at_ms: Option<i64>,
                        raw: &str,
                        drawn: &dyn Fn() -> Option<String>| {
            match held.remove(&revision) {
                Some(mut event) if event.is(kind, raw) => {
                    event.style = style;
                    event.created_at_ms = created_at_ms;
                    event
                },
                _ => {
                    #[cfg(test)]
                    {
                        self.parses += 1;
                    }
                    ParsedEvent::new(
                        revision,
                        kind,
                        style,
                        created_at_ms,
                        drawn().as_deref(),
                        raw,
                    )
                },
            }
        };
        for entry in run.entries.iter().filter(|entry| {
            matches!(
                entry.kind,
                RunEntryKind::Prompt
                    | RunEntryKind::Plan
                    | RunEntryKind::Question
                    | RunEntryKind::Summary
                    | RunEntryKind::Message
                    | RunEntryKind::ToolCall
                    | RunEntryKind::ToolResult
                    | RunEntryKind::Result
            )
        }) {
            let style = match entry.kind {
                RunEntryKind::Summary => SUMMARY_LABEL_STYLE,
                _ => LABEL_STYLE,
            };
            // `drawn` and `raw` diverge for exactly one kind: a prompt carrying a hook envelope
            // is *drawn* as `crate::envelope`'s reading of it and *recorded* as what arrived.
            // Everything else passes the same text twice. Keeping the two apart here is what
            // lets the timeline read as prose while `HistoryEvent::text` keeps its contract -
            // the whole of what it said, which is what the reader shows and what `r` shows raw.
            //
            // `digest` is `None` for everything a person typed, which is the common case and
            // the one that must cost nothing; the label stays `prompt:` either way, because a
            // prompt is what it is however it was written. Only asked when the entry is new.
            let prompt = matches!(entry.kind, RunEntryKind::Prompt);
            let event = take(
                Some(entry.revision),
                entry.kind.label(),
                style,
                entry.created_at_ms,
                &entry.text,
                &|| {
                    prompt
                        .then(|| crate::envelope::digest(&entry.text))
                        .flatten()
                },
            );
            self.events.push(event);
        }
        if let Some(summary) = run
            .summary
            .as_ref()
            .filter(|summary| !repeats_newest(run, summary))
        {
            let event = take(
                None,
                "summary",
                SUMMARY_LABEL_STYLE,
                summary.created_at_ms,
                &summary.text,
                &|| None,
            );
            self.events.push(event);
        }
        self.run = RunFacts::of(run);
    }

    /// The run as the history half draws it at `width`, its ages read against `now_ms`.
    pub fn lines(&mut self, width: usize, now_ms: i64, utc_offset_secs: i32) -> HistoryLines {
        let Some(inner) = wrap_width(width) else {
            return HistoryLines::default();
        };
        let mut entries: Vec<MdLine> = Vec::new();
        let mut events: Vec<HistoryEvent> = Vec::with_capacity(self.events.len());
        // First, so it is what the top of the scroll reaches and every span below still starts
        // where its rows do. It belongs to no event, so no cursor can land on it.
        if self.run.omitted_entries > 0 {
            entries.push(omitted_notice(inner));
        }
        // Walked rather than `flat_map`ped: each event's rows have to be counted before the next
        // one's begin, which is the whole of what a span is.
        for event in &mut self.events {
            let gutter = age_gutter(now_ms, event.created_at_ms);
            let key = (inner, gutter.width());
            let rows = match &event.rows {
                Some((held, rows)) if *held == key => rows,
                _ => {
                    let rows = cap_rows(
                        labelled_blocks(&event.blocks, event.kind, key.1, inner, event.style).0,
                        key.1,
                        event.complete,
                    );
                    &event.rows.insert((key, rows)).1
                },
            };
            events.push(HistoryEvent {
                revision: event.revision,
                start: entries.len(),
                len: rows.len(),
                kind: event.kind,
                created_at_ms: event.created_at_ms,
                text: Arc::clone(&event.raw),
            });
            let mut rows = rows.iter().cloned();
            if let Some(first) = rows.next() {
                entries.push(first.prepend(&gutter, AGE_STYLE));
            }
            entries.extend(rows);
            event.gutter = gutter;
        }
        HistoryLines {
            header: run_header(&self.run, inner, utc_offset_secs),
            // The run's own id rather than its kind: the id opens with the kind anyway
            // (`run_id`), so this says everything the kind said and names *which* run of that
            // agent the half is drawing - the one thing the pane could not tell you before.
            // Shortened, because the whole id does not fit a divider (`short_run_id`), and
            // without the run's state, which the selector row above already carries.
            label: short_run_id(&self.run.id),
            entries,
            events,
        }
    }

    /// Bring `lines` - what the last [`Self::lines`] returned, with nothing updated since - up
    /// to `now_ms`, and say whether it could.
    ///
    /// The clock moves nothing but the gutters, so this rewrites the first row of each event
    /// whose age now reads differently and leaves every other row where it is, rather than
    /// cloning the whole timeline out of the kept rows again as `lines` does. Refuses, touching
    /// nothing, when `lines` does not have the shape the kept rows would give it, or a gutter
    /// changed width - rows are wrapped beside the gutter, so that one needs `lines`.
    pub fn restamp(&mut self, lines: &mut HistoryLines, now_ms: i64) -> bool {
        if lines.events.len() != self.events.len() {
            return false;
        }
        let mut changed = Vec::new();
        for (index, (event, drawn)) in self.events.iter().zip(&lines.events).enumerate() {
            let Some(((_, indent), rows)) = &event.rows else {
                return false;
            };
            if drawn.revision != event.revision
                || drawn.len != rows.len()
                || drawn.end() > lines.entries.len()
            {
                return false;
            }
            let gutter = age_gutter(now_ms, event.created_at_ms);
            if gutter.width() != *indent {
                return false;
            }
            if gutter != event.gutter {
                changed.push((index, gutter));
            }
        }
        for (index, gutter) in changed {
            let event = &mut self.events[index];
            let first = event
                .rows
                .as_ref()
                .and_then(|(_, rows)| rows.first())
                .cloned();
            if let Some(first) = first {
                lines.entries[lines.events[index].start] = first.prepend(&gutter, AGE_STYLE);
            }
            event.gutter = gutter;
        }
        true
    }

    /// The kept parse of the event whose [`HistoryEvent::text`] is `text`, where it is the
    /// whole of that text - what a reader of the event would otherwise parse again. `None` for
    /// an event parsed only in part, or drawn from its envelope's digest rather than its text.
    ///
    /// Found by the text's allocation rather than by revision: `lines` hands out the one it
    /// holds, so a match is this very event and not an older one filed under the same revision.
    pub fn whole_blocks(&self, text: &Arc<str>) -> Option<Arc<[crate::markdown::MdBlock]>> {
        self.events
            .iter()
            .find(|event| event.whole && Arc::ptr_eq(&event.raw, text))
            .map(|event| Arc::clone(&event.blocks))
    }
}

/// What [`omitted_notice`] says happened.
const OMITTED_NOTICE: &str = "older events not loaded";

/// [`OMITTED_NOTICE`] in fewer words, for a half too narrow for the whole phrase.
const OMITTED_NOTICE_SHORT: &str = "older not loaded";

// The narrowest half that draws a timeline at all has room for the short phrase after the age
// gutter, so it is never cut. By `len`, since `width` is not const: the phrase is ASCII.
const _: () = assert!(OMITTED_NOTICE_SHORT.len() <= HISTORY_MIN_COLS - AGE_WIDTH);

/// What a timeline opens with when its run arrived without its oldest entries: see
/// [`AgentRun::omitted_entries`].
///
/// Without it the oldest row loaded reads as the start of the run. The second half says where
/// the rest can still be found, and goes first when the half is too narrow for both. Narrower
/// still, the notice says it in fewer words rather than being cut: `older events not` is a
/// sentence that stopped, where [`OMITTED_NOTICE_SHORT`] is a whole one. Always the one row, so
/// the event spans after it start where they did. Dim and under the labels, like the capped
/// marker: it is pane furniture, not something the run said.
fn omitted_notice(width: usize) -> MdLine {
    let room = width.saturating_sub(AGE_WIDTH);
    let notice = if OMITTED_NOTICE.width() <= room {
        fit_segments(&[OMITTED_NOTICE, "/ searches them"], room).0
    } else {
        OMITTED_NOTICE_SHORT.to_owned()
    };
    MdLine::plain("")
        .prepend(&notice, LABEL_STYLE)
        .indent(AGE_WIDTH)
}

/// A run's JSON reduced to its length and a hash, for telling a run sent again unchanged from
/// one that moved without holding on to the bytes it came as.
///
/// A long run is a quarter of a megabyte of JSON, and keeping it only to compare the next one
/// against it was a second whole copy of the run beside the parse. `DefaultHasher` rather than
/// a stable hash: the value is compared within one process and never written anywhere, which
/// is the opposite of what `envelope`'s FNV key is for.
pub fn run_payload_fingerprint(payload: &str) -> (usize, u64) {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(payload.as_bytes());
    (payload.len(), hasher.finish())
}

/// What [`run_header`] and the divider's label read of a run: its identity, without the
/// entries a [`ParsedRun`] holds parsed instead.
#[derive(Default)]
pub(super) struct RunFacts {
    id: String,
    session_id: String,
    kind: String,
    started_at_ms: i64,
    origin_run_id: Option<String>,
    previous_run_id: Option<String>,
    /// [`AgentRun::omitted_entries`], for the notice the timeline opens with.
    omitted_entries: usize,
}

impl RunFacts {
    pub(super) fn of(run: &AgentRun) -> Self {
        Self {
            id: run.id.clone(),
            session_id: run.session_id.clone(),
            kind: run.kind.clone(),
            started_at_ms: run.started_at_ms,
            origin_run_id: run.origin_run_id.clone(),
            previous_run_id: run.previous_run_id.clone(),
            omitted_entries: run.omitted_entries,
        }
    }
}

/// The run identity fields [`run_history`] pins above its timeline.
///
/// Both answer a question the timeline cannot. `short_run_id` on the divider says *which of
/// this agent's runs* is being drawn, in the eight characters a rule can spare; this says the
/// whole id, which is the one you paste into a `--resume`, a log grep or an `agent_run` call.
/// And every row below is stamped with a relative age, which stops being an answer the moment
/// the question is "which afternoon was that" - so the run's own start is drawn absolutely.
///
/// Wrapped rather than cut. A tail-cut uuid is a uuid nobody can use, and unlike the divider
/// (`fit_label`) this has as many rows as it needs.
pub(super) fn run_header(run: &RunFacts, width: usize, utc_offset_secs: i32) -> Vec<MdLine> {
    // `run.id` when the agent supplied no session id of its own: `run_id` keys such a run by
    // pane and start time, and naming that "session" would be a claim the run cannot make.
    let (label, value) = match run.session_id.trim() {
        "" => ("run", run.id.as_str()),
        session_id => ("session", session_id),
    };
    let mut rows = wrapped_field(
        "started",
        &wall_clock(
            yinz_tile::prelude::SignedUnixMillis::new(run.started_at_ms),
            utc_offset_secs,
        ),
        width,
    );
    rows.extend(wrapped_field(label, value, width));
    for (label, id) in [
        ("origin", run.origin_run_id.as_deref()),
        ("previous", run.previous_run_id.as_deref()),
    ] {
        if let Some(id) = id {
            let prefix = format!("{}:", run.kind);
            rows.extend(wrapped_field(
                label,
                id.strip_prefix(&prefix).unwrap_or(id),
                width,
            ));
        }
    }
    rows
}

/// One `label  value` row, plus a continuation row per further slice the value needs, each
/// indented under the value column so the field reads as one block.
///
/// The label column is at least [`HEADER_LABEL_WIDTH`] wide, with a separating space
/// even for the longer `previous` label.
fn wrapped_field(label: &str, value: &str, width: usize) -> Vec<MdLine> {
    let label_width = HEADER_LABEL_WIDTH.max(label.len() + 1);
    let label = format!("{label:<label_width$}");
    // At least one column, or a half narrow enough to leave none would loop forever below.
    let room = width.saturating_sub(label_width).max(1);
    // By character rather than by byte: a session id is a uuid today, but nothing stops a
    // hand-rolled `agent-report` from naming a run in any script at all, and a slice through
    // the middle of a character panics.
    let mut chars = value.chars().peekable();
    let mut rows = Vec::new();
    while chars.peek().is_some() {
        let slice: String = chars.by_ref().take(room).collect();
        let gutter = match rows.is_empty() {
            true => label.clone(),
            false => " ".repeat(label_width),
        };
        rows.push(MdLine::plain(slice).prepend(&gutter, HEADER_LABEL_STYLE));
    }
    rows
}

/// The width of the header's label column - `"session"` and a space, which is the longest of
/// them. Wider than the timeline's own [`AGE_WIDTH`] gutter on purpose: these rows are not part
/// of the timeline and lining them up with it would say they were.
pub(super) const HEADER_LABEL_WIDTH: usize = 8;

/// How a header label is painted: dim, like every other piece of pane furniture. The values
/// beside them are not - a timestamp and a session id are what the rows are for.
const HEADER_LABEL_STYLE: MdStyle = LABEL_STYLE;

/// Whether the newest summary already on the run's timeline says what `summary` says - the
/// same comparison [`push_summary_entry`] makes, asked of the rolling summary before it is
/// drawn under the entry it duplicates.
fn repeats_newest(run: &AgentRun, summary: &Summary) -> bool {
    run.entries
        .iter()
        .rev()
        .find(|entry| entry.kind == RunEntryKind::Summary)
        .is_some_and(|entry| entry.text == summary.text.trim())
}

/// The idle summary as the history half draws it, for a selected agent with no run loaded.
///
/// `waiting` is the flag on the row's own [`super::StripRow::Agent`], not `summary.awaiting`: the stored
/// flag outlives the screen it was read from, and a divider that went on saying `!` after the
/// user answered would contradict the row above it, which checks that the summary is still
/// about the current screen.
///
/// A talkative model is windowed by the caller rather than cut here, which is what the box
/// had to do with six rows to give.
pub fn summary_history(
    pane_id: u32,
    kind: &str,
    summary: &Summary,
    waiting: bool,
    width: usize,
    now_ms: i64,
) -> HistoryLines {
    let Some(inner) = wrap_width(width) else {
        return HistoryLines::default();
    };
    let mut label = format!("{pane_id} · {kind}");
    if waiting {
        // The same claim the strip's `!` marker makes, from the same flag, in the one place
        // the user is already looking. Ahead of the age because `fit_label` cuts from the
        // tail: a divider too narrow for both should lose how old the answer is rather than
        // that the agent is waiting on you.
        label.push_str(" !");
    }
    // The age goes on the divider rather than in a gutter: there is one thing below it and
    // no column of rows for the times to line up in.
    if let Some(then_ms) = summary.created_at_ms {
        label.push_str(&format!(" · {}", short_age(now_ms, then_ms)));
    }
    // No label: the divider above it already names the agent this is about, and there is
    // no second kind of row here for one to tell it apart from.
    let entries =
        crate::markdown::render(&summary.text, inner, crate::markdown::Options::compact());
    // Uncapped, unlike a run's events: there is only one of these and nothing beside it
    // competing for the room, so a talkative model costs nothing here. It is still one event,
    // so a cursor can land on it and open it in full the way it can any other.
    let events = match entries.is_empty() {
        true => Vec::new(),
        false => vec![HistoryEvent {
            revision: None,
            start: 0,
            len: entries.len(),
            kind: "summary",
            created_at_ms: summary.created_at_ms,
            text: summary.text.as_str().into(),
        }],
    };
    HistoryLines {
        label,
        entries,
        events,
        // No run, so neither of the two facts a header states is in hand: an idle agent's
        // summary is about the pane, not about a session with a start and an id.
        header: Vec::new(),
    }
}

/// The most rows one event of a run's timeline gets, the overflow marker included.
///
/// Four, because that is enough for a prompt or a question whole and enough of a result to know
/// whether it is the one you were looking for. Before this a single talkative `result:` could
/// fill the half and push everything either side of it out of view, which is the opposite of
/// what a timeline is for. Everything past the cap is still reachable - a cursor on the event
/// and `Enter` opens the whole of it - which is what makes this a cap rather than a truncation.
pub const MAX_ENTRY_ROWS: usize = 4;

/// One event's rows, cut to [`MAX_ENTRY_ROWS`] with the last of them saying there is more.
///
/// The marker takes one of the four rather than being squeezed in beside them: a row that says
/// "there is more here" is worth as much as the row of prose it replaces, and a cap that
/// silently dropped the tail would read as an agent that stopped mid-sentence.
///
/// It does not say how much more. It used to count the rows dropped, which meant wrapping the
/// whole of every event at every width only to throw all but four rows away - most of what a
/// resize of the agents pane cost, on a run of long plans and messages. [`labelled_blocks`]
/// now stops wrapping at the first row past the cap, and `Enter` on the event is how much.
///
/// `indent` is the entry's gutter width, so the marker sits in the column the entry's
/// continuation rows are indented to rather than out at the margin.
///
/// `complete` is whether `rows` were wrapped from the whole of the event. One that was parsed
/// only in part is capped whatever its head came to - see [`ENTRY_HEAD_BYTES`].
pub(super) fn cap_rows(mut rows: Vec<MdLine>, indent: usize, complete: bool) -> Vec<MdLine> {
    if complete && rows.len() <= MAX_ENTRY_ROWS {
        return rows;
    }
    rows.truncate(MAX_ENTRY_ROWS - 1);
    rows.push(
        MdLine::plain("")
            .prepend(CAPPED_MARKER, LABEL_STYLE)
            .indent(indent),
    );
    rows
}

/// What the last row of a capped event says. See [`cap_rows`].
pub(super) const CAPPED_MARKER: &str = "… more";

/// How many of an event's blocks the timeline parses.
///
/// Enough to tell whether it overflows at any width. Every block draws at least one row, and
/// [`wrap_head`] never wants more than one row past [`MAX_ENTRY_ROWS`], so the block after
/// these could only ever be cut off.
pub(super) const ENTRY_HEAD_BLOCKS: usize = MAX_ENTRY_ROWS + 1;

/// How many bytes of an event's text the timeline parses.
///
/// A block count alone does not bound the parse: a 100KB code fence or table is one block, and
/// pulldown-cmark reads the whole of its input before handing over the first. Four rows at even
/// five hundred columns is two thousand characters, so this leaves eight times that for markup
/// and multi-byte characters. An event longer than this is capped whatever its head comes to,
/// and `Enter` opens the whole of it from [`HistoryEvent::text`].
pub(super) const ENTRY_HEAD_BYTES: usize = 16 * 1024;

/// [`crate::markdown::wrap`] with compact blocks, stopped once it has more than `limit` rows.
///
/// All [`cap_rows`] needs to know is whether an event overflows, and the first row past the
/// cap answers that; the rows after it would be wrapped only to be dropped. That holds inside a
/// block as well as between them, since one block can be the whole of a long event.
fn wrap_head(blocks: &[crate::markdown::MdBlock], cols: usize, limit: usize) -> Vec<MdLine> {
    if blocks.is_empty() {
        return vec![MdLine::blank()];
    }
    let mut lines = Vec::new();
    for block in blocks {
        let Some(wanted) = (limit + 1)
            .checked_sub(lines.len())
            .filter(|rows| *rows > 0)
        else {
            break;
        };
        lines.extend(crate::markdown::render_block_head(block, cols, wanted));
    }
    lines
}

/// One history row's text as markdown, with `label` in front of it, and whether the label took
/// a row to itself.
///
/// The label rides on the first rendered line rather than being spliced into the source: a
/// result that opens with `# Heading` or `- one` must still be a heading or a bullet, and
/// `result: # Heading` on one line is neither. When the first block carries a prefix of its own,
/// a list marker or a quote bar, the label takes a line to itself instead, so it cannot be glued
/// onto a marker it does not belong to, and so the marker keeps the column its continuation rows
/// are indented to.
///
/// Which of the two it is has to be known before a width can be picked, which is why this takes
/// the text already parsed into blocks rather than rendering it twice. Parsed by the caller
/// rather than here so [`ParsedRun`] can keep the parse across widths.
///
/// The gutter's own text is not on the first row - only its width, `indent`, which the rows
/// below are indented by. The gutter is the one part of an entry that changes with the clock,
/// so leaving it off is what lets the wrapped rows be kept across minutes. Never empty, so a
/// caller can always put the gutter there.
///
/// Wrapped only as far as [`cap_rows`] needs: the first row past [`MAX_ENTRY_ROWS`] says an
/// event is capped, and nothing after it is ever drawn.
///
/// `style` is how the label itself is painted: dim for most kinds, because it is the pane
/// talking rather than the agent, and bold for a summary, which is the one kind worth picking
/// out of the timeline now that nothing stands above it.
pub(super) fn labelled_blocks(
    blocks: &[crate::markdown::MdBlock],
    label: &str,
    indent: usize,
    width: usize,
    style: MdStyle,
) -> (Vec<MdLine>, bool) {
    let label = format!("{label}: ");
    let inline = blocks
        .first()
        .is_some_and(|block| block.prefix.is_empty() && !block.chars.is_empty());
    if inline {
        // Wrapped to what is left after the gutter and the label rather than to the whole
        // half. Only the first row carries the label, so the rows below come out a little
        // short of the edge - but `print_text_with_coordinates` truncates rather than wraps,
        // so wrapping to the full width and *then* prepending would cut the tail off the
        // first row of every entry. The rows below are indented past the gutter instead of
        // starting at the margin, so the ages have a column of their own to be read down.
        let body = width
            .saturating_sub(indent)
            .saturating_sub(label.width())
            .max(1);
        let mut lines = wrap_head(blocks, body, MAX_ENTRY_ROWS);
        let first = lines.remove(0);
        let mut rows = vec![first.prepend(&label, style)];
        rows.extend(lines.into_iter().map(|line| line.indent(indent)));
        return (rows, false);
    }
    // Nothing shares the label's row in this branch, so the body gets the width back - all
    // but the gutter, which every row of an entry leaves clear.
    let mut lines = wrap_head(
        blocks,
        width.saturating_sub(indent).max(1),
        MAX_ENTRY_ROWS - 1,
    );
    let mut labelled = vec![MdLine::plain("").prepend(label.trim_end(), style)];
    labelled.extend(lines.drain(..).map(|line| line.indent(indent)));
    (labelled, true)
}

/// How a history row's `prompt: ` label is painted: dim, the way every other piece of pane
/// furniture is.
pub(super) const LABEL_STYLE: MdStyle = MdStyle {
    bold: false,
    dim: true,
    color: None,
    error: false,
    strike: false,
};

/// How wide the age column is: three columns for the age itself and a space after it, so
/// `now`, `5m`, `3h` and `2d` all line up and every label in the half starts at the same
/// place whether its entry knows when it was made or not.
pub const AGE_WIDTH: usize = 4;

/// How an age is painted: undimmed and in the navigation colour, because it is the one piece of
/// furniture on the row that is meant to be read down the column rather than through the
/// sentence.
const AGE_STYLE: MdStyle = MdStyle {
    bold: false,
    dim: false,
    color: Some(1),
    error: false,
    strike: false,
};

/// The left-hand column of a history row: how long ago the entry was made, right-aligned in
/// [`AGE_WIDTH`], or that much blank for an entry recorded before there was a time to record
/// - the space is kept either way so the labels stay in one column.
pub fn age_gutter(now_ms: i64, then_ms: Option<i64>) -> String {
    match then_ms {
        Some(then_ms) => format!(
            "{:>width$} ",
            short_age(now_ms, then_ms),
            width = AGE_WIDTH - 1
        ),
        None => " ".repeat(AGE_WIDTH),
    }
}

/// How long ago `then_ms` was, in the width a column can afford: `now` under a minute, then
/// whole minutes, hours and days - truncated, not rounded, since a history row cares that an
/// entry is roughly this old, not exactly.
///
/// Never wider than `AGE_WIDTH - 1`, which is what lets [`age_gutter`] right-align it and know
/// every label starts in the same column. That is what caps the day count: past `99d` the
/// answer is "older than this column can say", and a run three months gone is history, where
/// the exact day is not what the column is being read for.
/// Now, as an epoch millisecond — the unit every timestamp in this module is in: a run's
/// `started_at_ms`, an entry's `created_at_ms`, [`short_age`]'s two arguments.
///
/// Re-exported rather than written here: it lives in `yinz-tile` next to
/// `get_local_utc_offset`, which is the host command that makes any of this drawable at all.
pub fn now_ms() -> i64 {
    yinz_tile::prelude::now_ms().get()
}

pub fn short_age(now_ms: i64, then_ms: i64) -> String {
    let seconds = now_ms.saturating_sub(then_ms).max(0) / 1000;
    match seconds {
        s if s < 60 => "now".to_owned(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s => format!("{}d", (s / 86400).min(MAX_AGE_DAYS)),
    }
}

/// The oldest age [`short_age`] will name, so its answer fits the gutter. See there.
const MAX_AGE_DAYS: i64 = 99;

/// An epoch millisecond as the wall clock read where the session runs:
/// `"2026-09-04 14:32:07 -04:00"`.
///
/// Everything a relative [`short_age`] cannot answer: which day, and which minute of it. The
/// two are not alternatives - a run's start is drawn absolutely because it is asked about long
/// after the fact, and every entry under it is drawn as an age because what is being read there
/// is the spacing.
///
/// Re-exported from `yinz-tile`, which is where the civil-calendar arithmetic and the
/// `get_local_utc_offset` host command that feeds it now live, so every plugin that draws a
/// timestamp shares one copy. See `yinz_tile::clock`.
pub use yinz_tile::prelude::wall_clock;

/// How a `summary: ` label is painted instead: bold and undimmed. The summary used to be told
/// apart by standing above the timeline; it is a row of the timeline now, so its label is what
/// marks it.
const SUMMARY_LABEL_STYLE: MdStyle = MdStyle {
    bold: true,
    dim: false,
    color: None,
    error: false,
    strike: false,
};

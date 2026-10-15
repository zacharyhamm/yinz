//! Reading a prompt apart: what a person typed, and the machinery a harness wrapped round it.
//!
//! A `UserPromptSubmit` hook carries "what the user just asked for", and most of the time that
//! is a sentence. Sometimes it is not. Claude Code wakes an agent whose background task has
//! finished by submitting an envelope on its behalf:
//!
//! ```text
//! <task-notification>
//! <task-id>a10fe836c6a9fa4ca</task-id>
//! <output-file>/private/tmp/claude-501/…/a10fe836c6a9fa4ca.output</output-file>
//! <status>completed</status>
//! <summary>Agent "/code-review" finished</summary>
//! <note>…protocol boilerplate addressed to the agent…</note>
//! <result>…possibly tens of kilobytes…</result>
//! </task-notification>
//! ```
//!
//! Drawn as it arrives, that fills the four rows [`crate::agents::MAX_ENTRY_ROWS`] gives a
//! timeline entry with opening tags and a temp path, names the pane after them, and asks the
//! summariser what an agent is doing in XML. This module is what stands between the two.
//!
//! **It never rewrites the record.** `crate::agents::run_entry`'s "Nothing here is cut" still
//! holds: the raw prompt is what is stored, what `HistoryEvent::text` carries and what the
//! reader's `r` shows. Everything here is a *reading* of that text, computed where it is
//! wanted. Two consequences worth having: every run already on disk gets the better rendering
//! for free, and what is stored never becomes a function of which parser version was running
//! when the hook fired.
//!
//! agent already wraps a prompt in a tag of its own — `skills::expand` emits `<skill …>` — and
//! keeps it off the screen by carrying a parallel display string instead of parsing it back
//! out. That is the better pattern *when we are the producer*. Here we are not, so there is no
//! second string to carry, and this parse is what stands in for one.

use std::borrow::Cow;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::markdown::{self, escape_inline, MdLine};

// ---------------------------------------------------------------------------
// What a prompt is made of
// ---------------------------------------------------------------------------

/// One prompt read apart.
///
/// Borrowed all the way down. This runs on the render path of a pane that redraws on every
/// keystroke, and the overwhelming majority of prompts are prose with no envelope in them at
/// all — for those this allocates nothing whatever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt<'a> {
    /// Everything outside every envelope, in order, blank-line-joined. The words a person
    /// actually typed. Empty when the whole prompt was machinery.
    pub prose: Cow<'a, str>,
    /// The envelopes, in the order they appeared.
    pub envelopes: Vec<Envelope<'a>>,
}

/// One top-level element, and the child elements worth pulling out of it.
///
/// Two levels and no more. A tree would be the general answer to a question nobody is asking:
/// the three shapes that exist are a root with leaf children, a root with a body, and a run of
/// sibling leaves, and all three fit here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope<'a> {
    /// The root tag, as it arrived: `task-notification`, `system-reminder`, `command-name`.
    pub tag: &'a str,
    /// Everything between the tags. What a leaf element's text is, and what the fallback
    /// renderer draws the first line of.
    pub body: &'a str,
    /// The child elements this envelope's [`Kind`] asked for, in the order they appeared. A
    /// `Vec` rather than a map: a field may legitimately repeat, order is what the renderer
    /// draws in, and half a dozen pairs are cheaper to walk than to hash.
    pub fields: Vec<Field<'a>>,
    /// The element as it arrived, opening tag to closing tag — or to the end of what arrived,
    /// when `truncated`. The second half of that sentence is the whole of why [`dedupe_id`]
    /// refuses a cut envelope: a reading of this is a reading of what was *delivered*, which
    /// is not the same thing as what was sent.
    ///
    /// A host that absorbed siblings holds only its own element here, not theirs, on either
    /// side of it. Which is why [`Kind::id`] is a question only a kind that absorbs nothing
    /// answers today.
    pub raw: &'a str,
    /// No closing tag was ever found — the event was cut on the way in by
    /// `yinz_tile::agent::HookEvent::pruned_to`, or the writer was.
    pub truncated: bool,
}

impl<'a> Envelope<'a> {
    /// The text of the first child called `name`, trimmed, or `None` when there is none or it
    /// is empty. Every renderer below reads defensively through this: `fields` has no schema
    /// we control, and a harness that drops a field must cost us that line, not the row.
    /// Whether a field called `name` is here at all, empty or not. What tells a sibling that
    /// belongs to this envelope from one that belongs to the next of its kind.
    fn has(&self, name: &str) -> bool {
        self.fields.iter().any(|field| field.name == name)
    }

    pub fn field(&self, name: &str) -> Option<&'a str> {
        self.fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.text.trim())
            .filter(|text| !text.is_empty())
    }
}

/// One child element of an [`Envelope`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field<'a> {
    pub name: &'a str,
    pub text: &'a str,
    /// This child ran to the end of what arrived without its closing tag. Only ever the last
    /// one, and only on a prompt that was itself cut.
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// The scanner
// ---------------------------------------------------------------------------

/// Read `text` apart into prose and envelopes.
///
/// Not an XML parser — a line-anchored element scanner, and deliberately narrow. Four
/// conditions, all required:
///
/// 1. The opening tag begins a line: offset zero, or straight after a newline. Indentation is
///    admitted only for a tag the table already knows — see [`toplevel_opening_at`].
/// 2. The tag name is lowercase **and hyphenated** — `[a-z][a-z0-9]*(-[a-z0-9]+)+` — and the
///    element is not self-closing, which is a shape with no closing tag to find.
/// 3. The closing tag ends a line or hands over to the next element, **or** the text ends with
///    no closing tag at all.
/// 4. Top level only. The scan does not descend into an element it has claimed.
///
/// Condition 2 is what makes the unknown-tag fallback safe: `<div>`, `<b>`, `Vec<String>` and
/// `if x < y` cannot be envelopes, and every envelope a harness actually emits is hyphenated
/// anyway. Condition 3 is what stops a hyphenated word in angle brackets, written mid-sentence
/// by a person, from swallowing the rest of their prompt. Condition 4 is why a `<result>` full
/// of markdown stays the markdown it is.
///
/// The one shape this admits that nobody meant is a hyphenated lowercase tag alone on its lines
/// inside a fenced code block. It costs a fold row that says so, and `r` shows the source, so
/// it is not defended against.
pub fn parse(text: &str) -> Prompt<'_> {
    // Nothing to find, and the common case: one borrow, no allocation, no scan.
    if !text.contains('<') {
        return Prompt {
            prose: Cow::Borrowed(text),
            envelopes: Vec::new(),
        };
    }
    let mut envelopes: Vec<Envelope<'_>> = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let mut prose_from = 0;
    let mut at = 0;
    while at < text.len() {
        let Some((tag, start, body_from)) = toplevel_opening_at(text, at) else {
            at = next_line(text, at);
            continue;
        };
        let (body_to, after, truncated) = closing_of(text, body_from, tag);
        // Up to the tag rather than up to the line, so an indented element's indentation is not
        // part of it. `join_prose` trims each fragment anyway; this is what keeps `raw` honest.
        prose.push(&text[prose_from..start]);
        envelopes.push(Envelope {
            tag,
            body: &text[body_from..body_to],
            // Filled by `group`, which is the only thing that knows what kind this is and so
            // which children are worth pulling out.
            fields: Vec::new(),
            raw: &text[start..after],
            truncated,
        });
        prose_from = after;
        at = after;
    }
    prose.push(&text[prose_from..]);
    group(Prompt {
        prose: join_prose(prose),
        envelopes,
    })
}

/// Which rules an opening tag is held to.
///
/// The two conditions that make the scan safe — begins a line, and carries a hyphen — exist to
/// stop a match claiming text nobody marked for us. Inside an element this scanner has already
/// claimed there is nothing to protect: a child is only ever looked for by a name its [`Kind`]
/// asked about, so `<status>` is safe to recognise and `<b>` is never asked about, and a
/// harness putting two children on one line is a shape rather than a risk.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rules {
    Toplevel,
    Inside,
}

/// The top-level opening tag on the line `at` begins, if there is one: its name, where the tag
/// itself starts, and where its body begins.
///
/// Indentation is the whole of what this adds over [`opening_at`], and only for a tag
/// [`known_tag`] admits. Claude Code's slash-command expansion indents `<command-message>` and
/// `<command-args>` by twelve spaces, and `crate::text::sanitize` turns a leading tab into four
/// more, so a rule that wants the `<` in column zero loses two thirds of that shape to prose —
/// where it is neither escaped nor cut, prose being what a person typed.
///
/// The table is what keeps this narrow. A tag nobody registered still has to open a line of its
/// own, so a hyphenated tag alone on an *indented* line — which in markdown is a code block —
/// stays prose, and condition 1 goes on protecting the one shape it was written for.
fn toplevel_opening_at(text: &str, at: usize) -> Option<(&str, usize, usize)> {
    if at != 0 && text.as_bytes().get(at - 1) != Some(&b'\n') {
        return None;
    }
    if let Some((name, body)) = opening_at(text, at, Rules::Toplevel) {
        return Some((name, at, body));
    }
    let rest = text.get(at..)?;
    let indent = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    if indent == 0 {
        return None;
    }
    // Past the indentation the line-start condition is met by construction, so what is left to
    // hold this to is the table. `Rules::Inside` drops the hyphen rule with it, which costs
    // nothing: every tag `known_tag` admits is hyphenated, and the test walking `KINDS` says so.
    match opening_at(text, at + indent, Rules::Inside) {
        Some((name, body)) if known_tag(name) => Some((name, at + indent, body)),
        _ => None,
    }
}

/// The opening tag at `at`, if there is one: its name, and where its body begins.
///
/// Attributes are accepted and dropped — nothing here reads one, and a tag that grows one must
/// not stop being recognised.
fn opening_at(text: &str, at: usize, rules: Rules) -> Option<(&str, usize)> {
    if rules == Rules::Toplevel && at != 0 && text.as_bytes().get(at - 1) != Some(&b'\n') {
        return None;
    }
    let rest = text.get(at..)?.strip_prefix('<')?;
    let name_len = rest
        .find(|ch: char| !(ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'))
        .unwrap_or(rest.len());
    let name = &rest[..name_len];
    if !is_tag_name(name, rules) {
        return None;
    }
    // Up to the `>` that ends the opening tag. Bounded to this line and stopped by a second
    // `<`, so an unclosed bracket in prose cannot make the rest of the prompt an attribute
    // list.
    let tail = &rest[name_len..];
    let end = tail.find('>')?;
    if tail[..end].contains(['<', '\n']) {
        return None;
    }
    // `<some-thing/>` has no body and no closing tag, so claiming it would leave `closing_of`
    // looking for a closer that is not coming and the element swallowing the rest of the prompt.
    // Refused rather than read as an empty element: an empty one would still draw a row and name
    // a pane, which is a lot of screen to spend on a tag that said nothing.
    if tail[..end].ends_with('/') {
        return None;
    }
    Some((name, at + 1 + name_len + end + 1))
}

/// Whether `name` is a tag name this scanner will admit under `rules`.
fn is_tag_name(name: &str, rules: Rules) -> bool {
    if !name.starts_with(|ch: char| ch.is_ascii_lowercase()) || name.ends_with('-') {
        return false;
    }
    if name.contains("--") {
        return false;
    }
    rules == Rules::Inside || name.contains('-')
}

/// Where the element opened at `body_from` ends: `(body end, one past the closing tag, cut)`.
///
/// A closing tag has to end a line or hand over to the next element, but need not begin one,
/// because a leaf element is routinely written whole on one —
/// `<command-name>/plan</command-name>`. Nested elements of the same name are counted, so a
/// `<result>` quoting a `<result>` cannot close its parent early.
///
/// The counting is the looser half of the pair, knowingly: [`next_opening`] admits a nested
/// opening anywhere, so a body that names its own tag mid-sentence still deepens the count and
/// can leave the element reading as cut. Tightening that costs more than the shape is worth —
/// what a person writes mid-sentence is a *closing* tag, which is the half held to a rule.
///
/// Running off the end is not a failure: it is what a prompt cut by the broadcast cap looks
/// like. The body is then everything that arrived, and `cut` says so.
fn closing_of(text: &str, body_from: usize, tag: &str) -> (usize, usize, bool) {
    let closer = format!("</{tag}>");
    let mut depth = 1usize;
    let mut at = body_from;
    while at < text.len() {
        let close = next_closing(text, at, &closer);
        let open = next_opening(text, at, tag);
        // Whichever comes first. A nested opening before the next closer is a level down; a
        // closer before the next opening is a level up, and the last one is the answer.
        match close {
            Some((from, after)) if open.is_none_or(|(nested, _)| nested > from) => {
                depth -= 1;
                if depth == 0 {
                    return (from, after, false);
                }
                at = after;
            },
            _ => match open {
                Some((_, body)) => {
                    depth += 1;
                    at = body;
                },
                None => break,
            },
        }
    }
    (text.len(), text.len(), true)
}

/// The first `</tag>` at or after `from` that ends its line or hands over to the next element,
/// as `(its start, one past it)`.
///
/// Three things may follow a closer, trailing spaces aside: the end of the text, a line ending,
/// or another element. The first two are how a harness writing one field to a line ends one; the
/// third is how it ends a field when it writes two to a line, and
/// `<status>ok</status> <summary>hi</summary>` is a shape we have to read rather than one to
/// defend against. What all three still refuse is `the scanner looks for </result> at the end of
/// a line` — a person writing a closing tag mid-sentence, which is what the condition is for.
///
/// `\r` is admitted beside `\n` because the live path does not sanitise: `agent-service` reads
/// the prompt as it arrived, so a harness ending its lines the Windows way would otherwise have
/// every element it sends read as cut, and lose the identity a cut element has no right to.
fn next_closing(text: &str, from: usize, closer: &str) -> Option<(usize, usize)> {
    text.get(from..)?.match_indices(closer).find_map(|(at, _)| {
        let at = from + at;
        let after = at + closer.len();
        let rest = text[after..].trim_start_matches([' ', '\t']);
        matches!(
            rest.as_bytes().first(),
            None | Some(b'\n') | Some(b'\r') | Some(b'<')
        )
        .then_some((at, after))
    })
}

/// The first opening of `tag` at or after `from`, as `(its start, where its body begins)`.
fn next_opening(text: &str, from: usize, tag: &str) -> Option<(usize, usize)> {
    let open = format!("<{tag}");
    text.get(from..)?.match_indices(&open).find_map(|(at, _)| {
        let at = from + at;
        match opening_at(text, at, Rules::Inside) {
            Some((name, body)) if name == tag => Some((at, body)),
            _ => None,
        }
    })
}

/// The start of the line after the one `at` is in, or the end of the text.
fn next_line(text: &str, at: usize) -> usize {
    match text[at..].find('\n') {
        Some(offset) => at + offset + 1,
        None => text.len(),
    }
}

/// The prose fragments as one value, borrowing where it can.
fn join_prose(fragments: Vec<&str>) -> Cow<'_, str> {
    let mut kept = fragments
        .into_iter()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let Some(first) = kept.next() else {
        return Cow::Borrowed("");
    };
    match kept.next() {
        None => Cow::Borrowed(first),
        Some(second) => {
            let mut joined = format!("{first}\n\n{second}");
            for rest in kept {
                joined.push_str("\n\n");
                joined.push_str(rest);
            }
            Cow::Owned(joined)
        },
    }
}

/// Give each envelope the fields its kind asks for, and fold absorbed siblings into the
/// envelope they belong to.
///
/// Separate from the scan because only the [`Kind`] knows which children are worth pulling out,
/// and the scan is what finds the kind.
///
/// Absorption runs **both ways**. Which of a shape's elements a harness writes first is its own
/// business — Claude Code has spelled a slash command with the `<command-message>` ahead of the
/// `<command-name>` it belongs to and behind it — and a rule that only looked forward read the
/// first of those as two envelopes: a nameless message, and a command that had lost it.
fn group(prompt: Prompt<'_>) -> Prompt<'_> {
    let Prompt { prose, envelopes } = prompt;
    let mut grouped: Vec<Envelope<'_>> = Vec::with_capacity(envelopes.len());
    for envelope in envelopes {
        if let Some(host) = grouped.last_mut() {
            // Once each. A second `<command-message>` is a second command starting, not a first
            // command saying it twice, and a host that took it would leave the command it
            // belongs to nameless — which is the bug this whole function had in the other
            // direction.
            if kind_of(host.tag).absorbs.contains(&envelope.tag) && !host.has(envelope.tag) {
                host.fields.push(field_of(&envelope));
                continue;
            }
        }
        let kind = kind_of(envelope.tag);
        let mut envelope = envelope;
        if !kind.absorbs.is_empty() {
            // The siblings that arrived before the host they belong to, taken back off the end
            // and turned round again, so the fields read in the order the harness wrote them.
            // What is dropped with them is their own `fields`, which is sound for exactly one
            // reason: nothing any kind absorbs is itself a `KINDS` tag, so every one of them is
            // `GENERIC`, and `GENERIC` asks for no children.
            let mut before: Vec<Envelope<'_>> = Vec::new();
            while grouped.last().is_some_and(|prev| {
                kind.absorbs.contains(&prev.tag)
                    && !before.iter().any(|taken| taken.tag == prev.tag)
            }) {
                before.push(grouped.pop().expect("the one just looked at"));
            }
            envelope.fields.extend(before.iter().rev().map(field_of));
            // A kind that absorbs siblings is one whose own body is a value rather than a
            // container: `<command-name>/plan</command-name>` is the command. Naming it after
            // its own tag is what lets the renderer read every part of the shape one way.
            let own = field_of(&envelope);
            envelope.fields.push(own);
        }
        envelope.fields.extend(children(envelope.body, kind.fields));
        grouped.push(envelope);
    }
    Prompt {
        prose,
        envelopes: grouped,
    }
}

/// An envelope as a field of the one that absorbed it.
fn field_of<'a>(envelope: &Envelope<'a>) -> Field<'a> {
    Field {
        name: envelope.tag,
        text: envelope.body,
        truncated: envelope.truncated,
    }
}

/// The children of `body` that `names` asks for, in the order they appear.
///
/// Looked for by name rather than discovered, which is why these need no hyphen: `<status>` is
/// only recognised inside an envelope whose kind declared it, and nothing asks about `<b>`.
fn children<'a>(body: &'a str, names: &[&str]) -> Vec<Field<'a>> {
    if names.is_empty() || !body.contains('<') {
        return Vec::new();
    }
    let mut fields = Vec::new();
    let mut at = 0;
    while let Some(offset) = body.get(at..).and_then(|rest| rest.find('<')) {
        let start = at + offset;
        match opening_at(body, start, Rules::Inside) {
            Some((name, from)) if names.contains(&name) => {
                let (to, after, truncated) = closing_of(body, from, name);
                fields.push(Field {
                    name,
                    text: &body[from..to],
                    truncated,
                });
                at = after;
            },
            _ => at = start + 1,
        }
    }
    fields
}

// ---------------------------------------------------------------------------
// The kinds
// ---------------------------------------------------------------------------

/// One kind of envelope: how to recognise it, and how to say what it is.
///
/// A `&'static` table of function pointers rather than `Box<dyn …>`. A vector of boxed
/// trait objects built at load costs an
/// allocation per kind and an indirection per call, and buys extensibility across a crate
/// boundary that does not exist. A `&'static` table is also walkable by a test, which is how
/// `RunEntryKind::label` and `from_label` are already held to each other.
///
/// Adding a kind is a function per question and a line in [`KINDS`].
pub struct Kind {
    /// The root tag, lowercase and hyphenated.
    pub tag: &'static str,
    /// Sibling tags that belong to this one when they follow it directly. The slash-command
    /// shape is three top-level elements that mean one thing, and this is what makes them one
    /// [`Envelope`] so every renderer below sees one shape rather than three.
    pub absorbs: &'static [&'static str],
    /// The child tags worth pulling out of the body. Anything else in there stays in
    /// [`Envelope::body`], which `r` still shows.
    pub fields: &'static [&'static str],
    /// One line naming this, for a pane title and an archive row. `None` when there is nothing
    /// in it worth naming a pane after.
    pub title: fn(&Envelope) -> Option<String>,
    /// The collapsed form, appended as markdown source. Written into the caller's buffer rather
    /// than returned: a prompt may carry several envelopes and one `String` should serve.
    pub digest: fn(&Envelope, &mut String),
    /// The reader's folds, appended in the order they should be offered.
    pub sections: fn(&Envelope, &mut Vec<Section>),
    /// What the summariser should be told this said. `None` drops it entirely — which is what a
    /// `<note>` of protocol boilerplate is worth to a model being asked what an agent is doing.
    pub question: fn(&Envelope) -> Option<String>,
    /// What a redelivery of this exact envelope would share, for the ingest dedupe. `None`
    /// where an envelope has no identity of its own.
    ///
    /// Only ever asked of an envelope that arrived **whole**: [`dedupe_id`] drops a cut one
    /// before it reaches here, so an implementation may read [`Envelope::raw`] and every field
    /// as the whole of what was sent. A kind that `absorbs` siblings has to check its own
    /// fields' [`Field::truncated`] too — a cut child lands on a host whose own flag is
    /// `false`.
    pub id: fn(&Envelope) -> Option<String>,
}

pub static KINDS: &[&Kind] = &[&TASK_NOTIFICATION, &SYSTEM_REMINDER, &SLASH_COMMAND];

/// The kind an unregistered tag gets: its name, the first line of its body, and how much was
/// left out.
///
/// The point of a table is not that we know every envelope a harness will ever invent — it is
/// that one we have never seen still reads better than its own markup, and can be read whole
/// with `Enter`. Not in [`KINDS`], because nothing looks it up by tag, but a [`Kind`] all the
/// same so every path below has exactly one shape to handle.
static GENERIC: Kind = Kind {
    tag: "",
    absorbs: &[],
    fields: &[],
    title: generic_title,
    digest: generic_digest,
    sections: generic_sections,
    question: generic_question,
    id: |_| None,
};

/// Whether the table knows `tag`: a kind's own tag, or one some kind absorbs.
///
/// What [`toplevel_opening_at`] holds an indented tag to, and the only thing it is asked about.
fn known_tag(tag: &str) -> bool {
    KINDS
        .iter()
        .any(|kind| kind.tag == tag || kind.absorbs.contains(&tag))
}

/// The kind that reads `tag`, or [`GENERIC`].
fn kind_of(tag: &str) -> &'static Kind {
    KINDS
        .iter()
        .copied()
        .find(|kind| kind.tag == tag)
        .unwrap_or(&GENERIC)
}

// ---------------------------------------------------------------------------
// The four questions
// ---------------------------------------------------------------------------

/// The prompt as a timeline entry should draw it, as **markdown source**.
///
/// Markdown rather than styled lines because the timeline already parses, wraps, labels,
/// gutters and caps every entry it draws (`crate::agents::labelled`, `cap_rows`), and a second
/// renderer beside that one would have to reproduce all five. Every value interpolated here
/// goes through [`escape_inline`] first: a summary that happens to contain a `*` is a summary,
/// not emphasis.
///
/// The prose a person typed leads, and is **not** escaped and **not** cut — what a user wrote
/// is what the timeline is for, and it was already markdown before any of this. Only what a
/// machine wrote at them is bounded.
///
/// `None` for a prompt that is prose and nothing else: the common case, where the cheapest and
/// most honest thing to do is nothing at all.
pub fn digest(text: &str) -> Option<String> {
    let prompt = parse(text);
    if prompt.envelopes.is_empty() {
        return None;
    }
    let mut out = String::new();
    if !prompt.prose.is_empty() {
        out.push_str(&prompt.prose);
        out.push_str("\n\n");
    }
    for envelope in &prompt.envelopes {
        (kind_of(envelope.tag).digest)(envelope, &mut out);
    }
    Some(out.trim_end().to_owned())
}

/// One line naming this prompt, for a pane title and an archive row.
///
/// `None` for prose, whose callers already have a better line: the prompt itself.
pub fn title(text: &str) -> Option<String> {
    let prompt = parse(text);
    if !prompt.prose.trim().is_empty() {
        return None;
    }
    prompt
        .envelopes
        .iter()
        .find_map(|envelope| (kind_of(envelope.tag).title)(envelope))
}

/// What the summariser should be told this prompt said.
///
/// The whole point is that a model asked "what is this agent doing" gets a sentence rather than
/// an XML document. Prose passes through untouched; an envelope is reduced to the part of it
/// that answers the question, and one with no answer in it is dropped.
pub fn question(text: &str) -> String {
    let prompt = parse(text);
    if prompt.envelopes.is_empty() {
        return prompt.prose.into_owned();
    }
    let mut parts: Vec<String> = Vec::new();
    if !prompt.prose.is_empty() {
        parts.push(prompt.prose.into_owned());
    }
    parts.extend(
        prompt
            .envelopes
            .iter()
            .filter_map(|envelope| (kind_of(envelope.tag).question)(envelope)),
    );
    parts.join("\n\n")
}

/// The folds a reader opens this prompt into. Empty for a prompt that is prose, which is what
/// leaves the reader exactly as it was for everything that is not an envelope.
pub fn sections(text: &str) -> Vec<Section> {
    let prompt = parse(text);
    if prompt.envelopes.is_empty() {
        return Vec::new();
    }
    let mut sections = Vec::new();
    if !prompt.prose.is_empty() {
        sections.push(Section::of("prompt", &prompt.prose));
    }
    for envelope in &prompt.envelopes {
        (kind_of(envelope.tag).sections)(envelope, &mut sections);
    }
    sections
}

/// What a redelivery of this prompt would share, for the ingest dedupe. See
/// `crate::agents::run_event_id`.
///
/// **A cut envelope has no identity.** `HookEvent::pruned_to` takes a prompt at a fixed length
/// with no marker, so a `<task-notification>` whose `<result>` runs to tens of kilobytes always
/// arrives cut at the same offset — and two reports of one resumed task agree on every byte
/// before it. The bytes that tell them apart are exactly the bytes that did not arrive, so no
/// key computed from what did arrive can do it. That matters more than it looks: the ingest
/// path *drops* on a match rather than showing one, so a key that overclaims loses a real event
/// rather than showing a duplicate. Refusing is what falls back to the adjacent-only compare,
/// which errs the other way.
///
/// **Every envelope that has one, not the first.** A prompt is one delivery, so its identity is
/// the identity of everything identifiable in it — two background tasks reporting together are
/// `{A, B}`, and a later `{A, C}` is a different prompt rather than a redelivery of it.
pub fn dedupe_id(text: &str) -> Option<String> {
    let ids: Vec<String> = parse(text)
        .envelopes
        .iter()
        // Only ever the last one, since a cut element swallows the rest of the text. Spelt
        // per envelope anyway, because the rule is about what an identity may be taken from.
        .filter(|envelope| !envelope.truncated)
        .filter_map(|envelope| (kind_of(envelope.tag).id)(envelope))
        .collect();
    (!ids.is_empty()).then(|| ids.join("+"))
}

// ---------------------------------------------------------------------------
// The reader's folds
// ---------------------------------------------------------------------------

/// One fold of the reader.
///
/// Owned, unlike everything above it: `Reading` holds these for as long as the reader is up,
/// and the run behind them goes on growing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// What the fold row calls it: `summary`, `note`, `result`, or the tag of an element this
    /// scanner did not know.
    pub name: String,
    pub text: String,
    /// Whether the body is drawn. Meaningless for a one-line section, which is always drawn
    /// because there is nothing to fold away.
    pub open: bool,
    /// A file this section names, which `o` opens in a pane.
    pub path: Option<String>,
    /// How many lines `text` is, which is what a shut fold row reports.
    pub lines: usize,
    pub truncated: bool,
}

impl Section {
    /// A fold holding `text`, open if it is short enough to be worth showing at once.
    fn of(name: &str, text: &str) -> Section {
        let text = text.trim().to_owned();
        let lines = text.lines().count().max(1);
        Section {
            name: name.to_owned(),
            // A section with nothing to fold away is always drawn; a long one starts shut,
            // which is the whole reason the reader is a reader rather than a wall.
            open: lines <= 1,
            path: None,
            lines,
            text,
            truncated: false,
        }
    }

    /// [`Section::of`], shut whatever its length: what a `<note>` of boilerplate and a
    /// `<result>` of tens of kilobytes are both worth on opening.
    fn shut(name: &str, text: &str, truncated: bool) -> Section {
        Section {
            open: false,
            truncated,
            ..Section::of(name, text)
        }
    }

    /// Whether this section has a body worth folding away. One line is not: it fits the row
    /// its own name is on.
    pub fn foldable(&self) -> bool {
        self.lines > 1
    }
}

/// One section of [`wrap_sections`], parsed: its fold row, and its body when it is open.
///
/// Apart from the wrap so a caller that redraws the reader at many widths - every step of a
/// pane-edge drag - can keep this, which depends on nothing but the section, and parse again
/// only the one whose fold was toggled.
pub fn section_blocks(section: &Section) -> Vec<markdown::MdBlock> {
    let name = escape_inline(&section.name);
    let cut = match section.truncated {
        true => " · cut",
        false => "",
    };
    let source = match (section.foldable(), section.open) {
        (false, _) => match &section.path {
            Some(path) => format!("**{name}** {}", escape_inline(path)),
            None => format!("**{name}** {}", escape_inline(&section.text)),
        },
        (true, false) => format!("▸ **{name}** · {} lines{cut}", section.lines),
        (true, true) => format!(
            "▾ **{name}** · {} lines{cut}\n\n{}",
            section.lines, section.text
        ),
    };
    markdown::parse(&source)
}

/// The reader's body: every section's rows, and where each section's rows begin.
///
/// Rendered per section and concatenated rather than as one document, so a caller can put a
/// cursor on a *section* rather than on a line — the same bargain `HistoryLines::events`
/// strikes over its `entries`, and for the same reason. The spans tile `lines` exactly.
///
/// A fold's body is drawn flush left and **unescaped**: a `<result>` is prose the agent wrote,
/// and it gets rendered the way a `result:` entry already is, headings and lists and all.
/// Indenting it would cost the reader the columns it exists to provide, and four spaces is a
/// code block in markdown anyway.
///
/// Takes sections already parsed by [`section_blocks`], one slice per section.
pub fn wrap_sections<'a>(
    sections: impl IntoIterator<Item = &'a [markdown::MdBlock]>,
    width: usize,
) -> ReaderLines {
    let mut lines: Vec<MdLine> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    for blocks in sections {
        starts.push(lines.len());
        lines.extend(markdown::wrap(blocks, width, markdown::Options::compact()));
    }
    ReaderLines { lines, starts }
}

/// [`wrap_sections`]' answer: the rows, and one start per section.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReaderLines {
    pub lines: Vec<MdLine>,
    /// One entry per section: the first row of `lines` it owns.
    pub starts: Vec<usize>,
}

// ---------------------------------------------------------------------------
// task-notification
// ---------------------------------------------------------------------------

static TASK_NOTIFICATION: Kind = Kind {
    tag: "task-notification",
    absorbs: &[],
    fields: &[
        "task-id",
        "output-file",
        "status",
        "summary",
        "note",
        "result",
    ],
    title: task_title,
    digest: task_digest,
    sections: task_sections,
    question: task_question,
    id: task_id,
};

/// How much of a machine-written summary a collapsed row carries.
///
/// Forty-eight, measured against the narrowest half-pane worth protecting rather than picked.
/// At forty-six columns the body has forty-four; a row of an entry loses four to the age gutter
/// and the first loses eight more to `prompt: `, so the headline has thirty-two columns and
/// then forty. The status glyph, the short id and their separator spend thirteen of the first,
/// which leaves fifty-nine for the summary across two rows — forty-eight of summary, and the
/// slack a word wrap needs not to spill onto a third.
///
/// Two rows is the number that matters, because `crate::agents::MAX_ENTRY_ROWS` is four and the
/// row saying what `Enter` would show is the one worth protecting: a headline that spends three
/// leaves `cap_rows` to eat the affordances row, and an entry that does not say it has more
/// behind it is an entry nobody presses Enter on. `the_digest_fits_the_rows_a_timeline_entry_gets`
/// in `crate::agents` is what holds this to it.
///
/// A person's own prose is deliberately not held to this. See [`digest`].
const DIGEST_SUMMARY_COLS: usize = 48;

/// How much of a task id a row shows.
///
/// Eight, the length git settled on for the same job: the ids being told apart are the handful
/// one agent has been woken for, not every id there could be.
const SHORT_TASK_ID: usize = 8;

fn task_digest(envelope: &Envelope, out: &mut String) {
    let mut row = String::new();
    if let Some(mark) = status_mark(envelope.field("status")) {
        row.push_str(&mark);
        row.push(' ');
    }
    if let Some(id) = envelope.field("task-id") {
        row.push_str(&format!("**{}** · ", escape_inline(&short_id(id))));
    }
    match envelope.field("summary") {
        Some(summary) => row.push_str(&escape_inline(&cut(summary, DIGEST_SUMMARY_COLS))),
        // No summary is not no row: which task reported, and how it went, is most of what the
        // row was for.
        None => row.push_str("background task"),
    }
    out.push_str(row.trim_end_matches(" · "));
    out.push_str("\n\n");
    affordances(
        envelope,
        out,
        &[
            ("note", "note"),
            ("result", "result"),
            ("output-file", "output-file"),
        ],
    );
}

fn task_title(envelope: &Envelope) -> Option<String> {
    // `task · ` rather than the summary alone: a tab bar reading `Agent "…" finished` with no
    // marker reads as something the user asked for, and this pane was woken by a background
    // task nobody was watching.
    let summary = envelope
        .field("summary")
        .map(str::to_owned)
        .or_else(|| envelope.field("task-id").map(short_id))?;
    Some(format!("task · {summary}"))
}

fn task_sections(envelope: &Envelope, out: &mut Vec<Section>) {
    for name in ["status", "task-id", "summary"] {
        if let Some(text) = envelope.field(name) {
            out.push(Section::of(name, text));
        }
    }
    if let Some(path) = envelope.field("output-file") {
        out.push(Section {
            path: Some(path.to_owned()),
            ..Section::of("output-file", path)
        });
    }
    for name in ["note", "result"] {
        if let Some(field) = envelope.fields.iter().find(|field| field.name == name) {
            out.push(Section::shut(name, field.text, field.truncated));
        }
    }
}

fn task_question(envelope: &Envelope) -> Option<String> {
    let status = envelope.field("status").unwrap_or("finished");
    let id = envelope.field("task-id").map(short_id);
    let named = match id {
        Some(id) => format!("background task {id} {status}"),
        None => format!("a background task {status}"),
    };
    let mut question = match envelope.field("summary") {
        Some(summary) => format!("{named}: {summary}"),
        None => named,
    };
    // The result is what the turn is actually about. `note` is protocol boilerplate addressed
    // to the agent — a model asked what this agent is doing does not need to be told that a
    // task-id may notify twice — and `output-file` is a path it cannot read.
    if let Some(result) = envelope.field("result") {
        question.push_str("\n\n");
        question.push_str(result);
    }
    Some(question)
}

/// What a redelivery of this notification shares.
///
/// Not the task id alone. The envelope says so itself — *"the same task-id may notify more than
/// once"* — because a resumed agent notifies again with a **different result**, and that is a
/// second thing happening rather than a second delivery of the first. Hashing the whole element
/// is what tells the two apart: an exact redelivery from anywhere in the run collapses, and a
/// genuinely new notification for the same task lands as its own entry, where the short id on
/// its row is what makes the repetition legible.
///
/// Which is only true of an element that arrived whole, and is why [`dedupe_id`] will not hand
/// this a cut one: hashing a fixed-length prefix of a notification identifies the prefix, not
/// the notification.
fn task_id(envelope: &Envelope) -> Option<String> {
    let id = envelope.field("task-id")?;
    Some(format!(
        "task-notification:{id}:{:016x}",
        fnv1a(envelope.raw)
    ))
}

// ---------------------------------------------------------------------------
// system-reminder
// ---------------------------------------------------------------------------

static SYSTEM_REMINDER: Kind = Kind {
    tag: "system-reminder",
    absorbs: &[],
    fields: &[],
    // Never names a pane. A reminder is the harness talking to the agent about how to behave;
    // it says nothing about what the turn is for, which is the whole job of a title.
    title: |_| None,
    digest: |envelope, out| affordances(envelope, out, &[]),
    sections: |envelope, out| {
        out.push(Section::shut(
            "system-reminder",
            envelope.body,
            envelope.truncated,
        ))
    },
    question: |_| None,
    id: |_| None,
};

// ---------------------------------------------------------------------------
// command-name — the slash-command shape
// ---------------------------------------------------------------------------

static SLASH_COMMAND: Kind = Kind {
    tag: "command-name",
    // Three top-level siblings that mean one thing. Absorbing them is what lets every renderer
    // here read one envelope rather than reassembling a shape.
    absorbs: &["command-message", "command-args", "local-command-stdout"],
    fields: &[],
    title: command_title,
    digest: command_digest,
    sections: command_sections,
    question: |envelope| command_line(envelope).map(|line| format!("the user ran {line}")),
    id: |_| None,
};

/// `/plan design the renderer` — the command and what was typed after it.
fn command_line(envelope: &Envelope) -> Option<String> {
    let name = envelope.field("command-name")?;
    Some(match envelope.field("command-args") {
        Some(args) => format!("{name} {args}"),
        None => name.to_owned(),
    })
}

fn command_digest(envelope: &Envelope, out: &mut String) {
    if let Some(line) = command_line(envelope) {
        out.push_str(&escape_inline(&cut(&line, DIGEST_SUMMARY_COLS)));
        out.push_str("\n\n");
    }
    affordances(envelope, out, &[("local-command-stdout", "stdout")]);
}

fn command_title(envelope: &Envelope) -> Option<String> {
    command_line(envelope)
}

fn command_sections(envelope: &Envelope, out: &mut Vec<Section>) {
    if let Some(line) = command_line(envelope) {
        out.push(Section::of("command", &line));
    }
    if let Some(field) = envelope
        .fields
        .iter()
        .find(|field| field.name == "local-command-stdout")
    {
        out.push(Section::shut("stdout", field.text, field.truncated));
    }
}

// ---------------------------------------------------------------------------
// The unknown-tag fallback
// ---------------------------------------------------------------------------

/// An unknown envelope leads with the first line of what it said rather than with its tag,
/// because the marker row below already names the tag and a row that says `some-new-thing`
/// twice has spent one of four on nothing.
fn generic_digest(envelope: &Envelope, out: &mut String) {
    let first = first_line(envelope.body);
    match first.is_empty() {
        true => out.push_str(&escape_inline(envelope.tag)),
        false => out.push_str(&escape_inline(&cut(first, DIGEST_SUMMARY_COLS))),
    }
    out.push_str("\n\n");
    out.push_str(&mark(
        envelope.tag,
        envelope.body.trim().lines().count(),
        envelope.truncated,
    ));
    out.push_str("\n\n");
}

fn generic_title(envelope: &Envelope) -> Option<String> {
    let first = first_line(envelope.body);
    match first.is_empty() {
        true => Some(envelope.tag.to_owned()),
        false => Some(format!("{} · {first}", envelope.tag)),
    }
}

fn generic_sections(envelope: &Envelope, out: &mut Vec<Section>) {
    out.push(Section::shut(
        envelope.tag,
        envelope.body,
        envelope.truncated,
    ));
}

/// An unknown envelope is still *something the harness said to the agent*, so the summariser
/// gets its first line rather than nothing at all — but not its body, which we cannot judge.
fn generic_question(envelope: &Envelope) -> Option<String> {
    let first = first_line(envelope.body);
    match first.is_empty() {
        true => None,
        false => Some(format!("{}: {first}", envelope.tag)),
    }
}

// ---------------------------------------------------------------------------
// Shared pieces of a digest
// ---------------------------------------------------------------------------

/// The row that says what pressing `Enter` would show: `▸ note 2 lines  ▸ result 412 lines`.
///
/// Its own markdown block, so `Options::compact()` puts it hard against the summary above it
/// with no blank row between. `named` lists the fields worth a marker, in the order they should
/// appear, each with the word to call it by — a `local-command-stdout` spends a whole row of a
/// narrow half on its own name, and the reader calls that fold `stdout` anyway.
///
/// A kind with none of them advertises its own tag instead, which is what a `system-reminder`
/// under somebody's prose comes to: one word saying there is machinery here and `Enter` shows
/// it.
fn affordances(envelope: &Envelope, out: &mut String, named: &[(&str, &str)]) {
    let mut marks: Vec<String> = Vec::new();
    for (name, label) in named {
        let Some(field) = envelope.fields.iter().find(|field| field.name == *name) else {
            continue;
        };
        marks.push(mark(
            label,
            field.text.trim().lines().count(),
            field.truncated,
        ));
    }
    if marks.is_empty() {
        marks.push(mark(envelope.tag, 1, envelope.truncated));
    }
    out.push_str(&marks.join("  "));
    out.push_str("\n\n");
}

/// One `▸ name 12 lines` marker. The line count is dropped where there is only one, which is a
/// number that says nothing and a word that would push a real one off a narrow row.
fn mark(label: &str, lines: usize, truncated: bool) -> String {
    let cut = match truncated {
        true => " · cut",
        false => "",
    };
    match lines > 1 {
        true => format!("▸ {} {lines} lines{cut}", escape_inline(label)),
        false => format!("▸ {}{cut}", escape_inline(label)),
    }
}

/// A status as one glyph, or as a glyph and the word itself where the word is not one this
/// knows. Guessing at an unrecognised status would be worse than showing it.
fn status_mark(status: Option<&str>) -> Option<String> {
    let status = status?;
    Some(match status {
        "completed" | "success" | "succeeded" | "ok" => "✓".to_owned(),
        "failed" | "failure" | "error" => "✗".to_owned(),
        "running" | "in_progress" | "pending" => "…".to_owned(),
        other => format!("· {}", escape_inline(other)),
    })
}

/// `text` as one line, no wider than `cols` columns, with `…` where anything was left out.
///
/// Columns rather than characters because every number the budget was derived from is a column
/// — see [`DIGEST_SUMMARY_COLS`], which reasons in them from the pane width down. A summary
/// written in a script that spends two columns to the character is exactly the one that would
/// otherwise take a third row and cost the entry the row saying what `Enter` would show.
///
/// Broken at a word where there is one to break at: a summary that stops at `unsta…` reads as
/// a bug, and the row it is on has already given up on being complete. Only when a single word
/// is itself over the budget is it cut mid-word, because there is nothing else to do with it.
/// The `…` is spent out of the budget rather than added to it, so what comes back is never
/// wider than what was asked for.
fn cut(text: &str, cols: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.width() <= cols {
        return flat;
    }
    let budget = cols.saturating_sub(1);
    let mut width = 0;
    let mut end = flat.len();
    for (at, ch) in flat.char_indices() {
        width += ch.width().unwrap_or(0);
        if width > budget {
            end = at;
            break;
        }
    }
    let kept = &flat[..end];
    // The tail is mid-word unless the character that would have followed it is a space.
    let trimmed = match flat[end..].starts_with(' ') {
        true => kept,
        false => kept.rsplit_once(' ').map(|(head, _)| head).unwrap_or(kept),
    };
    format!("{}…", trimmed.trim_end_matches([' ', ',', '.', ';', ':']))
}

/// The first non-empty line of `text`, trimmed.
fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// A task id cut to [`SHORT_TASK_ID`].
fn short_id(id: &str) -> String {
    id.chars().take(SHORT_TASK_ID).collect()
}

/// FNV-1a over `text`.
///
/// Here rather than through a hasher because what is wanted is a *stable* number: this one
/// lands in a dedupe key that is compared against keys written by an earlier run of the
/// process, and `DefaultHasher` promises nothing across versions.
fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A notification of the shape Claude Code actually submits.
    fn notification() -> String {
        [
            "<task-notification>",
            "<task-id>a10fe836c6a9fa4ca</task-id>",
            "<output-file>/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output</output-file>",
            "<status>completed</status>",
            "<summary>Agent \"/code-review high up to 3 subagents\" finished</summary>",
            "<note>A task-notification fires each time this agent stops.",
            "The same task-id may notify more than once.</note>",
            "<result>The review found four issues.",
            "Two are in the scanner.</result>",
            "</task-notification>",
        ]
        .join("\n")
    }

    /// A notification for `task` whose result says `result`. The pairs below need two that
    /// agree for a while and then do not, which the fixed [`notification`] cannot give.
    fn notification_of(task: &str, result: &str) -> String {
        format!(
            "<task-notification>\n<task-id>{task}</task-id>\n<status>completed</status>\n<result>{result}</result>\n</task-notification>"
        )
    }

    fn names(sections: &[Section]) -> Vec<&str> {
        sections.iter().map(|s| s.name.as_str()).collect()
    }

    // -- the scanner --------------------------------------------------------

    /// The common case, and the one that must cost nothing: a prompt with no angle bracket in
    /// it is borrowed whole and never scanned.
    #[test]
    fn a_prompt_with_no_angle_bracket_is_prose_and_nothing_else() {
        let prompt = parse("fix the flaky reconnect test");
        assert_eq!(prompt.prose, "fix the flaky reconnect test");
        assert!(prompt.envelopes.is_empty());
        assert!(matches!(prompt.prose, Cow::Borrowed(_)), "it was copied");
        assert_eq!(digest("fix the flaky reconnect test"), None);
    }

    /// Every field the kind asked for comes out, in the order it arrived, and the raw slice is
    /// the whole element — which is what the reader's `r` shows.
    #[test]
    fn a_task_notification_becomes_its_fields() {
        let text = notification();
        let prompt = parse(&text);
        assert_eq!(prompt.prose, "");
        assert_eq!(prompt.envelopes.len(), 1);
        let envelope = &prompt.envelopes[0];
        assert_eq!(envelope.tag, "task-notification");
        assert!(!envelope.truncated);
        assert_eq!(envelope.raw, text.trim());
        assert_eq!(
            envelope.fields.iter().map(|f| f.name).collect::<Vec<_>>(),
            [
                "task-id",
                "output-file",
                "status",
                "summary",
                "note",
                "result"
            ]
        );
        assert_eq!(envelope.field("task-id"), Some("a10fe836c6a9fa4ca"));
        assert_eq!(envelope.field("status"), Some("completed"));
        assert_eq!(
            envelope.field("result"),
            Some("The review found four issues.\nTwo are in the scanner.")
        );
    }

    /// Two levels and no more. A `<result>` full of markup is a result, and the markup inside
    /// it comes back untouched rather than being read as structure.
    #[test]
    fn a_field_keeps_the_tags_nested_inside_it() {
        let text = "<task-notification>\n<result>see <b>this</b> and\n<other-thing>that</other-thing>\n</result>\n</task-notification>";
        let prompt = parse(text);
        assert_eq!(
            prompt.envelopes[0].field("result"),
            Some("see <b>this</b> and\n<other-thing>that</other-thing>")
        );
        assert_eq!(
            prompt.envelopes.len(),
            1,
            "the nested tag was not top level"
        );
    }

    /// A same-named child must not close its parent. Without the depth count the outer element
    /// would end at the inner one's closer and the rest of the prompt would become prose.
    #[test]
    fn a_same_named_child_does_not_close_its_parent_early() {
        let text = "<some-thing>\n<some-thing>inner</some-thing>\nouter tail\n</some-thing>\ntrailing prose";
        let prompt = parse(text);
        assert_eq!(prompt.envelopes.len(), 1);
        assert_eq!(
            prompt.envelopes[0].body.trim(),
            "<some-thing>inner</some-thing>\nouter tail"
        );
        assert_eq!(prompt.prose, "trailing prose");
    }

    /// `HookEvent::pruned_to` cuts a long prompt, so an element that never closes is a real
    /// case. Everything that arrived is kept, and the cut is reported rather than hidden.
    #[test]
    fn an_envelope_cut_off_mid_result_keeps_what_arrived_and_says_it_was_cut() {
        let text = "<task-notification>\n<status>completed</status>\n<result>the review found";
        let prompt = parse(text);
        let envelope = &prompt.envelopes[0];
        assert!(envelope.truncated, "the element ran off the end");
        assert_eq!(envelope.field("status"), Some("completed"));
        assert_eq!(envelope.field("result"), Some("the review found"));
        assert!(
            envelope
                .fields
                .iter()
                .find(|f| f.name == "result")
                .expect("a result")
                .truncated
        );
        assert!(digest(text).expect("a digest").contains("cut"));
    }

    /// The shapes a person writes that look like a tag and are not. Any of them matching would
    /// swallow the rest of what they typed.
    #[test]
    fn a_less_than_in_prose_is_not_an_envelope() {
        for text in [
            "make it a Vec<String> instead",
            "assert that if x < y then z",
            "wrap it in <b>bold</b> please",
            "the type is HashMap<String, Vec<u8>>",
        ] {
            let prompt = parse(text);
            assert!(
                prompt.envelopes.is_empty(),
                "{text:?} was read as an envelope"
            );
            assert_eq!(prompt.prose, text);
            assert_eq!(digest(text), None, "{text:?}");
        }
    }

    /// Condition 1. A hyphenated tag written mid-sentence is a person writing, not a harness.
    #[test]
    fn an_envelope_has_to_open_a_line_of_its_own() {
        let text = "rename it to <task-notification>foo</task-notification> in the docs";
        assert!(parse(text).envelopes.is_empty());
        assert_eq!(digest(text), None);
    }

    /// Condition 2, which is the whole of what makes the unknown-tag fallback safe: an
    /// unregistered tag has to be hyphenated before this claims it.
    #[test]
    fn only_a_hyphenated_tag_reaches_the_generic_fallback() {
        for bare in [
            "<div>\nx\n</div>",
            "<p>\nx\n</p>",
            "<Task-Note>\nx\n</Task-Note>",
        ] {
            assert!(parse(bare).envelopes.is_empty(), "{bare:?}");
        }
        let prompt = parse("<some-new-thing>\nfirst line\nsecond\n</some-new-thing>");
        assert_eq!(prompt.envelopes.len(), 1);
        assert_eq!(prompt.envelopes[0].tag, "some-new-thing");
    }

    /// Nothing here reads an attribute, and a tag that grows one must not stop being seen.
    #[test]
    fn attributes_on_a_tag_do_not_hide_it() {
        let prompt = parse(
            "<task-notification version=\"2\">\n<status>failed</status>\n</task-notification>",
        );
        assert_eq!(prompt.envelopes.len(), 1);
        assert_eq!(prompt.envelopes[0].field("status"), Some("failed"));
    }

    /// The commonest shape of all: a person's prompt with the harness's reminder stapled to it.
    /// What they typed has to survive whole and lead.
    #[test]
    fn a_system_reminder_appended_to_a_prompt_leaves_the_prose_alone() {
        let text = "rebase onto main and rerun the suite\n\n<system-reminder>\nRemember to check the plan mode state.\nIt is a long reminder.\n</system-reminder>";
        let prompt = parse(text);
        assert_eq!(prompt.prose, "rebase onto main and rerun the suite");
        assert_eq!(prompt.envelopes.len(), 1);

        let drawn = digest(text).expect("a digest");
        assert!(
            drawn.starts_with("rebase onto main and rerun the suite"),
            "{drawn}"
        );
        assert!(drawn.contains("▸ system-reminder"), "{drawn}");
        assert!(!drawn.contains("Remember to check"), "{drawn}");
        // The reminder never names a pane: it says how to behave, not what the turn is for.
        assert_eq!(title(text), None);
        assert_eq!(question(text), "rebase onto main and rerun the suite");
    }

    /// Three top-level siblings that mean one thing. Absorbing them is what lets the renderers
    /// read one shape rather than reassembling it three times over.
    #[test]
    fn a_slash_command_is_its_three_elements_read_as_one_thing() {
        let text = "<command-name>/plan</command-name>\n<command-args>design the renderer</command-args>\n<local-command-stdout>Enabled plan mode\nsecond line</local-command-stdout>";
        let prompt = parse(text);
        assert_eq!(prompt.envelopes.len(), 1, "{:?}", prompt.envelopes);
        assert_eq!(title(text).as_deref(), Some("/plan design the renderer"));

        let drawn = digest(text).expect("a digest");
        assert!(drawn.starts_with("/plan design the renderer"), "{drawn}");
        assert!(drawn.contains("▸ stdout 2 lines"), "{drawn}");
        assert_eq!(names(&sections(text)), ["command", "stdout"]);
    }

    /// Which of the three a harness writes first is its own business, and Claude Code has put
    /// the message ahead of the command it belongs to. Absorbed only forwards, that read as a
    /// nameless message and a command that had lost it — and named the pane after the message.
    #[test]
    fn a_command_message_before_the_name_is_still_one_command() {
        let text = "<command-message>plan is running…</command-message>\n<command-name>/plan</command-name>\n<command-args>design the renderer</command-args>";
        let prompt = parse(text);
        assert_eq!(prompt.envelopes.len(), 1, "{:?}", prompt.envelopes);
        let envelope = &prompt.envelopes[0];
        assert_eq!(envelope.tag, "command-name");
        assert_eq!(
            envelope.fields.iter().map(|f| f.name).collect::<Vec<_>>(),
            ["command-message", "command-name", "command-args"],
            "the fields read in the order they arrived"
        );
        assert_eq!(title(text).as_deref(), Some("/plan design the renderer"));
    }

    /// And a second command does not take the first one's siblings with it.
    #[test]
    fn a_second_slash_command_does_not_steal_the_first_ones_siblings() {
        let text = "<command-message>a</command-message>\n<command-name>/a</command-name>\n<command-message>b</command-message>\n<command-name>/b</command-name>";
        let prompt = parse(text);
        assert_eq!(prompt.envelopes.len(), 2, "{:?}", prompt.envelopes);
        assert_eq!(prompt.envelopes[0].field("command-message"), Some("a"));
        assert_eq!(prompt.envelopes[0].field("command-name"), Some("/a"));
        assert_eq!(prompt.envelopes[1].field("command-message"), Some("b"));
        assert_eq!(prompt.envelopes[1].field("command-name"), Some("/b"));
    }

    /// The shape as it actually arrives: the expansion indents every line after the first, and
    /// `crate::text::sanitize` turns a leading tab into four spaces more. Held to column zero,
    /// two thirds of the command lands in `prose`, which is neither escaped nor cut.
    #[test]
    fn an_indented_slash_command_expansion_is_still_one_envelope() {
        let text = "<command-name>/plan</command-name>\n            <command-message>plan</command-message>\n            <command-args>design the renderer</command-args>\n<local-command-stdout>Enabled plan mode</local-command-stdout>";
        let prompt = parse(text);
        assert_eq!(prompt.prose, "", "{:?}", prompt.prose);
        assert_eq!(prompt.envelopes.len(), 1, "{:?}", prompt.envelopes);
        assert_eq!(
            prompt.envelopes[0].raw, "<command-name>/plan</command-name>",
            "the indentation is not part of the element"
        );
        assert_eq!(title(text).as_deref(), Some("/plan design the renderer"));
        assert_eq!(names(&sections(text)), ["command", "stdout"]);
    }

    /// Condition 1 bends for a tag the table knows and for nothing else, because what it is
    /// there to protect is an indented line of somebody's code block.
    #[test]
    fn an_indented_tag_the_table_does_not_know_is_still_prose() {
        let text =
            "the shape it arrives in:\n\n    <some-new-thing>\n    one\n    </some-new-thing>";
        let prompt = parse(text);
        assert!(prompt.envelopes.is_empty(), "{:?}", prompt.envelopes);
        assert_eq!(digest(text), None);
    }

    /// A harness that writes two fields to one line is a shape, not a risk. Held to the
    /// end-of-line rule the first of them swallowed the rest of the element and reported itself
    /// cut, which also cost the notification its identity.
    #[test]
    fn two_fields_written_on_one_line_are_two_fields() {
        let text = "<task-notification>\n<status>completed</status> <summary>it finished</summary>\n</task-notification>";
        let envelope = &parse(text).envelopes[0];
        assert_eq!(envelope.field("status"), Some("completed"));
        assert_eq!(envelope.field("summary"), Some("it finished"));
        assert!(envelope.fields.iter().all(|field| !field.truncated));
    }

    /// And what the rule is still for: a result describing this scanner names the closing tag
    /// mid-sentence, where closing would end an element the writer never opened.
    #[test]
    fn a_closing_tag_written_mid_sentence_does_not_close_a_field_early() {
        let text = "<task-notification>\n<result>the scanner looks for </result> at the end of a line\nand then stops.</result>\n</task-notification>";
        let envelope = &parse(text).envelopes[0];
        assert_eq!(
            envelope.field("result"),
            Some("the scanner looks for </result> at the end of a line\nand then stops.")
        );
        assert!(!envelope.truncated);
    }

    /// A self-closing tag opens nothing, so claiming it left `closing_of` looking for a closer
    /// that was never coming — and the element swallowing the rest of the prompt.
    #[test]
    fn a_self_closing_tag_does_not_swallow_the_rest_of_the_prompt() {
        let text = "<some-thing/>\nfix the flaky reconnect test";
        let prompt = parse(text);
        assert!(prompt.envelopes.is_empty(), "{:?}", prompt.envelopes);
        assert_eq!(prompt.prose, text);
        assert_eq!(digest(text), None);
    }

    /// The same inside an envelope, where it would have taken every field after it with it.
    #[test]
    fn a_self_closing_child_does_not_run_to_the_end_of_the_envelope() {
        let text = "<task-notification>\n<status>completed</status>\n<result/>\n<summary>it finished</summary>\n</task-notification>";
        let envelope = &parse(text).envelopes[0];
        assert_eq!(envelope.field("summary"), Some("it finished"));
        assert_eq!(envelope.field("result"), None);
        assert!(envelope.fields.iter().all(|field| !field.truncated));
    }

    /// The live path does not sanitise — `agent-service` reads the prompt as it arrived — so a
    /// harness ending its lines the Windows way had every element it sent read as cut, and a
    /// cut element has no identity to dedupe by.
    #[test]
    fn a_notification_with_carriage_returns_is_not_read_as_cut() {
        let crlf = notification().replace('\n', "\r\n");
        let envelope = &parse(&crlf).envelopes[0];
        assert!(!envelope.truncated);
        assert_eq!(envelope.field("status"), Some("completed"));
        assert!(dedupe_id(&crlf).is_some(), "and so it has an identity");
    }

    // -- what a reader is told ----------------------------------------------

    /// The row exists to say which task reported and how it went, in that order, before it says
    /// anything else.
    #[test]
    fn the_digest_of_a_task_notification_says_the_status_the_id_and_the_summary() {
        let drawn = digest(&notification()).expect("a digest");
        let first = drawn.lines().next().expect("a first line");
        assert!(first.starts_with("✓ **a10fe836** ·"), "{first}");
        assert!(
            first.contains("/code-review high up to 3 subagents"),
            "{first}"
        );
        assert!(!drawn.contains("task-notification"), "{drawn}");
        assert!(!drawn.contains("/private/tmp"), "{drawn}");
        assert!(drawn.contains("▸ note 2 lines"), "{drawn}");
        assert!(drawn.contains("▸ result 2 lines"), "{drawn}");
        assert!(drawn.contains("▸ output-file"), "{drawn}");
    }

    /// The digest is markdown source that the timeline then parses, so a summary an agent wrote
    /// must not be able to restyle the row it lands in.
    #[test]
    fn the_digest_escapes_what_the_agent_wrote() {
        let text =
            "<task-notification>\n<summary># **x** | y ~~z~~ <b></summary>\n</task-notification>";
        let drawn = digest(text).expect("a digest");
        assert!(
            drawn.contains(r"\# \*\*x\*\* \| y \~\~z\~\~ \<b\>"),
            "{drawn}"
        );
        let rendered: String = markdown::render(&drawn, 200, markdown::Options::compact())
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("# **x** | y ~~z~~ <b>"), "{rendered}");
    }

    /// A harness that drops a field costs us that piece, not the row.
    #[test]
    fn a_notification_with_no_summary_still_says_which_task_it_was() {
        let text = "<task-notification>\n<task-id>a10fe836c6a9fa4ca</task-id>\n<status>completed</status>\n</task-notification>";
        let drawn = digest(text).expect("a digest");
        assert!(
            drawn.starts_with("✓ **a10fe836** · background task"),
            "{drawn}"
        );
        assert_eq!(title(text).as_deref(), Some("task · a10fe836"));

        let bare = "<task-notification>\n<summary>it finished</summary>\n</task-notification>";
        let drawn = digest(bare).expect("a digest");
        assert!(drawn.starts_with("it finished"), "{drawn}");
    }

    /// Guessing at a status we have never seen would be worse than showing it.
    #[test]
    fn an_unknown_status_is_shown_as_it_arrived() {
        let text = "<task-notification>\n<status>throttled</status>\n<summary>waiting</summary>\n</task-notification>";
        let drawn = digest(text).expect("a digest");
        assert!(drawn.starts_with("· throttled waiting"), "{drawn}");
    }

    /// The summariser is asked what the agent is doing. Tags, the note's protocol boilerplate
    /// and a path the model cannot read are all noise against that question.
    #[test]
    fn the_question_the_summariser_is_asked_carries_no_tags_and_no_note() {
        let asked = question(&notification());
        assert!(
            asked.starts_with("background task a10fe836 completed:"),
            "{asked}"
        );
        assert!(asked.contains("The review found four issues."), "{asked}");
        assert!(!asked.contains('<'), "{asked}");
        assert!(!asked.contains("notify more than once"), "{asked}");
        assert!(!asked.contains("/private/tmp"), "{asked}");
    }

    /// The short fields are why the event was drawn at all, so they are open; `note` and
    /// `result` are why it could not be drawn whole, so they are not.
    #[test]
    fn the_sections_a_reader_opens_start_with_the_short_ones_open() {
        let folds = sections(&notification());
        assert_eq!(
            names(&folds),
            [
                "status",
                "task-id",
                "summary",
                "output-file",
                "note",
                "result"
            ]
        );
        for fold in &folds {
            match fold.name.as_str() {
                "note" | "result" => assert!(!fold.open && fold.foldable(), "{}", fold.name),
                _ => assert!(fold.open, "{}", fold.name),
            }
        }
        let output = folds
            .iter()
            .find(|f| f.name == "output-file")
            .expect("a path");
        assert_eq!(
            output.path.as_deref(),
            Some("/private/tmp/claude-501/x/a10fe836c6a9fa4ca.output")
        );
    }

    /// An envelope nobody registered still reads better than its own markup, and is still
    /// openable whole. That is the point of the table having a fallback at all.
    #[test]
    fn an_unknown_tag_still_gets_a_row_and_a_fold() {
        let text = "<some-new-thing>\nThe first line of whatever it said\nand more below it\n</some-new-thing>";
        let drawn = digest(text).expect("a digest");
        // The first line of what it said leads; the tag is named once, on the marker row, so a
        // four-row entry does not spend one of them saying the same word twice.
        assert!(
            drawn.starts_with("The first line of whatever it said"),
            "{drawn}"
        );
        assert!(drawn.contains("▸ some-new-thing 2 lines"), "{drawn}");
        assert_eq!(drawn.matches("some-new-thing").count(), 1, "{drawn}");
        assert_eq!(names(&sections(text)), ["some-new-thing"]);
        assert!(question(text).starts_with("some-new-thing: The first line"));
    }

    /// The table's spellings are load-bearing in two directions — `kind_of` looks a tag up in
    /// it, and the scanner will only ever hand it a hyphenated lowercase name — so a typo in
    /// one is a kind that can never be reached.
    #[test]
    fn every_kind_in_the_table_has_a_hyphenated_lowercase_tag_and_no_two_claim_one() {
        let mut seen: Vec<&str> = Vec::new();
        for kind in KINDS {
            assert!(
                is_tag_name(kind.tag, Rules::Toplevel),
                "{:?} is not a tag the scanner can produce",
                kind.tag
            );
            assert!(!seen.contains(&kind.tag), "{:?} is claimed twice", kind.tag);
            assert!(
                std::ptr::eq(kind_of(kind.tag), *kind),
                "{:?} is not reachable by its tag",
                kind.tag
            );
            seen.push(kind.tag);
            for absorbed in kind.absorbs {
                assert!(is_tag_name(absorbed, Rules::Toplevel), "{absorbed:?}");
            }
        }
        assert!(std::ptr::eq(kind_of("no-such-thing"), &GENERIC));
    }

    /// The reason `DIGEST_SUMMARY_COLS` exists: a talkative harness must not be able to spend
    /// the rows the affordances line needs.
    #[test]
    fn an_enormous_result_costs_the_digest_one_row() {
        let text = format!(
            "<task-notification>\n<status>completed</status>\n<summary>{}</summary>\n<result>{}</result>\n</task-notification>",
            "a very long summary ".repeat(40),
            "a line of the result\n".repeat(4000),
        );
        let drawn = digest(&text).expect("a digest");
        assert!(drawn.len() < 400, "the digest is {} bytes", drawn.len());
        assert!(drawn.contains('…'), "the summary was not cut: {drawn}");
        assert!(drawn.contains("▸ result 4000 lines"), "{drawn}");
    }

    /// Forty-eight *columns*, not forty-eight characters. Counted by character, a summary in a
    /// script that spends two columns to each of them is twice as wide as the budget says, takes
    /// a third row, and leaves `cap_rows` to eat the row saying what `Enter` would show.
    #[test]
    fn a_summary_is_cut_to_columns_rather_than_characters() {
        let wide = "这是一个很长的摘要 ".repeat(8);
        let drawn = cut(&wide, DIGEST_SUMMARY_COLS);
        assert!(
            drawn.width() <= DIGEST_SUMMARY_COLS,
            "{} columns: {drawn}",
            drawn.width()
        );
        assert!(drawn.ends_with('…'), "{drawn}");
        // A word too long to break at is cut mid-word, and the `…` is spent out of the budget
        // rather than added to it, so even that case fits the row it was measured for.
        let one_word = "x".repeat(200);
        assert_eq!(
            cut(&one_word, DIGEST_SUMMARY_COLS).width(),
            DIGEST_SUMMARY_COLS
        );
    }

    /// Order is the whole of what a timeline is, and a prompt carrying several envelopes is no
    /// different from one carrying several paragraphs.
    #[test]
    fn a_prompt_carrying_several_envelopes_draws_them_in_the_order_they_arrived() {
        let text = "<first-thing>\none\n</first-thing>\n<second-thing>\ntwo\n</second-thing>";
        let prompt = parse(text);
        assert_eq!(
            prompt.envelopes.iter().map(|e| e.tag).collect::<Vec<_>>(),
            ["first-thing", "second-thing"]
        );
        let drawn = digest(text).expect("a digest");
        assert!(
            drawn.find("first-thing") < drawn.find("second-thing"),
            "{drawn}"
        );
    }

    // -- the dedupe key -----------------------------------------------------

    /// A redelivery of one notification is one thing, however far apart the two arrive.
    #[test]
    fn two_deliveries_of_one_notification_share_an_id() {
        let text = notification();
        assert_eq!(dedupe_id(&text), dedupe_id(&text));
        assert!(dedupe_id(&text)
            .expect("an id")
            .starts_with("task-notification:a10fe836c6a9fa4ca:"));
    }

    /// The envelope's own note says the same task-id may notify more than once — a resumed
    /// agent reports again with a *different result*, and that is a second thing happening
    /// rather than a second delivery of the first.
    #[test]
    fn two_notifications_for_one_task_do_not_share_an_id() {
        let first = notification();
        let second = first.replace(
            "The review found four issues.",
            "The review found one issue.",
        );
        assert_ne!(first, second);
        assert_ne!(dedupe_id(&first), dedupe_id(&second));
    }

    /// Nothing but a task notification has an identity of its own, so nothing else changes the
    /// adjacent-only dedupe the ingest path already does.
    #[test]
    fn only_a_task_notification_carries_a_dedupe_id() {
        assert_eq!(dedupe_id("plain prose"), None);
        assert_eq!(dedupe_id("<system-reminder>\nx\n</system-reminder>"), None);
        assert_eq!(dedupe_id("<some-new-thing>\nx\n</some-new-thing>"), None);
    }

    /// A cut element is a prefix of a notification, and a hash of it identifies the prefix.
    /// The task id is still right there — the point is that it is refused anyway, because an
    /// id built from it would claim to stand for an element we did not receive.
    #[test]
    fn a_cut_notification_has_no_identity() {
        let whole = notification_of("a10fe836c6a9fa4ca", "the review found four issues");
        let cut = &whole[..whole.len() - 20];

        let envelope = &parse(cut).envelopes[0];
        assert!(envelope.truncated, "the element ran off the end");
        assert_eq!(envelope.field("task-id"), Some("a10fe836c6a9fa4ca"));
        assert_eq!(dedupe_id(cut), None);
        assert!(dedupe_id(&whole).is_some(), "and a whole one still has one");
    }

    /// The collision, from the data rather than from an argument: `pruned_to` takes a prompt at
    /// a fixed length with no marker, so two reports of one task that agree up to it arrive
    /// byte for byte the same.
    ///
    /// Which is also why folding the cut flag and the length into the key would not have
    /// helped — both are equal across exactly the pair they would have to tell apart.
    #[test]
    fn two_notifications_cut_at_one_length_are_indistinguishable() {
        let shared = "a line of the review\n".repeat(20);
        let first = notification_of("a10fe836c6a9fa4ca", &format!("{shared}four issues"));
        let second = notification_of("a10fe836c6a9fa4ca", &format!("{shared}one issue"));
        assert_ne!(first, second);
        assert_ne!(
            dedupe_id(&first),
            dedupe_id(&second),
            "delivered whole they are two reports"
        );

        // Cut inside the part they agree on, which is where the cap lands on any result long
        // enough to be cut at all.
        let at = 300;
        assert_eq!(first[..at], second[..at], "the cut took the difference");
        for cut in [&first[..at], &second[..at]] {
            assert!(parse(cut).envelopes[0].truncated);
            assert_eq!(dedupe_id(cut), None);
        }
    }

    /// A prompt is one delivery, so its identity is the identity of everything identifiable in
    /// it. Two background tasks reporting together are one prompt, and a later prompt sharing
    /// only the first of them is a different prompt rather than a redelivery of it.
    #[test]
    fn a_prompt_carrying_two_notifications_is_identified_by_both() {
        let first = notification_of("aaaa1111", "the first task finished");
        let second = notification_of("bbbb2222", "the second task finished");
        let third = notification_of("cccc3333", "the third task finished");
        let both = format!("{first}\n{second}");

        assert_eq!(dedupe_id(&both), dedupe_id(&format!("{first}\n{second}")));
        assert_ne!(dedupe_id(&both), dedupe_id(&format!("{first}\n{third}")));
        assert_ne!(
            dedupe_id(&both),
            dedupe_id(&first),
            "one of the two is not the pair"
        );
    }

    // -- the reader's rows --------------------------------------------------

    /// The reader's rows for `sections`, parsed and wrapped the way the reader does.
    fn reader_lines(sections: &[Section], width: usize) -> ReaderLines {
        let blocks: Vec<Vec<markdown::MdBlock>> = sections.iter().map(section_blocks).collect();
        wrap_sections(blocks.iter().map(Vec::as_slice), width)
    }

    /// The spans have to tile the rows exactly, or a cursor lands on a section that does not
    /// own the row under it — the same invariant `HistoryLines::events` holds.
    #[test]
    fn a_readers_spans_tile_its_rows() {
        let mut folds = sections(&notification());
        folds[5].open = true;
        let rows = reader_lines(&folds, 46);
        assert_eq!(rows.starts.len(), folds.len());
        assert_eq!(rows.starts[0], 0);
        for pair in rows.starts.windows(2) {
            assert!(pair[0] < pair[1], "{:?}", rows.starts);
        }
        assert!(*rows.starts.last().expect("a last start") < rows.lines.len());
    }

    /// A shut fold costs its header and nothing else; opening it is what spends the rows. The
    /// body comes through as the markdown the agent wrote rather than as an escaped value.
    #[test]
    fn opening_a_fold_is_what_spends_the_rows() {
        let mut folds = sections(&notification());
        let shut = reader_lines(&folds, 60).lines.len();
        let result = folds
            .iter_mut()
            .find(|f| f.name == "result")
            .expect("a result");
        result.open = true;
        let open = reader_lines(&folds, 60).lines.len();
        assert!(open > shut, "{open} rows is not more than {shut}");

        let text: Vec<String> = reader_lines(&folds, 60)
            .lines
            .iter()
            .map(|line| line.text.clone())
            .collect();
        assert!(
            text.iter().any(|line| line.contains("▾ result")),
            "{text:?}"
        );
        assert!(
            text.iter()
                .any(|line| line.contains("The review found four issues.")),
            "{text:?}"
        );
        assert!(text.iter().any(|line| line.contains("▸ note")), "{text:?}");
        assert!(
            !text
                .iter()
                .any(|line| line.contains("task-notification fires")),
            "a shut fold drew its body: {text:?}"
        );
    }
}

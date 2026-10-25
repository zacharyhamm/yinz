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
#[path = "unit/envelope.rs"]
mod tests;

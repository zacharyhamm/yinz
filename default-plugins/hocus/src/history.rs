//! The chat archive: one row per conversation, one row per message.
//!
//! It used to be one JSON file per conversation under `/cache/chats`, and
//! listing the archive meant reading and fully parsing fifty of them purely to
//! recover the first line of each. The title is a column now, so the picker is
//! one fetch that reads titles instead of transcripts.
//!
//! Private namespace, not the shared one: nothing else reads these, and a
//! private namespace is derived from the plugin URL, so a debug build and a
//! release build archive to different places exactly as they did when this was
//! a directory — and exactly as they still do for `permissions.kdl`.
//!
//! Failures are reported rather than swallowed here, and swallowed by the
//! callers that have nowhere to put them. A full disk is still not a reason to
//! take the pane down.

use hocus_proto::store::{self, Backend};
use zellij_tile::prelude::*;

use crate::tools::ToolCall;
use crate::{Message, Role};

/// The directory the archive used to live in. Read once, by the import.
#[cfg(target_family = "wasm")]
const LEGACY_DIR: &str = "/cache/chats";

/// Chats offered in the picker, most recent first.
///
/// No longer a bound on how much work listing is — that is one fetch of titles
/// whatever this says. It is now only how long a list a person wants to scroll.
const MAX_LISTED: u32 = 50;

/// Transcripts read in by the one-time import, newest first.
///
/// Reading an unbounded directory of unbounded files would block the pane on the
/// first launch after the upgrade. What is left behind stays on disk.
#[cfg(target_family = "wasm")]
const MAX_IMPORTED: usize = 200;

/// Displayed length of a title before it is cut.
const TITLE_CHARS: usize = 60;

/// One archived chat, as the picker shows it.
pub struct Entry {
    pub id: u64,
    pub title: String,
}

impl Role {
    /// The spelling stored in the `role` column.
    ///
    /// Hand-written rather than serde's, so the column stays readable in the
    /// database and does not move if a serde attribute on `Role` ever changes —
    /// that type also crosses the API wire, where the spelling is not ours.
    fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            Role::Error => "error",
            Role::Notice => "notice",
        }
    }

    fn from_str(text: &str) -> Role {
        match text {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            "error" => Role::Error,
            // A row written by a later version with a role this one does not
            // know reads as an aside, which renders dim and says nothing false.
            _ => Role::Notice,
        }
    }
}

/// Address of one message. Makes every write of it an upsert.
fn message_uid(chat_id: u64, seq: usize) -> String {
    format!("{chat_id}:{seq}")
}

fn text_at(rows: &DataRows, row: &[DataValue], column: &str) -> Option<String> {
    match rows.get(row, column) {
        Some(DataValue::Text(text)) => Some(text.clone()),
        _ => None,
    }
}

fn int_at(rows: &DataRows, row: &[DataValue], column: &str) -> Option<u64> {
    match rows.get(row, column) {
        Some(DataValue::Int(value)) if *value >= 0 => Some(*value as u64),
        _ => None,
    }
}

/// The write that stores one message.
fn message_mutation(chat_id: u64, seq: usize, message: &Message) -> Mutation {
    Mutation::put("chat_message")
        .uid(message_uid(chat_id, seq))
        .set("chat_id", chat_id)
        .set("seq", seq)
        .set("role", message.role.as_str())
        .set("content", message.content.clone())
        // Always written, even when empty, because a Put only writes what it
        // names: a message that lost its tool calls would otherwise keep them.
        .set(
            "tool_calls",
            serde_json::to_string(&message.tool_calls).unwrap_or_else(|_| "[]".to_owned()),
        )
        .set("tool_call_id", message.tool_call_id.clone())
        .set("full_output", message.full_output)
        // Kept rather than recomputed on load: reopening a chat would otherwise
        // downgrade every result the user typed back to the raw JSON it was
        // presented to spare them.
        .set("display", message.display.clone())
        .build()
}

/// Save a transcript, writing only what has changed since `saved_len`.
///
/// Answers with the new `saved_len`. The first save of a chat allocates its id,
/// so an untouched chat leaves nothing behind, and `id` is filled in.
///
/// `reply_at` is the streaming placeholder, which grows in place. Rewriting from
/// there covers both the message that grew and anything pushed *behind* it — a
/// typed tool mid-reply, which is the case `mutable_len` exists for. Rewriting a
/// row that did not change is free; missing one that did loses the reply, so
/// this errs long.
pub fn save(
    backend: &impl Backend,
    id: &mut Option<u64>,
    messages: &[Message],
    saved_len: usize,
    reply_at: Option<usize>,
) -> Result<usize, String> {
    let title = title(messages);
    let chat_id = match *id {
        Some(existing) => existing,
        None => {
            // No uid: the store allocates the id, and there is nothing to
            // upsert against on a chat that has never been written.
            let report = backend.apply(vec![Mutation::put("chat")
                .set("title", title.clone())
                .build()])?;
            let new = report
                .ids
                .first()
                .copied()
                .ok_or("the store did not report an id for the new chat")?;
            *id.insert(new as u64)
        },
    };

    let mut mutations = vec![
        // Unconditional, and almost always matches nothing. It is the answer to
        // `pop_empty_placeholder`, which removes a message from the middle and
        // shifts every index after it — reachable because a save can happen
        // mid-stream. Without it the tail of a shortened transcript would
        // survive as rows nothing overwrites.
        Mutation::delete(
            "chat_message",
            Pred::eq("chat_id", chat_id).and(Pred::gte("seq", messages.len())),
        ),
        Mutation::update("chat", Pred::eq("id", chat_id as i64))
            .set("title", title)
            .build(),
    ];

    // A transcript that got *shorter* had a message taken out of the middle —
    // `pop_empty_placeholder` — so every index after it has shifted and the
    // whole thing has to be rewritten. There is no record of where the removal
    // was, and guessing wrong silently archives one message under another's
    // sequence number.
    //
    // Otherwise: from the last message written, which may have grown in place,
    // or from the streaming placeholder if that is further back. A tool typed
    // mid-reply sits *behind* the placeholder, so starting at the end would
    // leave the arriving reply unwritten. Rewriting a row that did not change
    // costs nothing — it is an upsert — and missing one loses the reply, so
    // this errs long in both directions.
    let from = if messages.len() < saved_len {
        0
    } else {
        saved_len
            .saturating_sub(1)
            .min(reply_at.unwrap_or(usize::MAX))
            .min(messages.len())
    };
    for (seq, message) in messages.iter().enumerate().skip(from) {
        mutations.push(message_mutation(chat_id, seq, message));
    }

    backend.apply(mutations)?;
    Ok(messages.len())
}

/// Read one transcript back.
///
/// Pages, because a long conversation is more rows than one fetch returns. The
/// default ordering is the only one `after_id` may be combined with, which this
/// uses — and then sorts by `seq` in the guest rather than relying on id order
/// agreeing with it. It does agree, since messages are only appended and only a
/// suffix is ever deleted, but that is a fact about today's writers.
pub fn load(backend: &impl Backend, id: u64) -> Option<Vec<Message>> {
    let mut found: Vec<(u64, Message)> = Vec::new();
    let mut cursor: Option<i64> = None;
    loop {
        let mut request = FetchRequest::entity("chat_message")
            .filter(Pred::eq("chat_id", id))
            .limit(MAX_FETCH_ROWS);
        if let Some(after) = cursor {
            request = request.after_id(after);
        }
        let rows = backend.fetch(&request).ok()?;
        for row in &rows.rows {
            let Some(seq) = int_at(&rows, row, "seq") else {
                continue;
            };
            found.push((
                seq,
                Message {
                    role: Role::from_str(
                        text_at(&rows, row, "role").as_deref().unwrap_or_default(),
                    ),
                    content: text_at(&rows, row, "content").unwrap_or_default(),
                    tool_calls: text_at(&rows, row, "tool_calls")
                        .and_then(|json| serde_json::from_str::<Vec<ToolCall>>(&json).ok())
                        .unwrap_or_default(),
                    tool_call_id: text_at(&rows, row, "tool_call_id"),
                    full_output: matches!(
                        rows.get(row, "full_output"),
                        Some(DataValue::Bool(true))
                    ),
                    // Absent for every row written before the column existed,
                    // which reads back as "show the content", i.e. exactly the
                    // behaviour those chats were archived with.
                    display: text_at(&rows, row, "display"),
                },
            ));
        }
        match rows.next_cursor {
            Some(next) if rows.truncated => cursor = Some(next),
            _ => break,
        }
    }
    if found.is_empty() {
        return None;
    }
    found.sort_by_key(|(seq, _)| *seq);
    Some(found.into_iter().map(|(_, message)| message).collect())
}

/// The archive, newest first.
///
/// One fetch, selecting the title rather than deriving it. This is the whole
/// point of the `chat` row: the version of this function that read fifty
/// transcripts to title them ran on the keystroke that opened the picker.
pub fn list(backend: &impl Backend) -> Vec<Entry> {
    let Ok(rows) = backend.fetch(
        &FetchRequest::entity("chat")
            // Id order rather than `updated_at`, which is what the file-per-chat
            // version did: ids climb, so this is still chat order, and reordering
            // the picker by recency is a change nobody asked for.
            .sort_by("id", SortDir::Desc)
            .limit(MAX_LISTED)
            .select(["id", "title"]),
    ) else {
        return Vec::new();
    };
    rows.rows
        .iter()
        .filter_map(|row| {
            Some(Entry {
                id: int_at(&rows, row, "id")?,
                title: text_at(&rows, row, "title").unwrap_or_else(|| "(empty)".to_owned()),
            })
        })
        .collect()
}

/// What the picker calls a chat: its opening question.
///
/// Control characters are stripped for the same reason `ui::wrap` strips them —
/// this is the one place model-adjacent text is printed without going through
/// wrapping, and an escape sequence must not be able to repaint the pane.
pub fn title(messages: &[Message]) -> String {
    let first_line = messages
        .iter()
        .find(|message| message.role == Role::User)
        .and_then(|message| message.content.trim().lines().next())
        .unwrap_or_default();

    let mut title: String = first_line
        .chars()
        .filter(|ch| !ch.is_control())
        .take(TITLE_CHARS)
        .collect();
    if first_line.chars().count() > TITLE_CHARS {
        title.push('…');
    }
    if title.trim().is_empty() {
        return "(empty)".to_owned();
    }
    title
}

// ---------------------------------------------------------------------------
// Prompt history
// ---------------------------------------------------------------------------

/// The file the up-arrow history lived in before the store.
#[cfg(target_family = "wasm")]
const LEGACY_PROMPTS: &str = "/cache/prompt-history.json";

/// Lines kept. The old file had no cap at all: it grew forever and was
/// rewritten whole on every submit.
const MAX_PROMPTS: u32 = 500;

/// The submitted lines, oldest first, as the input box wants them.
///
/// Fetched newest-first and reversed, because `Asc` with a limit would return
/// the *oldest* five hundred, which is the opposite of what a cap is for.
pub fn load_prompts(backend: &impl Backend) -> Vec<String> {
    let Ok(rows) = backend.fetch(
        &FetchRequest::entity("prompt")
            .sort_by("updated_at", SortDir::Desc)
            .limit(MAX_PROMPTS)
            .select(["text"]),
    ) else {
        return Vec::new();
    };
    let mut prompts: Vec<String> = rows
        .rows
        .iter()
        .filter_map(|row| text_at(&rows, row, "text"))
        .collect();
    prompts.reverse();
    prompts
}

/// Record one submitted line.
///
/// The text is the uid, so this upserts: a line submitted twice is one row
/// whose `updated_at` moves, which floats it back to the end of the history
/// rather than piling up a duplicate. The old file kept every one.
///
/// One row per submit, where the old path re-serialized and rewrote the entire
/// vector each time.
pub fn add_prompt(backend: &impl Backend, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let _ = backend.apply(vec![Mutation::put("prompt")
        .uid(line)
        .set("text", line)
        .build()]);
}

/// Drop everything past the cap.
///
/// Run at load rather than on the hot path, and off the rows already fetched —
/// so trimming costs no extra call. Nothing is deleted when the history is
/// under the cap, which is the normal case.
pub fn trim_prompts(backend: &impl Backend) {
    let Ok(rows) = backend.fetch(
        &FetchRequest::entity("prompt")
            .sort_by("updated_at", SortDir::Desc)
            .limit(MAX_PROMPTS)
            .select(["updated_at"]),
    ) else {
        return;
    };
    if (rows.len() as u32) < MAX_PROMPTS {
        return;
    }
    let Some(oldest_kept) = rows
        .rows
        .last()
        .and_then(|row| int_at(&rows, row, "updated_at"))
    else {
        return;
    };
    let _ = backend.apply(vec![Mutation::delete(
        "prompt",
        Pred::lt("updated_at", oldest_kept as i64),
    )]);
}

/// Read the old prompt-history file in, once.
#[cfg(target_family = "wasm")]
fn import_legacy_prompts(backend: &impl Backend) {
    let Ok(bytes) = std::fs::read(LEGACY_PROMPTS) else {
        return;
    };
    let lines: Vec<String> = serde_json::from_slice(&bytes).unwrap_or_default();
    // Oldest first, so the upserts leave `updated_at` in the order the file had.
    for line in lines {
        add_prompt(backend, &line);
    }
}

#[cfg(not(target_family = "wasm"))]
fn import_legacy_prompts(_backend: &impl Backend) {}

// ---------------------------------------------------------------------------
// The one-time import
// ---------------------------------------------------------------------------

/// Read the JSON transcripts in, once.
///
/// One apply per chat rather than one for everything: a long conversation is
/// then one bounded transaction, and a file that will not parse loses only
/// itself. Oldest first within what is imported, so the new dense ids keep the
/// order the old ones had.
#[cfg(target_family = "wasm")]
pub fn import_legacy(backend: &impl Backend) {
    let mut ids = legacy_ids();
    ids.sort_unstable();
    // Newest kept, oldest written first: take the tail, then walk it forwards.
    let start = ids.len().saturating_sub(MAX_IMPORTED);
    let dropped = start;
    for legacy in &ids[start..] {
        let Some(messages) = legacy_transcript(*legacy) else {
            continue;
        };
        let mut id = None;
        if save(backend, &mut id, &messages, 0, None).is_err() {
            continue;
        }
        if let Some(id) = id {
            let _ = backend.apply(vec![Mutation::update("chat", Pred::eq("id", id as i64))
                .set("legacy_id", *legacy)
                .build()]);
        }
    }
    if dropped > 0 {
        // Written straight out rather than posted to the io worker: this runs
        // during load, and says what was left on disk. Nowhere else to put it —
        // there is no conversation on screen yet.
        if let Some(line) = crate::log::line(
            None,
            0,
            0,
            "archive_import_truncated",
            serde_json::json!({ "dropped": dropped, "kept": MAX_IMPORTED }),
        ) {
            crate::log::write_line(None, &line);
        }
    }
}

#[cfg(target_family = "wasm")]
fn legacy_ids() -> Vec<u64> {
    let Ok(entries) = std::fs::read_dir(LEGACY_DIR) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            // `.jsonl` shares the directory and shares the stem; only the
            // transcripts are being read here.
            (path.extension()?.to_str()? == "json")
                .then(|| path.file_stem()?.to_str()?.parse().ok())
                .flatten()
        })
        .collect()
}

#[cfg(target_family = "wasm")]
fn legacy_transcript(id: u64) -> Option<Vec<Message>> {
    let json = std::fs::read(format!("{LEGACY_DIR}/{id}.json")).ok()?;
    serde_json::from_slice(&json).ok()
}

/// Native builds run the tests, where there is no `/cache` to read.
#[cfg(not(target_family = "wasm"))]
pub fn import_legacy(_backend: &impl Backend) {}

/// Declare the private entities and import the old files once.
///
/// The marker is written in the same batch as nothing else, but it is written
/// *after* the import rather than with it: an import is one apply per chat, so
/// there is no single transaction to join. A crash midway re-imports, which
/// duplicates chats — the alternative is a marker written first, where a crash
/// loses them. Duplicates are visible and deletable; a silent loss is not.
pub fn open(backend: &impl Backend) -> Result<(), String> {
    backend.define_all(&store::private_defs())?;
    if !imported(backend, "chats") {
        import_legacy(backend);
        backend.apply(vec![marker("chats")])?;
    }
    if !imported(backend, "prompts") {
        import_legacy_prompts(backend);
        backend.apply(vec![marker("prompts")])?;
    }
    trim_prompts(backend);
    Ok(())
}

pub(crate) fn marker(what: &str) -> Mutation {
    Mutation::put("import_marker")
        .uid(what)
        .set("what", what)
        .build()
}

pub(crate) fn imported(backend: &impl Backend, what: &str) -> bool {
    backend
        .count(&FetchRequest::entity("import_marker").filter(Pred::eq("what", what)))
        .map(|count| count > 0)
        // An error means the store is unreachable and the import would fail
        // too. Claiming it is done is the safe direction: a retry next launch
        // is fine, a double import is not.
        .unwrap_or(true)
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn backend() -> store::Local {
        let backend = store::Local::private("hocus");
        backend
            .define_all(&store::private_defs())
            .expect("declared the private entities");
        backend
    }

    #[test]
    fn a_chat_is_titled_by_its_opening_question() {
        let messages = vec![
            Message::notice("cancelled"),
            Message::user("why did the build fail"),
            Message::assistant("because"),
            Message::user("and the other one"),
        ];
        assert_eq!(title(&messages), "why did the build fail");
    }

    #[test]
    fn a_multi_line_question_is_titled_by_its_first_line() {
        assert_eq!(title(&[Message::user("first\nsecond")]), "first");
    }

    #[test]
    fn a_long_question_is_cut_and_marked() {
        let title = title(&[Message::user("a".repeat(TITLE_CHARS + 10))]);
        assert_eq!(title.chars().count(), TITLE_CHARS + 1);
        assert!(title.ends_with('…'));
    }

    /// An escape sequence in a title would be printed without going through
    /// `ui::wrap`, which is what strips them everywhere else.
    #[test]
    fn a_title_carries_no_control_characters() {
        assert_eq!(
            title(&[Message::user("safe\u{1b}[31mtext")]),
            "safe[31mtext"
        );
    }

    #[test]
    fn a_chat_with_nothing_to_title_it_still_has_a_title() {
        assert_eq!(title(&[]), "(empty)");
        assert_eq!(title(&[Message::assistant("no question")]), "(empty)");
    }

    #[test]
    fn every_role_survives_the_column() {
        for role in [
            Role::User,
            Role::Assistant,
            Role::Tool,
            Role::Error,
            Role::Notice,
        ] {
            assert_eq!(Role::from_str(role.as_str()), role);
        }
    }

    /// A row written by a later hocus must not lose the message, only the shade
    /// it is drawn in.
    #[test]
    fn an_unknown_role_reads_as_an_aside() {
        assert_eq!(Role::from_str("something_new"), Role::Notice);
    }

    #[test]
    fn a_transcript_survives_a_round_trip() {
        let backend = backend();
        let messages = vec![
            Message::user("why did the build fail"),
            Message::tool_calls(vec![ToolCall {
                id: "call_1".to_owned(),
                name: "read_pane".to_owned(),
                arguments: r#"{"pane_id":3}"#.to_owned(),
            }]),
            Message {
                role: Role::Tool,
                content: "the output".to_owned(),
                tool_call_id: Some("call_1".to_owned()),
                full_output: true,
                // Carried through the archive rather than recomputed: reopening
                // a chat would otherwise show the raw result this spared the
                // user in the first place.
                display: Some("## read_pane\n\nthe output".to_owned()),
                ..Message::default()
            },
            Message::assistant("because the test is flaky"),
        ];

        let mut id = None;
        save(&backend, &mut id, &messages, 0, None).expect("saved");
        let id = id.expect("an id was allocated");
        assert_eq!(load(&backend, id).expect("loaded"), messages);
    }

    #[test]
    fn the_first_save_allocates_an_id_and_later_ones_keep_it() {
        let backend = backend();
        let mut id = None;
        let messages = vec![Message::user("one")];
        let saved = save(&backend, &mut id, &messages, 0, None).expect("saved");
        let first = id.expect("an id");

        let messages = vec![Message::user("one"), Message::assistant("two")];
        save(&backend, &mut id, &messages, saved, None).expect("saved");
        assert_eq!(id, Some(first), "the id must not move");
        assert_eq!(load(&backend, first).expect("loaded").len(), 2);
    }

    /// The incremental path: only what is new gets written, and the result is
    /// the same as writing everything.
    #[test]
    fn an_incremental_save_matches_a_whole_one() {
        let backend = backend();
        let mut id = None;
        let mut messages = vec![Message::user("one")];
        let mut saved = save(&backend, &mut id, &messages, 0, None).expect("saved");

        for text in ["two", "three", "four"] {
            messages.push(Message::assistant(text));
            saved = save(&backend, &mut id, &messages, saved, None).expect("saved");
        }
        assert_eq!(load(&backend, id.expect("id")).expect("loaded"), messages);
    }

    /// The streaming placeholder grows in place, so the row for it has to be
    /// rewritten even though the count did not change.
    #[test]
    fn a_reply_that_grows_in_place_is_rewritten() {
        let backend = backend();
        let mut id = None;
        let mut messages = vec![Message::user("ask"), Message::assistant("par")];
        let saved = save(&backend, &mut id, &messages, 0, Some(1)).expect("saved");

        messages[1] = Message::assistant("partial reply");
        save(&backend, &mut id, &messages, saved, Some(1)).expect("saved");

        let loaded = load(&backend, id.expect("id")).expect("loaded");
        assert_eq!(loaded[1].content, "partial reply");
    }

    /// A typed tool mid-reply sits *behind* the placeholder, so a save has to
    /// rewrite from the placeholder rather than from the end.
    #[test]
    fn a_tool_appended_behind_the_placeholder_does_not_hide_the_reply() {
        let backend = backend();
        let mut id = None;
        let mut messages = vec![Message::user("ask"), Message::assistant("")];
        let saved = save(&backend, &mut id, &messages, 0, Some(1)).expect("saved");

        messages[1] = Message::assistant("the reply");
        messages.push(Message::notice("a typed tool"));
        save(&backend, &mut id, &messages, saved, Some(1)).expect("saved");

        let loaded = load(&backend, id.expect("id")).expect("loaded");
        assert_eq!(loaded[1].content, "the reply");
        assert_eq!(loaded[2].content, "a typed tool");
    }

    /// `pop_empty_placeholder` removes a message from the middle, so the
    /// transcript gets shorter and every index after it shifts. The tail must
    /// not survive as rows nothing overwrote.
    #[test]
    fn a_transcript_that_gets_shorter_leaves_no_tail_behind() {
        let backend = backend();
        let mut id = None;
        let messages = vec![
            Message::user("one"),
            Message::assistant("two"),
            Message::notice("three"),
        ];
        let saved = save(&backend, &mut id, &messages, 0, None).expect("saved");

        let shorter = vec![Message::user("one"), Message::notice("three")];
        save(&backend, &mut id, &shorter, saved, None).expect("saved");

        assert_eq!(load(&backend, id.expect("id")).expect("loaded"), shorter);
    }

    #[test]
    fn the_archive_lists_newest_first_with_its_titles() {
        let backend = backend();
        for text in ["first question", "second question"] {
            let mut id = None;
            save(&backend, &mut id, &[Message::user(text)], 0, None).expect("saved");
        }
        let listed = list(&backend);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].title, "second question");
        assert_eq!(listed[1].title, "first question");
    }

    /// The title follows the transcript: a chat whose first question arrives
    /// after the first save must not stay "(empty)".
    #[test]
    fn a_title_is_rewritten_when_the_transcript_gains_a_question() {
        let backend = backend();
        let mut id = None;
        let saved = save(&backend, &mut id, &[Message::notice("hello")], 0, None).expect("saved");
        assert_eq!(list(&backend)[0].title, "(empty)");

        let messages = vec![Message::notice("hello"), Message::user("the question")];
        save(&backend, &mut id, &messages, saved, None).expect("saved");
        assert_eq!(list(&backend)[0].title, "the question");
    }

    #[test]
    fn loading_a_chat_that_is_not_there_answers_nothing() {
        assert!(load(&backend(), 404).is_none());
    }

    /// The import marker is what stops a second launch reading the old files
    /// again. Emptiness cannot stand in for it: deleting every chat is a state a
    /// user can reach honestly.
    #[test]
    fn the_import_runs_once() {
        let backend = backend();
        assert!(!imported(&backend, "chats"));
        open(&backend).expect("opened");
        assert!(imported(&backend, "chats"));
    }
}

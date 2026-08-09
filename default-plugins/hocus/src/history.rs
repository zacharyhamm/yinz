//! The chat archive: one JSON file per conversation under `/cache/chats`.
//!
//! `/cache` is the only preopen worth writing to. Zellij `remove_dir_all`s the
//! plugin's `/data` directory on unload, `/host` is the user's project directory
//! and `/tmp` is the OS's to wipe. No permission is involved — the permission
//! set gates host commands, not WASI preopens.
//!
//! The directory is keyed on the plugin's URL, so a debug build and a release
//! build archive to different places, exactly as they do for `permissions.kdl`.
//!
//! Every failure here is swallowed. A full disk is not a reason to take the pane
//! down with it.

use std::fs;

use crate::{Message, Role};

const DIR: &str = "/cache/chats";
const PROMPT_HISTORY: &str = "/cache/prompt-history.json";

/// Chats offered in the picker, most recent first.
///
/// ponytail: `list` reads every file it lists in order to derive a title. Fine
/// for tens of chats; store the title in the file if it ever grows past that.
const MAX_LISTED: usize = 50;

/// Displayed length of a title before it is cut.
const TITLE_CHARS: usize = 60;

/// One archived chat, as the picker shows it.
pub struct Entry {
    pub id: u64,
    pub title: String,
}

fn path(id: u64) -> String {
    format!("{DIR}/{id}.json")
}

/// Serialize the transcript, allocating an id on first save so an untouched
/// chat never leaves a file behind. Returns the write job — id stays plugin-side
/// state, the write itself goes to the io worker (or straight to [`write`]).
pub fn save(id: &mut Option<u64>, messages: &[Message]) -> Option<(u64, String)> {
    let json = serde_json::to_string(messages).ok()?;
    let chat_id = match *id {
        Some(existing) => existing,
        None => *id.insert(next_id(&ids())),
    };
    Some((chat_id, json))
}

/// Write one serialized transcript out.
pub fn write(id: u64, json: &[u8]) {
    if fs::create_dir_all(DIR).is_err() {
        return;
    }
    let _ = fs::write(path(id), json);
}

pub fn load(id: u64) -> Option<Vec<Message>> {
    serde_json::from_slice(&fs::read(path(id)).ok()?).ok()
}

pub fn load_prompt_history() -> Vec<String> {
    parse_prompt_history(&fs::read(PROMPT_HISTORY).unwrap_or_default())
}

fn parse_prompt_history(json: &[u8]) -> Vec<String> {
    serde_json::from_slice(json).unwrap_or_default()
}

pub fn prompt_history_json(entries: &[String]) -> Option<String> {
    serde_json::to_string(entries).ok()
}

pub fn write_prompt_history(json: &[u8]) {
    let _ = fs::write(PROMPT_HISTORY, json);
}

/// The archive, newest first. Ids increase, so id order is chat order.
pub fn list() -> Vec<Entry> {
    let mut ids = ids();
    ids.sort_unstable_by(|a, b| b.cmp(a));
    ids.truncate(MAX_LISTED);
    ids.into_iter()
        .map(|id| Entry {
            id,
            title: title(&load(id).unwrap_or_default()),
        })
        .collect()
}

/// Numeric stems of every chat file, unordered. Anything else in the directory
/// is ignored rather than being an error.
fn ids() -> Vec<u64> {
    let Ok(entries) = fs::read_dir(DIR) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            entry
                .path()
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.parse().ok())
        })
        .collect()
}

/// One past the highest id in use. Monotonic and clock-free — a filename is not
/// worth depending on WASI's clock for, and ids in order are chats in order.
///
/// ponytail: two hocus panes saving a first message in the same instant would
/// pick the same id and one would lose. Fixing it needs a lock file, and costs a
/// pane nobody has.
fn next_id(ids: &[u64]) -> u64 {
    ids.iter().max().map_or(1, |highest| highest + 1)
}

/// What the picker calls a chat: its opening question.
///
/// Control characters are stripped for the same reason `ui::wrap` strips them —
/// this is the one place model-adjacent text is printed without going through
/// wrapping, and an escape sequence must not be able to repaint the pane.
fn title(messages: &[Message]) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_chat_gets_id_one() {
        assert_eq!(next_id(&[]), 1);
    }

    /// Ids must keep climbing past a deleted chat, or `list` would show the new
    /// one in the wrong place — and the second save would overwrite a survivor.
    #[test]
    fn ids_climb_past_gaps_rather_than_filling_them() {
        assert_eq!(next_id(&[1, 2, 7]), 8);
        assert_eq!(next_id(&[7, 1]), 8);
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
    fn prompt_history_uses_an_empty_list_for_invalid_json() {
        assert_eq!(parse_prompt_history(br#"["one","two"]"#), ["one", "two"]);
        assert!(parse_prompt_history(b"not json").is_empty());
    }
}

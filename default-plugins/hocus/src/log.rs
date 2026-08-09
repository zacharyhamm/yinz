//! JSONL logging for all chat sessions.
//!
//! Each chat writes to `/cache/chats/<id>.jsonl` and to the global
//! `/cache/hocus.jsonl`. One JSON object per line, one event per object.
//! Writes are append-only and errors are swallowed — a full disk must not
//! take the pane down.

use std::fs::{self, OpenOptions};
use std::io::Write;

const DIR: &str = "/cache/chats";
const GLOBAL: &str = "/cache/hocus.jsonl";

/// Max chars kept for a single string field before truncation.
pub const TRUNCATE_AT: usize = 20_000;
const TRUNCATE_SUFFIX: &str = "\n[truncated]";

/// Build one JSON line. `chat_id` may be None before the first save.
pub fn line(
    chat_id: Option<u64>,
    mono_ms: u64,
    seq: u64,
    kind: &str,
    payload: serde_json::Value,
) -> Option<String> {
    serde_json::to_string(&build_event(chat_id, mono_ms, seq, kind, payload)).ok()
}

/// Append a prebuilt line. Runs on the io worker, so the plugin's `update()`
/// never waits on the disk.
pub fn write_line(chat_id: Option<u64>, line: &str) {
    // Per-chat file when id is known.
    if let Some(id) = chat_id {
        let _ = fs::create_dir_all(DIR);
        let path = format!("{DIR}/{id}.jsonl");
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{line}");
        }
    }

    // Always also write global file for `tail -f` convenience.
    let _ = fs::create_dir_all("/cache");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(GLOBAL) {
        let _ = writeln!(f, "{line}");
    }
}

/// Truncate a string field to `TRUNCATE_AT` chars.
pub fn truncate_str(s: &str, limit: usize) -> String {
    if s.chars().count() <= limit {
        return s.to_owned();
    }
    let truncated: String = s.chars().take(limit).collect();
    format!("{truncated}{TRUNCATE_SUFFIX}")
}

pub fn truncate(s: &str) -> String {
    truncate_str(s, TRUNCATE_AT)
}

fn wall_ms() -> Option<u64> {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

fn build_event(
    chat_id: Option<u64>,
    mono_ms: u64,
    seq: u64,
    kind: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    let mut event = serde_json::Value::Object(serde_json::Map::new());
    if let serde_json::Value::Object(map) = payload {
        for (k, v) in map {
            event[k] = v;
        }
    }
    event["v"] = serde_json::json!(1);
    event["ts_mono_ms"] = serde_json::json!(mono_ms);
    event["chat_id"] = serde_json::json!(chat_id);
    event["seq"] = serde_json::json!(seq);
    event["kind"] = serde_json::json!(kind);
    if let Some(wall) = wall_ms() {
        event["ts_wall_ms"] = serde_json::json!(wall);
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_base() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "hocus-test-{}-{}",
            std::process::id(),
            // Use a per-test suffix via thread id hash.
            format!("{:?}", std::thread::current().id())
                .replace(|c: char| !c.is_alphanumeric(), "_")
        ));
        p
    }

    fn write_event_to(
        dir: &str,
        global: &str,
        chat_id: Option<u64>,
        mono_ms: u64,
        seq: u64,
        kind: &str,
        payload: serde_json::Value,
    ) {
        let event = build_event(chat_id, mono_ms, seq, kind, payload);
        let line = serde_json::to_string(&event).expect("serialize");
        if let Some(id) = chat_id {
            let _ = fs::create_dir_all(dir);
            let path = format!("{dir}/{id}.jsonl");
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(f, "{line}");
            }
        }
        let _ = fs::create_dir_all(std::path::Path::new(global).parent().unwrap());
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(global) {
            let _ = writeln!(f, "{line}");
        }
    }

    #[test]
    fn truncate_short_string_is_unchanged() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_is_cut_and_marked() {
        let s = "a".repeat(20);
        let out = truncate_str(&s, 10);
        assert!(out.starts_with(&"a".repeat(10)));
        assert!(out.ends_with(TRUNCATE_SUFFIX));
    }

    #[test]
    fn truncate_at_boundary_is_not_truncated() {
        let s = "a".repeat(10);
        assert_eq!(truncate_str(&s, 10), s);
    }

    #[test]
    fn truncate_handles_unicode() {
        let s = "é".repeat(10);
        let out = truncate_str(&s, 5);
        assert!(out.chars().count() == 5 + TRUNCATE_SUFFIX.chars().count());
    }

    #[test]
    fn append_does_not_panic_without_id() {
        let base = temp_base();
        let dir = base.join("chats").to_string_lossy().into_owned();
        let global = base.join("hocus.jsonl").to_string_lossy().into_owned();
        write_event_to(
            &dir,
            &global,
            None,
            0,
            1,
            "test",
            serde_json::json!({ "content": "hi" }),
        );
        let content = fs::read_to_string(&global).expect("global file exists");
        assert!(content.contains("\"kind\":\"test\""));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn append_with_id_does_not_panic() {
        let base = temp_base();
        let dir = base.join("chats").to_string_lossy().into_owned();
        let global = base.join("hocus.jsonl").to_string_lossy().into_owned();
        write_event_to(
            &dir,
            &global,
            Some(99999),
            123,
            1,
            "test",
            serde_json::json!({ "content": "hi" }),
        );
        let chat_path = format!("{dir}/99999.jsonl");
        let chat_content = fs::read_to_string(&chat_path).expect("chat file exists");
        assert!(chat_content.contains("\"chat_id\":99999"));
        let global_content = fs::read_to_string(&global).expect("global file exists");
        assert!(global_content.contains("\"kind\":\"test\""));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn event_serializes_with_expected_keys() {
        let payload = serde_json::json!({"content": "hello", "model": "gpt-4o"});
        let event = build_event(Some(42), 100, 1, "user_input", payload);
        assert_eq!(event["v"], 1);
        assert_eq!(event["kind"], "user_input");
        assert_eq!(event["content"], "hello");
        assert_eq!(event["chat_id"], 42);
        assert_eq!(event["model"], "gpt-4o");
    }

    #[test]
    fn payload_cannot_overwrite_envelope() {
        let payload = serde_json::json!({"kind": "evil", "chat_id": 999, "v": 99, "content": "hi"});
        let event = build_event(Some(42), 100, 1, "user_input", payload);
        assert_eq!(event["kind"], "user_input");
        assert_eq!(event["chat_id"], 42);
        assert_eq!(event["v"], 1);
        assert_eq!(event["content"], "hi");
    }
}

use crate::*;
use serde::{Deserialize, Serialize};
use yinz_tile::prelude::*;

// ---------------------------------------------------------------------------
// Workers — separate wasm instances of this same binary, driven only by
// post_message_to. They share the /cache preopen but no memory with the plugin.
// ---------------------------------------------------------------------------

/// Fire-and-forget JSONL appends, off the plugin's `update()` path. One worker
/// with a FIFO mailbox, so log lines land in post order. No replies.
///
/// The transcript and the prompt history used to come through here too. They go
/// to the data store now, which is a host call rather than a blocking write —
/// and could not come through here anyway: a new chat's id comes back from the
/// apply, and this mailbox has no way to hand anything back.
#[derive(Default, Serialize, Deserialize)]
struct IoWorker;

impl YinzWorker<'_> for IoWorker {
    fn on_message(&mut self, message: String, payload: String) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&payload) else {
            return;
        };
        if message.as_str() == "log" {
            log::write_line(
                v["chat_id"].as_u64(),
                v["line"].as_str().unwrap_or_default(),
            )
        }
    }
}

register_worker!(IoWorker, io_worker, IO_WORKER);

/// Queue a JSONL append on the io worker (addressed as "io" — the host owns the
/// `_worker` suffix).
pub(crate) fn post_log(
    chat_id: Option<u64>,
    mono_ms: u64,
    seq: u64,
    kind: &str,
    payload: serde_json::Value,
) {
    if let Some(line) = log::line(chat_id, mono_ms, seq, kind, payload) {
        post_message_to(PluginMessage::new_to_worker(
            "io",
            "log",
            &serde_json::json!({ "chat_id": chat_id, "line": line }).to_string(),
        ));
    }
}

/// What a terminal pane is showing, routed through the existing `read_pane` rather
/// than a second host call, so there is only one implementation of "read a pane" to
/// get wrong. Used by `agent_list` previews and the busy/idle poll.
pub(crate) fn read_pane_text(pane_id: u32) -> String {
    let call = ToolCall {
        id: String::new(),
        name: "read_pane".to_owned(),
        arguments: serde_json::json!({
            "pane_id": pane_id,
            "is_plugin": false,
            "full_scrollback": false,
        })
        .to_string(),
    };
    tools::dispatch(&call)
}

use crate::*;
use hocus_proto::panes::terminal_panes;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

// ---------------------------------------------------------------------------
// Workers — separate wasm instances of this same binary, driven only by
// post_message_to. They share the /cache preopen but no memory with the plugin.
// ---------------------------------------------------------------------------

/// Fire-and-forget file writes, off the plugin's `update()` path. One worker
/// with a FIFO mailbox, so log lines and saves land in post order. No replies.
#[derive(Default, Serialize, Deserialize)]
struct IoWorker;

impl ZellijWorker<'_> for IoWorker {
    fn on_message(&mut self, message: String, payload: String) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&payload) else {
            return;
        };
        match message.as_str() {
            "log" => log::write_line(
                v["chat_id"].as_u64(),
                v["line"].as_str().unwrap_or_default(),
            ),
            "save" => {
                if let (Some(id), Some(json)) = (v["id"].as_u64(), v["json"].as_str()) {
                    history::write(id, json.as_bytes());
                }
            },
            "prompt_history" => {
                if let Some(json) = v["json"].as_str() {
                    history::write_prompt_history(json.as_bytes());
                }
            },
            _ => {},
        }
    }
}

register_worker!(IoWorker, io_worker, IO_WORKER);

/// Queue a transcript write on the io worker (addressed as "io" — the host owns
/// the `_worker` suffix).
pub(crate) fn post_save(id: u64, json: String) {
    post_message_to(PluginMessage::new_to_worker(
        "io",
        "save",
        &serde_json::json!({ "id": id, "json": json }).to_string(),
    ));
}

pub(crate) fn post_prompt_history(entries: &[String]) {
    let Some(json) = history::prompt_history_json(entries) else {
        return;
    };
    post_message_to(PluginMessage::new_to_worker(
        "io",
        "prompt_history",
        &serde_json::json!({ "json": json }).to_string(),
    ));
}

/// Queue a JSONL append on the io worker.
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
    match tools::dispatch(&call) {
        Dispatch::Done(text) => text,
        Dispatch::Deferred => "pane read cannot be deferred".to_owned(),
    }
}

/// The host work of one agent poll: fresh pane snapshot, the running command of
/// each pane, and a scrollback hash per running agent for busy/idle. A free
/// function so the poll worker and the synchronous in-plugin poll are the same
/// code. O(panes) blocking host calls — the reason the worker exists.
///
/// Pulled, not pushed. `Event::PaneUpdate` goes only to plugins of the active tab
/// and to background plugins, so a Hocus launched into a global pane — which is in
/// no tab — receives it never, and every agent tool saw an empty session.
///
/// The manifest carries `terminal_command`, which is what a pane was *launched*
/// with and so is `None` for every pane the user opened as a shell. Detection
/// needs what is running now, which is a second call per pane.
pub(crate) fn gather_poll(
    manual_kinds: &BTreeMap<u32, String>,
    activity: &mut agents::Activity,
) -> Option<Vec<agents::AgentPane>> {
    let session = tools::current_session()?;
    let mut panes = terminal_panes(&session.panes, &session.tabs);
    // ponytail: one blocking host round-trip per terminal pane, every refresh.
    // Fine at a dozen panes; batch it if a session ever gets big enough to feel.
    for pane in &mut panes {
        if let Some(command) = tools::pane_running_command(pane.pane_id) {
            pane.command = Some(command);
        }
        pane.cwd = tools::pane_cwd(pane.pane_id);
    }
    // A pane that closed took its observations with it.
    activity.retain(|id, _| panes.iter().any(|pane| pane.pane_id == *id));
    // ponytail: one read_pane per running agent per poll, to tell busy from
    // idle. Scales with agents, not panes; batch with the refresh if it feels.
    // An agent whose TUI repaints a spinner every frame never reads idle —
    // false "busy" is the safe direction.
    let running: Vec<u32> = panes
        .iter()
        .filter(|pane| {
            pane.status == agents::AgentStatus::Running
                && agents::classify(pane, manual_kinds).is_some()
        })
        .map(|pane| pane.pane_id)
        .collect();
    for pane_id in running {
        agents::observe(activity, pane_id, &read_pane_text(pane_id));
    }
    Some(panes)
}

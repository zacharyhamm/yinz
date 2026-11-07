//! The native agent view behind the web plugin's `AgentView` command.
//!
//! Runs on a blocking thread spawned by the plugin thread's `PluginInstruction::Web(AgentView)`
//! arm, reusing the palette's native paths so the web view and the palette stay identical.
//! Replies are `{"ok": …}` or `{"error": {"code", "message"}}`, with codes `invalid`,
//! `not_found`, `target_changed`, `paused`, `unavailable` and `failed`.

use yinz_bus::thread_bus::ThreadSenders;
use yinz_storage::data_access::DataAccess;
use yinz_types::internal_ids::ClientId;

/// Answer one agent-view operation (`{"op":"list"}`, …).
pub(crate) fn handle(
    _request: &str,
    _client: ClientId,
    _senders: &ThreadSenders,
    _data: &DataAccess,
) -> serde_json::Value {
    serde_json::json!({
        "error": {"code": "unavailable", "message": "the agent view is not available in this build"}
    })
}

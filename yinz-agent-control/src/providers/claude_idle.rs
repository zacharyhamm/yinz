use crate::host::AgentHost;
use yinz_types::internal_ids::TerminalId;
use yinz_types::pane_id::PaneId;

/// The text Claude Code shows in its own desktop notification once it has sat idle at
/// its prompt.
pub const IDLE_REMINDER: &str = "Claude is waiting for your input";

/// Whether a desktop notification from this pane is Claude's idle reminder.
///
/// Claude Code writes this notification itself (OSC 9, 777 or 99), separate from its
/// hooks, so no structured signal identifies it. This exact TUI text must follow
/// upstream changes. Only panes yinz knows run Claude match, so another program
/// that prints the same words still notifies.
pub fn idle_reminder(
    host: &impl AgentHost,
    title: &str,
    body: &str,
    terminal_id: TerminalId,
) -> bool {
    (body.trim() == IDLE_REMINDER || (body.trim().is_empty() && title.trim() == IDLE_REMINDER))
        && host
            .pane_info(PaneId::Terminal(terminal_id))
            .and_then(|info| info.agent)
            .is_some_and(|agent| agent.kind == "claude")
}

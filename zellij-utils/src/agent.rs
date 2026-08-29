//! What an agent pane says about itself.
//!
//! A pane's agent record (`PaneInfo::agent`) is written by three roads: the spawn path, for a
//! pane a plugin opened as an agent pane; a plugin, through `PluginCommand::SetPaneAgent`; and
//! the agent's own hooks, through `zellij action agent-report --hook-json`. This module is the
//! reading of a Claude Code hook event that the third road needs, kept here so the binary and
//! the plugins that draw the result cannot drift apart.

use crate::data::{AgentInfo, AgentSource, AgentState};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The event name that ends a session, and so ends the pane's record.
pub const HOOK_SESSION_END: &str = "SessionEnd";

/// The `Notification` kinds that mean the agent is blocked on the user.
///
/// Most notifications are not about being waited on — `auth_success` and the
/// `quota_auto_resume_*` family say nothing about the turn, and `idle_prompt` fires a minute
/// *after* a finished turn, which is idleness rather than a question. Naming the ones that
/// count, rather than excluding the ones that do not, means a notification kind added in a
/// later Claude Code cannot silently start reading as waiting.
pub const WAITING_NOTIFICATIONS: &[&str] = &[
    "permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
    "agent_needs_input",
    "worker_permission_prompt",
];

/// The longest tool hint a record carries. A status row has no room for an argument, and
/// `tool_input` is free-form text of arbitrary size.
pub const HOOK_DETAIL_CHARS: usize = 24;

/// A Claude Code hook event, as its JSON arrives.
///
/// Every field but the event name is optional, because the payload's shape differs between
/// events and between versions, and an unrecognised one must not cost us the event.
///
/// Deliberately *not* named for `input::hooks::HookPoint`, which is a different mechanism
/// entirely — that one is zellij handing a moment of its own to a plugin.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEvent {
    /// `SessionStart`, `PreToolUse`, `Stop`, … — see [`hook_state`].
    pub hook_event_name: String,
    /// Claude Code's own session id. Kept so a pane that is restarted in place can be told
    /// from one that merely went quiet.
    #[serde(default)]
    pub session_id: String,
    /// Why a `SessionStart` fired: `startup`, `resume`, `clear`, `compact`, or `fork`.
    #[serde(default)]
    pub source: Option<String>,
    /// The folder Claude Code is working in, which is not always the pane's shell folder —
    /// the agent is what moved, and this is the one that says where to.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Which notification this is, on a `Notification`. Most of them say nothing about whether
    /// the user is being waited on; [`hook_state`] is where the ones that do are named.
    #[serde(default)]
    pub notification_type: Option<String>,
    /// The tool about to run, or just finished, on a `PreToolUse`/`PostToolUse`.
    #[serde(default)]
    pub tool_name: Option<String>,
    /// That tool's arguments. Free-form, attacker-influenced text of arbitrary size — whatever
    /// is taken from it is truncated hard and drawn as plain text.
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
}

/// The state one hook event names, or `None` for an event that says nothing about the agent's
/// state — an unrecognised name, or a notification that is not about the user being waited on.
///
/// `SessionEnd` is `None` here too, and deliberately: it does not move the record to a state,
/// it ends one. [`hook_outcome`] is the reading that tells the two apart.
pub fn hook_state(event: &HookEvent) -> Option<AgentState> {
    match event.hook_event_name.as_str() {
        "SessionStart"
            if matches!(event.source.as_deref(), Some("startup" | "resume" | "fork")) =>
        {
            Some(AgentState::Idle)
        },
        "Stop" => Some(AgentState::Idle),
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => Some(AgentState::Busy),
        "PermissionRequest" => Some(AgentState::Waiting),
        "Notification" => event
            .notification_type
            .as_deref()
            .filter(|kind| WAITING_NOTIFICATIONS.contains(kind))
            .map(|_| AgentState::Waiting),
        _ => None,
    }
}

/// The few words a record carries for the tool a `PreToolUse` names.
///
/// The tool's own name, and one argument's worth of hint where the tool has an obvious one:
/// which file, or which program. Everything else about `tool_input` is ignored rather than
/// summarised — it is free-form text of arbitrary size that the agent's *input* can steer, so
/// what little is taken is cut to [`HOOK_DETAIL_CHARS`] and stripped of anything that is not
/// printable.
///
/// `None` for any event that is not a tool starting, which is what clears the hint when the
/// tool finishes.
pub fn hook_detail(event: &HookEvent) -> Option<String> {
    if event.hook_event_name != "PreToolUse" {
        return None;
    }
    let tool = clean_hint(event.tool_name.as_deref()?)?;
    let input = event.tool_input.as_ref();
    let hint = match tool.as_str() {
        // The program, not the command line: a row has room for `git`, not for the rest of it.
        "Bash" => input
            .and_then(|input| input.get("command")?.as_str())
            .and_then(|command| command.split_whitespace().next()),
        "Read" | "Edit" | "Write" | "NotebookEdit" => input
            .and_then(|input| input.get("file_path")?.as_str())
            .map(|path| {
                Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(path)
            }),
        _ => None,
    };
    match hint.and_then(clean_hint) {
        Some(hint) => Some(format!("{tool} {hint}")),
        None => Some(tool),
    }
}

/// One word of agent-supplied text, made safe to put in a row: printable characters only, no
/// whitespace of any kind, and cut to a length a sidebar has.
pub fn clean_hint(text: &str) -> Option<String> {
    let mut chars = text
        .chars()
        .filter(|c| !c.is_control() && !c.is_whitespace());
    let mut hint: String = chars.by_ref().take(HOOK_DETAIL_CHARS).collect();
    if hint.is_empty() {
        return None;
    }
    // A cut word is marked, so a truncated file name does not read as the whole one.
    if chars.next().is_some() {
        hint.pop();
        hint.push('…');
    }
    Some(hint)
}

/// What one hook event does to the pane's agent record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookOutcome {
    /// The session is over: the pane no longer runs an agent, so the record goes.
    Ended,
    /// The agent reported a state; this is the whole record, ready to set.
    Reported(AgentInfo),
    /// The event says nothing about the agent's state, and the record stays as it was.
    Nothing,
}

/// Read one hook event from an agent of `kind`. Every reported record is complete — kind,
/// state, hint, folder, session — so setting it needs no merge with what came before.
pub fn hook_outcome(kind: &str, event: &HookEvent) -> HookOutcome {
    if event.hook_event_name == HOOK_SESSION_END {
        return HookOutcome::Ended;
    }
    match hook_state(event) {
        Some(state) => HookOutcome::Reported(AgentInfo {
            kind: kind.to_owned(),
            state,
            source: AgentSource::Reported,
            detail: hook_detail(event),
            cwd: event.cwd.clone(),
            session_id: (!event.session_id.is_empty()).then(|| event.session_id.clone()),
        }),
        None => HookOutcome::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str) -> HookEvent {
        HookEvent {
            hook_event_name: name.to_owned(),
            session_id: "s1".to_owned(),
            cwd: Some("/work".to_owned()),
            ..Default::default()
        }
    }

    fn notification(kind: &str) -> HookEvent {
        HookEvent {
            notification_type: Some(kind.to_owned()),
            ..event("Notification")
        }
    }

    #[test]
    fn each_hook_event_names_the_state_it_means() {
        let start = |source: &str| HookEvent {
            source: Some(source.to_owned()),
            ..event("SessionStart")
        };
        assert_eq!(hook_state(&start("startup")), Some(AgentState::Idle));
        assert_eq!(hook_state(&start("resume")), Some(AgentState::Idle));
        assert_eq!(hook_state(&start("clear")), None, "a clear is mid-turn");
        assert_eq!(hook_state(&event("SessionStart")), None);
        assert_eq!(hook_state(&event("Stop")), Some(AgentState::Idle));
        assert_eq!(
            hook_state(&event("UserPromptSubmit")),
            Some(AgentState::Busy)
        );
        assert_eq!(hook_state(&event("PreToolUse")), Some(AgentState::Busy));
        assert_eq!(hook_state(&event("PostToolUse")), Some(AgentState::Busy));
        assert_eq!(
            hook_state(&event("PermissionRequest")),
            Some(AgentState::Waiting)
        );
        assert_eq!(
            hook_state(&notification("permission_prompt")),
            Some(AgentState::Waiting)
        );
        assert_eq!(
            hook_state(&notification("agent_needs_input")),
            Some(AgentState::Waiting)
        );
        assert_eq!(hook_state(&notification("idle_prompt")), None);
        assert_eq!(hook_state(&notification("quota_auto_resume_fired")), None);
        assert_eq!(hook_state(&event(HOOK_SESSION_END)), None);
        assert_eq!(hook_state(&event("SomethingNewInAYearsTime")), None);
    }

    #[test]
    fn a_reported_record_is_complete_and_an_end_is_an_end() {
        let pre = HookEvent {
            tool_name: Some("Bash".to_owned()),
            tool_input: Some(serde_json::json!({ "command": "cargo build --release" })),
            ..event("PreToolUse")
        };
        assert_eq!(
            hook_outcome("claude", &pre),
            HookOutcome::Reported(AgentInfo {
                kind: "claude".to_owned(),
                state: AgentState::Busy,
                source: AgentSource::Reported,
                detail: Some("Bash cargo".to_owned()),
                cwd: Some("/work".to_owned()),
                session_id: Some("s1".to_owned()),
            })
        );
        assert_eq!(
            hook_outcome("claude", &event(HOOK_SESSION_END)),
            HookOutcome::Ended
        );
        assert_eq!(
            hook_outcome("claude", &notification("auth_success")),
            HookOutcome::Nothing
        );
        match hook_outcome("claude", &event("PostToolUse")) {
            HookOutcome::Reported(info) => {
                assert_eq!(info.detail, None, "a finished tool clears the hint")
            },
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_hint_is_one_printable_word_cut_to_size() {
        assert_eq!(clean_hint("  git\tstatus "), Some("gitstatus".to_owned()));
        assert_eq!(clean_hint("\u{7}"), None);
        let long = "a".repeat(HOOK_DETAIL_CHARS + 5);
        let hint = clean_hint(&long).unwrap();
        assert_eq!(hint.chars().count(), HOOK_DETAIL_CHARS);
        assert!(hint.ends_with('…'));
    }
}

//! What an agent pane says about itself.
//!
//! A pane's agent record (`PaneInfo::agent`) is written by three roads: the spawn path, for a
//! pane a plugin opened as an agent pane; a plugin, through `PluginCommand::SetPaneAgent`; and
//! the agent's own hooks, through `zellij action agent-report --hook-json`. This module is the
//! reading of a Claude Code hook event that the third road needs, kept here so the binary and
//! the plugins that draw the result cannot drift apart.

use crate::data::{AgentInfo, AgentSource, AgentState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

/// The Claude Code hook events a record is read from, in the order Claude Code fires them.
///
/// One list for the two things that wire it: `zellij hocus hooks install`, which writes it
/// into a settings file, and a spawned Claude Code pane, which is handed it inline as
/// `--settings`. A `Notification` matcher would have to name the notification kinds that mean
/// "blocked on the user" - a list Claude Code adds to between versions, and one already written
/// in [`WAITING_NOTIFICATIONS`] where it can be tested - so every entry fires on every
/// occurrence and the reading happens here.
pub const CLAUDE_HOOK_EVENTS: &[&str] = &[
    // The feed begins, and the pane is proved to be running Claude Code.
    "SessionStart",
    // Work starts.
    "UserPromptSubmit",
    // Work continues, and names the tool it is in.
    "PreToolUse",
    "PostToolUse",
    // A tool that failed is still the agent working; without this the record sits on the
    // failed tool's name until the next one starts.
    "PostToolUseFailure",
    // A worker starting and finishing under an agent that is working either way. Wired
    // so a long subagent run keeps saying so rather than falling silent between the
    // parent's own tool calls.
    "SubagentStart",
    "SubagentStop",
    // Compaction is work the user did not ask for and is waiting on all the same.
    "PreCompact",
    "PostCompact",
    // Blocked on the user, said as the prompt goes up.
    "PermissionRequest",
    // Auto mode refusing a tool call: not a wait, but the agent carrying on, and the
    // record would otherwise still be showing the tool that never ran.
    "PermissionDenied",
    // An MCP server asking for input, and the answer coming back. The `Notification`
    // pair for the same exchange fires about six seconds later; these two are the
    // instant version of it.
    "Elicitation",
    "ElicitationResult",
    // The same, from the other direction: the backstop for a Claude Code too old to
    // have `PermissionRequest`, and the only signal for an MCP form or a background
    // session wanting input.
    "Notification",
    // The turn ended. Carries `last_assistant_message`, which is what an idle summary
    // is made from - see `docs/AGENT_HOOKS.md`.
    "Stop",
    // The turn ended in an API error. Carries the same message field, and is the only
    // thing that says so: without it a failed turn leaves the record on `busy` until the
    // user types again, which for a record that expires on nothing is the worst case
    // there is.
    "StopFailure",
    // The feed ends.
    "SessionEnd",
];

/// Seconds a hook may take before Claude Code abandons it.
///
/// Explicit because the default is ten minutes and a hook blocks the turn: a report against a
/// socket whose server has gone must not be able to stall an agent. Five rather than one
/// because `SessionEnd` is special-cased at the other end - every `SessionEnd` hook shares a
/// 1.5s budget, raised only to match an individual timeout - and a feed that ends is worth
/// waiting a moment for.
pub const CLAUDE_HOOK_TIMEOUT_SECONDS: u64 = 5;

/// The command a spawned Claude Code pane's hooks run, with no script in between: the pane
/// inherits `ZELLIJ_PANE_ID`, which the action reads itself, and `zellij` is on the path the
/// pane's shell found it on.
pub const CLAUDE_HOOK_REPORT_COMMAND: &str = "zellij action agent-report --hook-json";

/// The pipe the raw event is broadcast on beside the record, and the `--args` key naming the
/// pane it fired in.
///
/// `agent-report --hook-json` sets the pane's record *and* hands the event itself to whatever
/// plugin wants more than a record holds - hocus's daemon reads `last_assistant_message` off a
/// `Stop` to make a summary, and `permission_mode` off a `PermissionRequest` to decide whether
/// a wait is worth interrupting somebody over. Spelled here rather than in the plugin crate so
/// the two ends cannot drift: a pipe name that does not match is silent.
pub const AGENT_HOOK_PIPE: &str = "agent_hook";
pub const AGENT_HOOK_PANE_ID_ARG: &str = "pane_id";

/// One handler entry, as Claude Code's settings spell it.
pub fn claude_hook_handler(command: &str) -> Value {
    json!({
        "type": "command",
        "command": command,
        "timeout": CLAUDE_HOOK_TIMEOUT_SECONDS,
    })
}

/// A Claude Code settings document whose `hooks` wire every event in [`CLAUDE_HOOK_EVENTS`]
/// to `command`. What `zellij hocus hooks install` merges into a settings file, and what a
/// spawned pane is handed whole as `--settings`; the two cannot disagree about the events
/// because this is the one place they are listed.
pub fn claude_hook_settings(command: &str) -> Value {
    let hooks: serde_json::Map<String, Value> = CLAUDE_HOOK_EVENTS
        .iter()
        // No `matcher`: an entry without one fires on every occurrence of the event.
        .map(|event| {
            (
                (*event).to_owned(),
                json!([{ "hooks": [claude_hook_handler(command)] }]),
            )
        })
        .collect();
    json!({ "hooks": hooks })
}

/// The program a command pane's agent record is named for: the command's file name, looked
/// through an `env(1)` wrapper - `env K=V claude …` is `claude`, because `env` execs it and the
/// pane is running the agent, not the wrapper. `None` when there is no program to name.
pub fn agent_kind_of(program: &Path, args: &[String]) -> Option<String> {
    let name = program.file_name()?.to_string_lossy().to_string();
    if name != "env" {
        return Some(name);
    }
    // `env`'s own options and assignments come first; the program is the first word that
    // is neither. `-u NAME` takes a value, which is skipped with it.
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "-u" || arg == "--unset" || arg == "-C" || arg == "--chdir" {
            skip_next = true;
            continue;
        }
        if arg.starts_with('-') || arg.contains('=') {
            continue;
        }
        return Path::new(arg)
            .file_name()
            .map(|name| name.to_string_lossy().to_string());
    }
    None
}

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

/// The `Notification` kinds that mean the agent has *finished*, rather than that it is
/// blocked. Named separately from [`WAITING_NOTIFICATIONS`] for the same reason that list is
/// an allowlist: a kind that means neither must go on saying nothing.
pub const IDLE_NOTIFICATIONS: &[&str] = &["agent_completed"];

/// The tools whose `PreToolUse` *is* the question going up.
///
/// Every other tool starting is work, but these two do nothing except put a prompt in front of
/// the user and wait. Their `PostToolUse` is ordinary work again, which is what clears the wait
/// - so a tool named here needs no special case on the way out.
pub const WAITING_TOOLS: &[&str] = &["AskUserQuestion", "ExitPlanMode"];

/// The permission modes that answer *every* permission prompt without a person.
///
/// A permission prompt reported under one of these has nobody to interrupt: either the mode
/// decided already, or the prompt that fired is one the user has said in advance they do not
/// want to see. The state is still recorded - it is what the agent said - but see
/// [`waiting_needs_a_person`].
///
/// `acceptEdits` is deliberately not here: it answers one kind of prompt rather than all of
/// them, which is what [`AUTO_ACCEPTED_EDIT_TOOLS`] names.
pub const UNATTENDED_MODES: &[&str] = &["bypassPermissions", "dontAsk", "auto"];

/// The tools whose permission prompt `acceptEdits` answers by itself.
///
/// That mode accepts *file edits*, and nothing else. A `Bash` prompt under it stops and waits
/// exactly as it would under `default`, which is why the mode alone cannot decide whether a
/// wait is real.
pub const AUTO_ACCEPTED_EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// The longest tool hint a record carries. A status row has no room for an argument, and
/// `tool_input` is free-form text of arbitrary size.
pub const HOOK_DETAIL_CHARS: usize = 24;

/// The longest description of what an agent is blocked on. Longer than [`HOOK_DETAIL_CHARS`]
/// because its home is a toast, which has two lines and fifty columns, rather than one cell of
/// a sidebar row.
pub const WAITING_DETAIL_CHARS: usize = 80;

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
    /// What the agent said last, on a `Stop` or a `StopFailure`.
    ///
    /// Claude Code's own description of the field is "avoids the need to read and parse the
    /// transcript file", and that is exactly what it buys here: an idle summary made from the
    /// agent's own words rather than from a photograph of its terminal. Optional in Claude
    /// Code's schema, so a turn that ended without one falls back.
    ///
    /// Too big for the pane's record - this is read by a plugin off the broadcast event, not
    /// carried on [`AgentInfo`].
    #[serde(default)]
    pub last_assistant_message: Option<String>,
    /// The notification's text, on a `Notification`, and the MCP server's question on an
    /// `Elicitation`. Required in both of Claude Code's schemas; optional here for the same
    /// reason every other field is — a missing one must not cost us the event.
    #[serde(default)]
    pub message: Option<String>,
    /// The permission mode the session is in: `default`, `plan`, `acceptEdits`, `auto`,
    /// `dontAsk`, `bypassPermissions`. Carried on every event.
    ///
    /// Read for one thing only: a mode that answers permission prompts by itself has nobody to
    /// interrupt, so a wait reported under it is not worth a toast. See
    /// [`waiting_needs_a_person`].
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// What the user just asked for, on a `UserPromptSubmit`.
    ///
    /// Held by a reader so the summary made at the end of the turn can say what the turn was
    /// *for*. A description of an answer with no sight of the question is half a description.
    #[serde(default)]
    pub prompt: Option<String>,
    /// The subagent this event came from, when it came from one at all.
    ///
    /// Present only for a hook firing inside a subagent, which is the one thing that makes a
    /// `Stop`-shaped event not mean the pane's own turn ended: a `SubagentStop` is one worker
    /// finishing while the agent above it works on.
    #[serde(default)]
    pub agent_id: Option<String>,
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
        // A turn that died on an API error ends just as surely as one that finished, and
        // `StopFailure` is the only event that says so. Without it the record sits on `busy`
        // until the user types again — the worst case for a record that, by design, expires on
        // nothing.
        "Stop" | "StopFailure" => Some(AgentState::Idle),
        // The two tools that are themselves a question. Everything else starting is work; see
        // [`WAITING_TOOLS`]. This arm must precede the general `PreToolUse` one below.
        "PreToolUse"
            if event
                .tool_name
                .as_deref()
                .is_some_and(|tool| WAITING_TOOLS.contains(&tool)) =>
        {
            Some(AgentState::Waiting)
        },
        // An MCP server asking for input, said as the form goes up. The `Notification` that
        // follows it is the same backstop `PermissionRequest` has.
        "Elicitation" => Some(AgentState::Waiting),
        "PermissionRequest" => Some(AgentState::Waiting),
        // Everything that is the agent doing something. `SubagentStart`/`SubagentStop` are a
        // worker beginning and ending under an agent that is working either way, and compaction
        // is work the user did not ask for but is waiting on all the same.
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "PostToolUseFailure"
        | "PermissionDenied" | "ElicitationResult" | "SubagentStart" | "SubagentStop"
        | "PreCompact" | "PostCompact" => Some(AgentState::Busy),
        "Notification" => match event.notification_type.as_deref() {
            Some(kind) if WAITING_NOTIFICATIONS.contains(&kind) => Some(AgentState::Waiting),
            Some(kind) if IDLE_NOTIFICATIONS.contains(&kind) => Some(AgentState::Idle),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a reported wait is one a person has to answer.
///
/// `false` only where the prompt is about to answer itself: the agent is momentarily blocked,
/// and recording that is right, but interrupting somebody over it is not. Read only by
/// whatever would interrupt — the state, the record and the row all take the agent at its word.
///
/// The gate is on the event *and* the mode, because a mode only ever answers a **permission
/// prompt**. Of the five ways into [`AgentState::Waiting`], three are a person being addressed
/// directly under any mode at all — the two [`WAITING_TOOLS`], which put a question or a plan
/// in front of the user, and an `Elicitation`, which is an MCP server's own form. Silencing
/// those because the session happens to be in `acceptEdits` leaves a pane blocked with nothing
/// to say so.
///
/// A waiting `Notification` is never suppressed either, and that is the deliberate half. It is
/// the backstop, firing only once its prompt has gone about six seconds unanswered — which is
/// itself the evidence that nothing auto-resolved it — and because an interruption is gated on
/// the busy→waiting edge it can only ever be the sole signal for that prompt in the first
/// place.
///
/// That leaves `PermissionRequest`, where the mode decides: everything under
/// [`UNATTENDED_MODES`], and only the file-editing tools under `acceptEdits`. A request naming
/// no tool is treated as attended, because a wait announced once too often costs a toast and
/// one missed costs a blocked agent nobody is told about.
pub fn waiting_needs_a_person(event: &HookEvent) -> bool {
    if event.hook_event_name != "PermissionRequest" {
        return true;
    }
    let Some(mode) = event.permission_mode.as_deref() else {
        return true;
    };
    if UNATTENDED_MODES.contains(&mode) {
        return false;
    }
    if mode != "acceptEdits" {
        return true;
    }
    !event
        .tool_name
        .as_deref()
        .is_some_and(|tool| AUTO_ACCEPTED_EDIT_TOOLS.contains(&tool))
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
    tool_summary(event)
}

/// A tool named, with one argument's worth of hint where it has an obvious one.
///
/// Shared by [`hook_detail`], which puts it on the record for a tool that is running, and by
/// [`waiting_detail`], which describes a tool that is waiting to be allowed to run. One reading
/// of `tool_input`, so the two cannot name the same tool call two different ways.
fn tool_summary(event: &HookEvent) -> Option<String> {
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

/// What the agent is blocked on, in one line, or `None` for an event that is not a wait.
///
/// The counterpart of [`hook_detail`] for the other half of a record. Every source here is
/// text the agent or an MCP server wrote, so all of it goes through [`clean_line`] — but with
/// [`WAITING_DETAIL_CHARS`] rather than the row's much tighter cap, because what this feeds is
/// a toast with two lines to spend.
///
/// Read defensively throughout: `tool_input` has no schema we control, and a Claude Code that
/// reshapes `AskUserQuestion`'s arguments must cost us the question's text, not the fact that a
/// question was asked.
pub fn waiting_detail(event: &HookEvent) -> Option<String> {
    match event.hook_event_name.as_str() {
        "PreToolUse" => match event.tool_name.as_deref() {
            Some("AskUserQuestion") => event
                .tool_input
                .as_ref()
                .and_then(|input| input.get("questions")?.get(0).cloned())
                .and_then(|question| {
                    let text = question.get("question").and_then(|v| v.as_str());
                    let header = question.get("header").and_then(|v| v.as_str());
                    text.or(header).and_then(clean_line)
                })
                .or_else(|| Some("a question".to_owned())),
            Some("ExitPlanMode") => Some("approve the plan".to_owned()),
            _ => None,
        },
        // The tool it wants to run is the whole of what is being asked. No fallback:
        // `tool_name` is required in Claude Code's own schema, and a bare "permission" beside a
        // `waiting` that already says as much is a word for nothing.
        "PermissionRequest" => tool_summary(event),
        // Both of these carry the sentence Claude Code would have shown the user, which is a
        // better line than anything reconstructed from the event around it.
        "Elicitation" | "Notification" => event.message.as_deref().and_then(clean_line),
        _ => None,
    }
}

/// A line of agent-supplied text, made safe to show: printable characters only, runs of
/// whitespace collapsed to one space, and cut to [`WAITING_DETAIL_CHARS`].
///
/// Unlike [`clean_hint`], whitespace is collapsed rather than removed — this is a sentence
/// rather than one word, and a question with its spaces taken out is not readable.
pub fn clean_line(text: &str) -> Option<String> {
    let mut line = String::with_capacity(text.len().min(WAITING_DETAIL_CHARS));
    let mut cut = false;
    let mut pending_space = false;
    for ch in text.chars() {
        // Whitespace *first*, because a newline and a tab are control characters too, and
        // dropping them rather than collapsing them welds the words on either side of a line
        // break into one.
        if ch.is_whitespace() {
            // Held rather than pushed, so a trailing run never reaches the string and a cut
            // never lands on one.
            pending_space = !line.is_empty();
            continue;
        }
        if ch.is_control() {
            continue;
        }
        // The `…` a cut adds has to fit inside the cap as well, so one column is held back for
        // it: the bound is on the finished line, not on what was kept.
        if line.chars().count() + usize::from(pending_space) >= WAITING_DETAIL_CHARS {
            cut = true;
            break;
        }
        if pending_space {
            line.push(' ');
            pending_space = false;
        }
        line.push(ch);
    }
    if line.is_empty() {
        return None;
    }
    if cut {
        line.push('…');
    }
    Some(line)
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
            waiting_on: waiting_detail(event),
            cwd: event.cwd.clone(),
            session_id: (!event.session_id.is_empty()).then(|| event.session_id.clone()),
        }),
        None => HookOutcome::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hook_settings_wire_every_event_to_the_command() {
        let settings = claude_hook_settings("zellij action agent-report --hook-json");
        let hooks = settings["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), CLAUDE_HOOK_EVENTS.len());
        for event in CLAUDE_HOOK_EVENTS {
            let handler = &hooks[*event][0]["hooks"][0];
            assert_eq!(handler["type"], "command", "{event}");
            assert_eq!(handler["command"], "zellij action agent-report --hook-json");
            assert_eq!(handler["timeout"], CLAUDE_HOOK_TIMEOUT_SECONDS);
            assert!(
                hooks[*event][0].get("matcher").is_none(),
                "{event} fires on every occurrence"
            );
        }
    }

    #[test]
    fn an_env_wrapper_is_looked_through_for_the_agents_name() {
        let args = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            agent_kind_of(Path::new("/usr/bin/claude"), &[]).as_deref(),
            Some("claude")
        );
        assert_eq!(
            agent_kind_of(
                Path::new("env"),
                &args(&["FOO=1", "BAR=x=y", "claude", "--model", "opus"])
            )
            .as_deref(),
            Some("claude")
        );
        assert_eq!(
            agent_kind_of(
                Path::new("/usr/bin/env"),
                &args(&["-i", "-u", "HOME", "codex"])
            )
            .as_deref(),
            Some("codex")
        );
        assert_eq!(agent_kind_of(Path::new("env"), &args(&["FOO=1"])), None);
    }

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

        // The nine events the table grew by. A turn that died on an API error ended; a
        // subagent, a compaction and a refused tool call are all the agent working; an MCP
        // form is a person being asked.
        assert_eq!(hook_state(&event("StopFailure")), Some(AgentState::Idle));
        for busy in [
            "PostToolUseFailure",
            "PermissionDenied",
            "ElicitationResult",
            "SubagentStart",
            "SubagentStop",
            "PreCompact",
            "PostCompact",
        ] {
            assert_eq!(hook_state(&event(busy)), Some(AgentState::Busy), "{busy}");
        }
        assert_eq!(hook_state(&event("Elicitation")), Some(AgentState::Waiting));
        assert_eq!(
            hook_state(&notification("agent_completed")),
            Some(AgentState::Idle)
        );

        // The two tools that *are* the question. Every other tool starting is work.
        let tool = |name: &str, tool: &str| HookEvent {
            tool_name: Some(tool.to_owned()),
            ..event(name)
        };
        for asking in ["AskUserQuestion", "ExitPlanMode"] {
            assert_eq!(
                hook_state(&tool("PreToolUse", asking)),
                Some(AgentState::Waiting),
                "{asking}"
            );
            assert_eq!(
                hook_state(&tool("PostToolUse", asking)),
                Some(AgentState::Busy),
                "{asking} finishing is work again"
            );
        }
        assert_eq!(
            hook_state(&tool("PreToolUse", "Bash")),
            Some(AgentState::Busy)
        );

        // Every wired event says something, or is `SessionEnd`, which ends a record rather
        // than moving it. An event wired for nothing is a hook run for nothing.
        for wired in CLAUDE_HOOK_EVENTS {
            if *wired == HOOK_SESSION_END || *wired == "Notification" {
                continue;
            }
            let said = match *wired {
                "SessionStart" => hook_state(&start("startup")),
                "PreToolUse" | "PostToolUse" => hook_state(&tool(wired, "Bash")),
                _ => hook_state(&event(wired)),
            };
            assert!(said.is_some(), "{wired} is wired and says nothing");
        }
    }

    /// A wait a mode answers by itself is still a wait; it is just not one to interrupt
    /// somebody over. The gate is on the event *and* the mode, because a mode only ever
    /// answers a permission prompt.
    #[test]
    fn a_wait_the_mode_answers_needs_nobody() {
        let request = |tool: Option<&str>, mode: Option<&str>| HookEvent {
            permission_mode: mode.map(str::to_owned),
            tool_name: tool.map(str::to_owned),
            ..event("PermissionRequest")
        };
        assert!(waiting_needs_a_person(&request(Some("Bash"), None)));
        assert!(waiting_needs_a_person(&request(
            Some("Bash"),
            Some("default")
        )));
        assert!(waiting_needs_a_person(&request(Some("Bash"), Some("plan"))));
        for unattended in UNATTENDED_MODES {
            assert!(
                !waiting_needs_a_person(&request(Some("Bash"), Some(unattended))),
                "{unattended} answers every prompt"
            );
        }

        // `acceptEdits` accepts *edits*. A `Bash` prompt under it blocks exactly as it would
        // under `default`, and the mode alone cannot tell the two apart.
        for tool in AUTO_ACCEPTED_EDIT_TOOLS {
            assert!(
                !waiting_needs_a_person(&request(Some(tool), Some("acceptEdits"))),
                "{tool} is accepted without anybody"
            );
        }
        for tool in ["Bash", "WebFetch", "mcp__github__create_pull_request"] {
            assert!(
                waiting_needs_a_person(&request(Some(tool), Some("acceptEdits"))),
                "{tool} still stops and waits"
            );
        }
        // No tool named is treated as attended: a toast too many costs a line, and one too
        // few costs a blocked agent nobody hears about.
        assert!(waiting_needs_a_person(&request(None, Some("acceptEdits"))));

        // The three waits that address a person directly, and the backstop. None of them is a
        // permission prompt, so no mode answers any of them.
        for mode in ["acceptEdits", "bypassPermissions", "dontAsk", "auto"] {
            for tool in WAITING_TOOLS {
                let asking = HookEvent {
                    tool_name: Some((*tool).to_owned()),
                    permission_mode: Some(mode.to_owned()),
                    ..event("PreToolUse")
                };
                assert!(
                    waiting_needs_a_person(&asking),
                    "{tool} under {mode} is still a question"
                );
            }
            let elicitation = HookEvent {
                permission_mode: Some(mode.to_owned()),
                ..event("Elicitation")
            };
            assert!(waiting_needs_a_person(&elicitation), "{mode}");
            // The backstop fires only once its prompt has gone unanswered, which is itself
            // the evidence that the mode did not answer it.
            for kind in ["permission_prompt", "worker_permission_prompt"] {
                let backstop = HookEvent {
                    permission_mode: Some(mode.to_owned()),
                    ..notification(kind)
                };
                assert!(
                    waiting_needs_a_person(&backstop),
                    "{kind} under {mode} went unanswered for six seconds"
                );
            }
        }
    }

    /// What a record says the agent is blocked on. Every source is text the agent or an MCP
    /// server wrote, so all of it is cleaned and cut.
    #[test]
    fn a_wait_names_what_it_is_waiting_for() {
        let ask = HookEvent {
            tool_name: Some("AskUserQuestion".to_owned()),
            tool_input: Some(serde_json::json!({
                "questions": [{ "question": "Which library\nfor dates?", "header": "Library" }]
            })),
            ..event("PreToolUse")
        };
        assert_eq!(
            waiting_detail(&ask).as_deref(),
            Some("Which library for dates?"),
            "a line, not a word: the newline collapses to a space"
        );
        let headerless = HookEvent {
            tool_input: Some(serde_json::json!({ "questions": [{ "header": "Library" }] })),
            ..ask.clone()
        };
        assert_eq!(waiting_detail(&headerless).as_deref(), Some("Library"));
        let shapeless = HookEvent {
            tool_input: Some(serde_json::json!({ "nothing": "we know" })),
            ..ask
        };
        assert_eq!(
            waiting_detail(&shapeless).as_deref(),
            Some("a question"),
            "a reshaped tool_input costs the text, not the fact that a question was asked"
        );

        let plan = HookEvent {
            tool_name: Some("ExitPlanMode".to_owned()),
            ..event("PreToolUse")
        };
        assert_eq!(waiting_detail(&plan).as_deref(), Some("approve the plan"));

        let permission = HookEvent {
            tool_name: Some("Bash".to_owned()),
            tool_input: Some(serde_json::json!({ "command": "rm -rf target" })),
            ..event("PermissionRequest")
        };
        assert_eq!(waiting_detail(&permission).as_deref(), Some("Bash rm"));

        let elicitation = HookEvent {
            message: Some("Which account should I use?".to_owned()),
            ..event("Elicitation")
        };
        assert_eq!(
            waiting_detail(&elicitation).as_deref(),
            Some("Which account should I use?")
        );

        // Nothing that is not a wait names one.
        assert_eq!(waiting_detail(&event("PostToolUse")), None);
        assert_eq!(waiting_detail(&event("Stop")), None);

        // A long question is cut to what a toast holds, and the cut is marked.
        let long = HookEvent {
            message: Some("q ".repeat(200)),
            ..event("Notification")
        };
        let line = waiting_detail(&long).expect("a line");
        assert_eq!(line.chars().count(), WAITING_DETAIL_CHARS);
        assert!(line.ends_with('…'));
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
                waiting_on: None,
                cwd: Some("/work".to_owned()),
                session_id: Some("s1".to_owned()),
            })
        );
        // A wait carries the question as well as the tool: the record is what a row draws
        // *and* what an interruption is written from.
        let permission = HookEvent {
            tool_name: Some("Bash".to_owned()),
            tool_input: Some(serde_json::json!({ "command": "rm -rf target" })),
            ..event("PermissionRequest")
        };
        match hook_outcome("claude", &permission) {
            HookOutcome::Reported(info) => {
                assert_eq!(info.state, AgentState::Waiting);
                assert_eq!(info.waiting_on.as_deref(), Some("Bash rm"));
            },
            other => panic!("{other:?}"),
        }
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

    /// A `Stop` as Claude Code actually writes it, with every field this module reads.
    ///
    /// Pinned against the shipped schema rather than the docs: the field that matters most
    /// here, `last_assistant_message`, is optional in Claude Code's own definition, and a
    /// rename would otherwise cost a summary silently.
    #[test]
    fn a_stop_event_carries_the_agents_last_words() {
        let event: HookEvent = serde_json::from_str(
            r#"{
                "session_id": "abc",
                "transcript_path": "/tmp/t.jsonl",
                "cwd": "/work",
                "permission_mode": "default",
                "hook_event_name": "Stop",
                "stop_hook_active": false,
                "last_assistant_message": "Ran the tests; they pass."
            }"#,
        )
        .expect("parsed");
        assert_eq!(event.hook_event_name, "Stop");
        assert_eq!(event.session_id, "abc");
        assert_eq!(event.cwd.as_deref(), Some("/work"));
        assert_eq!(event.permission_mode.as_deref(), Some("default"));
        assert_eq!(
            event.last_assistant_message.as_deref(),
            Some("Ran the tests; they pass."),
        );
    }

    /// Everything but the event name is optional, and unknown fields are ignored: a Claude
    /// Code that adds one, or drops one we do not read, must not cost us the event.
    #[test]
    fn a_hook_event_parses_with_nothing_but_its_name() {
        let event: HookEvent = serde_json::from_str(
            r#"{"hook_event_name":"Stop","something_new_in_a_years_time":{"a":1}}"#,
        )
        .expect("parsed");
        assert_eq!(event.hook_event_name, "Stop");
        assert!(event.session_id.is_empty());
        assert_eq!(event.last_assistant_message, None);
        assert_eq!(event.message, None);
        assert_eq!(event.prompt, None);
        assert_eq!(event.agent_id, None);
    }

    /// The two events that name what the user is being asked, each in its own field.
    #[test]
    fn a_notification_and_a_prompt_submission_each_carry_their_text() {
        let event: HookEvent = serde_json::from_str(
            r#"{"hook_event_name":"Notification","notification_type":"permission_prompt",
                "message":"Claude needs your permission to use Bash","title":"Claude Code"}"#,
        )
        .expect("parsed");
        assert_eq!(
            event.notification_type.as_deref(),
            Some("permission_prompt")
        );
        assert_eq!(
            event.message.as_deref(),
            Some("Claude needs your permission to use Bash"),
        );

        let event: HookEvent = serde_json::from_str(
            r#"{"hook_event_name":"UserPromptSubmit","prompt":"fix the flaky test","source":"user"}"#,
        )
        .expect("parsed");
        assert_eq!(event.prompt.as_deref(), Some("fix the flaky test"));
    }
}
